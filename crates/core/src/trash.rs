// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::assets::hex_sha256;
use crate::collections::{read_collection_schema, relations::remove_deleted_targets};
use crate::integrations::feed::RangeSnapshot;
use crate::store::{commit_removed_relations, read_page};
use crate::{Error, Result, Store};
use rusqlite::{Connection, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashPreview {
    pub pages: usize,
    pub branches: usize,
    pub affected_records: usize,
    pub templates: usize,
    pub confirmation: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrashRemoval {
    pub pages: usize,
    pub updated_records: usize,
}

#[derive(Serialize)]
struct TrashSnapshot {
    pages: Vec<(String, String, u32)>,
    affected_records: Vec<(String, String, u32)>,
    templates: Vec<String>,
}

impl TrashSnapshot {
    fn read(db: &Connection) -> Result<Self> {
        let mut pages = db.prepare(
            "SELECT id, trash_root_id, revision FROM pages
             WHERE trash_root_id IS NOT NULL ORDER BY id",
        )?;
        let mut records = db.prepare(
            "SELECT DISTINCT source.id, r.collection_id, source.revision
             FROM pages target JOIN relation_edges e ON e.target_page_id = target.id
             JOIN collection_records r ON r.page_id = e.source_page_id
             JOIN pages source ON source.id = r.page_id
             WHERE target.trash_root_id IS NOT NULL AND source.trash_root_id IS NULL
             ORDER BY source.id",
        )?;
        let mut templates = db.prepare(
            "SELECT t.id FROM templates t JOIN collections c ON c.id = t.collection_id
             JOIN pages p ON p.id = c.container_page_id
             WHERE p.trash_root_id IS NOT NULL ORDER BY t.id",
        )?;
        Ok(Self {
            pages: pages
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?,
            affected_records: records
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?,
            templates: templates
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
        })
    }

    fn confirmation(&self, workspace: &str) -> Result<String> {
        let contents = serde_json::to_vec(&("emptyTrash", workspace, self))?;
        Ok(hex_sha256(&Sha256::digest(contents)))
    }
}

impl Store {
    pub fn preview_empty_trash(&self, workspace: &str) -> Result<TrashPreview> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let snapshot = TrashSnapshot::read(&tx)?;
        Ok(TrashPreview {
            pages: snapshot.pages.len(),
            branches: snapshot
                .pages
                .iter()
                .filter(|(id, root, _)| id == root)
                .count(),
            affected_records: snapshot.affected_records.len(),
            templates: snapshot.templates.len(),
            confirmation: snapshot.confirmation(workspace)?,
        })
    }

    pub fn empty_trash(&self, workspace: &str, confirmation: &str) -> Result<TrashRemoval> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let snapshot = TrashSnapshot::read(&tx)?;
        if snapshot.confirmation(workspace)? != confirmation {
            return Err(Error::new(
                "TRASH_CHANGED",
                "回收站或相关记录已变化，请重新检查后确认。",
            ));
        }
        if snapshot.pages.is_empty() {
            return Ok(TrashRemoval {
                pages: 0,
                updated_records: 0,
            });
        }
        let coverage = RangeSnapshot::capture(&tx)?;
        tx.execute_batch(
            "INSERT INTO deleted_collections
             SELECT c.id FROM collections c JOIN pages p ON p.id = c.container_page_id
             WHERE p.trash_root_id IS NOT NULL;
             INSERT INTO deleted_records
             SELECT r.page_id, r.collection_id FROM collection_records r JOIN pages p ON p.id = r.page_id
             WHERE p.trash_root_id IS NOT NULL;",
        )?;
        let mut schemas = HashMap::new();
        for (page_id, collection_id, _) in &snapshot.affected_records {
            let schema = match schemas.entry(collection_id) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(read_collection_schema(&tx, collection_id)?)
                }
            };
            let mut page = read_page(&tx, page_id)?;
            let values = page
                .properties
                .as_mut()
                .ok_or_else(|| Error::new("RELATION_SCHEMA", "关联记录缺少属性。"))?;
            remove_deleted_targets(&tx, &schema.properties, values)?;
            commit_removed_relations(&tx, &mut page)?;
        }
        tx.execute_batch(include_str!("trash.sql"))?;
        // An archived object's tombstone cannot be served once its ancestry
        // is gone. Reuse the scope-change protocol to reconcile those clients.
        coverage.finish(&tx)?;
        tx.commit()?;
        Ok(TrashRemoval {
            pages: snapshot.pages.len(),
            updated_records: snapshot.affected_records.len(),
        })
    }
}
