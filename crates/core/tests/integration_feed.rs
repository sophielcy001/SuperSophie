// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::{Connection, params};
use serde_json::json;
use std::{collections::HashMap, path::Path};
use zhiye_core::{
    CatalogItem, IntegrationDraft, IntegrationGrant as Grant, IntegrationScope as Scope,
    IssuedIntegration, PageDraft, PageVersion, PropertyDraft, PropertyKind, Store,
};

fn setup() -> (tempfile::TempDir, Store, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("变更流测试").unwrap().id;
    (root, store, space)
}

fn issue(store: &Store, space: &str, grant: Grant, extra: &[Scope]) -> IssuedIntegration {
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: "变更订阅者".into(),
                scopes: std::iter::once(Scope::Read)
                    .chain(extra.iter().copied())
                    .collect(),
                grants: vec![grant],
            },
        )
        .unwrap()
}

fn database(root: &Path, space: &str) -> Connection {
    Connection::open(root.join(space).join("workspace.sqlite")).unwrap()
}

fn initial(store: &Store, space: &str, token: &str) -> String {
    store
        .integration_catalog(space, token, None, None)
        .unwrap()
        .change_cursor
        .unwrap()
}

#[test]
fn keyset_scan_and_fixed_horizon_reconcile_inserts_edits_and_deletes_after_restart() {
    let (root, store, space) = setup();
    let tree = store.create_page(&space, None, "公开分支", "file").unwrap();
    let hidden = store.create_page(&space, None, "私有分支", "file").unwrap();
    let removed = store
        .create_page(&space, Some(&tree.info.id), "取走的页", "file")
        .unwrap();
    let (collection, container) = store
        .create_collection(&space, Some(&tree.info.id), "记录库")
        .unwrap();
    let credentials = issue(
        &store,
        &space,
        Grant::Subtree(tree.info.id.clone()),
        &[Scope::Changes, Scope::ReadAssets],
    );
    let token = &credentials.token;

    let first = store
        .integration_catalog(&space, token, Some(1), None)
        .unwrap();
    assert!(
        matches!(&first.items[..], [CatalogItem::Collection { id, revision: 1, .. }] if id == &collection.id)
    );
    let start = first.change_cursor.unwrap();
    let asset = store
        .add_attachment(&space, &tree.info.id, "清单.txt", "例子".as_bytes())
        .unwrap()
        .attachment;
    store
        .rename_attachment(&space, &tree.info.id, &asset.id, "改名.txt")
        .unwrap();
    let schema = store
        .save_property(
            &space,
            &collection.id,
            1,
            PropertyDraft {
                id: None,
                name: "评分".into(),
                kind: PropertyKind::Number,
                options: vec![],
            },
        )
        .unwrap();
    let record = store
        .create_page(&space, Some(&container.info.id), "新记录", "file")
        .unwrap();
    store.archive_page(&space, &removed.info.id, 1).unwrap();
    store
        .save_page(
            &space,
            &hidden.info.id,
            1,
            PageDraft {
                title: "私有更新".into(),
                blocks: vec![],
            },
        )
        .unwrap();

    let remaining = store
        .integration_catalog(&space, token, None, first.catalog_cursor.as_deref())
        .unwrap();
    let listed = serde_json::to_string(&remaining.items).unwrap();
    assert!(remaining.change_cursor.is_none());
    assert!(!listed.contains(&asset.id));
    assert!(!listed.contains(&removed.info.id));
    assert!(!listed.contains(&hidden.info.id));

    let sweep = store
        .integration_changes(&space, token, Some(1), &start)
        .unwrap();
    assert!(sweep.has_more);
    assert_eq!(sweep.items[0].id, asset.id);
    let replay = store
        .integration_changes(&space, token, Some(1), &start)
        .unwrap();
    assert_eq!(sweep.items[0].event_id, replay.items[0].event_id);
    let after_horizon = store
        .create_page(&space, Some(&tree.info.id), "水位后的页", "file")
        .unwrap();
    let watermark = sweep.watermark;
    let mut offset = sweep.next_cursor;
    let mut seen = sweep.items;
    drop(store);
    let store = Store::open(root.path()).unwrap();
    loop {
        let next = store
            .integration_changes(&space, token, Some(1), &offset)
            .unwrap();
        assert_eq!(next.watermark, watermark);
        offset = next.next_cursor;
        seen.extend(next.items);
        if !next.has_more {
            break;
        }
    }
    let latest: HashMap<_, _> = seen
        .iter()
        .map(|event| (event.id.as_str(), event))
        .collect();
    assert_eq!(latest[asset.id.as_str()].revision, 2);
    assert_eq!(
        latest[collection.id.as_str()].revision,
        i64::from(schema.schema_revision)
    );
    assert_eq!(latest[record.info.id.as_str()].action, "upsert");
    assert_eq!(latest[removed.info.id.as_str()].action, "delete");
    assert_eq!(latest[removed.info.id.as_str()].revision, 2);
    assert!(!latest.contains_key(after_horizon.info.id.as_str()));
    assert!(!latest.contains_key(hidden.info.id.as_str()));
    assert!(seen.iter().all(|event| {
        serde_json::to_value(event)
            .unwrap()
            .as_object()
            .unwrap()
            .len()
            == 5
    }));

    let next = store
        .integration_changes(&space, token, Some(1), &offset)
        .unwrap();
    assert!(!next.has_more);
    assert_eq!(next.items.len(), 1);
    assert_eq!(next.items[0].id, after_horizon.info.id);

    let no_assets = issue(
        &store,
        &space,
        Grant::Subtree(tree.info.id.clone()),
        &[Scope::Changes],
    );
    let no_asset_first = store
        .integration_catalog(&space, &no_assets.token, None, None)
        .unwrap();
    assert!(
        !no_asset_first
            .items
            .iter()
            .any(|item| matches!(item, CatalogItem::Asset { .. }))
    );
    store
        .rename_attachment(&space, &tree.info.id, &asset.id, "仍不可见.txt")
        .unwrap();
    let no_assets_changes = store
        .integration_changes(
            &space,
            &no_assets.token,
            Some(1),
            &no_asset_first.change_cursor.unwrap(),
        )
        .unwrap();
    assert!(!no_assets_changes.has_more && no_assets_changes.items.is_empty());
    let read_only = issue(&store, &space, Grant::Page(tree.info.id), &[]);
    let start = initial(&store, &space, &read_only.token);
    assert_eq!(
        store
            .integration_changes(&space, &read_only.token, None, &start)
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
}

#[test]
fn only_affected_subjects_rebuild_and_tombstones_require_prior_coverage() {
    let (root, store, space) = setup();
    let from = store.create_page(&space, None, "第一分支", "file").unwrap();
    let to = store.create_page(&space, None, "第二分支", "file").unwrap();
    let child = store
        .create_page(&space, Some(&from.info.id), "将移动", "file")
        .unwrap();
    let trashed = store
        .create_page(&space, Some(&child.info.id), "从属归档", "file")
        .unwrap();
    let outgoing = issue(
        &store,
        &space,
        Grant::Subtree(from.info.id.clone()),
        &[Scope::Changes],
    );
    let incoming = issue(
        &store,
        &space,
        Grant::Subtree(to.info.id.clone()),
        &[Scope::Changes],
    );
    let direct = issue(
        &store,
        &space,
        Grant::Subtree(child.info.id.clone()),
        &[Scope::Changes],
    );
    let old = store
        .integration_catalog(&space, &outgoing.token, Some(1), None)
        .unwrap();
    let other_start = initial(&store, &space, &incoming.token);
    let direct_start = initial(&store, &space, &direct.token);
    assert_eq!(
        store
            .integration_changes(&space, &incoming.token, None, &direct_start)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    store.archive_page(&space, &trashed.info.id, 1).unwrap();

    let moved = store
        .move_pages_with_impact(
            &space,
            &[PageVersion {
                id: child.info.id.clone(),
                revision: 1,
            }],
            Some(&to.info.id),
        )
        .unwrap();
    let mut impacts: Vec<_> = moved
        .impacts
        .into_iter()
        .map(|impact| (impact.added, impact.removed))
        .collect();
    impacts.sort_unstable();
    assert_eq!(impacts, [(0, 1), (1, 0)]);
    for (token, cursor) in [
        (&outgoing.token, old.change_cursor.unwrap()),
        (&incoming.token, other_start),
    ] {
        assert_eq!(
            store
                .integration_changes(&space, token, None, &cursor)
                .unwrap_err()
                .code,
            "CURSOR_INVALID"
        );
    }
    assert_eq!(
        store
            .integration_catalog(&space, &outgoing.token, None, old.catalog_cursor.as_deref())
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    let direct_changes = store
        .integration_changes(&space, &direct.token, None, &direct_start)
        .unwrap();
    assert!(
        direct_changes
            .items
            .iter()
            .any(|event| event.id == trashed.info.id && event.action == "delete")
    );
    let fresh = initial(&store, &space, &incoming.token);
    let clean = store
        .integration_changes(&space, &incoming.token, None, &fresh)
        .unwrap();
    assert!(clean.items.is_empty());
    store.restore_page(&space, &trashed.info.id, 2).unwrap();
    assert!(
        store
            .integration_changes(&space, &incoming.token, None, &clean.next_cursor)
            .unwrap()
            .items
            .iter()
            .any(|event| event.id == trashed.info.id
                && event.revision == 3
                && event.action == "upsert")
    );

    let old_grant = initial(&store, &space, &incoming.token);
    database(root.path(), &space)
        .execute(
            "UPDATE integrations SET scopes = ?2 WHERE id = ?1",
            params![
                incoming.integration.id,
                json!(["content:read", "changes:read", "assets:read"]).to_string()
            ],
        )
        .unwrap();
    assert_eq!(
        store
            .integration_changes(&space, &incoming.token, None, &old_grant)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
}

#[test]
fn feed_pruning_expires_a_missing_prefix_and_a_failed_write_emits_nothing() {
    let (root, store, space) = setup();
    let page = store.create_page(&space, None, "修改样本", "file").unwrap();
    let follower = issue(
        &store,
        &space,
        Grant::Page(page.info.id.clone()),
        &[Scope::Changes],
    );
    let token = &follower.token;
    let start = initial(&store, &space, token);
    let db = database(root.path(), &space);
    db.execute_batch(
        "CREATE TRIGGER deny_revision BEFORE INSERT ON revisions
        BEGIN SELECT raise(ABORT, 'fixture rollback'); END;",
    )
    .unwrap();
    assert!(store.archive_page(&space, &page.info.id, 1).is_err());
    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
    assert!(
        store
            .integration_changes(&space, token, None, &start)
            .unwrap()
            .items
            .is_empty()
    );
    db.execute_batch("DROP TRIGGER deny_revision;").unwrap();

    let rewrite = |revision, title: &str| {
        store
            .save_page(
                &space,
                &page.info.id,
                revision,
                PageDraft {
                    title: title.into(),
                    blocks: vec![],
                },
            )
            .unwrap()
    };
    assert_eq!(rewrite(1, "保留窗口前的记录").revision, 2);
    db.execute_batch("UPDATE feed_events SET created_at = 1; UPDATE feed_state SET pruned_at = 1;")
        .unwrap();
    assert_eq!(rewrite(2, "保留窗口内的记录").revision, 3);
    assert_eq!(
        store
            .integration_changes(&space, token, None, &start)
            .unwrap_err()
            .code,
        "CURSOR_EXPIRED"
    );
    let count: u32 = db
        .query_row("SELECT count(*) FROM feed_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 1);
    assert!(
        store
            .integration_changes(&space, token, None, &initial(&store, &space, token))
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn v7_workspace_keeps_credentials_and_sanitizes_legacy_catalog_metadata() {
    let (root, store, space) = setup();
    let parent = store.create_page(&space, None, "私有父页", "file").unwrap();
    let page = store
        .create_page(&space, Some(&parent.info.id), "旧页", "file")
        .unwrap();
    let grant = issue(
        &store,
        &space,
        Grant::Page(page.info.id.clone()),
        &[Scope::Changes],
    );
    support::remove_v8_schema(&database(root.path(), &space));
    let catalog = store
        .integration_catalog(&space, &grant.token, None, None)
        .unwrap();
    assert!(
        matches!(&catalog.items[..], [CatalogItem::Page { id, parent_id: None, revision: 1, .. }] if id == &page.info.id)
    );
    let backup = Connection::open(
        root.path()
            .join("upgrade-backups")
            .join(format!("{space}-v7.sqlite")),
    )
    .unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        7
    );
    assert_eq!(
        database(root.path(), &space)
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        21
    );
    assert_eq!(store.history(&space, &page.info.id).unwrap(), vec![page]);
    assert!(
        store
            .integration_changes(&space, &grant.token, None, &catalog.change_cursor.unwrap())
            .unwrap()
            .items
            .is_empty()
    );
}
