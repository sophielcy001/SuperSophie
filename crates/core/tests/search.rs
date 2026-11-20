// SPDX-License-Identifier: AGPL-3.0-or-later

#[path = "support/m2_fixture.rs"]
mod m2_fixture;
mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashSet};
use std::io::Cursor;
use tempfile::TempDir;
use uuid::Uuid;
use zhiye_core::{
    Block, FilterOperator, Page, PageDraft, RecordDraft, SearchRequest, SearchResponse, Store,
    ViewFilter,
};

fn workspace() -> (TempDir, Store, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("检索测试").unwrap().id;
    (root, store, space)
}

fn block(kind: &str, payload: Value, children: Vec<Block>) -> Block {
    Block {
        id: Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children,
        extra: Default::default(),
    }
}

fn paragraph(text: &str) -> Block {
    block(
        "paragraph",
        json!({"inline": [{"type": "text", "text": text}]}),
        vec![],
    )
}

fn document(
    store: &Store,
    space: &str,
    parent: Option<&str>,
    title: &str,
    blocks: Vec<Block>,
) -> Page {
    let page = store.create_page(space, parent, title, "file").unwrap();
    store
        .save_page(
            space,
            &page.info.id,
            1,
            PageDraft {
                title: title.into(),
                blocks,
            },
        )
        .unwrap();
    store.page(space, &page.info.id).unwrap()
}

fn finish_index(store: &Store, space: &str) {
    while store.advance_search_index(space).unwrap().pending > 0 {}
}

fn search(store: &Store, space: &str, query: &str) -> SearchResponse {
    store
        .search_text(
            space,
            &SearchRequest {
                query: query.into(),
                ..Default::default()
            },
        )
        .unwrap()
}

#[test]
fn short_chinese_mixed_text_and_literal_code_return_original_snippets_and_block_ids() {
    let (_root, store, space) = workspace();
    let folder = store
        .create_page(&space, None, "工程文档", "folder")
        .unwrap();
    let line = paragraph("📙 在这里阅读全文，深入阅读 SQLite 事务与混合内容。");
    let line_id = line.id.clone();
    let list = block(
        "bulletList",
        json!({}),
        vec![block("listItem", json!({}), vec![line])],
    );
    let code = block(
        "codeBlock",
        json!({"text": "const note = api::run(\"海棠\"); // C++ != SQL"}),
        vec![],
    );
    let code_id = code.id.clone();
    let child = document(
        &store,
        &space,
        Some(&folder.info.id),
        "实现记录",
        vec![list, code, paragraph("阅读之后附注")],
    );
    let title = document(&store, &space, None, "阅读计划", vec![]);
    assert_eq!(search(&store, &space, "阅读").index.pending, 3);
    finish_index(&store, &space);

    for term in ["阅", "阅读"] {
        let found = search(&store, &space, term);
        assert_eq!(found.hits.len(), 2, "{term}");
        assert_eq!(found.hits[0].page.id, title.info.id);
        assert!(found.hits[0].block_id.is_none());
        let result = &found.hits[1];
        assert_eq!(result.page.id, child.info.id);
        assert_eq!(
            result.page.parent_id.as_deref(),
            Some(folder.info.id.as_str())
        );
        assert_eq!(result.block_id.as_deref(), Some(line_id.as_str()));
        let excerpt = result.snippet.as_ref().unwrap();
        assert_eq!(excerpt.matched, term);
        assert!(excerpt.before.contains("📙"));
    }
    let mixed = search(&store, &space, "阅读 sqlite");
    assert_eq!(
        mixed.hits[0].snippet.as_ref().unwrap().matched,
        "阅读 SQLite"
    );
    assert_eq!(mixed.hits[0].block_id.as_deref(), Some(line_id.as_str()));
    for term in ["::", "c++", "\"海棠\"", "run(\"海棠\")", "!= SQL"] {
        let result = search(&store, &space, term);
        assert_eq!(result.hits.len(), 1, "{term}");
        assert_eq!(result.hits[0].block_id.as_deref(), Some(code_id.as_str()));
    }
    assert!(search(&store, &space, "* AND \"").hits.is_empty());
}

