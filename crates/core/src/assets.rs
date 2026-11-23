// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::model::{new_id, valid_id};
use crate::store::{ensure_active, ensure_active_info, now, read_page, read_page_info};
use crate::{Error, PageCover, Result, Store, image_extension};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

pub const MAX_ATTACHMENT_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_PREVIEW_BYTES: u64 = 24 * 1024 * 1024;
const FIELDS: &str = "id,page_id,name,sha256,bytes,media_type,created_at";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Attachment {
    pub id: String,
    pub page_id: String,
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub media_type: String,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentImport {
    pub attachment: Attachment,
    pub already_attached: bool,
    pub reused_blob: bool,
}

impl Store {
    pub fn rename_attachment(
        &self,
        workspace: &str,
        page: &str,
        id: &str,
        name: &str,
    ) -> Result<Attachment> {
        let name = name.trim();
        if !valid_attachment_name(name) {
            return Err(Error::new("INVALID_ATTACHMENT", "附件名称不受支持。"));
        }
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        ensure_active(&read_page(&tx, page)?)?;
        let mut attachment = read_attachment(&tx, page, id)?;
        let duplicate = tx.query_row("SELECT EXISTS(SELECT 1 FROM attachments WHERE page_id=?1 AND name=?2 AND sha256=?3 AND id<>?4)",
            params![page, name, attachment.sha256, id], |row| row.get::<_, bool>(0))?;
        if duplicate {
            return Err(Error::new(
                "ATTACHMENT_DUPLICATE",
                "此页面已有相同名称和内容的附件。",
            ));
        }
        tx.execute(
            "UPDATE attachments SET name=?1 WHERE id=?2 AND page_id=?3",
            params![name, id, page],
        )?;
        tx.commit()?;
        attachment.name = name.into();
        Ok(attachment)
    }
    pub fn attachments(&self, workspace: &str, page: &str) -> Result<Vec<Attachment>> {
        let db = self.connection(workspace)?;
        ensure_active(&read_page(&db, page)?)?;
        read_attachments(&db, Some(page))
    }

    pub fn add_attachment(
        &self,
        workspace: &str,
        page: &str,
        name: &str,
        source: impl Read,
    ) -> Result<AttachmentImport> {
        self.attach_file(workspace, page, name, source, false)
    }

    pub fn add_image(
        &self,
        workspace: &str,
        page: &str,
        name: &str,
        source: impl Read,
    ) -> Result<AttachmentImport> {
        self.attach_file(workspace, page, name, source, true)
    }

    fn attach_file(
        &self,
        workspace: &str,
        page: &str,
        name: &str,
        source: impl Read,
        image_only: bool,
    ) -> Result<AttachmentImport> {
        let mut db = self.connection(workspace)?;
        let _lease = self.pin_blobs()?;
        ensure_active(&read_page(&db, page)?)?;
        let directory = blob_directory(&self.data_root().join(workspace), true)?;
        let (attachment, reused_blob) = stage_file(&directory, page, name, source, image_only)?;

        let tx = db.transaction()?;
        ensure_active(&read_page(&tx, page)?)?;
        let existing = matching_attachment(&tx, &attachment)?;
        if let Some(existing) = existing {
            tx.commit()?;
            return Ok(AttachmentImport {
                attachment: existing,
                already_attached: true,
                reused_blob,
            });
        }
        insert_attachment(&tx, &attachment)?;
        tx.commit()?;
        Ok(AttachmentImport {
            attachment,
            already_attached: false,
            reused_blob,
        })
    }

    pub fn attachment(&self, workspace: &str, page: &str, id: &str) -> Result<Attachment> {
        let db = self.connection(workspace)?;
        ensure_active_info(&read_page_info(&db, page)?)?;
        read_attachment(&db, page, id)
    }

    pub fn copy_attachment(
        &self,
        workspace: &str,
        page: &str,
        id: &str,
        destination: impl Write,
    ) -> Result<Attachment> {
        let db = self.connection(workspace)?;
        let _lease = self.pin_blobs()?;
        ensure_active_info(&read_page_info(&db, page)?)?;
        let attachment = read_attachment(&db, page, id)?;
        self.copy_blob(workspace, &attachment, destination)?;
        Ok(attachment)
    }

    pub fn image_attachments(
        &self,
        workspace: &str,
        page: &str,
        blocks: &[crate::Block],
    ) -> Result<Vec<Attachment>> {
        self.referenced_attachments(workspace, page, blocks, true)
    }

    pub fn body_attachments(
        &self,
        workspace: &str,
        page: &str,
        blocks: &[crate::Block],
    ) -> Result<Vec<Attachment>> {
        self.referenced_attachments(workspace, page, blocks, false)
    }

    pub fn markdown_attachments(
        &self,
        workspace: &str,
        page: &crate::Page,
    ) -> Result<Vec<Attachment>> {
        let mut assets = self.body_attachments(workspace, &page.info.id, &page.blocks)?;
        if let Some(cover) = &page.cover
            && !assets.iter().any(|asset| asset.id == cover.attachment_id)
        {
            assets.push(self.attachment(workspace, &page.info.id, &cover.attachment_id)?);
        }
        Ok(assets)
    }

    fn referenced_attachments(
        &self,
        workspace: &str,
        page: &str,
        blocks: &[crate::Block],
        images_only: bool,
    ) -> Result<Vec<Attachment>> {
        let db = self.connection(workspace)?;
        ensure_active(&read_page(&db, page)?)?;
        let mut assets = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for reference in crate::rich_blocks::attachment_references(blocks) {
            if !images_only || reference.media_type.is_some() {
                let asset = resolve_reference(&db, page, &reference)?;
                if seen.insert(asset.id.clone()) {
                    assets.push(asset);
                }
            }
        }
        Ok(assets)
    }

    pub(crate) fn copy_blob(
        &self,
        workspace: &str,
        attachment: &Attachment,
        destination: impl Write,
    ) -> Result<()> {
        let content = transfer(
            self.open_attachment_blob(workspace, attachment)?,
            destination,
            attachment.bytes,
        )?;
        check_content(&content, attachment)
    }

    pub(crate) fn open_attachment_blob(
        &self,
        workspace: &str,
        attachment: &Attachment,
    ) -> Result<File> {
        let workspace_path = self.data_root().join(workspace);
        if fs::canonicalize(&workspace_path)? != workspace_path {
            return Err(Error::new(
                "WORKSPACE_PATH",
                "附件存储不能指向其他工作空间。",
            ));
        }
        let directory = blob_directory(&workspace_path, false)?;
        open_blob(&directory, attachment)
    }
}

pub(crate) fn validate_page_attachments(db: &Connection, page: &crate::Page) -> Result<()> {
    for reference in crate::rich_blocks::attachment_references(&page.blocks) {
        resolve_reference(db, &page.info.id, &reference)?;
    }
    if let Some(cover) = &page.cover
        && read_page_cover(db, &page.info.id, &cover.attachment_id)? != *cover
    {
        return Err(invalid_page_cover());
    }
    Ok(())
}

impl PageCover {
    pub(crate) fn matches(&self, asset: &Attachment, page: &str) -> bool {
        asset.id == self.attachment_id
            && asset.page_id == page
            && asset.media_type == self.media_type
            && image_extension(&asset.media_type).is_some()
            && asset.bytes <= MAX_PREVIEW_BYTES
    }
}

pub(crate) fn invalid_page_cover() -> Error {
    Error::new(
        "PAGE_COVER",
        "请选择本页中不超过 24 MiB 的受支持图片作为封面。",
    )
}

pub(crate) fn read_page_cover(db: &Connection, page: &str, id: &str) -> Result<PageCover> {
    let asset = read_attachment(db, page, id).map_err(|error| {
        if error.code == "ATTACHMENT_NOT_FOUND" {
            invalid_page_cover()
        } else {
            error
        }
    })?;
    let cover = PageCover {
        attachment_id: asset.id.clone(),
        media_type: asset.media_type.clone(),
    };
    if !cover.matches(&asset, page) {
        return Err(invalid_page_cover());
    }
    Ok(cover)
}

fn resolve_reference(
    db: &Connection,
    page: &str,
    reference: &crate::rich_blocks::AttachmentReference<'_>,
) -> Result<Attachment> {
    let attachment = read_attachment(db, page, reference.id).map_err(|error| {
        if error.code == "ATTACHMENT_NOT_FOUND" {
            reference.missing()
        } else {
            error
        }
    })?;
    if !reference.matches(&attachment, page) {
        return Err(reference.missing());
    }
    Ok(attachment)
}

pub(crate) fn read_attachment(db: &Connection, page: &str, id: &str) -> Result<Attachment> {
    db.query_row(
        &format!("SELECT {FIELDS} FROM attachments WHERE page_id=?1 AND id=?2"),
        [page, id],
        row_attachment,
    )
    .optional()?
    .ok_or_else(|| Error::new("ATTACHMENT_NOT_FOUND", "找不到该页面的附件。"))
}

pub(crate) fn matching_attachment(
    db: &Connection,
    candidate: &Attachment,
) -> Result<Option<Attachment>> {
    Ok(db
        .query_row(
            &format!("SELECT {FIELDS} FROM attachments WHERE page_id=?1 AND name=?2 AND sha256=?3"),
            params![candidate.page_id, candidate.name, candidate.sha256],
            row_attachment,
        )
        .optional()?)
}

fn row_attachment(row: &Row<'_>) -> rusqlite::Result<Attachment> {
    Ok(Attachment {
        id: row.get(0)?,
        page_id: row.get(1)?,
        name: row.get(2)?,
        sha256: row.get(3)?,
        bytes: row.get::<_, u32>(4)?.into(),
        media_type: row.get(5)?,
        created_at: row.get(6)?,
    })
}

pub(crate) fn read_attachments(db: &Connection, page: Option<&str>) -> Result<Vec<Attachment>> {
    let mut query = db.prepare(&format!(
        "SELECT {FIELDS} FROM attachments WHERE (?1 IS NULL OR page_id=?1) ORDER BY created_at,id"
    ))?;
    Ok(query
        .query_map([page], row_attachment)?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn insert_attachment(db: &Connection, attachment: &Attachment) -> Result<()> {
    if !valid_attachment(attachment) {
        return Err(Error::new("INVALID_ATTACHMENT", "附件元数据无效。"));
    }
    db.execute("INSERT INTO attachments (id,page_id,name,sha256,bytes,media_type,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7)",
        params![attachment.id, attachment.page_id, attachment.name, attachment.sha256, attachment.bytes as i64, attachment.media_type, attachment.created_at])?;
    Ok(())
}

pub(crate) fn valid_attachment(attachment: &Attachment) -> bool {
    valid_id(&attachment.id)
        && valid_id(&attachment.page_id)
        && valid_attachment_name(&attachment.name)
        && valid_hash(&attachment.sha256)
        && attachment.bytes <= MAX_ATTACHMENT_BYTES
        && attachment.created_at >= 0
        && matches!(
            attachment.media_type.as_str(),
            "image/png"
                | "image/jpeg"
                | "image/gif"
                | "image/webp"
                | "image/bmp"
                | "application/pdf"
                | "application/octet-stream"
        )
}

pub(crate) fn valid_attachment_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 255
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

pub(crate) fn stage_attachment(
    directory: &Path,
    page: &str,
    name: &str,
    source: impl Read,
) -> Result<(Attachment, bool)> {
    stage_file(directory, page, name, source, false)
}

fn stage_file(
    directory: &Path,
    page: &str,
    name: &str,
    source: impl Read,
    image_only: bool,
) -> Result<(Attachment, bool)> {
    if !valid_attachment_name(name) {
        return Err(Error::new("INVALID_ATTACHMENT", "附件名称不受支持。"));
    }
    let mut staging = tempfile::NamedTempFile::new_in(directory)?;
    let limit = if image_only {
        MAX_PREVIEW_BYTES
    } else {
        MAX_ATTACHMENT_BYTES
    };
    let content = transfer(source, &mut staging, limit)?;
    if image_only && crate::image_extension(content.media_type).is_none() {
        return Err(Error::new(
            "IMAGE_TYPE",
            "请选择 PNG、JPEG、GIF、WebP 或 BMP 图片。",
        ));
    }
    let attachment = Attachment {
        id: new_id(),
        page_id: page.into(),
        name: name.into(),
        sha256: content.sha256,
        bytes: content.bytes,
        media_type: content.media_type.into(),
        created_at: now(),
    };
    let reused_blob = persist_blob(staging, directory, &attachment)?;
    Ok((attachment, reused_blob))
}

pub(crate) fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

pub(crate) fn blob_directory(workspace: &Path, create: bool) -> Result<PathBuf> {
    let workspace = fs::canonicalize(workspace)?;
    let directory = workspace.join("blobs");
    if create {
        fs::create_dir_all(&directory)?;
    }
    if fs::canonicalize(&directory)? != directory {
        return Err(Error::new("WORKSPACE_PATH", "附件存储不能指向其他位置。"));
    }
    Ok(directory)
}

pub(crate) struct Content {
    pub sha256: String,
    pub bytes: u64,
    pub media_type: &'static str,
}

pub(crate) fn transfer(
    mut source: impl Read,
    mut destination: impl Write,
    limit: u64,
) -> Result<Content> {
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut header = Vec::with_capacity(16);
    let mut bytes = 0_u64;
    loop {
        let n = match source.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if n == 0 {
            break;
        }
        bytes += n as u64;
        if bytes > limit {
            return Err(Error::new("FILE_TOO_LARGE", "附件超过允许的文件大小。"));
        }
        if header.len() < 16 {
            header.extend_from_slice(&buffer[..n.min(16 - header.len())]);
        }
        let chunk = &buffer[..n];
        hash.update(chunk);
        destination.write_all(chunk)?;
    }
    Ok(Content {
        sha256: hex_sha256(&hash.finalize()),
        bytes,
        media_type: detect_type(&header),
    })
}

pub(crate) fn hex_sha256(hash: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut output = String::with_capacity(hash.len() * 2);
    for byte in hash {
        output.push(HEX[usize::from(byte >> 4)].into());
        output.push(HEX[usize::from(byte & 15)].into());
    }
    output
}

fn detect_type(header: &[u8]) -> &'static str {
    if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if header.starts_with(b"\xff\xd8\xff") {
        "image/jpeg"
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        "image/gif"
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else if header.starts_with(b"BM") {
        "image/bmp"
    } else if header.starts_with(b"%PDF-") {
        "application/pdf"
    } else {
        "application/octet-stream"
    }
}

pub(crate) fn check_content(content: &Content, attachment: &Attachment) -> Result<()> {
    if content.bytes != attachment.bytes
        || content.sha256 != attachment.sha256
        || content.media_type != attachment.media_type
    {
        return Err(Error::new(
            "ATTACHMENT_CHECKSUM",
            "附件原件的内容校验失败。",
        ));
    }
    Ok(())
}

fn verify_blob(directory: &Path, attachment: &Attachment, output: impl Write) -> Result<()> {
    let content = transfer(open_blob(directory, attachment)?, output, attachment.bytes)?;
    check_content(&content, attachment)
}

fn open_blob(directory: &Path, attachment: &Attachment) -> Result<File> {
    if !valid_attachment(attachment) {
        return Err(Error::new("INVALID_ATTACHMENT", "附件元数据无效。"));
    }
    let path = directory.join(&attachment.sha256);
    if !path.is_file() {
        return Err(Error::new("ATTACHMENT_MISSING", "找不到附件原件。"));
    }
    if fs::canonicalize(&path)? != path {
        return Err(Error::new("WORKSPACE_PATH", "附件原件不能指向其他位置。"));
    }
    Ok(File::open(path)?)
}

pub(crate) fn persist_blob(
    staging: tempfile::NamedTempFile,
    directory: &Path,
    attachment: &Attachment,
) -> Result<bool> {
    staging.as_file().sync_all()?;
    match staging.persist_noclobber(directory.join(&attachment.sha256)) {
        Ok(_) => Ok(false),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
            verify_blob(directory, attachment, io::sink()).map_err(|error| {
                if error.code == "ATTACHMENT_MISSING" {
                    Error::new("WORKSPACE_PATH", "附件原件的存储位置不可用，无法保存。")
                } else {
                    error
                }
            })?;
            Ok(true)
        }
        Err(error) => Err(error.error.into()),
    }
}
