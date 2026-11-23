// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::path::Path;
use zhiye_core::{
    Block, ChangeEvent, IntegrationDraft, IntegrationGrant as Grant, IntegrationScope as Scope,
    IssuedIntegration, Page, PageDraft, PageVersion, Store,
};

fn create(store: &Store, workspace: &str, parent: Option<&str>) -> Page {
    store
        .create_page(workspace, parent, "合成页", "file")
        .unwrap()
}

fn block(kind: &str, payload: Value) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children: vec![],
        extra: Default::default(),
    }
}

fn paragraph(text: &str) -> Block {
    block(
        "paragraph",
        json!({"inline":[{"type":"text", "text":text}]}),
    )
}

fn reference(source: &Page, anchor: &Block) -> Block {
    block(
        "blockReference",
        json!({"pageId":source.info.id,"blockId":anchor.id}),
    )
}

fn save(store: &Store, space: &str, page: &Page, blocks: Vec<Block>) -> Page {
    store
        .save_page(
            space,
            &page.info.id,
            page.info.revision,
            PageDraft {
                title: page.info.title.clone(),
                blocks,
            },
        )
        .unwrap();
    store.page(space, &page.info.id).unwrap()
}

fn issue(store: &Store, space: &str, grants: Vec<Grant>) -> IssuedIntegration {
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: "派生验收".into(),
                scopes: [Scope::Read, Scope::Changes].into(),
                grants,
            },
        )
        .unwrap()
}

fn cursor(store: &Store, space: &str, integration: &IssuedIntegration) -> String {
    store
        .integration_catalog(space, &integration.token, None, None)
        .unwrap()
        .change_cursor
        .unwrap()
}

fn changes(
    store: &Store,
    space: &str,
    integration: &IssuedIntegration,
    cursor: &str,
) -> Vec<ChangeEvent> {
    let mut cursor = cursor.to_owned();
    let mut result = Vec::new();
    loop {
        let page = store
            .integration_changes(space, &integration.token, Some(1), &cursor)
            .unwrap();
        result.extend(page.items);
        cursor = page.next_cursor;
        if !page.has_more {
            return result;
        }
    }
}

fn derived(events: &[ChangeEvent]) -> Vec<&ChangeEvent> {
    events
        .iter()
        .filter(|event| event.action == "derived.invalidated")
        .collect()
}

fn one_notice(events: &[ChangeEvent], page: &Page) -> String {
    let notices = derived(events);
    assert_eq!(notices.len(), 1, "{events:?}");
    let notice = notices[0];
    assert_eq!(notice.id, page.info.id);
    assert_eq!(notice.revision, i64::from(page.info.revision));
    assert_eq!(notice.kind, "page");
    notice.event_id.clone()
}

fn version(page: &Page) -> PageVersion {
    PageVersion {
        id: page.info.id.clone(),
        revision: page.info.revision,
    }
}

fn database(root: &Path, space: &str) -> Connection {
    Connection::open(root.join(space).join("workspace.sqlite")).unwrap()
}

