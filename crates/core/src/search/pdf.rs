// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{entry_filter, normalized};
use crate::assets::read_attachment;
use crate::collections::Arguments;
use crate::model::valid_id;
use crate::{Attachment, Error, MAX_PREVIEW_BYTES, Result, Store};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

pub(crate) const MIME: &str = "application/pdf";
const MAX_INDEXED_PAGES: usize = 2048;
const MAX_PAGE_TEXT_BYTES: usize = 256 * 1024;
const MAX_DOCUMENT_TEXT_BYTES: usize = 4 * 1024 * 1024;

const SOURCES: &str = "
    CREATE TABLE pdf_index_state (
        slot INTEGER PRIMARY KEY CHECK(slot = 1),
        generation INTEGER NOT NULL CHECK(generation > 0),
        revision INTEGER NOT NULL CHECK(revision >= 0)
    );
    INSERT INTO pdf_index_state VALUES (1, 1, 0);
    CREATE INDEX attachments_pdf ON attachments(created_at, id)
        WHERE media_type = 'application/pdf';
    CREATE TABLE pdf_index_documents (
        attachment_id TEXT PRIMARY KEY REFERENCES attachments(id) ON DELETE CASCADE,
        sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
        status TEXT NOT NULL CHECK(status IN ('indexed','partial','noText','locked','unreadable','limitReached')),
        page_count INTEGER CHECK(page_count > 0),
        indexed_pages INTEGER NOT NULL CHECK(indexed_pages >= 0),
        revision INTEGER NOT NULL CHECK(revision > 0)
    );
";

const INDEX: &str = "
    CREATE TABLE pdf_text_entries (
        attachment_id TEXT NOT NULL REFERENCES pdf_index_documents(attachment_id) ON DELETE CASCADE,
        page_number INTEGER NOT NULL CHECK(page_number > 0),
        original TEXT NOT NULL,
        normalized TEXT NOT NULL,
        UNIQUE(attachment_id, page_number)
    );
    CREATE VIRTUAL TABLE pdf_text_fts USING fts5(
        normalized, content='pdf_text_entries', tokenize='trigram case_sensitive 1', columnsize=0
    );
    CREATE TRIGGER pdf_text_insert AFTER INSERT ON pdf_text_entries BEGIN
        INSERT INTO pdf_text_fts(rowid, normalized) VALUES(new.rowid, new.normalized);
    END;
    CREATE TRIGGER pdf_text_remove AFTER DELETE ON pdf_text_entries BEGIN
        INSERT INTO pdf_text_fts(pdf_text_fts, rowid, normalized) VALUES('delete', old.rowid, old.normalized);
    END;
";

pub(crate) fn install(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(SOURCES)?;
    tx.execute_batch(INDEX)?;
    Ok(())
}

pub(super) fn rebuild(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DROP TABLE pdf_text_entries;
         DROP TABLE pdf_text_fts;
         DELETE FROM pdf_index_documents;
         UPDATE pdf_index_state SET generation = generation + 1, revision = revision + 1 WHERE slot = 1;",
    )?;
    tx.execute_batch(INDEX)?;
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PdfTarget {
    pub attachment_id: String,
    pub sha256: String,
    pub page_number: u32,
}

impl PdfTarget {
    pub(crate) fn matches(&self, attachment: &Attachment) -> bool {
        attachment.id == self.attachment_id
            && attachment.sha256 == self.sha256
            && attachment.media_type == MIME
    }
}

