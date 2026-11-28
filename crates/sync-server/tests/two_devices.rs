// SPDX-License-Identifier: AGPL-3.0-or-later

//! Two local stores synchronizing through the service engine.

use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use zhiye_core::sync_protocol::{BlobQuery, PushResult};
use zhiye_core::{
    Block, Page, PageDraft, PropertyDraft, PropertyKind, RecordDraft, RelationDraft, Store,
    SyncReport,
};
use zhiye_sync_server::{Device, Engine};

struct Client {
    _directory: tempfile::TempDir,
    store: Store,
    device: Device,
}

struct Setup {
    engine: Engine,
    first: Client,
    second: Client,
    workspace: String,
}

fn paragraph(text: &str) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline": [{"type": "text", "text": text}]}),
        children: vec![],
        extra: Default::default(),
    }
}

fn client(engine: &Engine, name: &str) -> Client {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let issued = engine.register_device(name).unwrap();
    let device = engine.authenticate(&issued.token).unwrap();
    Client {
        _directory: directory,
        store,
        device,
    }
}

/// The first device owns a workspace; the second receives it from the service.
fn setup() -> Setup {
    let engine = Engine::open_for_tests().unwrap();
    let first = client(&engine, "书房电脑");
    let second = client(&engine, "笔记本");
    let workspace = first.store.create_workspace("个人资料").unwrap();
    engine
        .put_workspace(&first.device, &workspace.id, &workspace.name)
        .unwrap();
    let listed = engine.workspaces(&second.device).unwrap();
    second
        .store
        .create_synced_workspace(&listed[0].id, &listed[0].name)
        .unwrap();
    Setup {
        engine,
        first,
        second,
        workspace: workspace.id,
    }
}

/// Pull, push everything, pull again; the order a desktop client uses.
fn sync(engine: &Engine, client: &Client, workspace: &str) -> SyncReport {
    let mut total = SyncReport::default();
    let mut merge = |report: SyncReport| {
        total.accepted += report.accepted;
        total.applied += report.applied;
        total.conflicts.extend(report.conflicts);
        total.pending = report.pending;
        total.cursor = report.cursor;
    };
    let pull = |merge: &mut dyn FnMut(SyncReport)| loop {
        let cursor = client.store.sync_status(workspace).unwrap().cursor;
        let response = engine
            .pull(&client.device, workspace, cursor, None)
            .unwrap();
        download(engine, client, workspace, &response.blob_hashes());
        let more = response.has_more;
        merge(client.store.sync_apply_pull(workspace, &response).unwrap());
        if !more {
            break;
        }
    };
    pull(&mut merge);
    loop {
        let push = client.store.sync_prepare_push(workspace).unwrap();
        let Some(request) = push.request else { break };
        upload(engine, client, workspace, &request.blob_hashes());
        let response = engine.push(&client.device, workspace, &request).unwrap();
        merge(client.store.sync_apply_push(workspace, &response).unwrap());
        if push.remaining == 0 {
            break;
        }
    }
    pull(&mut merge);
    total
}

fn upload(engine: &Engine, client: &Client, workspace: &str, hashes: &[String]) {
    let query = BlobQuery {
        hashes: hashes.to_vec(),
    };
    for hash in engine
        .missing_blobs(&client.device, workspace, &query)
        .unwrap()
        .missing
    {
        let mut bytes = Vec::new();
        client
            .store
            .sync_open_blob(workspace, &hash)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        engine
            .put_blob(&client.device, workspace, &hash, &bytes)
            .unwrap();
    }
}

fn download(engine: &Engine, client: &Client, workspace: &str, hashes: &[String]) {
    for hash in client.store.sync_missing_blobs(workspace, hashes).unwrap() {
        let bytes = engine.blob(&client.device, workspace, &hash).unwrap();
        client
            .store
            .sync_store_blob(workspace, &hash, bytes.as_slice())
            .unwrap();
    }
}

/// Attachments (id, name, SHA-256) and cover of each active page.
type Files = BTreeMap<String, (Vec<(String, String, String)>, Option<String>)>;

fn files(store: &Store, workspace: &str) -> Files {
    store
        .pages(workspace)
        .unwrap()
        .into_iter()
        .filter(|info| info.trash_root_id.is_none())
        .map(|info| {
            let page = store.page(workspace, &info.id).unwrap();
            let attachments = store
                .attachments(workspace, &info.id)
                .unwrap()
                .into_iter()
                .map(|item| (item.id, item.name, item.sha256))
                .collect();
            (
                info.id,
                (attachments, page.cover.map(|cover| cover.attachment_id)),
            )
        })
        .collect()
}

