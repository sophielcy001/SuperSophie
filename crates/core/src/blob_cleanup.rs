// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::assets::{blob_directory, hex_sha256, valid_hash};
use crate::store::now;
use crate::{Error, Result, Store};
use rusqlite::{Transaction, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    fs, io,
    path::{Path, PathBuf},
    sync::{RwLockReadGuard, RwLockWriteGuard, TryLockError},
};

const QUARANTINE_MILLIS: i64 = 7 * 24 * 60 * 60 * 1000;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobCleanupPreview {
    pub waiting: usize,
    pub files: usize,
    pub bytes: u64,
    pub confirmation: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobCleanup {
    pub removed: usize,
    pub bytes: u64,
    pub failed: usize,
}

#[derive(Serialize)]
struct Candidate {
    bytes: u64,
    since: i64,
}

struct Inventory {
    eligible: BTreeMap<String, Candidate>,
    waiting: usize,
}

impl Inventory {
    fn read(tx: &Transaction<'_>, directory: Option<&Path>, time: i64) -> Result<Self> {
        let mut existing = tx.prepare("SELECT sha256, unreferenced_since FROM blob_quarantine")?;
        let mut existing = existing
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<String, i64>>>()?;
        let mut inventory = Self {
            eligible: BTreeMap::new(),
            waiting: 0,
        };
        if let Some(directory) = directory {
            let mut references = tx.prepare(
                "SELECT sha256 FROM attachments
                 UNION SELECT json_extract(a.value, '$.sha256')
                 FROM templates t, json_each(t.snapshot) p, json_each(p.value, '$.attachments') a",
            )?;
            let references = references
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<HashSet<_>>>()?;
            let mut mark = tx.prepare("INSERT INTO blob_quarantine VALUES (?1, ?2)")?;
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                let Ok(hash) = entry.file_name().into_string() else {
                    continue;
                };
                if !valid_hash(&hash) || references.contains(&hash) || !entry.file_type()?.is_file()
                {
                    continue;
                }
                let since = match existing.remove(&hash) {
                    Some(since) => since,
                    None => {
                        mark.execute(params![hash, time])?;
                        time
                    }
                };
                if time.saturating_sub(since) >= QUARANTINE_MILLIS {
                    inventory.eligible.insert(
                        hash,
                        Candidate {
                            bytes: entry.metadata()?.len(),
                            since,
                        },
                    );
                } else {
                    inventory.waiting += 1;
                }
            }
        }
        let mut clear = tx.prepare("DELETE FROM blob_quarantine WHERE sha256 = ?1")?;
        for hash in existing.keys() {
            clear.execute([hash])?;
        }
        Ok(inventory)
    }

    fn confirmation(&self, workspace: &str) -> Result<String> {
        Ok(hex_sha256(&Sha256::digest(serde_json::to_vec(&(
            "cleanBlobs",
            workspace,
            &self.eligible,
        ))?)))
    }
}

impl Store {
    pub(crate) fn pin_blobs(&self) -> Result<RwLockReadGuard<'_, ()>> {
        self.blob_gate.read().map_err(|_| blob_state())
    }

    fn lock_for_cleanup(&self) -> Result<RwLockWriteGuard<'_, ()>> {
        self.blob_gate.try_write().map_err(|error| match error {
            TryLockError::WouldBlock => {
                Error::new("BLOBS_BUSY", "正在读取或写入附件原件，请稍后重试。")
            }
            TryLockError::Poisoned(_) => blob_state(),
        })
    }

    pub fn preview_blob_cleanup(&self, workspace: &str) -> Result<BlobCleanupPreview> {
        let mut db = self.connection(workspace)?;
        let _lease = self.lock_for_cleanup()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let directory = existing_blobs(&self.data_root().join(workspace))?;
        let inventory = Inventory::read(&tx, directory.as_deref(), now())?;
        let preview = BlobCleanupPreview {
            waiting: inventory.waiting,
            files: inventory.eligible.len(),
            bytes: inventory.eligible.values().map(|item| item.bytes).sum(),
            confirmation: inventory.confirmation(workspace)?,
        };
        tx.commit()?;
        Ok(preview)
    }

    pub fn clean_blobs(&self, workspace: &str, confirmation: &str) -> Result<BlobCleanup> {
        let mut db = self.connection(workspace)?;
        let _lease = self.lock_for_cleanup()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let directory = existing_blobs(&self.data_root().join(workspace))?;
        let inventory = Inventory::read(&tx, directory.as_deref(), now())?;
        if inventory.confirmation(workspace)? != confirmation {
            return Err(Error::new(
                "BLOBS_CHANGED",
                "可清理的原件已变化，请重新检查后确认。",
            ));
        }
        let mut result = BlobCleanup::default();
        if let Some(directory) = directory {
            let mut clear = tx.prepare("DELETE FROM blob_quarantine WHERE sha256 = ?1")?;
            for (hash, candidate) in inventory.eligible {
                match fs::remove_file(directory.join(&hash)) {
                    Ok(()) => {
                        clear.execute([hash])?;
                        result.removed += 1;
                        result.bytes += candidate.bytes;
                    }
                    Err(_) => result.failed += 1,
                }
            }
        }
        tx.commit()?;
        Ok(result)
    }
}

fn existing_blobs(workspace: &Path) -> Result<Option<PathBuf>> {
    match fs::symlink_metadata(workspace.join("blobs")) {
        Ok(_) => blob_directory(workspace, false).map(Some),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn blob_state() -> Error {
    Error::new("STORE_STATE", "附件服务需要重新启动。")
}
