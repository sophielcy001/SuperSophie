// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use zhiye_core::{
    Block, Collection, FilterOperator, IntegrationCollectionQuery, IntegrationDraft,
    IntegrationFilter, IntegrationGrant as Grant, IntegrationScope as Scope, IntegrationSearchKind,
    IntegrationSearchRequest, IntegrationSort, IssuedIntegration, Page, PageDraft, PropertyDraft,
    PropertyKind, RecordDraft, SortDirection, Store,
};

fn setup() -> (tempfile::TempDir, Store, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("授权查询测试").unwrap().id;
    (root, store, space)
}

fn issue(store: &Store, space: &str, grants: Vec<Grant>, assets: bool) -> IssuedIntegration {
    let mut scopes = std::collections::BTreeSet::from([Scope::Read]);
    if assets {
        scopes.insert(Scope::ReadAssets);
    }
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: "查询测试".into(),
                scopes,
                grants,
            },
        )
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

fn title_and_body(store: &Store, space: &str, page: &Page, blocks: Vec<Block>) {
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
}

fn request(term: &str) -> IntegrationSearchRequest {
    IntegrationSearchRequest {
        query: term.into(),
        ..Default::default()
    }
}

fn property(
    store: &Store,
    space: &str,
    c: &Collection,
    name: &str,
    kind: PropertyKind,
) -> Collection {
    store
        .save_property(
            space,
            &c.id,
            c.schema_revision,
            PropertyDraft {
                id: None,
                name: name.into(),
                kind,
                options: vec![],
            },
        )
        .unwrap()
}

#[test]
fn search_rechecks_the_projected_text_before_matching_or_making_a_snippet() {
    let (_root, store, space) = setup();
    let outside = store
        .create_page(&space, None, "私密标签第三个字", "file")
        .unwrap();
    let folder = store.create_page(&space, None, "授权区", "file").unwrap();
    let note = store
        .create_page(&space, Some(&folder.info.id), "样稿", "file")
        .unwrap();
    let attachment = store
        .add_image(
            &space,
            &note.info.id,
            "图.png",
            &b"\x89PNG\r\n\x1a\nexample"[..],
        )
        .unwrap()
        .attachment;
    let first = block(
        "paragraph",
        json!({"inline":[
            {"type":"pageReference","pageId":outside.info.id,"label":"候选命中私密标签"}
        ]}),
    );
    let second = block(
        "paragraph",
        json!({"inline":[
            {"type":"text","text":"前方候选命中"},
            {"type":"pageReference","pageId":outside.info.id,"label":"左边界标签"},
            {"type":"text","text":"可见片段"},
            {"type":"pageReference","pageId":outside.info.id,"label":"右边界标签"},
            {"type":"text","text":"尾部"}
        ]}),
    );
    let image = block(
        "attachmentImage",
        json!({"attachmentId":attachment.id,"mediaType":"image/png","alt":"秘密附图说明"}),
    );
    let opaque = block("extensionPayload", json!({"text": "不可见扩展标记"}));
    title_and_body(
        &store,
        &space,
        &note,
        vec![first, second.clone(), image, opaque],
    );

    let grants = vec![Grant::Subtree(folder.info.id.clone())];
    let reader = issue(&store, &space, grants.clone(), false);
    let download = issue(&store, &space, grants, true);
    for after_index in [false, true] {
        if after_index {
            while store.advance_search_index(&space).unwrap().pending != 0 {}
        }
        let search = |query| {
            store
                .integration_search(&space, &reader.token, &request(query))
                .unwrap()
        };
        for forbidden in [
            "私密标签",
            "秘",
            "右边界标签",
            "不可见扩展",
            "可见片段尾部",
            "秘密附图",
        ] {
            assert!(search(forbidden).items.is_empty(), "term: {forbidden}");
        }
        let hits = search("候选命中");
        assert_eq!(hits.items.len(), 1);
        let hit = &hits.items[0];
        assert_eq!(hit.page_id, note.info.id);
        assert_eq!(hit.block_id.as_deref(), Some(second.id.as_str()));
        let snippet = hit.snippet.as_ref().unwrap();
        assert_eq!(snippet.before, "前方");
        assert!(snippet.after.is_empty());
        assert_eq!(
            search("尾部").items[0].block_id.as_deref(),
            Some(second.id.as_str())
        );
        assert_eq!(
            store
                .integration_search(&space, &download.token, &request("秘密附图"))
                .unwrap()
                .items
                .len(),
            1
        );
    }
}