#[test]
fn queued_work_is_bounded_and_survives_reopen_revision_change_trash_and_restore() {
    let (root, store, space) = workspace();
    for number in 0..53 {
        store
            .create_page(&space, None, &format!("排队页面 {number:02}"), "file")
            .unwrap();
    }
    assert_eq!(store.advance_search_index(&space).unwrap().pending, 5);
    let first = search(&store, &space, "排队页面");
    assert_eq!(first.hits.len(), 20);
    assert_eq!(first.next_offset, Some(20));
    let second = store
        .search_text(
            &space,
            &SearchRequest {
                query: "排队页面".into(),
                offset: 20,
                ..Default::default()
            },
        )
        .unwrap();
    let first_ids: HashSet<_> = first.hits.iter().map(|hit| &hit.page.id).collect();
    assert!(
        second
            .hits
            .iter()
            .all(|hit| !first_ids.contains(&hit.page.id))
    );
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(search(&store, &space, "排队").index.pending, 5);
    let original = document(
        &store,
        &space,
        None,
        "编辑样例",
        vec![paragraph("最初词汇")],
    );
    finish_index(&store, &space);
    let fresh = paragraph("新的词汇");
    let latest = store
        .save_page(
            &space,
            &original.info.id,
            original.info.revision,
            PageDraft {
                title: original.info.title.clone(),
                blocks: vec![fresh.clone()],
            },
        )
        .unwrap();
    let stale = search(&store, &space, "最初词汇");
    assert_eq!(stale.index.pending, 1);
    assert!(stale.hits.is_empty());
    finish_index(&store, &space);
    assert_eq!(
        search(&store, &space, "新的词汇").hits[0]
            .block_id
            .as_deref(),
        Some(fresh.id.as_str())
    );
    store
        .archive_page(&space, &latest.id, latest.revision)
        .unwrap();
    assert!(search(&store, &space, "新的词汇").hits.is_empty());
    finish_index(&store, &space);
    store
        .restore_page(&space, &latest.id, latest.revision + 1)
        .unwrap();
    finish_index(&store, &space);
    let active = search(&store, &space, "新的词汇").hits.remove(0);
    assert_eq!(active.page.id, latest.id);
    store
        .restore_version(
            &space,
            &latest.id,
            active.page.revision,
            original.info.revision,
        )
        .unwrap();
    finish_index(&store, &space);
    assert!(search(&store, &space, "新的词汇").hits.is_empty());
    assert_eq!(
        search(&store, &space, "最初词汇").hits[0]
            .block_id
            .as_deref(),
        Some(original.blocks[0].id.as_str())
    );
}

#[test]
fn filters_use_current_record_properties_before_page_selection() {
    let (_root, store, space) = workspace();
    let other = store.create_workspace("另一空间").unwrap().id;
    let (database, container) = store.create_collection(&space, None, "论文库").unwrap();
    let schema = store.collection(&space, &database.id).unwrap();
    let status = &schema.properties[0];
    let picked = &status.options[1].id;
    let good = document(
        &store,
        &space,
        Some(&container.info.id),
        "进行中的文章",
        vec![paragraph("海棠检索")],
    );
    let ignored = document(
        &store,
        &space,
        Some(&container.info.id),
        "其他文章",
        vec![paragraph("海棠检索")],
    );
    document(
        &store,
        &space,
        None,
        "普通文档",
        vec![paragraph("海棠检索")],
    );
    document(
        &store,
        &other,
        None,
        "独立空间",
        vec![paragraph("海棠检索")],
    );
    let updated = store
        .save_record(
            &space,
            &database.id,
            &good.info.id,
            good.info.revision,
            RecordDraft {
                title: good.info.title.clone(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([(status.id.clone(), json!(picked))]),
            },
        )
        .unwrap();
    finish_index(&store, &space);
    finish_index(&store, &other);
    assert_eq!(search(&store, &space, "海棠检索").hits.len(), 3);
    assert_eq!(search(&store, &other, "海棠检索").hits.len(), 1);
    let mut request = SearchRequest {
        query: "海棠检索".into(),
        collection_id: Some(database.id.clone()),
        filter: Some(ViewFilter {
            field_id: status.id.clone(),
            operator: FilterOperator::Equal,
            value: Some(json!(picked)),
        }),
        ..Default::default()
    };
    let found = store.search_text(&space, &request).unwrap();
    assert_eq!(found.hits.len(), 1);
    assert_eq!(found.hits[0].page.id, updated.info.id);
    assert!(found.hits[0].block_id.is_some());
    request.query.clear();
    request.filter = Some(ViewFilter {
        field_id: status.id.clone(),
        operator: FilterOperator::Empty,
        value: None,
    });
    let unfilled = store.search_text(&space, &request).unwrap();
    assert_eq!(unfilled.hits.len(), 1);
    assert_eq!(unfilled.hits[0].page.id, ignored.info.id);
    request.collection_id = None;
    assert_eq!(
        store.search_text(&space, &request).unwrap_err().code,
        "SEARCH_SCOPE"
    );
}

#[test]
fn migration_and_native_restore_enqueue_rebuildable_search_data() {
    let (root, store, space) = workspace();
    let page = document(
        &store,
        &space,
        None,
        "旧工作空间",
        vec![paragraph("历史迁移正文")],
    );
    let path = root.path().join(&space).join("workspace.sqlite");
    drop(store);
    {
        let db = Connection::open(&path).unwrap();
        support::remove_v7_schema(&db);
        db.execute_batch(
            "DROP TABLE integration_operations; DROP TABLE integration_revisions;
             DROP TABLE integration_grants; DROP TABLE integrations;
             DROP TRIGGER search_page_insert; DROP TRIGGER search_page_revision;
             DROP TABLE search_entries; DROP TABLE search_fts; DROP TABLE search_pending;
             DROP TABLE templates; DROP TABLE import_receipts; DROP TABLE favorites;
             DROP TABLE recent_pages; DROP TABLE workspace_inbox;
             PRAGMA user_version = 3;",
        )
        .unwrap();
    }
    let store = Store::open(root.path()).unwrap();
    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
    assert_eq!(search(&store, &space, "迁移正文").index.pending, 1);
    let snapshot = root
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v3.sqlite"));
    assert_eq!(
        Connection::open(snapshot)
            .unwrap()
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        3
    );
    finish_index(&store, &space);
    assert_eq!(search(&store, &space, "迁移正文").hits.len(), 1);
    Connection::open(&path)
        .unwrap()
        .execute(
            "INSERT INTO search_fts(search_fts) VALUES('delete-all')",
            [],
        )
        .unwrap();
    assert!(search(&store, &space, "迁移正文").hits.is_empty());
    assert_eq!(store.rebuild_search_index(&space).unwrap().pending, 1);
    finish_index(&store, &space);
    assert_eq!(
        search(&store, &space, "迁移正文").hits[0].page.id,
        page.info.id
    );
    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    archive.set_position(0);
    let restored = store.read_workspace_archive(archive).unwrap();
    assert_eq!(search(&store, &restored.id, "迁移正文").index.pending, 1);
    finish_index(&store, &restored.id);
    assert_eq!(
        search(&store, &restored.id, "迁移正文").hits[0].page.id,
        page.info.id
    );
}

#[test]
fn a_failed_batch_does_not_consume_pending_work() {
    let (root, store, space) = workspace();
    let page = document(
        &store,
        &space,
        None,
        "有故障的索引",
        vec![paragraph("可以重试的正文")],
    );
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER simulate_index_failure BEFORE INSERT ON search_entries
         BEGIN SELECT raise(ABORT, 'index write failed'); END;",
    )
    .unwrap();
    assert!(store.advance_search_index(&space).is_err());
    assert_eq!(search(&store, &space, "重试").index.pending, 1);
    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
    db.execute_batch("DROP TRIGGER simulate_index_failure")
        .unwrap();
    finish_index(&store, &space);
    assert_eq!(search(&store, &space, "重试").hits[0].page.id, page.info.id);
}

