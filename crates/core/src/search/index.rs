// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{SearchIndexStatus, normalized};
use crate::{Block, Result, Store};
use rusqlite::{Connection, Statement, Transaction, TransactionBehavior, params};

const BATCH_PAGES: u32 = 48;
const BATCH_BYTES: usize = 2 * 1024 * 1024;

const QUEUE_SCHEMA: &str = "
    CREATE TABLE search_pending (
        page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
        priority INTEGER NOT NULL
    );
    CREATE INDEX search_pending_priority ON search_pending(priority DESC);
    CREATE TRIGGER search_page_insert AFTER INSERT ON pages BEGIN
        INSERT INTO search_pending(page_id, priority) VALUES(new.id, 1);
    END;
    CREATE TRIGGER search_page_revision AFTER UPDATE OF revision ON pages
    WHEN new.revision != old.revision BEGIN
        INSERT INTO search_pending(page_id, priority) VALUES(new.id, 1)
        ON CONFLICT(page_id) DO UPDATE SET priority = 1;
    END;
";

const INDEX_SCHEMA: &str = "
    CREATE TABLE search_entries (
        page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
        revision INTEGER NOT NULL,
        block_id TEXT,
        position INTEGER NOT NULL,
        original TEXT NOT NULL,
        normalized TEXT NOT NULL,
        UNIQUE(page_id, position)
    );
    CREATE VIRTUAL TABLE search_fts USING fts5(
        normalized, content='search_entries', tokenize='trigram case_sensitive 1', columnsize=0
    );
    CREATE TRIGGER search_entry_insert AFTER INSERT ON search_entries BEGIN
        INSERT INTO search_fts(rowid, normalized) VALUES(new.rowid, new.normalized);
    END;
    CREATE TRIGGER search_entry_remove AFTER DELETE ON search_entries BEGIN
        INSERT INTO search_fts(search_fts, rowid, normalized) VALUES('delete', old.rowid, old.normalized);
    END;
";

pub(crate) fn install(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(QUEUE_SCHEMA)?;
    tx.execute_batch(INDEX_SCHEMA)?;
    enqueue_all(tx)
}

fn enqueue_all(tx: &Transaction<'_>) -> Result<()> {
    tx.execute(
        "INSERT INTO search_pending(page_id, priority)
         SELECT id, 0 FROM pages WHERE trash_root_id IS NULL ORDER BY rowid",
        [],
    )?;
    Ok(())
}

pub(super) fn status(db: &Connection) -> Result<SearchIndexStatus> {
    Ok(SearchIndexStatus {
        pending: db.query_row("SELECT count(*) FROM search_pending", [], |row| row.get(0))?,
        pdf: super::pdf::progress(db)?,
    })
}

impl Store {
    pub fn advance_search_index(&self, workspace: &str) -> Result<SearchIndexStatus> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        apply_batch(&tx)?;
        let progress = status(&tx)?;
        tx.commit()?;
        Ok(progress)
    }

    pub fn rebuild_search_index(&self, workspace: &str) -> Result<SearchIndexStatus> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute_batch(
            "DROP TRIGGER IF EXISTS search_page_insert;
             DROP TRIGGER IF EXISTS search_page_revision;
             DROP TABLE IF EXISTS search_entries;
             DROP TABLE IF EXISTS search_fts;
             DROP TABLE IF EXISTS search_pending;",
        )?;
        install(&tx)?;
        super::pdf::rebuild(&tx)?;
        let progress = status(&tx)?;
        tx.commit()?;
        Ok(progress)
    }
}

struct Job {
    id: String,
    revision: u32,
    title: String,
    blocks: Option<String>,
}

fn take_batch(tx: &Transaction<'_>) -> Result<Vec<Job>> {
    let mut statement = tx.prepare(
        "SELECT p.id, p.revision, p.title,
                CASE WHEN p.trash_root_id IS NULL THEN p.blocks END
         FROM search_pending q JOIN pages p ON p.id = q.page_id
         ORDER BY q.priority DESC, q.rowid LIMIT ?1",
    )?;
    let mut rows = statement.query([BATCH_PAGES])?;
    let mut batch = Vec::new();
    let mut bytes = 0;
    while let Some(row) = rows.next()? {
        let blocks: Option<String> = row.get(3)?;
        bytes += blocks.as_ref().map_or(0, String::len);
        if bytes > BATCH_BYTES && !batch.is_empty() {
            break;
        }
        batch.push(Job {
            id: row.get(0)?,
            revision: row.get(1)?,
            title: row.get(2)?,
            blocks,
        });
    }
    Ok(batch)
}

fn apply_batch(tx: &Transaction<'_>) -> Result<()> {
    let batch = take_batch(tx)?;
    let mut entries = tx.prepare(
        "INSERT INTO search_entries(page_id, revision, block_id, position, original, normalized)
         VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    let mut remove = tx.prepare("DELETE FROM search_entries WHERE page_id = ?1")?;
    let mut complete = tx.prepare("DELETE FROM search_pending WHERE page_id = ?1")?;
    for job in batch {
        remove.execute([&job.id])?;
        if let Some(data) = &job.blocks {
            let blocks: Vec<Block> = serde_json::from_str(data)?;
            let mut indexer = PageIndexer {
                entries: &mut entries,
                job: &job,
                position: 0,
            };
            indexer.insert(None, &job.title)?;
            indexer.walk(&blocks)?;
        }
        complete.execute([&job.id])?;
    }
    Ok(())
}

struct PageIndexer<'a, 'db> {
    entries: &'a mut Statement<'db>,
    job: &'a Job,
    position: u32,
}

impl PageIndexer<'_, '_> {
    fn insert(&mut self, block_id: Option<&str>, text: &str) -> Result<()> {
        self.entries.execute(params![
            self.job.id,
            self.job.revision,
            block_id,
            self.position,
            text,
            normalized(text),
        ])?;
        self.position += 1;
        Ok(())
    }

    fn walk(&mut self, blocks: &[Block]) -> Result<()> {
        for block in blocks {
            if let Some(text) = crate::rich_blocks::image(block)
                .map(|image| image.alt)
                .or_else(|| crate::rich_blocks::toggle_title(block))
                .or_else(|| crate::rich_blocks::latex(block))
                .or_else(|| crate::rich_blocks::bookmark(block).and_then(|(_, title)| title))
                .or_else(|| block.payload.get("text").and_then(|value| value.as_str()))
            {
                if !text.trim().is_empty() {
                    self.insert(Some(&block.id), text)?;
                }
            } else if let Some(items) = block
                .payload
                .get("inline")
                .and_then(|value| value.as_array())
            {
                let mut text = String::new();
                for item in items {
                    if item.get("type").and_then(|value| value.as_str()) == Some("hardBreak") {
                        text.push('\n');
                    } else if let Some(value) = item
                        .get("text")
                        .or_else(|| item.get("label"))
                        .and_then(|value| value.as_str())
                    {
                        text.push_str(value);
                    }
                }
                if !text.trim().is_empty() {
                    self.insert(Some(&block.id), &text)?;
                }
            }
            self.walk(&block.children)?;
        }
        Ok(())
    }
}
