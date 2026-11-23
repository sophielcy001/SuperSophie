// SPDX-License-Identifier: AGPL-3.0-or-later

mod links;
mod plan;
mod source;

use super::{ImportKind, ImportOutcome, find_receipt, write_receipt};
use crate::assets::blob_directory;
use crate::model::new_id;
use crate::store::{ensure_active_info, new_page, read_page_info};
use crate::{Error, Result, Store};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use source::{Inventory, attachment_name, stem};
use std::{
    io::{Read, Seek},
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationBatch {
    pub id: String,
    pub kind: String,
    pub source: String,
    pub status: String,
    pub page_id: Option<String>,
    pub detail: Option<String>,
}

impl Store {
    pub fn import_notion_zip(
        &self,
        workspace: &str,
        parent: Option<&str>,
        name: &str,
        input: impl Read + Seek,
        cancel: &AtomicBool,
    ) -> Result<Vec<ImportOutcome>> {
        self.run_migration(
            workspace,
            parent,
            ImportKind::NotionZip,
            name,
            cancel,
            |inventory, name| inventory.notion_archive(name, input),
        )
    }

    pub fn import_obsidian_vault(
        &self,
        workspace: &str,
        parent: Option<&str>,
        root: &Path,
        cancel: &AtomicBool,
    ) -> Result<Vec<ImportOutcome>> {
        let name = root
            .file_name()
            .and_then(|part| part.to_str())
            .unwrap_or("Obsidian");
        self.run_migration(
            workspace,
            parent,
            ImportKind::ObsidianVault,
            name,
            cancel,
            |inventory, _| inventory.vault(root),
        )
    }

    pub fn latest_migration(&self, workspace: &str) -> Result<Option<MigrationBatch>> {
        self.read_latest_migration(workspace, None)
    }

    pub fn latest_migration_for(
        &self,
        workspace: &str,
        native_content: bool,
    ) -> Result<Option<MigrationBatch>> {
        self.read_latest_migration(workspace, Some(native_content))
    }

    fn read_latest_migration(
        &self,
        workspace: &str,
        native_content: Option<bool>,
    ) -> Result<Option<MigrationBatch>> {
        let db = self.connection(workspace)?;
        Ok(db.query_row(
            "SELECT id, kind, source,
                    CASE WHEN status='preparing' AND session_id<>?1 THEN 'interrupted' ELSE status END,
                    output_page_id, detail
             FROM migration_batches
             WHERE ?2 IS NULL OR (kind = 'nativeContent') = ?2
             ORDER BY rowid DESC LIMIT 1",
            params![self.session_id, native_content],
            |row| Ok(MigrationBatch {
                id: row.get(0)?, kind: row.get(1)?, source: row.get(2)?, status: row.get(3)?,
                page_id: row.get(4)?, detail: row.get(5)?,
            }),
        ).optional()?)
    }

    fn run_migration(
        &self,
        workspace: &str,
        parent: Option<&str>,
        kind: ImportKind,
        name: &str,
        cancel: &AtomicBool,
        load: impl FnOnce(&mut Inventory<'_>, &str) -> Result<()>,
    ) -> Result<Vec<ImportOutcome>> {
        let filename = attachment_name(name);
        let title = if kind == ImportKind::NotionZip {
            stem(&filename)
        } else {
            filename.as_str()
        };
        let mut db = self.connection(workspace)?;
        let _lease = self.pin_blobs()?;
        validate_parent(&db, parent)?;
        let id = begin_batch(&db, &self.session_id, kind.key(), &filename)?;
        let result = (|| {
            check_cancel(cancel)?;
            let root = new_page(parent, &plan::page_title(title), "folder")?;
            let root_id = root.info.id.clone();
            let directory = blob_directory(&self.data_root().join(workspace), true)?;
            let mut inventory = Inventory::new(&directory, &root_id, cancel);
            load(&mut inventory, &filename)?;
            check_cancel(cancel)?;
            if inventory.files.is_empty() && inventory.container.is_none() {
                return Err(migration_error("未找到可导入或留存的文件。"));
            }
            let fingerprint = inventory.fingerprint();
            let destination = parent.unwrap_or_default();
            let mut batch = ImportOutcome::new(filename.clone());
            let mut migration = plan::Plan::new(root, &inventory, kind, cancel)?;
            check_cancel(cancel)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            validate_parent(&tx, parent)?;
            if let Some((previous, trashed)) =
                find_receipt(&tx, kind, destination, &filename, &fingerprint)?
            {
                batch.duplicate(previous.clone(), trashed);
                complete_batch(&tx, &id, &previous, None)?;
                tx.commit()?;
                return Ok(vec![batch]);
            }
            batch.original = migration.container_original.take();
            if batch.original.is_none() && kind == ImportKind::NotionZip {
                batch.note("完整 ZIP 超过单附件上限。包中可读取的原件分别留存。");
            }
            if inventory.parts > 0 {
                batch.note(format!("已合并 {} 个 Notion 导出分卷。", inventory.parts));
            }
            let mut entries = migration.publish(&tx, cancel)?;
            check_cancel(cancel)?;
            write_receipt(
                &tx,
                kind.key(),
                destination,
                &filename,
                &fingerprint,
                &root_id,
            )?;
            complete_batch(&tx, &id, &root_id, None)?;
            tx.commit()?;
            batch.imported(root_id.clone(), 0);
            batch.note("已将迁移内容放入同一个页面分支。");
            let mut report = vec![batch];
            report.append(&mut entries);
            report.append(&mut inventory.skipped);
            bound_report(&mut report);
            Ok(report)
        })();
        if let Err(error) = &result {
            fail_batch(&db, &id, error);
        }
        result
    }
}

pub(crate) fn begin_batch(
    db: &Connection,
    session: &str,
    kind: &str,
    source: &str,
) -> Result<String> {
    let id = new_id();
    db.execute(
        "INSERT INTO migration_batches(id, session_id, kind, source, status)
         VALUES (?1, ?2, ?3, ?4, 'preparing')",
        params![id, session, kind, attachment_name(source)],
    )?;
    Ok(id)
}

pub(crate) fn complete_batch(
    db: &Connection,
    id: &str,
    page: &str,
    detail: Option<&str>,
) -> Result<()> {
    db.execute(
        "UPDATE migration_batches SET status='completed', output_page_id=?2, detail=?3 WHERE id=?1",
        params![id, page, detail],
    )?;
    Ok(())
}

pub(crate) fn fail_batch(db: &Connection, id: &str, error: &Error) {
    let status = if error.code == "IMPORT_CANCELLED" {
        "cancelled"
    } else {
        "failed"
    };
    // If the disk is full, the preparing row is reported as interrupted on reopen.
    let _ = db.execute(
        "UPDATE migration_batches SET status=?2, detail=?3 WHERE id=?1",
        params![id, status, error.message],
    );
}

fn validate_parent(db: &Connection, parent: Option<&str>) -> Result<()> {
    if let Some(id) = parent {
        ensure_active_info(&read_page_info(db, id)?)?;
    }
    Ok(())
}

fn bound_report(report: &mut [ImportOutcome]) {
    let mut remaining = 2 * 1024 * 1024;
    for entry in report {
        let total = entry.notes.len();
        entry.notes.retain(|note| {
            if note.len() > remaining {
                return false;
            }
            remaining -= note.len();
            true
        });
        entry.notes_omitted += total - entry.notes.len();
    }
}

fn migration_error(message: impl Into<String>) -> Error {
    Error::new("MIGRATION_FORMAT", message)
}

fn cancelled() -> Error {
    Error::new("IMPORT_CANCELLED", "迁移已取消。再次选择来源即可重试。")
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err(cancelled())
    } else {
        Ok(())
    }
}