#[test]
#[ignore = "M2 合成数据规模基线，按需运行"]
fn measures_m2_search_and_record_baseline() {
    use m2_fixture::{ATTACHMENTS, LOGICAL_SHA256, RECORDS, SEARCHES, block_count, seed};
    use std::time::Instant;

    let (_root, store, space) = workspace();
    let fixture = seed(&store, &space).unwrap();
    assert_eq!(fixture.sha256, LOGICAL_SHA256);
    assert_eq!(store.pages(&space).unwrap().len(), RECORDS + 1);
    assert_eq!(
        store.page(&space, &fixture.container).unwrap().info.title,
        "知识库基线"
    );
    let started = Instant::now();
    let mut batch_max_ms = 0.0_f64;
    loop {
        let batch = Instant::now();
        let index = store.advance_search_index(&space).unwrap();
        batch_max_ms = batch_max_ms.max(batch.elapsed().as_secs_f64() * 1_000.0);
        if index.pending == 0 {
            break;
        }
    }
    let index_ms = started.elapsed().as_millis() as u64;
    let mut search_samples = serde_json::Map::new();
    for (term, expected) in SEARCHES {
        let request = SearchRequest {
            query: term.into(),
            ..Default::default()
        };
        let mut samples = Vec::new();
        for _ in 0..25 {
            let start = Instant::now();
            let result = store.search_text(&space, &request).unwrap();
            samples.push(start.elapsed().as_secs_f64() * 1_000.0);
            assert_eq!(result.hits.len(), expected, "{term}");
            assert_eq!(result.index.pending, 0);
        }
        search_samples.insert(term.into(), json!(p95(samples)));
    }
    let mut record_samples = Vec::new();
    for _ in 0..25 {
        let start = Instant::now();
        let result = store
            .query_records(&space, &fixture.collection, &fixture.view, 0)
            .unwrap();
        record_samples.push(start.elapsed().as_secs_f64() * 1_000.0);
        assert_eq!(result.records.len(), 50);
        assert_eq!(result.total, fixture.count);
    }
    println!(
        "M2_BASELINE {}",
        json!({
            "fixtureSha256": fixture.sha256,
            "pages": RECORDS + 1,
            "blocks": block_count(),
            "records": RECORDS,
            "attachments": ATTACHMENTS,
            "seedMs": fixture.seed_ms,
            "indexBuildMs": index_ms,
            "indexBatchMaxMs": batch_max_ms,
            "samplesPerQuery": 25,
            "searchCoreP95Ms": search_samples,
            "recordFilterAndSortCoreP95Ms": p95(record_samples)
        })
    );
}

fn p95(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples[(samples.len() * 95).div_ceil(100) - 1]
}