fn png() -> Vec<u8> {
    std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.verify/rich-blocks/fixtures/validation-image.png"
    ))
    .unwrap()
}

/// Comparable content: id -> (parent, title, trashed, blocks).
fn tree(
    store: &Store,
    workspace: &str,
) -> BTreeMap<String, (Option<String>, String, bool, String)> {
    store
        .pages(workspace)
        .unwrap()
        .into_iter()
        .map(|info| {
            let page = store.page(workspace, &info.id).unwrap();
            (
                info.id.clone(),
                (
                    info.parent_id,
                    info.title,
                    info.trash_root_id.is_some(),
                    serde_json::to_string(&page.blocks).unwrap(),
                ),
            )
        })
        .collect()
}

fn save(store: &Store, workspace: &str, page: &Page, title: &str, blocks: Vec<Block>) -> Page {
    let current = store.page(workspace, &page.info.id).unwrap();
    store
        .save_page(
            workspace,
            &page.info.id,
            current.info.revision,
            PageDraft {
                title: title.into(),
                blocks,
            },
        )
        .unwrap();
    store.page(workspace, &page.info.id).unwrap()
}

fn empty_trash(store: &Store, workspace: &str) {
    let preview = store.preview_empty_trash(workspace).unwrap();
    store.empty_trash(workspace, &preview.confirmation).unwrap();
}

#[test]
fn second_device_receives_tree_and_edits_flow_back() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let parent = first
        .store
        .create_page(&workspace, None, "项目", "📁")
        .unwrap();
    let child = first
        .store
        .create_page(&workspace, Some(&parent.info.id), "会议纪要", "📄")
        .unwrap();
    let child = save(
        &first.store,
        &workspace,
        &child,
        "会议纪要",
        vec![paragraph("周一讨论同步")],
    );

    let sent = sync(&engine, &first, &workspace);
    assert_eq!(sent.accepted, 2);
    assert!(sent.conflicts.is_empty());
    let received = sync(&engine, &second, &workspace);
    assert_eq!(received.applied, 2);
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
    let copy = second.store.page(&workspace, &child.info.id).unwrap();
    assert_eq!(
        copy.blocks[0].id, child.blocks[0].id,
        "block identity is kept"
    );

    // Nothing left to send after receiving.
    assert!(
        second
            .store
            .sync_prepare_push(&workspace)
            .unwrap()
            .request
            .is_none()
    );

    let edited = save(
        &second.store,
        &workspace,
        &child,
        "会议纪要（已整理）",
        vec![child.blocks[0].clone(), paragraph("笔记本补充")],
    );
    assert_eq!(sync(&engine, &second, &workspace).accepted, 1);
    let applied = sync(&engine, &first, &workspace);
    assert_eq!(applied.applied, 1);
    assert!(applied.conflicts.is_empty());
    let back = first.store.page(&workspace, &child.info.id).unwrap();
    assert_eq!(back.info.title, "会议纪要（已整理）");
    assert_eq!(back.blocks, edited.blocks);
    assert_eq!(
        back.info.revision,
        child.info.revision + 1,
        "one local revision per remote change"
    );
    assert_eq!(
        first
            .store
            .history(&workspace, &child.info.id)
            .unwrap()
            .len(),
        back.info.revision as usize
    );
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
}