#[test]
fn captured_projection_filters_hidden_edits_and_coalesces_visible_changes_across_archive_batches() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("派生权限与批次").unwrap().id;
    let branch = create(&store, &space, None);
    let private_one = create(&store, &space, None);
    let private_two = create(&store, &space, None);
    let referring = create(&store, &space, Some(&branch.info.id));
    let source = create(&store, &space, Some(&branch.info.id));
    let original = block(
        "paragraph",
        json!({"inline":[
            {"type":"text","text":"可见原文"},
            {"type":"pageReference","pageId":private_one.info.id,"label":"隐藏标签"},
            {"type":"pageReference","pageId":private_two.info.id,"label":"另一个隐藏标签"},
        ]}),
    );
    let unrelated = paragraph("不在引用范围内的段落");
    let source = save(
        &store,
        &space,
        &source,
        vec![original.clone(), unrelated.clone()],
    );
    let a = reference(&source, &original);
    let ref_id = a.id.clone();
    let referring = save(
        &store,
        &space,
        &referring,
        vec![a, reference(&source, &original)],
    );
    let limited = issue(&store, &space, vec![Grant::Subtree(branch.info.id.clone())]);
    let complete = issue(
        &store,
        &space,
        vec![
            Grant::Subtree(branch.info.id),
            Grant::Page(private_one.info.id.clone()),
            Grant::Page(private_two.info.id.clone()),
        ],
    );
    let page_only = issue(&store, &space, vec![Grant::Page(referring.info.id.clone())]);
    let subscriber_cursor = |sub| cursor(&store, &space, sub);
    let preview = |sub: &IssuedIntegration| {
        store
            .integration_reference(&space, &sub.token, &referring.info.id, &ref_id)
            .unwrap()
    };
    let initial_limited = preview(&limited);
    let initial_full = preview(&complete);
    let initial_write_etag = store
        .integration_page(&space, &complete.token, &referring.info.id)
        .unwrap()
        .etag;
    let (for_limited, for_complete, for_page_only) = (
        subscriber_cursor(&limited),
        subscriber_cursor(&complete),
        subscriber_cursor(&page_only),
    );

    let mut edited = original.clone();
    edited.payload["inline"][1]["label"] = json!("只替换隐藏标签");
    let source = save(&store, &space, &source, vec![edited.clone(), unrelated]);
    assert!(store.advance_derived_changes(&space).unwrap());
    let events = changes(&store, &space, &complete, &for_complete);
    one_notice(&events, &referring);
    assert_ne!(
        preview(&complete).representation_version,
        initial_full.representation_version
    );
    assert_eq!(
        preview(&limited).representation_version,
        initial_limited.representation_version
    );
    assert!(derived(&changes(&store, &space, &limited, &for_limited)).is_empty());
    assert!(changes(&store, &space, &page_only, &for_page_only).is_empty());

    let for_complete = subscriber_cursor(&complete);
    let source = save(
        &store,
        &space,
        &source,
        vec![edited.clone(), paragraph("只改无关段落")],
    );
    assert!(store.advance_derived_changes(&space).unwrap());
    assert!(derived(&changes(&store, &space, &complete, &for_complete)).is_empty());

    let (for_limited, for_complete) = (subscriber_cursor(&limited), subscriber_cursor(&complete));
    store
        .archive_pages(&space, &[version(&private_one), version(&private_two)])
        .unwrap();
    let overlay_version = preview(&complete).representation_version;
    assert!(store.advance_derived_changes(&space).unwrap());
    one_notice(
        &changes(&store, &space, &complete, &for_complete),
        &referring,
    );
    assert!(changes(&store, &space, &limited, &for_limited).is_empty());
    assert_ne!(overlay_version, initial_full.representation_version);
    assert_eq!(
        preview(&limited).representation_version,
        initial_limited.representation_version
    );

    let for_complete = subscriber_cursor(&complete);
    edited.payload["inline"][1]["label"] = json!("归档期间改动的隐藏标签");
    let source = save(&store, &space, &source, vec![edited.clone()]);
    store
        .restore_pages(
            &space,
            &[
                PageVersion {
                    revision: private_one.info.revision + 1,
                    ..version(&private_one)
                },
                PageVersion {
                    revision: private_two.info.revision + 1,
                    ..version(&private_two)
                },
            ],
        )
        .unwrap();
    // Both jobs are processed after the restore. The first diff must still
    // use the visibility that existed at commit rather than the current state.
    assert!(store.advance_derived_changes(&space).unwrap());
    assert!(derived(&changes(&store, &space, &complete, &for_complete)).is_empty());
    assert!(store.advance_derived_changes(&space).unwrap());
    one_notice(
        &changes(&store, &space, &complete, &for_complete),
        &referring,
    );

    let (for_limited, for_complete) = (subscriber_cursor(&limited), subscriber_cursor(&complete));
    store
        .archive_page(&space, &source.info.id, source.info.revision)
        .unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    let events = changes(&store, &space, &complete, &for_complete);
    let archived = one_notice(&events, &referring);
    assert_eq!(
        archived,
        one_notice(&changes(&store, &space, &limited, &for_limited), &referring)
    );
    let for_complete = subscriber_cursor(&complete);
    store
        .restore_page(&space, &source.info.id, source.info.revision + 1)
        .unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    assert_ne!(
        archived,
        one_notice(
            &changes(&store, &space, &complete, &for_complete),
            &referring
        )
    );
    assert_eq!(
        store
            .integration_page(&space, &complete.token, &referring.info.id)
            .unwrap()
            .etag,
        initial_write_etag
    );
    assert_eq!(store.history(&space, &referring.info.id).unwrap().len(), 2);
}

