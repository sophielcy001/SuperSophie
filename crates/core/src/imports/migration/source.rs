// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{check_cancel, migration_error};
use crate::assets::{hex_sha256, stage_attachment, valid_attachment_name};
use crate::{Attachment, Error, ImportOutcome, ImportStatus, MAX_ATTACHMENT_BYTES, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};
use zip::{CompressionMethod, ZipArchive};

const MAX_ENTRIES: usize = 4_096;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_PATH: usize = 1024;
const MAX_DEPTH: usize = 32;

pub(super) struct Inventory<'a> {
    pub files: BTreeMap<String, Attachment>,
    pub skipped: Vec<ImportOutcome>,
    pub container: Option<Attachment>,
    pub parts: usize,
    directory: &'a Path,
    owner: &'a str,
    cancel: &'a AtomicBool,
    entries: usize,
    bytes: u64,
}

impl<'a> Inventory<'a> {
    pub fn new(directory: &'a Path, owner: &'a str, cancel: &'a AtomicBool) -> Self {
        Self {
            files: BTreeMap::new(),
            skipped: Vec::new(),
            container: None,
            parts: 0,
            directory,
            owner,
            cancel,
            entries: 0,
            bytes: 0,
        }
    }

    pub fn blob(&self, attachment: &Attachment) -> PathBuf {
        self.directory.join(&attachment.sha256)
    }

    pub fn body_spool(&self) -> Result<File> {
        Ok(tempfile::tempfile_in(self.directory)?)
    }

    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        hash.update(b"zhiye.migration.inventory.v1\0");
        for (path, attachment) in &self.files {
            hash.update(path.as_bytes());
            hash.update(b"\0");
            hash.update(attachment.sha256.as_bytes());
            hash.update(b"\0");
        }
        let mut skipped: Vec<_> = self
            .skipped
            .iter()
            .map(|entry| (&entry.name, &entry.notes))
            .collect();
        skipped.sort();
        for (path, notes) in skipped {
            hash.update(path.as_bytes());
            hash.update(b"\0");
            for note in notes {
                hash.update(note.as_bytes());
                hash.update(b"\0");
            }
        }
        hex_sha256(&hash.finalize())
    }

    fn count_entry(&mut self) -> Result<()> {
        check_cancel(self.cancel)?;
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            return Err(Error::new("IMPORT_SIZE", "迁移包最多包含 4,096 个源条目。"));
        }
        Ok(())
    }

    fn skip(&mut self, path: &str, reason: impl Into<String>) {
        let mut entry = ImportOutcome::failed(report_name(path), reason.into());
        entry.status = ImportStatus::Unsupported;
        self.skipped.push(entry);
    }

    fn stage(&mut self, path: String, bytes: u64, input: impl Read) -> Result<()> {
        if bytes > MAX_ATTACHMENT_BYTES {
            self.skip(&path, "原件超过 100 MiB，保留在原来源。");
            return Ok(());
        }
        if self.bytes + bytes > MAX_BYTES {
            return Err(Error::new(
                "IMPORT_SIZE",
                "迁移包展开后的原件总量超过 1 GiB。",
            ));
        }
        let reader = CheckedRead {
            input: input.take(bytes + 1),
            cancel: self.cancel,
        };
        let (attachment, _) = stage_attachment(
            self.directory,
            self.owner,
            &attachment_name(file_name(&path)),
            reader,
        )
        .map_err(|error| {
            if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                super::cancelled()
            } else {
                error
            }
        })?;
        if attachment.bytes != bytes {
            return Err(migration_error(format!(
                "「{path}」读取时大小发生变化，请重新选择来源。"
            )));
        }
        self.bytes += bytes;
        if let Some(previous) = self.files.get(&path) {
            if previous.sha256 == attachment.sha256 {
                return Ok(());
            }
            return Err(migration_error(format!(
                "迁移包中有不同原件使用相同路径「{path}」。"
            )));
        }
        self.files.insert(path, attachment);
        Ok(())
    }

    pub fn notion_archive(&mut self, name: &str, mut input: impl Read + Seek) -> Result<()> {
        let size = input.seek(SeekFrom::End(0))?;
        if size > MAX_BYTES {
            return Err(Error::new("IMPORT_SIZE", "Notion ZIP 超过 1 GiB。"));
        }
        input.rewind()?;
        if size <= MAX_ATTACHMENT_BYTES {
            check_cancel(self.cancel)?;
            let (attachment, _) = stage_attachment(
                self.directory,
                self.owner,
                name,
                CheckedRead {
                    input: (&mut input).take(size + 1),
                    cancel: self.cancel,
                },
            )
            .map_err(|error| {
                if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    super::cancelled()
                } else {
                    error
                }
            })?;
            if attachment.bytes != size {
                return Err(migration_error("ZIP 在导入时发生变化，请重新选择原件。"));
            }
            let snapshot = File::open(self.blob(&attachment))?;
            self.container = Some(attachment);
            self.zip_entries(snapshot)?;
        } else {
            self.zip_entries(&mut input)?;
        }
        if self
            .files
            .keys()
            .all(|path| is_part(path) || path.eq_ignore_ascii_case("index.html"))
        {
            let parts: Vec<_> = self
                .files
                .iter()
                .filter(|(path, _)| is_part(path))
                .map(|(_, part)| self.blob(part))
                .collect();
            self.parts = parts.len();
            for part in parts {
                self.zip_entries(File::open(part)?)?;
            }
        }
        Ok(())
    }

    fn zip_entries(&mut self, input: impl Read + Seek) -> Result<()> {
        let mut archive = ZipArchive::new(input).map_err(zip_error)?;
        if archive.len() > MAX_ENTRIES.saturating_sub(self.entries) {
            return Err(Error::new("IMPORT_SIZE", "迁移包最多包含 4,096 个源条目。"));
        }
        for index in 0..archive.len() {
            self.count_entry()?;
            let raw = archive.by_index_raw(index).map_err(zip_error)?;
            let raw_name = match raw.name() {
                Ok(name) => name.into_owned(),
                Err(_) => {
                    self.skip(
                        &format!("ZIP 条目 {}", index + 1),
                        "文件名无法解码，原条目留在 ZIP。",
                    );
                    continue;
                }
            };
            let is_dir = raw.is_dir();
            let name = raw_name
                .strip_suffix('/')
                .filter(|_| is_dir)
                .unwrap_or(&raw_name);
            if !safe_path(name) {
                self.skip(name, "路径不符合迁移格式。原条目留在来源 ZIP。");
                continue;
            }
            if ignored(name) {
                self.skip(name, "应用设置或系统元数据保留在原来源。");
                continue;
            }
            if is_dir {
                continue;
            }
            if !raw.is_file()
                || raw.is_symlink()
                || raw.enclosed_name().is_none()
                || raw.encrypted()
                || !matches!(
                    raw.compression(),
                    CompressionMethod::Stored | CompressionMethod::Deflated
                )
                || raw
                    .unix_mode()
                    .is_some_and(|mode| !matches!(mode & 0o170000, 0 | 0o100000))
            {
                self.skip(name, "ZIP 条目类型或压缩方式不受支持。原条目留在来源 ZIP。");
                continue;
            }
            let bytes = raw.size();
            drop(raw);
            let entry = archive.by_index(index).map_err(zip_error)?;
            self.stage(name.to_owned(), bytes, entry)?;
        }
        Ok(())
    }

    pub fn vault(&mut self, root: &Path) -> Result<()> {
        let root = fs::canonicalize(root)?;
        if self.directory.starts_with(&root)
            || root.starts_with(self.directory.parent().unwrap_or(self.directory))
        {
            return Err(migration_error(
                "请选择位于知页工作空间以外的 Obsidian 目录。",
            ));
        }
        self.walk_dir(&root, &root, "")
    }

    fn walk_dir(&mut self, root: &Path, directory: &Path, prefix: &str) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            self.count_entry()?;
            let entry = entry?;
            let filename = entry.file_name();
            let Some(name) = filename.to_str() else {
                self.skip(
                    &format!("{prefix}{}", filename.to_string_lossy()),
                    "文件名无法表示为 UTF-8。",
                );
                continue;
            };
            let relative = format!("{prefix}{name}");
            if !safe_path(&relative) || ignored(&relative) {
                self.skip(&relative, "应用设置、系统元数据或不合规路径保留在原来源。");
                continue;
            }
            let entry_type = entry.file_type()?;
            if !entry_type.is_dir() && !entry_type.is_file() {
                self.skip(&relative, "只读取所选目录中的普通文件和文件夹。");
                continue;
            }
            let path = fs::canonicalize(entry.path())?;
            if !path.starts_with(root) || path != entry.path() {
                self.skip(&relative, "符号链接保留在原目录。");
                continue;
            }
            if entry_type.is_dir() {
                self.walk_dir(root, &path, &format!("{relative}/"))?;
            } else {
                let file = File::open(&path)?;
                self.stage(relative, file.metadata()?.len(), file)?;
            }
        }
        Ok(())
    }
}