#[test]
fn concurrent_offline_edits_keep_both_versions() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let page = first
        .store
        .create_page(&workspace, None, "读书笔记", "📄")
        .unwrap();
    sync(&engine, &first, &workspace);
    sync(&engine, &second, &workspace);

    save(
        &first.store,
        &workspace,
        &page,
        "读书笔记",
        vec![paragraph("书房的修改")],
    );
    save(
        &second.store,
        &workspace,
        &page,
        "读书笔记 v2",
        vec![paragraph("笔记本的修改")],
    );

    assert!(sync(&engine, &first, &workspace).conflicts.is_empty());
    let report = sync(&engine, &second, &workspace);
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    let conflict = &report.conflicts[0];
    assert_eq!(conflict.kind, "EDIT_EDIT");
    assert_eq!(conflict.page_id, page.info.id);
    let copy_id = conflict.copy_page_id.clone().unwrap();

    let original = second.store.page(&workspace, &page.info.id).unwrap();
    assert_eq!(original.info.title, "读书笔记");
    assert_eq!(
        original.blocks[0].payload["inline"][0]["text"],
        "书房的修改"
    );
    let copy = second.store.page(&workspace, &copy_id).unwrap();
    assert_eq!(copy.info.title, "读书笔记 v2（冲突副本）");
    assert_eq!(copy.blocks[0].payload["inline"][0]["text"], "笔记本的修改");
    assert_ne!(copy.blocks[0].id, original.blocks[0].id);
    // The second device's local revision is preserved in history.
    let history = second.store.history(&workspace, &page.info.id).unwrap();
    assert!(
        history
            .iter()
            .any(|revision| revision.info.title == "读书笔记 v2")
    );

    assert!(sync(&engine, &first, &workspace).conflicts.is_empty());
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
    assert_eq!(first.store.pages(&workspace).unwrap().len(), 2);
}

#[test]
fn delete_and_edit_conflicts_keep_the_edit() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let kept = first
        .store
        .create_page(&workspace, None, "仍在使用", "📄")
        .unwrap();
    let restored = first
        .store
        .create_page(&workspace, None, "被重新编辑", "📄")
        .unwrap();
    sync(&engine, &first, &workspace);
    sync(&engine, &second, &workspace);

    // First deletes `kept` permanently while second edits it offline.
    let current = first.store.page(&workspace, &kept.info.id).unwrap();
    first
        .store
        .archive_page(&workspace, &kept.info.id, current.info.revision)
        .unwrap();
    empty_trash(&first.store, &workspace);
    save(
        &second.store,
        &workspace,
        &kept,
        "仍在使用",
        vec![paragraph("离线补充")],
    );
    // Second deletes `restored` permanently while first edits it offline.
    let current = second.store.page(&workspace, &restored.info.id).unwrap();
    second
        .store
        .archive_page(&workspace, &restored.info.id, current.info.revision)
        .unwrap();
    empty_trash(&second.store, &workspace);
    save(
        &first.store,
        &workspace,
        &restored,
        "被重新编辑",
        vec![paragraph("书房补充")],
    );

    sync(&engine, &first, &workspace);
    let report = sync(&engine, &second, &workspace);
    let mut kinds: Vec<_> = report
        .conflicts
        .iter()
        .map(|c| (c.kind, c.page_id.clone()))
        .collect();
    kinds.sort();
    let mut expected = vec![
        ("DELETE_EDIT", kept.info.id.clone()),
        ("EDIT_DELETE", restored.info.id.clone()),
    ];
    expected.sort();
    assert_eq!(kinds, expected, "{report:?}");
    sync(&engine, &first, &workspace);

    for client in [&first, &second] {
        let pages = tree(&client.store, &workspace);
        assert_eq!(pages.len(), 2);
        assert!(pages[&kept.info.id].3.contains("离线补充"));
        assert!(pages[&restored.info.id].3.contains("书房补充"));
    }
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
}

#[test]
fn trash_and_permanent_deletion_propagate() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let root = first
        .store
        .create_page(&workspace, None, "旧项目", "📁")
        .unwrap();
    let child = first
        .store
        .create_page(&workspace, Some(&root.info.id), "旧资料", "📄")
        .unwrap();
    sync(&engine, &first, &workspace);
    sync(&engine, &second, &workspace);

    let current = first.store.page(&workspace, &root.info.id).unwrap();
    first
        .store
        .archive_page(&workspace, &root.info.id, current.info.revision)
        .unwrap();
    sync(&engine, &first, &workspace);
    sync(&engine, &second, &workspace);
    for id in [&root.info.id, &child.info.id] {
        let page = second.store.page(&workspace, id).unwrap();
        assert_eq!(
            page.info.trash_root_id.as_deref(),
            Some(root.info.id.as_str())
        );
    }
    // The trashed subtree can be restored from the second device's trash.
    let current = second.store.page(&workspace, &root.info.id).unwrap();
    second
        .store
        .restore_page(&workspace, &root.info.id, current.info.revision)
        .unwrap();
    sync(&engine, &second, &workspace);
    sync(&engine, &first, &workspace);
    assert!(
        first
            .store
            .page(&workspace, &child.info.id)
            .unwrap()
            .info
            .trash_root_id
            .is_none()
    );
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );

    // Permanent deletion elsewhere moves pages into the local trash, never erases them.
    let current = first.store.page(&workspace, &root.info.id).unwrap();
    first
        .store
        .archive_page(&workspace, &root.info.id, current.info.revision)
        .unwrap();
    empty_trash(&first.store, &workspace);
    assert!(first.store.pages(&workspace).unwrap().is_empty());
    sync(&engine, &first, &workspace);
    let report = sync(&engine, &second, &workspace);
    assert!(report.conflicts.is_empty(), "{report:?}");
    let left = second.store.pages(&workspace).unwrap();
    assert_eq!(left.len(), 2);
    assert!(left.iter().all(|page| page.trash_root_id.is_some()));
    // Clearing that trash does not send another deletion.
    empty_trash(&second.store, &workspace);
    assert!(
        second
            .store
            .sync_prepare_push(&workspace)
            .unwrap()
            .request
            .is_none()
    );
}