#[test]
fn v9_upgrade_retains_cursor_and_failed_publication_replays_once_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("可恢复失效任务").unwrap().id;
    let branch = create(&store, &space, None);
    let original = paragraph("开始文本");
    let source = create(&store, &space, Some(&branch.info.id));
    let source = save(&store, &space, &source, vec![original.clone()]);
    let referring = create(&store, &space, Some(&branch.info.id));
    let referring = save(
        &store,
        &space,
        &referring,
        vec![reference(&source, &original)],
    );
    let reader = issue(&store, &space, vec![Grant::Subtree(branch.info.id)]);
    let starting = cursor(&store, &space, &reader);
    support::remove_v10_schema(&database(root.path(), &space));
    assert!(changes(&store, &space, &reader, &starting).is_empty());
    let before = root
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v9.sqlite"));
    let before = Connection::open(before).unwrap();
    assert_eq!(
        before
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        9
    );
    let db = database(root.path(), &space);
    db.execute_batch(
        "CREATE TRIGGER test_enqueue BEFORE INSERT ON derived_jobs BEGIN SELECT RAISE(ABORT,'fixture error'); END;",
    ).unwrap();
    let mut updated = original;
    updated.payload["inline"][0]["text"] = json!("下一个版本");
    let save_source = || {
        store.save_page(
            &space,
            &source.info.id,
            source.info.revision,
            PageDraft {
                title: source.info.title.clone(),
                blocks: vec![updated.clone()],
            },
        )
    };
    assert!(save_source().is_err());
    assert_eq!(store.page(&space, &source.info.id).unwrap(), source);
    assert!(changes(&store, &space, &reader, &starting).is_empty());
    db.execute_batch("DROP TRIGGER test_enqueue;").unwrap();
    let revised = save_source().unwrap();
    db.execute_batch(
        "CREATE TRIGGER test_publish BEFORE INSERT ON feed_derived_audience BEGIN SELECT RAISE(ABORT,'fixture error'); END;",
    ).unwrap();
    assert!(store.advance_derived_changes(&space).is_err());
    assert!(!store.advance_derived_changes(&space).unwrap());
    assert_eq!(
        db.query_row("SELECT attempts FROM derived_jobs", [], |row| row
            .get::<_, u32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM feed_events WHERE derived=1",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(store.page(&space, &source.info.id).unwrap().info, revised);
    db.execute_batch("DROP TRIGGER test_publish; UPDATE derived_jobs SET retry_at = 0;")
        .unwrap();
    drop(db);
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    assert!(!store.advance_derived_changes(&space).unwrap());
    let event = one_notice(&changes(&store, &space, &reader, &starting), &referring);
    assert_eq!(
        event,
        one_notice(&changes(&store, &space, &reader, &starting), &referring)
    );
}

#[test]
fn stale_grants_late_consumers_and_replaced_edges_do_not_receive_pending_work() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("作用域漂移").unwrap().id;
    let branch = create(&store, &space, None);
    let source = create(&store, &space, Some(&branch.info.id));
    let original = paragraph("前值");
    let source = save(&store, &space, &source, vec![original.clone()]);
    let referring = create(&store, &space, Some(&branch.info.id));
    let referring = save(
        &store,
        &space,
        &referring,
        vec![reference(&source, &original)],
    );
    let unchanged = issue(
        &store,
        &space,
        vec![
            Grant::Page(referring.info.id.clone()),
            Grant::Page(source.info.id.clone()),
        ],
    );
    let scope = issue(&store, &space, vec![Grant::Subtree(branch.info.id.clone())]);
    let revoked = issue(&store, &space, vec![Grant::Subtree(branch.info.id.clone())]);
    let (keep, expired) = (
        cursor(&store, &space, &unchanged),
        cursor(&store, &space, &scope),
    );
    let mut revised_block = original;
    revised_block.payload["inline"][0]["text"] = json!("新值");
    let source = save(&store, &space, &source, vec![revised_block.clone()]);
    let moved = store
        .move_page(&space, &source.info.id, source.info.revision, None)
        .unwrap();
    let moved = store
        .move_page(
            &space,
            &source.info.id,
            moved.revision,
            Some(&branch.info.id),
        )
        .unwrap();
    store
        .revoke_integration(&space, &revoked.integration.id)
        .unwrap();
    let current = cursor(&store, &space, &scope);
    let late = issue(&store, &space, vec![Grant::Subtree(branch.info.id)]);
    let late_start = cursor(&store, &space, &late);
    assert!(store.advance_derived_changes(&space).unwrap());
    one_notice(&changes(&store, &space, &unchanged, &keep), &referring);
    assert_eq!(
        store
            .integration_changes(&space, &scope.token, None, &expired)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    assert!(changes(&store, &space, &scope, &current).is_empty());
    assert!(changes(&store, &space, &late, &late_start).is_empty());
    assert_eq!(
        store
            .integration_catalog(&space, &revoked.token, None, None)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );

    let earlier = cursor(&store, &space, &unchanged);
    let source = Page {
        info: moved,
        ..source
    };
    revised_block.payload["inline"][0]["text"] = json!("待投递值");
    save(&store, &space, &source, vec![revised_block]);
    save(&store, &space, &referring, vec![paragraph("删除原引用")]);
    assert!(store.advance_derived_changes(&space).unwrap());
    assert!(derived(&changes(&store, &space, &unchanged, &earlier)).is_empty());
}

#[test]
fn fanout_resumes_unacknowledged_pages_after_a_partial_batch() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("多处引用批次").unwrap().id;
    let branch = create(&store, &space, None);
    let source = create(&store, &space, Some(&branch.info.id));
    let mut anchor = paragraph("源页版本一");
    let source = save(&store, &space, &source, vec![anchor.clone()]);
    let expected: std::collections::BTreeSet<_> = (0..19)
        .map(|_| {
            let page = create(&store, &space, Some(&branch.info.id));
            save(&store, &space, &page, vec![reference(&source, &anchor)])
                .info
                .id
        })
        .collect();
    let reader = issue(&store, &space, vec![Grant::Subtree(branch.info.id)]);
    let start = cursor(&store, &space, &reader);
    anchor.payload["inline"][0]["text"] = json!("源页版本二");
    save(&store, &space, &source, vec![anchor]);
    assert!(store.advance_derived_changes(&space).unwrap());
    let batch = store
        .integration_changes(&space, &reader.token, Some(100), &start)
        .unwrap();
    assert!(!batch.has_more);
    let first_count = derived(&batch.items).len();
    assert!((1..expected.len()).contains(&first_count));
    drop(store);
    let store = Store::open(root.path()).unwrap();
    assert!(store.advance_derived_changes(&space).unwrap());
    assert!(!store.advance_derived_changes(&space).unwrap());
    let next = changes(&store, &space, &reader, &batch.next_cursor);
    assert_eq!(derived(&next).len() + first_count, expected.len());
    let received = derived(&batch.items)
        .into_iter()
        .chain(derived(&next))
        .map(|event| event.id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(expected, received);
}