struct CheckedRead<'a, T> {
    input: T,
    cancel: &'a AtomicBool,
}

impl<T: Read> Read for CheckedRead<'_, T> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(std::io::Error::other("迁移已取消。"));
        }
        self.input.read(buffer)
    }
}

fn zip_error(error: zip::result::ZipError) -> Error {
    migration_error(format!("无法读取 Notion ZIP。{error}"))
}

fn is_part(path: &str) -> bool {
    extension(path).eq_ignore_ascii_case("zip")
        && file_name(path).to_ascii_lowercase().starts_with("export-")
}

pub(super) fn safe_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= MAX_PATH
        && path.split('/').count() <= MAX_DEPTH
        && !path.contains(['\\', ':'])
        && path.split('/').all(|part| {
            !part.trim().is_empty()
                && !matches!(part, "." | "..")
                && part.chars().count() <= 255
                && !part.chars().any(char::is_control)
        })
}

fn ignored(path: &str) -> bool {
    path.split('/').any(|part| {
        matches!(
            part,
            ".obsidian" | ".trash" | ".git" | "__MACOSX" | ".DS_Store"
        )
    })
}

pub(super) fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

pub(super) fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

pub(super) fn stem(path: &str) -> &str {
    let filename = file_name(path);
    filename
        .rsplit_once('.')
        .filter(|(stem, _)| !stem.is_empty())
        .map_or(path, |(_, extension)| {
            &path[..path.len() - extension.len() - 1]
        })
}

pub(super) fn extension(path: &str) -> &str {
    file_name(path).rsplit_once('.').map_or("", |(_, ext)| ext)
}

pub(super) fn is_markdown(path: &str) -> bool {
    ["md", "markdown"]
        .iter()
        .any(|ext| extension(path).eq_ignore_ascii_case(ext))
}

pub(super) fn report_name(path: &str) -> String {
    path.chars()
        .take(MAX_PATH)
        .map(|ch| if ch.is_control() { '�' } else { ch })
        .collect()
}

pub(super) fn attachment_name(name: &str) -> String {
    let converted: String = name
        .chars()
        .take(255)
        .map(|ch| {
            if ch.is_control() || "/\\:*?\"<>|".contains(ch) {
                '-'
            } else {
                ch
            }
        })
        .collect();
    let trimmed = converted.trim();
    if valid_attachment_name(trimmed) {
        trimmed.into()
    } else {
        "原件".into()
    }
}