#[test]
fn lost_push_responses_are_replayed_without_duplicates() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    first
        .store
        .create_page(&workspace, None, "离线一", "📄")
        .unwrap();
    first
        .store
        .create_page(&workspace, None, "离线二", "📄")
        .unwrap();
    let request = first
        .store
        .sync_prepare_push(&workspace)
        .unwrap()
        .request
        .unwrap();
    let delivered = engine.push(&first.device, &workspace, &request).unwrap();
    // The response is lost; the client prepares again and retries.
    let retry = first
        .store
        .sync_prepare_push(&workspace)
        .unwrap()
        .request
        .unwrap();
    assert_eq!(retry, request, "unchanged content keeps its operation IDs");
    let replayed = engine.push(&first.device, &workspace, &retry).unwrap();
    assert_eq!(replayed, delivered);
    let report = first.store.sync_apply_push(&workspace, &replayed).unwrap();
    assert_eq!(report.accepted, 2);
    // Applying the same response twice is harmless.
    assert_eq!(
        first
            .store
            .sync_apply_push(&workspace, &replayed)
            .unwrap()
            .accepted,
        0
    );
    assert!(
        first
            .store
            .sync_prepare_push(&workspace)
            .unwrap()
            .request
            .is_none()
    );
    assert!(
        replayed
            .results
            .iter()
            .all(|result| matches!(result, PushResult::Accepted { version: 1, .. }))
    );
    sync(&engine, &second, &workspace);
    assert_eq!(second.store.pages(&workspace).unwrap().len(), 2);
}

#[test]
fn crossing_moves_do_not_form_a_cycle() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let left = first
        .store
        .create_page(&workspace, None, "甲", "📄")
        .unwrap();
    let right = first
        .store
        .create_page(&workspace, None, "乙", "📄")
        .unwrap();
    sync(&engine, &first, &workspace);
    sync(&engine, &second, &workspace);

    first
        .store
        .move_page(&workspace, &left.info.id, 1, Some(&right.info.id))
        .unwrap();
    second
        .store
        .move_page(&workspace, &right.info.id, 1, Some(&left.info.id))
        .unwrap();
    sync(&engine, &first, &workspace);
    let report = sync(&engine, &second, &workspace);
    assert!(
        report
            .conflicts
            .iter()
            .any(|conflict| conflict.kind == "PARENT_CYCLE"),
        "{report:?}"
    );
    sync(&engine, &first, &workspace);
    let pages = tree(&first.store, &workspace);
    assert_eq!(pages, tree(&second.store, &workspace));
    let roots = pages.values().filter(|page| page.0.is_none()).count();
    assert_eq!(roots, 1, "{pages:?}");
}

