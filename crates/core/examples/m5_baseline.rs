// SPDX-License-Identifier: AGPL-3.0-or-later

#[path = "../tests/support/m2_fixture.rs"]
mod m2_fixture;

use m2_fixture::{ATTACHMENTS, LOGICAL_SHA256, RECORDS, SEARCHES, block_count, elapsed_ms};
use serde_json::{Map, Value, json};
use std::{fs, path::Path, time::Instant};
use zhiye_core::{BackupPolicy, PageDraft, SearchRequest, Store};

const SAMPLES: usize = 25;
const EDITOR_PAGES: usize = SAMPLES + 1;
const BLOCKS_PER_EDITOR_PAGE: usize = 200;

fn measure(mut action: impl FnMut(usize)) -> Value {
    let samples: Vec<f64> = (0..SAMPLES)
        .map(|index| {
            let start = Instant::now();
            action(index);
            elapsed_ms(start)
        })
        .collect();
    statistics(&samples)
}

fn statistics(samples: &[f64]) -> Value {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let percentile = |rate: usize| sorted[(sorted.len() * rate).div_ceil(100) - 1];
    json!({
        "n": samples.len(),
        "p50Ms": percentile(50),
        "p95Ms": percentile(95),
        "maxMs": sorted.last(),
        "samplesMs": samples,
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let arg = std::env::args_os()
        .nth(1)
        .ok_or("usage: m5_baseline <new data directory>")?;
    let root = Path::new(&arg);
    if root.exists() && fs::read_dir(root)?.next().is_some() {
        return Err("The synthetic baseline requires a new empty directory".into());
    }
    let store = Store::open(root)?;
    let workspace = store.create_workspace("知页 M5 固定基线")?.id;
    store.set_backup_policy(BackupPolicy {
        enabled: false,
        ..BackupPolicy::default()
    })?;
    let mut editors = Vec::with_capacity(EDITOR_PAGES);
    for number in 0..EDITOR_PAGES {
        let title = format!("普通页 {number:02}");
        let page = store.create_page(&workspace, None, &title, "file")?;
        let blocks = (1..=BLOCKS_PER_EDITOR_PAGE)
            .map(|block| {
                m2_fixture::paragraph(&format!(
                    "Editor sample {number:02} · paragraph {block:03}."
                ))
            })
            .collect();
        let saved = store.save_page(
            &workspace,
            &page.info.id,
            page.info.revision,
            PageDraft { title, blocks },
        )?;
        editors.push(saved);
    }
    let fixture = m2_fixture::seed(&store, &workspace)?;
    assert_eq!(fixture.sha256, LOGICAL_SHA256);
    let index_started = Instant::now();
    let mut batches = Vec::new();
    loop {
        let batch = Instant::now();
        let progress = store.advance_search_index(&workspace)?;
        batches.push(elapsed_ms(batch));
        if progress.pending == 0 {
            break;
        }
    }
    let index_build_ms = elapsed_ms(index_started);
    let mut searches = Map::new();
    for (query, expected) in SEARCHES {
        let request = SearchRequest {
            query: query.into(),
            ..Default::default()
        };
        searches.insert(
            query.into(),
            measure(|_| {
                let answer = store.search_text(&workspace, &request).unwrap();
                assert_eq!(answer.hits.len(), expected, "{query}");
                assert_eq!(answer.index.pending, 0);
            }),
        );
    }
    let records = measure(|_| {
        let result = store
            .query_records(&workspace, &fixture.collection, &fixture.view, 0)
            .unwrap();
        assert_eq!(result.records.len(), 50);
        assert_eq!(result.total, fixture.count);
        assert_eq!(result.records[0].title, "资料 09998");
    });
    let ordinary = measure(|sample| {
        let page = store.page(&workspace, &editors[sample + 1].id).unwrap();
        assert_eq!(page.blocks.len(), BLOCKS_PER_EDITOR_PAGE);
    });
    let metadata = measure(|_| {
        assert_eq!(
            store.pages(&workspace).unwrap().len(),
            RECORDS + 1 + EDITOR_PAGES
        );
    });
    let db_path = store.data_root().join(&workspace).join("workspace.sqlite");
    let db_bytes = db_path.metadata()?.len();
    let db =
        rusqlite::Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let (pages, blocks, rows, attachments): (u32, u32, u32, u32) = db.query_row(
        "SELECT (SELECT count(*) FROM pages), (SELECT count(*) FROM block_ids),
                (SELECT count(*) FROM collection_records), (SELECT count(*) FROM attachments)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    assert_eq!(
        (pages, blocks, rows, attachments),
        (
            (RECORDS + 1 + EDITOR_PAGES) as u32,
            (block_count() + EDITOR_PAGES * BLOCKS_PER_EDITOR_PAGE) as u32,
            RECORDS as u32,
            ATTACHMENTS as u32,
        )
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "version": env!("CARGO_PKG_VERSION"),
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "workspace": workspace, "collection": fixture.collection, "container": fixture.container, "view": fixture.view,
            "ordinaryPages": editors, "searchCases": SEARCHES,
            "base": { "logicalSha256": fixture.sha256, "pages": RECORDS + 1, "blocks": block_count(), "records": RECORDS, "attachments": ATTACHMENTS },
            "measuredWorkspace": { "pages": pages, "blocks": blocks, "records": rows, "attachments": attachments, "databaseBytes": db_bytes },
            "core": { "seedMs": fixture.seed_ms, "firstIndexBuildMs": index_build_ms,
                "indexBatches": statistics(&batches), "search": searches, "filteredRecords": records,
                "ordinaryPageRead": ordinary, "pageMetadataRead": metadata },
        }))?
    );
    Ok(())
}
