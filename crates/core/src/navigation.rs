// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::model::valid_id;
use crate::store::{create_page_in, ensure_active, now, read_page};
use crate::{Error, Page, Result, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

const RECENT_LIMIT: usize = 50;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Navigation {
    pub inbox_page_id: Option<String>,
    pub favorites: Vec<String>,
    pub recent_pages: Vec<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct NavigationSnapshot {
    pub(crate) inbox_page_id: Option<String>,
    pub(crate) favorites: Vec<PageEntry>,
    pub(crate) recent_pages: Vec<PageEntry>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PageEntry {
    pub(crate) page_id: String,
    timestamp: i64,
}

impl Store {
    pub fn navigation(&self, workspace: &str) -> Result<Navigation> {
        read_navigation(&self.connection(workspace)?)
    }

    pub fn set_favorite(&self, workspace: &str, page: &str, favorite: bool) -> Result<Navigation> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if favorite {
            ensure_active(&read_page(&tx, page)?)?;
            tx.execute(
                "INSERT INTO favorites VALUES (?1, ?2) ON CONFLICT(page_id) DO NOTHING",
                params![page, now()],
            )?;
        } else {
            tx.execute("DELETE FROM favorites WHERE page_id = ?1", [page])?;
        }
        let state = read_navigation(&tx)?;
        tx.commit()?;
        Ok(state)
    }

    pub fn visit_page(&self, workspace: &str, page: &str) -> Result<Navigation> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_active(&read_page(&tx, page)?)?;
        // Refresh insertion order even when two visits share the same timestamp.
        tx.execute("DELETE FROM recent_pages WHERE page_id = ?1", [page])?;
        tx.execute(
            "INSERT INTO recent_pages VALUES (?1, ?2)",
            params![page, now()],
        )?;
        tx.execute(
            "DELETE FROM recent_pages WHERE rowid IN
             (SELECT rowid FROM recent_pages ORDER BY rowid DESC LIMIT -1 OFFSET ?1)",
            [RECENT_LIMIT as u32],
        )?;
        let state = read_navigation(&tx)?;
        tx.commit()?;
        Ok(state)
    }

    pub fn set_inbox(&self, workspace: &str, page: &str) -> Result<Navigation> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        ensure_active(&read_page(&tx, page)?)?;
        write_inbox(&tx, page)?;
        let state = read_navigation(&tx)?;
        tx.commit()?;
        Ok(state)
    }

    pub fn open_inbox(&self, workspace: &str) -> Result<Page> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let page = ensure_inbox(&tx)?;
        tx.commit()?;
        Ok(page)
    }

    pub fn capture_page(&self, workspace: &str, title: &str) -> Result<Page> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let inbox = ensure_inbox(&tx)?;
        let page = create_page_in(&tx, Some(&inbox.info.id), title, "file")?;
        tx.commit()?;
        Ok(page)
    }
}

fn inbox(db: &Connection, active: bool) -> Result<Option<String>> {
    let stored = db
        .query_row(
            "SELECT page_id FROM workspace_inbox WHERE slot = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if !active {
        return Ok(stored);
    }
    if let Some(stored) = stored {
        return Ok(db
            .query_row(
                "SELECT id FROM pages WHERE id = ?1 AND trash_root_id IS NULL",
                [&stored],
                |row| row.get(0),
            )
            .optional()?);
    }
    // Older ZIP files may contain a starter inbox. Search root pages even when
    // most of the workspace is active and has no matching inbox.
    Ok(db
        .query_row(
            "SELECT id FROM pages INDEXED BY pages_parent WHERE parent_id IS NULL AND icon = 'inbox'
             AND trash_root_id IS NULL ORDER BY rowid LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?)
}

fn write_inbox(db: &Connection, id: &str) -> Result<()> {
    db.execute(
        "INSERT INTO workspace_inbox VALUES (1, ?1)
         ON CONFLICT(slot) DO UPDATE SET page_id = excluded.page_id",
        [id],
    )?;
    Ok(())
}

fn ensure_inbox(tx: &Transaction<'_>) -> Result<Page> {
    let page = match inbox(tx, true)? {
        Some(id) => read_page(tx, &id)?,
        None => create_page_in(tx, None, "收集箱", "inbox")?,
    };
    write_inbox(tx, &page.info.id)?;
    Ok(page)
}

fn read_navigation(db: &Connection) -> Result<Navigation> {
    let active = |sql| -> Result<Vec<String>> {
        let mut query = db.prepare(sql)?;
        Ok(query
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?)
    };
    Ok(Navigation {
        inbox_page_id: inbox(db, true)?,
        favorites: active(
            "SELECT f.page_id FROM favorites f
             WHERE EXISTS (SELECT 1 FROM pages p WHERE p.id = f.page_id AND p.trash_root_id IS NULL)
             ORDER BY f.rowid DESC",
        )?,
        recent_pages: active(
            "SELECT r.page_id FROM recent_pages r
             WHERE EXISTS (SELECT 1 FROM pages p WHERE p.id = r.page_id AND p.trash_root_id IS NULL)
             ORDER BY r.rowid DESC",
        )?,
    })
}

pub(crate) fn snapshot_navigation(db: &Connection) -> Result<NavigationSnapshot> {
    let entries = |sql| -> Result<Vec<PageEntry>> {
        let mut query = db.prepare(sql)?;
        Ok(query
            .query_map([], |row| {
                Ok(PageEntry {
                    page_id: row.get(0)?,
                    timestamp: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    };
    Ok(NavigationSnapshot {
        inbox_page_id: inbox(db, false)?,
        favorites: entries("SELECT page_id, added_at FROM favorites ORDER BY rowid")?,
        recent_pages: entries("SELECT page_id, visited_at FROM recent_pages ORDER BY rowid")?,
    })
}

pub(crate) fn restore_navigation(
    db: &Connection,
    snapshot: &NavigationSnapshot,
    pages: &HashSet<&str>,
) -> Result<()> {
    validate_navigation(snapshot, pages)?;
    if let Some(id) = &snapshot.inbox_page_id {
        write_inbox(db, id)?;
    }
    for (items, sql) in [
        (&snapshot.favorites, "INSERT INTO favorites VALUES (?1, ?2)"),
        (
            &snapshot.recent_pages,
            "INSERT INTO recent_pages VALUES (?1, ?2)",
        ),
    ] {
        let mut insert = db.prepare(sql)?;
        for entry in items {
            insert.execute(params![entry.page_id, entry.timestamp])?;
        }
    }
    Ok(())
}

pub(crate) fn validate_navigation(
    snapshot: &NavigationSnapshot,
    pages: &HashSet<&str>,
) -> Result<()> {
    if snapshot.recent_pages.len() > RECENT_LIMIT {
        return Err(invalid_snapshot());
    }
    if snapshot
        .inbox_page_id
        .as_deref()
        .is_some_and(|id| !pages.contains(id))
    {
        return Err(invalid_snapshot());
    }
    for items in [&snapshot.favorites, &snapshot.recent_pages] {
        let mut seen = HashSet::new();
        for entry in items {
            if entry.timestamp < 0
                || !valid_id(&entry.page_id)
                || !pages.contains(entry.page_id.as_str())
                || !seen.insert(&entry.page_id)
            {
                return Err(invalid_snapshot());
            }
        }
    }
    Ok(())
}

fn invalid_snapshot() -> Error {
    Error::new(
        "INVALID_BUNDLE",
        "工作空间包中有无效的收藏、收集箱或最近访问设置。",
    )
}