#[test]
fn databases_records_and_relations_sync() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let ws = workspace.as_str();
    let (books, _) = first.store.create_collection(ws, None, "书单").unwrap();
    let (authors, _) = first.store.create_collection(ws, None, "作者").unwrap();
    let schema = first.store.collection(ws, &books.id).unwrap();
    let schema = first
        .store
        .save_property(
            ws,
            &books.id,
            schema.schema_revision,
            PropertyDraft {
                id: None,
                name: "评分".into(),
                kind: PropertyKind::Number,
                options: vec![],
            },
        )
        .unwrap();
    let schema = first
        .store
        .create_relation(
            ws,
            &books.id,
            schema.schema_revision,
            RelationDraft {
                name: "作者".into(),
                collection_id: authors.id.clone(),
                inverse_name: Some("著作".into()),
            },
        )
        .unwrap();
    let field = |name: &str| {
        schema
            .properties
            .iter()
            .find(|item| item.name == name)
            .unwrap()
            .clone()
    };
    let (rating, relation, status) = (field("评分"), field("作者"), field("状态"));
    let author = first
        .store
        .create_page(ws, Some(&authors.container_page_id), "卡尼曼", "file")
        .unwrap();
    let book = first
        .store
        .create_page(ws, Some(&books.container_page_id), "思考，快与慢", "book")
        .unwrap();
    first
        .store
        .save_record(
            ws,
            &books.id,
            &book.info.id,
            book.info.revision,
            RecordDraft {
                title: "思考，快与慢".into(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([
                    (rating.id.clone(), json!(9.5)),
                    (status.id.clone(), json!(status.options[1].id)),
                    (relation.id.clone(), json!({"add": [author.info.id]})),
                ]),
            },
        )
        .unwrap();
    assert!(first.store.sync_status(ws).unwrap().skipped.is_empty());

    sync(&engine, &first, ws);
    let report = sync(&engine, &second, ws);
    assert!(report.pending.is_empty(), "{report:?}");
    assert!(report.conflicts.is_empty(), "{report:?}");
    for id in [&books.id, &authors.id] {
        let (mine, theirs) = (
            first.store.collection(ws, id).unwrap(),
            second.store.collection(ws, id).unwrap(),
        );
        assert_eq!(
            (&mine.container_page_id, &mine.properties, &mine.views),
            (&theirs.container_page_id, &theirs.properties, &theirs.views)
        );
    }
    let record = second.store.record(ws, &books.id, &book.info.id).unwrap();
    assert_eq!(record.properties[&rating.id], json!(9.5));
    assert_eq!(record.properties[&status.id], json!(status.options[1].id));
    assert_eq!(record.properties[&relation.id], json!([author.info.id]));
    let inverse = relation
        .relation
        .as_ref()
        .unwrap()
        .inverse_property_id
        .clone()
        .unwrap();
    let mirrored = second
        .store
        .record(ws, &authors.id, &author.info.id)
        .unwrap();
    assert_eq!(mirrored.properties[&inverse], json!([book.info.id]));
    assert_eq!(
        first
            .store
            .record(ws, &books.id, &book.info.id)
            .unwrap()
            .created_at,
        record.created_at
    );
    assert_eq!(tree(&first.store, ws), tree(&second.store, ws));

    // A record edited on the second device flows back.
    let current = second.store.page(ws, &book.info.id).unwrap();
    let revision = second
        .store
        .collection(ws, &books.id)
        .unwrap()
        .schema_revision;
    second
        .store
        .save_record(
            ws,
            &books.id,
            &book.info.id,
            current.info.revision,
            RecordDraft {
                title: "思考，快与慢".into(),
                schema_revision: revision,
                properties: BTreeMap::from([(rating.id.clone(), json!(7))]),
            },
        )
        .unwrap();
    sync(&engine, &second, ws);
    assert!(sync(&engine, &first, ws).conflicts.is_empty());
    assert_eq!(
        first
            .store
            .record(ws, &books.id, &book.info.id)
            .unwrap()
            .properties[&rating.id],
        json!(7)
    );

    // Both devices add a property offline: the definitions are merged.
    for (client, name) in [(&first, "页数"), (&second, "出版社")] {
        let schema = client.store.collection(ws, &books.id).unwrap();
        client
            .store
            .save_property(
                ws,
                &books.id,
                schema.schema_revision,
                PropertyDraft {
                    id: None,
                    name: name.into(),
                    kind: PropertyKind::Text,
                    options: vec![],
                },
            )
            .unwrap();
    }
    sync(&engine, &first, ws);
    let merged = sync(&engine, &second, ws);
    assert!(
        merged
            .conflicts
            .iter()
            .any(|conflict| conflict.kind == "SCHEMA_MERGED"
                && conflict.page_id == books.container_page_id),
        "{merged:?}"
    );
    sync(&engine, &first, ws);
    for client in [&first, &second] {
        let names: Vec<_> = client
            .store
            .collection(ws, &books.id)
            .unwrap()
            .properties
            .into_iter()
            .map(|item| item.name)
            .collect();
        assert!(
            names.contains(&"页数".to_owned()) && names.contains(&"出版社".to_owned()),
            "{names:?}"
        );
        assert_eq!(client.store.sync_status(ws).unwrap().unsent, 0);
    }
    assert_eq!(
        first.store.collection(ws, &books.id).unwrap().properties,
        second.store.collection(ws, &books.id).unwrap().properties
    );

    // Both devices edit the same record: the copy stays in the database without relations.
    for (client, value) in [(&first, 1), (&second, 2)] {
        let current = client.store.page(ws, &book.info.id).unwrap();
        let revision = client
            .store
            .collection(ws, &books.id)
            .unwrap()
            .schema_revision;
        client
            .store
            .save_record(
                ws,
                &books.id,
                &book.info.id,
                current.info.revision,
                RecordDraft {
                    title: "思考，快与慢".into(),
                    schema_revision: revision,
                    properties: BTreeMap::from([(rating.id.clone(), json!(value))]),
                },
            )
            .unwrap();
    }
    sync(&engine, &first, ws);
    let report = sync(&engine, &second, ws);
    let copy = report
        .conflicts
        .iter()
        .find(|conflict| conflict.kind == "EDIT_EDIT")
        .and_then(|conflict| conflict.copy_page_id.clone())
        .unwrap_or_else(|| panic!("{report:?}"));
    let copied = second.store.record(ws, &books.id, &copy).unwrap();
    assert_eq!(copied.properties[&rating.id], json!(2));
    assert!(!copied.properties.contains_key(&relation.id));
    assert_eq!(
        second
            .store
            .record(ws, &books.id, &book.info.id)
            .unwrap()
            .properties[&rating.id],
        json!(1)
    );
    sync(&engine, &first, ws);
    assert!(first.store.record(ws, &books.id, &copy).is_ok());
    assert_eq!(tree(&first.store, ws), tree(&second.store, ws));
    for client in [&first, &second] {
        assert_eq!(client.store.sync_status(ws).unwrap().unsent, 0);
    }
}