pub(crate) fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfTextMatch {
    #[serde(flatten)]
    pub target: PdfTarget,
    pub name: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfProgress {
    pub pending: u32,
    pub indexed: u32,
    pub partial: u32,
    pub no_text: u32,
    pub failed: u32,
    pub revision: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfIndexPoll {
    pub progress: PdfProgress,
    pub job: Option<PdfIndexJob>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfIndexJob {
    pub generation: i64,
    pub attachment: Attachment,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PdfDocumentStatus {
    pub attachment_id: String,
    pub status: Option<String>,
    pub page_count: Option<u32>,
    pub indexed_pages: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PdfExtractionStatus {
    Indexed,
    Partial,
    NoText,
    Locked,
    Unreadable,
    LimitReached,
}

impl PdfExtractionStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Indexed => "indexed",
            Self::Partial => "partial",
            Self::NoText => "noText",
            Self::Locked => "locked",
            Self::Unreadable => "unreadable",
            Self::LimitReached => "limitReached",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PdfIndexResult {
    pub generation: i64,
    pub attachment_id: String,
    pub sha256: String,
    pub status: PdfExtractionStatus,
    pub page_count: Option<u32>,
    pub pages: Vec<PdfPageText>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PdfPageText {
    pub page_number: u32,
    pub text: String,
}

fn invalid_result() -> Error {
    Error::new("PDF_INDEX_RESULT", "PDF 文字索引结果的范围或格式无效。")
}

impl PdfIndexResult {
    fn validate(&self) -> Result<()> {
        let with_text = matches!(
            self.status,
            PdfExtractionStatus::Indexed | PdfExtractionStatus::Partial
        );
        if self.generation <= 0
            || !valid_id(&self.attachment_id)
            || !valid_sha256(&self.sha256)
            || with_text == self.pages.is_empty()
            || self.page_count == Some(0)
            || (self.status == PdfExtractionStatus::NoText && self.page_count.is_none())
            || self.pages.len() > MAX_INDEXED_PAGES
        {
            return Err(invalid_result());
        }
        let total = self.page_count.unwrap_or(0);
        let mut previous = 0;
        let mut bytes = 0;
        for page in &self.pages {
            bytes += page.text.len();
            if page.page_number <= previous
                || page.page_number > total
                || page.text.len() > MAX_PAGE_TEXT_BYTES
                || bytes > MAX_DOCUMENT_TEXT_BYTES
                || page.text.trim().is_empty()
                || page.text.contains('\0')
            {
                return Err(invalid_result());
            }
            previous = page.page_number;
        }
        Ok(())
    }
}

// Start from the partial PDF index even in large workspaces without any PDFs.
pub(crate) const LIVE_ATTACHMENTS: &str = "
    FROM attachments a CROSS JOIN pages p ON p.id = a.page_id AND p.trash_root_id IS NULL
    LEFT JOIN pdf_index_documents d ON d.attachment_id = a.id AND d.sha256 = a.sha256
    WHERE a.media_type = 'application/pdf'
";

pub(crate) const PROGRESS_FIELDS: &str = "
    count(*) FILTER (WHERE d.attachment_id IS NULL),
    count(*) FILTER (WHERE d.status = 'indexed'),
    count(*) FILTER (WHERE d.status = 'partial'),
    count(*) FILTER (WHERE d.status = 'noText'),
    count(*) FILTER (WHERE d.status IN ('unreadable','locked','limitReached'))
";

pub(crate) fn read_progress(row: &Row<'_>) -> rusqlite::Result<PdfProgress> {
    Ok(PdfProgress {
        pending: row.get(0)?,
        indexed: row.get(1)?,
        partial: row.get(2)?,
        no_text: row.get(3)?,
        failed: row.get(4)?,
        revision: row.get(5)?,
    })
}

pub(super) fn progress(db: &Connection) -> Result<PdfProgress> {
    Ok(db.query_row(
        &format!(
            "SELECT {PROGRESS_FIELDS}, (SELECT revision FROM pdf_index_state WHERE slot = 1)
             {LIVE_ATTACHMENTS}"
        ),
        [],
        read_progress,
    )?)
}

pub(crate) fn matching_entries(term: &str, arguments: &mut Arguments) -> (&'static str, String) {
    entry_filter(
        term,
        arguments,
        "pdf_text_entries e",
        "pdf_text_fts JOIN pdf_text_entries e ON e.rowid = pdf_text_fts.rowid",
        "pdf_text_fts",
    )
}

// The attachment hash is the document version. Recheck it at every search,
// independently of how near the index is to finishing a rebuild.
pub(crate) const MATCHING_ORIGINAL: &str = "
    JOIN pdf_index_documents d ON d.attachment_id = e.attachment_id
    JOIN attachments a ON a.id = d.attachment_id AND a.sha256 = d.sha256 AND a.media_type = 'application/pdf'
";

impl Store {
    pub fn poll_pdf_index(&self, workspace: &str) -> Result<PdfIndexPoll> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let progress = progress(&tx)?;
        let job = if progress.pending == 0 {
            None
        } else {
            let pending: Option<(String, String, i64)> = tx.query_row(
                &format!("SELECT a.id, a.page_id, (SELECT generation FROM pdf_index_state WHERE slot = 1)
                          {LIVE_ATTACHMENTS} AND d.attachment_id IS NULL ORDER BY a.created_at, a.id LIMIT 1"),
                [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            ).optional()?;
            if let Some((id, page, generation)) = pending {
                Some(PdfIndexJob {
                    generation,
                    attachment: read_attachment(&tx, &page, &id)?,
                })
            } else {
                None
            }
        };
        tx.commit()?;
        Ok(PdfIndexPoll { progress, job })
    }

    pub fn pdf_document_status(
        &self,
        workspace: &str,
        page: &str,
    ) -> Result<Vec<PdfDocumentStatus>> {
        let db = self.connection(workspace)?;
        let mut statement = db.prepare(&format!(
            "SELECT a.id, d.status, d.page_count, COALESCE(d.indexed_pages, 0)
             {LIVE_ATTACHMENTS} AND a.page_id = ?1 ORDER BY a.created_at, a.id"
        ))?;
        Ok(statement
            .query_map([page], |row| {
                Ok(PdfDocumentStatus {
                    attachment_id: row.get(0)?,
                    status: row.get(1)?,
                    page_count: row.get(2)?,
                    indexed_pages: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn finish_pdf_index(&self, workspace: &str, result: PdfIndexResult) -> Result<PdfProgress> {
        result.validate()?;
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let pending_bytes: Option<u32> = tx
            .query_row(
                &format!(
                    "SELECT a.bytes {LIVE_ATTACHMENTS}
                      AND a.id = ?1 AND a.sha256 = ?2 AND d.attachment_id IS NULL
                      AND (SELECT generation FROM pdf_index_state WHERE slot = 1) = ?3"
                ),
                params![result.attachment_id, result.sha256, result.generation],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(bytes) = pending_bytes {
            if u64::from(bytes) > MAX_PREVIEW_BYTES
                && result.status != PdfExtractionStatus::LimitReached
            {
                return Err(invalid_result());
            }
            let revision: i64 = tx.query_row(
                "UPDATE pdf_index_state SET revision = revision + 1 WHERE slot = 1 RETURNING revision",
                [], |row| row.get(0),
            )?;
            tx.execute(
                "DELETE FROM pdf_index_documents WHERE attachment_id = ?1",
                [&result.attachment_id],
            )?;
            tx.execute(
                "INSERT INTO pdf_index_documents(attachment_id, sha256, status, page_count, indexed_pages, revision)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![result.attachment_id, result.sha256, result.status.label(), result.page_count, result.pages.len() as u32, revision],
            )?;
            let mut insert = tx.prepare(
                "INSERT INTO pdf_text_entries(attachment_id,page_number,original,normalized) VALUES(?1,?2,?3,?4)",
            )?;
            for page in result.pages {
                insert.execute(params![
                    result.attachment_id,
                    page.page_number,
                    page.text,
                    normalized(&page.text)
                ])?;
            }
        }
        let status = progress(&tx)?;
        tx.commit()?;
        Ok(status)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::StatementStatus;

    #[test]
    fn progress_work_is_independent_of_non_pdf_pages() {
        let mut db = Connection::open_in_memory().unwrap();
        let tx = db.transaction().unwrap();
        crate::store::initialize_schema(&tx, crate::migrations::VERSION).unwrap();
        tx.commit().unwrap();
        let measure = || {
            let mut statement = db
                .prepare(&format!(
                    "SELECT {PROGRESS_FIELDS}, (SELECT revision FROM pdf_index_state WHERE slot = 1)
                     {LIVE_ATTACHMENTS}"
                ))
                .unwrap();
            let progress = statement.query_row([], read_progress).unwrap();
            assert_eq!(progress.pending, 0);
            assert_eq!(progress.indexed, 0);
            statement.get_status(StatementStatus::VmStep)
        };
        let empty_steps = measure();
        db.execute_batch(
            "WITH RECURSIVE numbers(n) AS (
                VALUES(1) UNION ALL SELECT n + 1 FROM numbers WHERE n < 512
             )
             INSERT INTO pages (id, title, revision, updated_at, icon, blocks, body_text)
             SELECT printf('page-%d', n), 'test page', 1, 0, 'file', '[]', '' FROM numbers;",
        )
        .unwrap();
        let populated_steps = measure();
        assert!(
            populated_steps <= empty_steps + 16,
            "PDF progress used {empty_steps} steps with no pages and {populated_steps} with 512 pages"
        );
    }
}
