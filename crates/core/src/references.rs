// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::model::valid_id;
use crate::store::{ensure_active_info, read_page, read_page_info};
use crate::{Block, Error, Result, Store};
use rusqlite::{Connection, Transaction, params};
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Clone, Copy)]
pub(crate) struct BlockTarget<'a> {
    pub page_id: &'a str,
    pub block_id: &'a str,
}

pub(crate) fn block_target(block: &Block) -> Option<BlockTarget<'_>> {
    if block.kind != "blockReference"
        || block.schema_version != 1
        || !block.extra.is_empty()
        || !block.children.is_empty()
        || block.payload.as_object()?.len() != 2
    {
        return None;
    }
    Some(BlockTarget {
        page_id: block.payload["pageId"].as_str().filter(|id| valid_id(id))?,
        block_id: block.payload["blockId"]
            .as_str()
            .filter(|id| valid_id(id))?,
    })
}

pub(crate) fn walk(blocks: &[Block]) -> impl Iterator<Item = &Block> {
    let mut stack = vec![blocks.iter()];
    std::iter::from_fn(move || {
        while let Some(level) = stack.last_mut() {
            if let Some(block) = level.next() {
                if !block.children.is_empty() {
                    stack.push(block.children.iter());
                }
                return Some(block);
            }
            stack.pop();
        }
        None
    })
}

pub(crate) fn index_page(db: &Connection, page_id: &str, blocks: &[Block]) -> Result<()> {
    db.execute("DELETE FROM page_links WHERE page_id = ?1", [page_id])?;
    let mut insert = db.prepare_cached("INSERT INTO page_links VALUES (?1, ?2, ?3, ?4)")?;
    for block in crate::integrations::visible_blocks(blocks) {
        if let Some(target) = block_target(block) {
            insert.execute(params![page_id, block.id, target.page_id, target.block_id])?;
        }
        if let Some(items) = crate::integrations::reference_inline(block) {
            let targets: BTreeSet<_> = items
                .iter()
                .filter(|item| item["type"] == "pageReference")
                .filter_map(|item| item["pageId"].as_str())
                .collect();
            for target in targets {
                insert.execute(params![page_id, block.id, target, ""])?;
            }
        }
    }
    Ok(())
}

pub(crate) fn migrate(db: &Transaction<'_>) -> Result<()> {
    db.execute_batch(
        "ALTER TABLE block_ids ADD COLUMN reference_anchor INTEGER NOT NULL DEFAULT 0
            CHECK (reference_anchor IN (0, 1, 2));
        CREATE TABLE page_links (
            page_id TEXT NOT NULL REFERENCES pages(id),
            block_id TEXT NOT NULL,
            target_page_id TEXT NOT NULL,
            target_block_id TEXT NOT NULL,
            PRIMARY KEY (page_id, block_id, target_page_id, target_block_id)
        ) WITHOUT ROWID;
        CREATE INDEX page_links_target ON page_links(target_page_id, page_id, block_id, target_block_id);",
    )?;
    let mut pages = db.prepare("SELECT id, blocks FROM pages")?;
    let mut update =
        db.prepare("UPDATE block_ids SET reference_anchor = ?1 WHERE id = ?2 AND page_id = ?3")?;
    let mut rows = pages.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        let blocks: Vec<Block> = serde_json::from_str(&row.get::<_, String>(1)?)?;
        for block in crate::integrations::source_anchors(&blocks) {
            update.execute(params![anchor_kind(block), block.id, id])?;
        }
        index_page(db, &id, &blocks)?;
    }
    db.pragma_update(None, "user_version", 9)?;
    Ok(())
}

pub(crate) fn anchor_kind(block: &Block) -> i32 {
    if matches!(block.kind.as_str(), "attachmentImage" | "attachmentFile") {
        2
    } else {
        1
    }
}

#[derive(Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum DesktopReference {
    Available {
        revision: u32,
        title: String,
        block: Block,
    },
    Unavailable {
        revision: Option<u32>,
    },
    Unchanged,
}

#[derive(Debug, Serialize)]
pub struct Backlink {
    pub page_id: String,
    pub block_id: String,
    pub title: String,
    pub icon: String,
    pub target_kind: &'static str,
}