#[test]
fn child_received_before_parent_waits_then_lands() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let parent = first
        .store
        .create_page(&workspace, None, "上级", "📁")
        .unwrap();
    let child = first
        .store
        .create_page(&workspace, Some(&parent.info.id), "下级", "📄")
        .unwrap();
    sync(&engine, &first, &workspace);

    // Deliver the child alone first, as a page boundary might.
    let all = engine.pull(&second.device, &workspace, 0, None).unwrap();
    let child_only = zhiye_core::sync_protocol::PullResponse {
        changes: all
            .changes
            .iter()
            .filter(|change| change.object_id == child.info.id)
            .cloned()
            .collect(),
        next_seq: 0,
        has_more: true,
    };
    let waiting = second
        .store
        .sync_apply_pull(&workspace, &child_only)
        .unwrap();
    assert_eq!(waiting.pending.len(), 1);
    assert!(second.store.pages(&workspace).unwrap().is_empty());
    let landed = second.store.sync_apply_pull(&workspace, &all).unwrap();
    assert!(landed.pending.is_empty(), "{landed:?}");
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
}

fn block(kind: &str, payload: serde_json::Value) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children: vec![],
        extra: Default::default(),
    }
}

#[test]
fn attachments_images_and_covers_follow_their_pages() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let page = first
        .store
        .create_page(&workspace, None, "观测记录", "📄")
        .unwrap();
    let id = page.info.id.clone();
    let image = first
        .store
        .add_image(&workspace, &id, "海湾.png", png().as_slice())
        .unwrap()
        .attachment;
    let file = first
        .store
        .add_attachment(&workspace, &id, "研究备注.txt", "知页附件原件\n".as_bytes())
        .unwrap()
        .attachment;
    save(
        &first.store,
        &workspace,
        &page,
        "观测记录",
        vec![
            paragraph("图片如下"),
            block(
                "attachmentImage",
                json!({"attachmentId": image.id, "mediaType": image.media_type, "alt": "海湾"}),
            ),
            block("attachmentFile", json!({"attachmentId": file.id})),
        ],
    );
    let current = first.store.page(&workspace, &id).unwrap();
    first
        .store
        .set_page_appearance(
            &workspace,
            &id,
            current.info.revision,
            "📄",
            Some(&image.id),
        )
        .unwrap();
    assert!(
        first
            .store
            .sync_status(&workspace)
            .unwrap()
            .skipped
            .is_empty()
    );

    sync(&engine, &first, &workspace);
    assert_eq!(sync(&engine, &second, &workspace).applied, 1);
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
    assert_eq!(
        files(&first.store, &workspace),
        files(&second.store, &workspace)
    );
    let mut copied = Vec::new();
    second
        .store
        .copy_attachment(&workspace, &id, &image.id, &mut copied)
        .unwrap();
    assert_eq!(copied, png());
    assert_eq!(
        second
            .store
            .page(&workspace, &id)
            .unwrap()
            .cover
            .unwrap()
            .attachment_id,
        image.id
    );

    // Renaming an attachment does not create a page revision but still syncs.
    second
        .store
        .rename_attachment(&workspace, &id, &file.id, "实验原件.txt")
        .unwrap();
    assert_eq!(second.store.sync_status(&workspace).unwrap().unsent, 1);
    sync(&engine, &second, &workspace);
    sync(&engine, &first, &workspace);
    assert_eq!(
        first
            .store
            .attachment(&workspace, &id, &file.id)
            .unwrap()
            .name,
        "实验原件.txt"
    );
    for client in [&first, &second] {
        assert_eq!(client.store.sync_status(&workspace).unwrap().unsent, 0);
    }

    // An edit conflict copies the attachments for the conflict copy.
    let base = second.store.page(&workspace, &id).unwrap();
    save(
        &first.store,
        &workspace,
        &base,
        "观测记录（书房）",
        base.blocks.clone(),
    );
    save(
        &second.store,
        &workspace,
        &base,
        "观测记录（笔记本）",
        base.blocks.clone(),
    );
    sync(&engine, &first, &workspace);
    let report = sync(&engine, &second, &workspace);
    assert_eq!(report.conflicts.len(), 1, "{report:?}");
    let copy_id = report.conflicts[0].copy_page_id.clone().unwrap();
    let copy = second.store.page(&workspace, &copy_id).unwrap();
    let copy_files = second.store.attachments(&workspace, &copy_id).unwrap();
    assert_eq!(copy_files.len(), 2);
    assert!(
        copy_files
            .iter()
            .all(|item| item.id != image.id && item.id != file.id)
    );
    let copy_image = copy_files
        .iter()
        .find(|item| item.name == "海湾.png")
        .unwrap();
    assert_eq!(copy.blocks[1].payload["attachmentId"], copy_image.id);
    assert_eq!(copy.cover.unwrap().attachment_id, copy_image.id);
    sync(&engine, &first, &workspace);
    assert_eq!(
        tree(&first.store, &workspace),
        tree(&second.store, &workspace)
    );
    assert_eq!(
        files(&first.store, &workspace),
        files(&second.store, &workspace)
    );
    for client in [&first, &second] {
        assert_eq!(client.store.sync_status(&workspace).unwrap().unsent, 0);
    }
}

