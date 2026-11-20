// SPDX-License-Identifier: AGPL-3.0-or-later

mod csv;
mod markdown;
mod migration;

pub use csv::CsvDestination;
pub use migration::MigrationBatch;
pub(crate) use migration::{begin_batch, check_cancel, complete_batch, fail_batch};

use crate::assets::{
    blob_directory, hex_sha256, insert_attachment, stage_attachment, valid_attachment_name,
    valid_hash,
};
use crate::{Error, Result, Store};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const MAX_MARKDOWN_IMPORT_BYTES: usize = 1024 * 1024;
pub const MAX_MARKDOWN_IMPORT_FILES: usize = 32;
pub const MAX_CSV_IMPORT_BYTES: usize = 4 * 1024 * 1024;
const NOTE_LIMIT: usize = 120;

pub struct ImportFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportStatus {
    Imported,
    Duplicate,
    Preserved,
    Unsupported,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOriginal {
    pub page_id: String,
    pub attachment_id: String,
    pub name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub name: String,
    pub status: ImportStatus,
    pub page_id: Option<String>,
    pub records: usize,
    pub notes: Vec<String>,
    pub notes_omitted: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original: Option<ImportOriginal>,
    #[serde(skip_serializing_if = "is_zero")]
    pub unmatched_links: usize,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

impl ImportOutcome {
    fn new(name: String) -> Self {
        Self {
            name,
            status: ImportStatus::Failed,
            page_id: None,
            records: 0,
            notes: Vec::new(),
            notes_omitted: 0,
            original: None,
            unmatched_links: 0,
        }
    }

    pub fn failed(name: String, message: String) -> Self {
        let mut outcome = Self::new(name);
        outcome.note(message);
        outcome
    }

    fn note(&mut self, message: impl Into<String>) {
        if self.notes.len() < NOTE_LIMIT {
            self.notes.push(message.into());
        } else {
            self.notes_omitted += 1;
        }
    }

    fn duplicate(&mut self, id: String, trashed: bool) {
        self.status = ImportStatus::Duplicate;
        self.page_id = Some(id);
        self.note(if trashed {
            "相同原件已导入到此位置，原导入结果在回收站。"
        } else {
            "相同原件已导入到此位置。"
        });
    }

    fn imported(&mut self, id: String, records: usize) {
        self.status = ImportStatus::Imported;
        self.page_id = Some(id);
        self.records = records;
    }
}

struct Source {
    name: String,
    bytes: Vec<u8>,
    sha256: String,
}

impl Source {
    fn new(file: ImportFile, extensions: &[&str], max_bytes: usize) -> Result<Self> {
        let extension = file
            .name
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase());
        if !valid_attachment_name(&file.name)
            || !extension
                .as_deref()
                .is_some_and(|ext| extensions.contains(&ext))
        {
            return Err(Error::new("IMPORT_FORMAT", "文件名或扩展名不受支持。"));
        }
        if file.bytes.len() > max_bytes {
            return Err(Error::new(
                "IMPORT_SIZE",
                format!("原件大小超过 {} MiB。", max_bytes / (1024 * 1024)),
            ));
        }
        let sha256 = hex_sha256(&Sha256::digest(&file.bytes));
        Ok(Self {
            name: file.name,
            bytes: file.bytes,
            sha256,
        })
    }

    fn stem(&self) -> &str {
        self.name
            .rsplit_once('.')
            .map_or(&self.name, |(stem, _)| stem)
    }

    fn text(&self) -> Result<&str> {
        let bytes = self
            .bytes
            .strip_prefix(b"\xEF\xBB\xBF")
            .unwrap_or(&self.bytes);
        std::str::from_utf8(bytes).map_err(|_| {
            Error::new(
                "IMPORT_ENCODING",
                "文件使用其他编码，请转换为 UTF-8 后重试。",
            )
        })
    }

    fn save(
        &self,
        store: &Store,
        db: &Connection,
        workspace: &str,
        kind: ImportKind,
        destination: &str,
        page: &str,
    ) -> Result<ImportOriginal> {
        let attached: Option<String> = db
            .query_row(
                "SELECT id FROM attachments WHERE page_id = ?1 AND name = ?2 AND sha256 = ?3",
                params![page, self.name, self.sha256],
                |row| row.get(0),
            )
            .optional()?;
        // Even when a page already has this attachment, verify its original before committing.
        let directory = blob_directory(&store.data_root().join(workspace), true)?;
        let (attachment, _) =
            stage_attachment(&directory, page, &self.name, self.bytes.as_slice())?;
        let attachment_id = if let Some(id) = attached {
            id
        } else {
            insert_attachment(db, &attachment)?;
            attachment.id
        };
        write_receipt(db, kind.key(), destination, &self.name, &self.sha256, page)?;
        Ok(ImportOriginal {
            page_id: page.into(),
            attachment_id,
            name: self.name.clone(),
        })
    }

    fn earlier(
        &self,
        db: &Connection,
        kind: ImportKind,
        destination: &str,
    ) -> Result<Option<(String, bool)>> {
        find_receipt(db, kind, destination, &self.name, &self.sha256)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImportKind {
    Markdown,
    CsvNew,
    CsvExisting,
    NotionZip,
    ObsidianVault,
}

impl ImportKind {
    fn key(self) -> &'static str {
        match self {
            Self::Markdown => "markdown",
            Self::CsvNew => "csvNew",
            Self::CsvExisting => "csvExisting",
            Self::NotionZip => "notionZip",
            Self::ObsidianVault => "obsidianVault",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportReceipt {
    pub(crate) kind: String,
    pub(crate) destination: String,
    name: String,
    sha256: String,
    pub(crate) output_page_id: String,
}

pub(crate) fn snapshot_receipts(db: &Connection) -> Result<Vec<ImportReceipt>> {
    let mut query = db.prepare(
        "SELECT kind, destination, name, sha256, output_page_id FROM import_receipts ORDER BY rowid",
    )?;
    Ok(query
        .query_map([], |row| {
            Ok(ImportReceipt {
                kind: row.get(0)?,
                destination: row.get(1)?,
                name: row.get(2)?,
                sha256: row.get(3)?,
                output_page_id: row.get(4)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn restore_receipts(
    db: &Connection,
    receipts: &[ImportReceipt],
    pages: &HashSet<&str>,
    collections: &HashMap<&str, &str>,
) -> Result<()> {
    validate_receipts(receipts, pages, collections)?;
    for receipt in receipts {
        write_receipt(
            db,
            &receipt.kind,
            &receipt.destination,
            &receipt.name,
            &receipt.sha256,
            &receipt.output_page_id,
        )?;
    }
    Ok(())
}

pub(crate) fn validate_receipts(
    receipts: &[ImportReceipt],
    pages: &HashSet<&str>,
    collections: &HashMap<&str, &str>,
) -> Result<()> {
    let mut seen = HashSet::new();
    for receipt in receipts {
        let destination = receipt.destination.as_str();
        let page = receipt.output_page_id.as_str();
        let is_root_or_page = destination.is_empty() || pages.contains(destination);
        let valid_target = match receipt.kind.as_str() {
            "markdown" | "notionZip" | "obsidianVault" => is_root_or_page,
            "csvNew" => is_root_or_page && collections.values().any(|id| *id == page),
            "csvExisting" => collections.get(destination) == Some(&page),
            _ => false,
        };
        if !valid_target
            || !pages.contains(page)
            || !valid_attachment_name(&receipt.name)
            || !valid_hash(&receipt.sha256)
            || !seen.insert((&receipt.kind, destination, &receipt.name, &receipt.sha256))
        {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "工作空间包中有无效或重复的导入凭据。",
            ));
        }
    }
    Ok(())
}

fn write_receipt(
    db: &Connection,
    kind: &str,
    destination: &str,
    name: &str,
    sha256: &str,
    page: &str,
) -> Result<()> {
    db.execute(
        "INSERT INTO import_receipts (kind, destination, name, sha256, output_page_id) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![kind, destination, name, sha256, page],
    )?;
    Ok(())
}

fn find_receipt(
    db: &Connection,
    kind: ImportKind,
    destination: &str,
    name: &str,
    sha256: &str,
) -> Result<Option<(String, bool)>> {
    Ok(db
        .query_row(
            "SELECT p.id, p.trash_root_id IS NOT NULL FROM import_receipts r
         JOIN pages p ON r.output_page_id = p.id
         WHERE r.kind=?1 AND r.destination=?2 AND r.name=?3 AND r.sha256=?4",
            params![kind.key(), destination, name, sha256],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?)
}