#[derive(Debug, Serialize)]
pub struct BacklinkPage {
    pub items: Vec<Backlink>,
    pub next_cursor: Option<String>,
}

impl Store {
    pub fn desktop_reference(
        &self,
        workspace: &str,
        page_id: &str,
        block_id: &str,
        known_revision: Option<u32>,
    ) -> Result<DesktopReference> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let info = match read_page_info(&tx, page_id) {
            Ok(info) if info.trash_root_id.is_none() => info,
            Ok(_) => return Ok(DesktopReference::Unavailable { revision: None }),
            Err(error) if error.code == "PAGE_NOT_FOUND" => {
                return Ok(DesktopReference::Unavailable { revision: None });
            }
            Err(error) => return Err(error),
        };
        let revision = info.revision;
        if Some(revision) == known_revision {
            return Ok(DesktopReference::Unchanged);
        }
        let page = read_page(&tx, page_id)?;
        let block =
            crate::integrations::source_anchors(&page.blocks).find(|block| block.id == block_id);
        Ok(match block {
            Some(block) => DesktopReference::Available {
                revision,
                title: info.title,
                block: crate::encode_desktop_blocks(vec![block.clone()])?.remove(0),
            },
            None => DesktopReference::Unavailable {
                revision: Some(revision),
            },
        })
    }

    pub fn backlinks(
        &self,
        workspace: &str,
        page_id: &str,
        after: Option<&str>,
    ) -> Result<BacklinkPage> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        ensure_active_info(&read_page_info(&tx, page_id)?)?;
        backlink_page(&tx, page_id, after, 50, None)
    }
}

pub(crate) fn backlink_page(
    db: &Connection,
    page_id: &str,
    after: Option<&str>,
    limit: usize,
    access: Option<&crate::integrations::Access>,
) -> Result<BacklinkPage> {
    use crate::collections::Arguments;
    use rusqlite::params_from_iter;

    let (after_page, after_block) = match after {
        None => ("", ""),
        Some(value) => value
            .split_once('/')
            .filter(|(page, block)| valid_id(page) && valid_id(block))
            .ok_or_else(|| Error::new("INVALID_CURSOR", "反向链接续页位置无效。"))?,
    };
    let mut arguments = Arguments::default();
    let (cte, scope) = match access {
        None => (String::new(), ""),
        Some(access) => (
            access.readable_pages_cte(&mut arguments),
            "JOIN readable_pages r ON r.id = l.page_id",
        ),
    };
    let target = arguments.bind(page_id.to_owned());
    let after_page = arguments.bind(after_page.to_owned());
    let after_block = arguments.bind(after_block.to_owned());
    let anchor = arguments.bind(1 + i32::from(access.is_none_or(|access| access.reads_assets())));
    let stop = arguments.bind(limit as i64 + 1);
    let mut query = db.prepare(&format!(
        "{cte}
         SELECT DISTINCT l.page_id, l.block_id, p.title, p.icon, l.target_block_id <> ''
         FROM page_links l JOIN pages p ON p.id = l.page_id {scope}
         WHERE l.target_page_id = {target} AND p.trash_root_id IS NULL
           AND (l.page_id, l.block_id) > ({after_page}, {after_block})
           AND (l.target_block_id = '' OR EXISTS (
               SELECT 1 FROM block_ids b WHERE b.id = l.target_block_id AND b.page_id = {target}
                 AND b.reference_anchor BETWEEN 1 AND {anchor}
           ))
         ORDER BY l.page_id, l.block_id LIMIT {stop}",
    ))?;
    let mut items = query
        .query_map(params_from_iter(arguments.0), |row| {
            Ok(Backlink {
                page_id: row.get(0)?,
                block_id: row.get(1)?,
                title: row.get(2)?,
                icon: row.get(3)?,
                target_kind: if row.get::<_, bool>(4)? {
                    "block"
                } else {
                    "page"
                },
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let next_cursor = if items.len() > limit {
        items.pop();
        items
            .last()
            .map(|last| format!("{}/{}", last.page_id, last.block_id))
    } else {
        None
    };
    Ok(BacklinkPage { items, next_cursor })
}