#[test]
fn pages_wait_for_originals_and_missing_originals_are_reported() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let page = first
        .store
        .create_page(&workspace, None, "附件页", "📄")
        .unwrap();
    let file = first
        .store
        .add_attachment(&workspace, &page.info.id, "原件.bin", [7_u8; 64].as_slice())
        .unwrap()
        .attachment;
    save(
        &first.store,
        &workspace,
        &page,
        "附件页",
        vec![block("attachmentFile", json!({"attachmentId": file.id}))],
    );
    sync(&engine, &first, &workspace);

    // Applying before the original is downloaded parks the page.
    let response = engine.pull(&second.device, &workspace, 0, None).unwrap();
    assert_eq!(response.blob_hashes(), std::slice::from_ref(&file.sha256));
    let waiting = second.store.sync_apply_pull(&workspace, &response).unwrap();
    assert_eq!(waiting.pending.len(), 1);
    assert!(
        waiting.pending[0].reason.contains("附件原件"),
        "{waiting:?}"
    );
    assert!(second.store.pages(&workspace).unwrap().is_empty());
    assert!(
        second
            .store
            .sync_store_blob(&workspace, &file.sha256, [8_u8; 64].as_slice())
            .is_err(),
        "content that does not match the hash is rejected"
    );
    download(&engine, &second, &workspace, &response.blob_hashes());
    let landed = second.store.sync_apply_pull(&workspace, &response).unwrap();
    assert!(landed.pending.is_empty(), "{landed:?}");
    assert_eq!(
        files(&first.store, &workspace),
        files(&second.store, &workspace)
    );

    // A page whose original vanished locally is reported instead of sent.
    std::fs::remove_file(
        first
            ._directory
            .path()
            .join(&workspace)
            .join("blobs")
            .join(&file.sha256),
    )
    .unwrap();
    let current = first.store.page(&workspace, &page.info.id).unwrap();
    save(
        &first.store,
        &workspace,
        &current,
        "附件页（改）",
        current.blocks.clone(),
    );
    let status = first.store.sync_status(&workspace).unwrap();
    assert_eq!(status.skipped[0].reason, "ORIGINAL_MISSING");
    assert!(
        first
            .store
            .sync_prepare_push(&workspace)
            .unwrap()
            .request
            .is_none()
    );
}

