// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, path::Path, time::Instant};
use zhiye_core::{BackupPolicy, Block, Page, PageInfo, SearchRequest, Store};

const BLOCKS_PER_PAGE: usize = 10;
const PAGES_PER_GROUP: usize = 256;
const UNIQUE_QUERY: &str = "lastpressuretarget";

fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

fn page_id(index: usize) -> String {
    format!("a0000000-0000-4000-8000-{index:012x}")
}

fn page_at(index: usize, pages: usize) -> Page {
    let group = index / PAGES_PER_GROUP;
    let branch = index.is_multiple_of(PAGES_PER_GROUP);
    let title = if branch {
        format!("压测目录 {group:03}")
    } else {
        format!("压测页面 {index:06}")
    };
    let blocks = (0..BLOCKS_PER_PAGE)
        .map(|block| {
            let number = index * BLOCKS_PER_PAGE + block;
            let text = if index + 1 == pages && block == 0 {
                UNIQUE_QUERY
            } else {
                "中文压测段"
            };
            Block {
                id: format!("b0000000-0000-4000-8000-{number:012x}"),
                kind: "paragraph".into(),
                schema_version: 1,
                payload: json!({"inline":[{"type":"text","text":text}]}),
                children: vec![],
                extra: Default::default(),
            }
        })
        .collect();
    Page {
        info: PageInfo {
            id: page_id(index),
            title,
            revision: 1,
            updated_at: 1_720_000_000_000,
            parent_id: (!branch).then(|| page_id(group * PAGES_PER_GROUP)),
            icon: (if branch { "folder" } else { "file" }).into(),
            trash_root_id: None,
        },
        blocks,
        properties: None,
        historical_properties: Vec::new(),
        cover: None,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = args
        .next()
        .ok_or("usage: m5_pressure <new data directory> [pages, default 100000]")?;
    let pages: usize = args
        .next()
        .map(|value| value.to_string_lossy().parse())
        .transpose()?
        .unwrap_or(100_000);
    assert!((1_000..=100_000).contains(&pages));
    let root = Path::new(&root);
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        return Err("The pressure fixture requires a new empty directory".into());
    }
    let store = Store::open(root)?;
    let workspace = store.create_workspace("知页 M5 规模压测")?.id;
    store.set_backup_policy(BackupPolicy {
        enabled: false,
        ..BackupPolicy::default()
    })?;
    let path = store.data_root().join(&workspace).join("workspace.sqlite");
    let started = Instant::now();
    let mut hash = Sha256::new();
    // Bulk fixture input keeps setup time independent of individual user saves.
    // Core initializes the real schema and runs the index, reads, and queries.
    let mut db = Connection::open(&path)?;
    db.execute_batch("PRAGMA foreign_keys=ON; PRAGMA synchronous=FULL;")?;
    for start in (0..pages).step_by(512) {
        let tx = db.transaction()?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO pages(id,parent_id,title,revision,updated_at,icon,blocks,body_text)
                 VALUES(?1,?2,?3,1,?4,?5,?6,?7)",
            )?;
            let mut revision =
                tx.prepare("INSERT INTO revisions(page_id,revision,snapshot) VALUES(?1,1,?2)")?;
            let mut block = tx.prepare("INSERT INTO block_ids(id,page_id) VALUES(?1,?2)")?;
            for number in start..(start + 512).min(pages) {
                let page = page_at(number, pages);
                let snapshot = serde_json::to_string(&page)?;
                hash.update(snapshot.as_bytes());
                let blocks = serde_json::to_string(&page.blocks)?;
                let body = page
                    .blocks
                    .iter()
                    .map(|item| item.payload["inline"][0]["text"].as_str().unwrap())
                    .collect::<Vec<_>>()
                    .join("\n");
                insert.execute(params![
                    page.info.id,
                    page.info.parent_id,
                    page.info.title,
                    page.info.updated_at,
                    page.info.icon,
                    blocks,
                    body
                ])?;
                revision.execute(params![page.info.id, snapshot])?;
                for item in &page.blocks {
                    block.execute(params![item.id, page.info.id])?;
                }
            }
        }
        tx.commit()?;
        if start % (512 * 20) == 0 {
            eprintln!("Pressure pages generated: {}", (start + 512).min(pages));
        }
    }
    let total_blocks: u32 = db.query_row("SELECT count(*) FROM block_ids", [], |row| row.get(0))?;
    assert_eq!(total_blocks as usize, pages * BLOCKS_PER_PAGE);
    assert!(
        db.prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_none()
    );
    drop(db);
    let seed_ms = ms(started);
    let before_index_bytes = fs::metadata(&path)?.len();
    let indexed = Instant::now();
    let mut max_batch_ms: f64 = 0.0;
    let mut batch_count = 0;
    loop {
        let batch = Instant::now();
        let status = store.advance_search_index(&workspace)?;
        max_batch_ms = max_batch_ms.max(ms(batch));
        batch_count += 1;
        if batch_count % 200 == 0 {
            eprintln!("Pressure index pages pending: {}", status.pending);
        }
        if status.pending == 0 {
            break;
        }
    }
    let index_ms = ms(indexed);
    let start = Instant::now();
    let info = store.pages(&workspace)?;
    let metadata_ms = ms(start);
    assert_eq!(info.len(), pages);
    let last = info.last().unwrap();
    let read = Instant::now();
    assert_eq!(store.page(&workspace, &last.id)?, page_at(pages - 1, pages));
    let last_page_read_ms = ms(read);
    let mut queries = Vec::<Value>::new();
    for (term, expected) in [
        (UNIQUE_QUERY, 1),
        ("中文压测", 20),
        ("中", 20),
        ("findnothing", 0),
    ] {
        let request = SearchRequest {
            query: term.into(),
            ..Default::default()
        };
        let mut duration = Vec::new();
        for _ in 0..5 {
            let now = Instant::now();
            let response = store.search_text(&workspace, &request)?;
            duration.push(ms(now));
            assert_eq!(response.hits.len(), expected, "{term}");
            assert_eq!(response.index.pending, 0);
        }
        queries.push(json!({"query":term, "hits":expected, "samplesMs":duration}));
    }
    let digest: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "workspace": workspace, "pages": pages, "blocks": total_blocks,
            "logicalSha256": digest, "ordinaryPages": [info.first()], "lastPage": last, "uniqueQuery": UNIQUE_QUERY,
            "treeRoots": pages.div_ceil(PAGES_PER_GROUP), "seedMs": seed_ms,
            "databaseBytesBeforeIndex": before_index_bytes, "databaseBytes": fs::metadata(path)?.len(),
            "indexBuildMs": index_ms, "indexBatches": batch_count, "indexBatchMaxMs": max_batch_ms,
            "metadataReadMs": metadata_ms, "lastPageReadMs": last_page_read_ms, "search": queries,
        }))?
    );
    Ok(())
}