#[test]
fn queries_match_point_access_and_hide_attachment_values_and_predicates() {
    let (_root, store, space) = setup();
    let (summary, container) = store.create_collection(&space, None, "海洋观察").unwrap();
    let c = store.collection(&space, &summary.id).unwrap();
    let c = property(&store, &space, &c, "评分", PropertyKind::Number);
    let c = property(&store, &space, &c, "原件", PropertyKind::Attachment);
    let score = c.properties[1].id.clone();
    let files = c.properties[2].id.clone();
    let strong = store
        .create_page(&space, Some(&container.info.id), "青岚一", "file")
        .unwrap();
    let weak = store
        .create_page(&space, Some(&container.info.id), "青岚二", "file")
        .unwrap();
    let file = store
        .add_attachment(&space, &strong.info.id, "原件.txt", &b"fixture"[..])
        .unwrap()
        .attachment;
    for (page, properties) in [
        (
            &strong,
            BTreeMap::from([(score.clone(), json!(3)), (files.clone(), json!([file.id]))]),
        ),
        (&weak, BTreeMap::from([(score.clone(), json!(1))])),
    ] {
        store
            .save_record(
                &space,
                &c.id,
                &page.info.id,
                1,
                RecordDraft {
                    title: page.info.title.clone(),
                    schema_revision: c.schema_revision,
                    properties,
                },
            )
            .unwrap();
    }

    let only_container = issue(
        &store,
        &space,
        vec![Grant::Page(container.info.id.clone())],
        false,
    );
    assert_eq!(
        store
            .integration_collection(&space, &only_container.token, &c.id)
            .unwrap()
            .schema_revision,
        c.schema_revision
    );
    assert!(
        store
            .integration_collection_query(&space, &only_container.token, &c.id, &Default::default())
            .unwrap()
            .page
            .items
            .is_empty()
    );

    let only_record = issue(
        &store,
        &space,
        vec![Grant::Page(strong.info.id.clone())],
        false,
    );
    let search = store
        .integration_search(&space, &only_record.token, &request("青岚"))
        .unwrap();
    assert_eq!(search.items.len(), 1);
    assert_eq!(search.items[0].collection_id, None);
    assert_eq!(
        store
            .integration_collection(&space, &only_record.token, &c.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );

    let reader = issue(
        &store,
        &space,
        vec![
            Grant::Collection(c.id.clone()),
            Grant::Page(strong.info.id.clone()),
        ],
        false,
    );
    let sorted = IntegrationCollectionQuery {
        sort: Some(IntegrationSort {
            field_id: score,
            direction: SortDirection::Desc,
        }),
        limit: Some(1),
        ..Default::default()
    };
    let first = store
        .integration_collection_query(&space, &reader.token, &c.id, &sorted)
        .unwrap()
        .page;
    assert_eq!(first.items[0].page_id, strong.info.id);
    assert_eq!(first.items[0].properties.len(), 1);
    assert!(first.items[0].properties_restricted);
    let second = store
        .integration_collection_query(
            &space,
            &reader.token,
            &c.id,
            &IntegrationCollectionQuery {
                cursor: first.next_cursor.clone(),
                ..sorted
            },
        )
        .unwrap()
        .page;
    assert_eq!(second.items[0].page_id, weak.info.id);
    assert_eq!(second.snapshot_revision, first.snapshot_revision);
    assert!(second.next_cursor.is_none());

    let nonempty = IntegrationFilter {
        field_id: files.clone(),
        operator: FilterOperator::NotEmpty,
        value: None,
    };
    let filtered = IntegrationCollectionQuery {
        filters: vec![nonempty.clone()],
        ..Default::default()
    };
    assert_eq!(
        store
            .integration_collection_query(&space, &reader.token, &c.id, &filtered)
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    let search_filter = IntegrationSearchRequest {
        filters: vec![nonempty],
        collection_id: Some(c.id.clone()),
        ..request("青岚")
    };
    assert_eq!(
        store
            .integration_search(&space, &reader.token, &search_filter)
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );

    let with_assets = issue(&store, &space, vec![Grant::Collection(c.id.clone())], true);
    let matched = store
        .integration_collection_query(&space, &with_assets.token, &c.id, &filtered)
        .unwrap()
        .page
        .items;
    assert_eq!(matched.len(), 1);
    assert_eq!(matched[0].properties[&files], json!([file.id]));
    assert!(!matched[0].properties_restricted);
    assert_eq!(
        store
            .integration_search(&space, &with_assets.token, &search_filter)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn query_cursor_binds_subject_and_filter_and_survives_index_maintenance_only() {
    let (_root, store, space) = setup();
    let folder = store.create_page(&space, None, "根", "file").unwrap();
    let pages: Vec<_> = (0..3)
        .map(|index| {
            store
                .create_page(
                    &space,
                    Some(&folder.info.id),
                    &format!("搜索快照 {index}"),
                    "file",
                )
                .unwrap()
        })
        .collect();
    let grants = vec![Grant::Subtree(folder.info.id.clone())];
    let first = issue(&store, &space, grants.clone(), false);
    let second = issue(&store, &space, grants, false);
    let mut query = IntegrationSearchRequest {
        limit: Some(1),
        ..request("搜索快照")
    };
    let initial = store
        .integration_search(&space, &first.token, &query)
        .unwrap();
    query.cursor = initial.next_cursor.clone();
    while store.advance_search_index(&space).unwrap().pending != 0 {}
    let continued = store
        .integration_search(&space, &first.token, &query)
        .unwrap();
    assert_eq!(continued.snapshot_revision, initial.snapshot_revision);
    assert_ne!(continued.items[0].page_id, initial.items[0].page_id);
    store.rebuild_search_index(&space).unwrap();
    assert_eq!(
        store
            .integration_search(&space, &first.token, &query)
            .unwrap()
            .items[0]
            .page_id,
        continued.items[0].page_id
    );
    assert_eq!(
        store
            .integration_search(&space, &second.token, &query)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    query.kind = Some(IntegrationSearchKind::Record);
    assert_eq!(
        store
            .integration_search(&space, &first.token, &query)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    query.kind = None;

    let original = query.cursor.clone().unwrap();
    query.cursor = Some(original.replacen("1.1.", "1.2.", 1));
    assert_eq!(
        store
            .integration_search(&space, &first.token, &query)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    query.cursor = Some(original);
    assert!(
        store
            .integration_search(&space, &first.token, &query)
            .is_ok()
    );
    store.move_page(&space, &pages[0].info.id, 1, None).unwrap();
    assert_eq!(
        store
            .integration_search(&space, &first.token, &query)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    query.cursor = None;
    query.limit = Some(50);
    let new = store
        .integration_search(&space, &first.token, &query)
        .unwrap();
    assert_eq!(new.items.len(), 2);
    assert!(!new.items.iter().any(|hit| hit.page_id == pages[0].info.id));
    store
        .revoke_integration(&space, &first.integration.id)
        .unwrap();
    assert_eq!(
        store
            .integration_search(&space, &first.token, &query)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
}

#[test]
fn version_six_upgrade_preserves_integrations_and_mints_a_workspace_cursor_key() {
    let (root, store, space) = setup();
    let parent = store.create_page(&space, None, "迁移样例", "file").unwrap();
    let grant = issue(&store, &space, vec![Grant::Subtree(parent.info.id)], false);
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    support::remove_v7_schema(&db);
    drop(db);
    assert_eq!(
        store
            .integration_search(&space, &grant.token, &request("迁移样例"))
            .unwrap()
            .items
            .len(),
        1
    );
    let backup = root
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v6.sqlite"));
    let backup = Connection::open(backup).unwrap();
    assert_eq!(
        backup
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        6
    );
    assert_eq!(
        backup
            .query_row("SELECT count(*) FROM integrations", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}