#[test]
fn records_wait_for_related_records_and_databases_follow_the_trash() {
    let Setup {
        engine,
        first,
        second,
        workspace,
    } = setup();
    let ws = workspace.as_str();
    let (books, _) = first.store.create_collection(ws, None, "书单").unwrap();
    let schema = first.store.collection(ws, &books.id).unwrap();
    let schema = first
        .store
        .create_relation(
            ws,
            &books.id,
            schema.schema_revision,
            RelationDraft {
                name: "相关".into(),
                collection_id: books.id.clone(),
                inverse_name: None,
            },
        )
        .unwrap();
    let relation = schema
        .properties
        .iter()
        .find(|item| item.name == "相关")
        .unwrap()
        .id
        .clone();
    let target = first
        .store
        .create_page(ws, Some(&books.container_page_id), "被引用的书", "book")
        .unwrap();
    let source = first
        .store
        .create_page(ws, Some(&books.container_page_id), "引用别人的书", "book")
        .unwrap();
    first
        .store
        .save_record(
            ws,
            &books.id,
            &source.info.id,
            source.info.revision,
            RecordDraft {
                title: "引用别人的书".into(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([(relation.clone(), json!({"add": [target.info.id]}))]),
            },
        )
        .unwrap();
    sync(&engine, &first, ws);

    // Deliver everything except the related record.
    let all = engine.pull(&second.device, ws, 0, None).unwrap();
    let partial = zhiye_core::sync_protocol::PullResponse {
        changes: all
            .changes
            .iter()
            .filter(|change| change.object_id != target.info.id)
            .cloned()
            .collect(),
        next_seq: 0,
        has_more: true,
    };
    let waiting = second.store.sync_apply_pull(ws, &partial).unwrap();
    assert_eq!(waiting.pending.len(), 1, "{waiting:?}");
    assert_eq!(waiting.pending[0].object_id, source.info.id);
    assert!(waiting.pending[0].reason.contains("关联"));
    let landed = second.store.sync_apply_pull(ws, &all).unwrap();
    assert!(landed.pending.is_empty(), "{landed:?}");
    assert_eq!(
        second
            .store
            .record(ws, &books.id, &source.info.id)
            .unwrap()
            .properties[&relation],
        json!([target.info.id])
    );
    // Catch up the cursor through the normal path.
    sync(&engine, &second, ws);
    assert_eq!(tree(&first.store, ws), tree(&second.store, ws));

    // Trashing the database moves its records along; purging sends tombstones.
    let container = first.store.page(ws, &books.container_page_id).unwrap();
    first
        .store
        .archive_page(ws, &container.info.id, container.info.revision)
        .unwrap();
    sync(&engine, &first, ws);
    sync(&engine, &second, ws);
    assert_eq!(tree(&first.store, ws), tree(&second.store, ws));
    assert!(
        second
            .store
            .pages(ws)
            .unwrap()
            .iter()
            .all(|page| page.trash_root_id.as_deref() == Some(container.info.id.as_str()))
    );
    empty_trash(&first.store, ws);
    let sent = sync(&engine, &first, ws);
    assert_eq!(
        sent.accepted, 4,
        "container, definition and two records: {sent:?}"
    );
    let report = sync(&engine, &second, ws);
    assert!(report.conflicts.is_empty(), "{report:?}");
    empty_trash(&second.store, ws);
    for client in [&first, &second] {
        assert!(client.store.pages(ws).unwrap().is_empty());
        assert_eq!(client.store.sync_status(ws).unwrap().unsent, 0);
    }
}
