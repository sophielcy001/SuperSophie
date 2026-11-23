// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::{io::Cursor, path::Path, sync::Barrier, thread};
use zhiye_core::{
    Block, IntegrationDraft, IntegrationGrant as Grant, IntegrationMutation as Mutation,
    IntegrationScope as Scope, IssuedIntegration, PageDraft, PagePatch, ProposalAction,
    ProposalStatus, Store,
};

fn setup() -> (tempfile::TempDir, Store, String) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("测试整合").unwrap();
    (root, store, space.id)
}

fn issue(store: &Store, space: &str, scopes: &[Scope], grants: Vec<Grant>) -> IssuedIntegration {
    store
        .issue_integration(
            space,
            IntegrationDraft {
                name: "离线测试脚本".into(),
                scopes: scopes.iter().copied().collect(),
                grants,
            },
        )
        .unwrap()
}

fn block(kind: &str, schema_version: u32, payload: Value, children: Vec<Block>) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version,
        payload,
        children,
        extra: Default::default(),
    }
}

fn patch(page: &str, title: &str) -> Mutation {
    Mutation::PatchPage {
        page_id: page.into(),
        patch: PagePatch {
            title: Some(title.into()),
            icon: None,
        },
    }
}

fn database(root: &Path, workspace: &str) -> Connection {
    Connection::open(root.join(workspace).join("workspace.sqlite")).unwrap()
}

fn operation_count(root: &Path, workspace: &str) -> u32 {
    database(root, workspace)
        .query_row("SELECT count(*) FROM integration_operations", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn token_expiry_rechecks_reads_writes_downloads_and_proposals() {
    let (root, store, space) = setup();
    let note = store.create_page(&space, None, "到期演练", "file").unwrap();
    let page = &note.info.id;
    let asset = store
        .add_attachment(
            &space,
            page,
            "演练原件.bin",
            Cursor::new(vec![7; 128 * 1024]),
        )
        .unwrap()
        .attachment;
    let draft = || IntegrationDraft {
        name: "短期集成".into(),
        scopes: [
            Scope::Read,
            Scope::Update,
            Scope::ReadAssets,
            Scope::Changes,
            Scope::Propose,
        ]
        .into_iter()
        .collect(),
        grants: vec![Grant::Page(page.clone())],
    };
    for invalid in [note.info.updated_at - 1, i64::MAX] {
        let reason = store
            .issue_integration_with_expiry(&space, draft(), Some(invalid))
            .err()
            .unwrap();
        assert_eq!(reason.code, "INTEGRATION_EXPIRY");
    }
    assert!(store.integrations(&space).unwrap().is_empty());
    let expires = note.info.updated_at + 3_600_000;
    let issued = store
        .issue_integration_with_expiry(&space, draft(), Some(expires))
        .unwrap();
    assert_eq!(issued.integration.expires_at, Some(expires));
    assert_eq!(
        store.integrations(&space).unwrap()[0].expires_at,
        Some(expires)
    );
    let token = &issued.token;
    let db = database(root.path(), &space);
    let original = store.page(&space, page).unwrap();
    let projected = store.integration_page(&space, token, page).unwrap();
    let cursor = store
        .integration_catalog(&space, token, None, None)
        .unwrap()
        .change_cursor
        .unwrap();
    let proposal = store.submit_integration_proposal(&space, token, "draft", serde_json::from_value(json!({
        "page_id": page, "base_revision": original.info.revision, "changes": {"title": "到期前的建议"},
    })).unwrap()).unwrap();
    let review = store.proposal_review(&space, &proposal.id).unwrap();
    let mut download = store
        .open_integration_asset(&space, token, &asset.id)
        .unwrap();
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, token, &mut download)
            .unwrap()
            .unwrap()
            .len(),
        64 * 1024
    );

    // Move the fixture's expiry into the past without changing the machine clock.
    db.execute(
        "UPDATE integrations SET created_at = created_at - 2000, expires_at = created_at - 1000 WHERE id = ?1",
        [&issued.integration.id],
    )
    .unwrap();
    assert_eq!(
        store
            .integration_page(&space, token, page)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(
        store
            .integration_changes(&space, token, None, &cursor)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, token, &mut download)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(
        store
            .mutate_as_integration(
                &space,
                token,
                "late-write",
                Some(&projected.etag),
                patch(page, "过期后修改")
            )
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    let reviewed = store
        .review_proposal(
            &space,
            &proposal.id,
            review.summary.review_revision,
            ProposalAction::Accept,
        )
        .unwrap();
    assert_eq!(reviewed.summary.status, ProposalStatus::Conflict);
    assert_eq!(
        reviewed.summary.conflict.as_deref(),
        Some("authorization_changed")
    );
    assert_eq!(store.page(&space, page).unwrap(), original);
    let entry = store.integrations(&space).unwrap().remove(0);
    assert_eq!(entry.id, issued.integration.id);
    assert!(entry.expires_at.unwrap() < expires && entry.revoked_at.is_none());
}

#[test]
fn v19_upgrade_preserves_old_integration_without_a_deadline() {
    let (root, store, space) = setup();
    let page = store.create_page(&space, None, "旧库正文", "file").unwrap();
    let issued = issue(
        &store,
        &space,
        &[Scope::Read],
        vec![Grant::Page(page.info.id.clone())],
    );
    assert!(issued.integration.expires_at.is_none());
    support::remove_v20_schema(&database(root.path(), &space));

    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
    let entry = store.integrations(&space).unwrap().remove(0);
    assert_eq!(entry.id, issued.integration.id);
    assert!(entry.expires_at.is_none() && entry.revoked_at.is_none());
    assert!(
        store
            .integration_page(&space, &issued.token, &page.info.id)
            .is_ok()
    );
    let directory = root.path().join("upgrade-backups");
    let snapshot = Connection::open(directory.join(format!("{space}-v19.sqlite"))).unwrap();
    assert_eq!(
        snapshot
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        19
    );
    assert!(
        directory
            .join(format!("{space}-v19.zhiye-recovery.zip"))
            .is_file()
    );
    let current = database(root.path(), &space);
    assert_eq!(
        current
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        21
    );
}

#[test]
fn tokens_bind_workspace_and_resource_grants_follow_current_membership() {
    let (_root, store, space) = setup();
    let other = store.create_workspace("其他空间").unwrap();
    let folder = store.create_page(&space, None, "授权根", "file").unwrap();
    let child = store
        .create_page(&space, Some(&folder.info.id), "动态子页", "file")
        .unwrap();
    let (db, collection) = store.create_collection(&space, None, "数据库").unwrap();
    let record = store
        .create_page(&space, Some(&collection.info.id), "记录", "file")
        .unwrap();
    let notes = store
        .create_page(&space, Some(&record.info.id), "记录下的私有便笺", "file")
        .unwrap();

    let integration = issue(
        &store,
        &space,
        &[Scope::Read],
        vec![
            Grant::Subtree(folder.info.id.clone()),
            Grant::Collection(db.id.clone()),
        ],
    );
    let token = &integration.token;
    for id in [
        &folder.info.id,
        &child.info.id,
        &record.info.id,
        &collection.info.id,
    ] {
        assert!(store.integration_page(&space, token, id).is_ok());
    }
    assert_eq!(
        store
            .integration_page(&space, token, &notes.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        store
            .integration_page(&space, "", &child.info.id)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(
        store
            .integration_page(&other.id, token, &child.info.id)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );

    let future = store
        .create_page(&space, Some(&child.info.id), "新增后代", "file")
        .unwrap();
    let next_record = store
        .create_page(&space, Some(&collection.info.id), "新增记录", "file")
        .unwrap();
    assert!(
        store
            .integration_page(&space, token, &future.info.id)
            .is_ok()
    );
    assert!(
        store
            .integration_page(&space, token, &next_record.info.id)
            .is_ok()
    );
    let pinned = issue(
        &store,
        &space,
        &[Scope::Read],
        vec![Grant::Page(child.info.id.clone())],
    );
    assert_eq!(
        store
            .integration_page(&space, &pinned.token, &future.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    let moved = store
        .move_page(&space, &child.info.id, child.info.revision, None)
        .unwrap();
    assert_eq!(
        store
            .integration_page(&space, token, &future.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    assert!(
        store
            .integration_page(&space, &pinned.token, &child.info.id)
            .is_ok()
    );
    store
        .archive_page(&space, &child.info.id, moved.revision)
        .unwrap();
    assert_eq!(
        store
            .integration_page(&space, &pinned.token, &child.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
}

#[test]
fn projected_blocks_hide_references_opaque_data_and_ungranted_attachments() {
    let (_root, store, space) = setup();
    let parent = store.create_page(&space, None, "隐藏上级", "file").unwrap();
    let note = store
        .create_page(&space, Some(&parent.info.id), "公开页", "file")
        .unwrap();
    let private = store.create_page(&space, None, "保密目标", "file").unwrap();
    let visible = store
        .create_page(&space, None, "另一个公开页", "file")
        .unwrap();
    let own_asset = store
        .add_attachment(
            &space,
            &note.info.id,
            "图.png",
            &b"\x89PNG\r\n\x1a\nfixture"[..],
        )
        .unwrap()
        .attachment;
    let foreign_asset = store
        .add_attachment(
            &space,
            &private.info.id,
            "private.txt",
            &b"hidden attachment"[..],
        )
        .unwrap()
        .attachment;
    let paragraph = block(
        "paragraph",
        1,
        json!({"inline": [
            {"type": "text", "text": "可读内容"},
            {"type": "pageReference", "pageId": private.info.id, "label": "保密标签甲"},
            {"type": "pageReference", "pageId": private.info.id, "label": "保密标签乙"},
            {"type": "pageReference", "pageId": visible.info.id, "label": "可读引用"}
        ]}),
        vec![],
    );
    let child = block(
        "paragraph",
        1,
        json!({"inline": [{"type": "text", "text": "未来结构内部"}]}),
        vec![],
    );
    let opaque = block(
        "futurePrivateBlock",
        9,
        json!({"secret": private.info.id}),
        vec![child.clone()],
    );
    let image = block(
        "attachmentImage",
        1,
        json!({"attachmentId": own_asset.id, "mediaType": "image/png", "alt": "原件名"}),
        vec![],
    );
    store
        .save_page(
            &space,
            &note.info.id,
            note.info.revision,
            PageDraft {
                title: note.info.title,
                blocks: vec![paragraph.clone(), opaque.clone(), image],
            },
        )
        .unwrap();

    let grants = vec![
        Grant::Page(note.info.id.clone()),
        Grant::Page(visible.info.id.clone()),
    ];
    let restricted = issue(&store, &space, &[Scope::Read], grants.clone());
    let page = store
        .integration_page(&space, &restricted.token, &note.info.id)
        .unwrap();
    assert_eq!(page.parent_id, None);
    assert_eq!(
        page.blocks[0].payload["inline"].as_array().unwrap().len(),
        3
    );
    assert!(page.blocks[0].restricted);
    assert_eq!(
        page.blocks[0].payload["inline"][1],
        json!({"type":"restricted","label":"不可访问"})
    );
    assert_eq!(page.blocks[1].kind, "restricted");
    assert!(page.blocks[1].children.is_empty());
    assert_eq!(page.blocks[2].kind, "restricted");
    let output = serde_json::to_string(&page).unwrap();
    for hidden in [&parent.info.id, &private.info.id, &child.id, &own_asset.id] {
        assert!(!output.contains(hidden));
    }
    assert!(!output.contains("保密标签"));
    assert!(!output.contains("未来结构内部"));
    assert_eq!(
        store
            .integration_block(&space, &restricted.token, &child.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    assert_eq!(
        store
            .integration_block(&space, &restricted.token, &paragraph.id)
            .unwrap()
            .page_id,
        note.info.id
    );
    assert_eq!(
        store
            .integration_asset(&space, &restricted.token, &own_asset.id)
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );

    let with_assets = issue(&store, &space, &[Scope::Read, Scope::ReadAssets], grants);
    let page = store
        .integration_page(&space, &with_assets.token, &note.info.id)
        .unwrap();
    assert_eq!(page.blocks[2].payload["attachmentId"], own_asset.id);
    assert_eq!(
        store
            .integration_asset(&space, &with_assets.token, &own_asset.id)
            .unwrap(),
        own_asset
    );
    let mut download = store
        .open_integration_asset(&space, &with_assets.token, &own_asset.id)
        .unwrap();
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, &with_assets.token, &mut download)
            .unwrap()
            .as_deref(),
        Some(&b"\x89PNG\r\n\x1a\nfixture"[..])
    );
    assert_eq!(
        store
            .integration_asset(&space, &with_assets.token, &foreign_asset.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
}

#[test]
fn mutations_check_scope_revisions_and_idempotency_in_the_commit() {
    let (root, store, space) = setup();
    let folder = store.create_page(&space, None, "授权目录", "file").unwrap();
    let note = store
        .create_page(&space, Some(&folder.info.id), "笔记", "file")
        .unwrap();
    let writer = issue(
        &store,
        &space,
        &[Scope::Read, Scope::Create, Scope::Update],
        vec![Grant::Subtree(folder.info.id.clone())],
    );
    let reader = issue(
        &store,
        &space,
        &[Scope::Read],
        vec![Grant::Page(note.info.id.clone())],
    );
    let etag = store
        .integration_page(&space, &writer.token, &note.info.id)
        .unwrap()
        .etag;
    let change = || patch(&note.info.id, "集成修改");
    assert_eq!(
        store
            .mutate_as_integration(&space, &reader.token, "readonly", Some(&etag), change())
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "no-precondition", None, change())
            .unwrap_err()
            .code,
        "PRECONDITION_REQUIRED"
    );
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "stale", Some("\"stale\""), change())
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    assert_eq!(operation_count(root.path(), &space), 0);
    assert_eq!(store.history(&space, &note.info.id).unwrap().len(), 1);

    let first = store
        .mutate_as_integration(&space, &writer.token, "change", Some(&etag), change())
        .unwrap();
    assert_eq!(first.revision, 2);
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "change", Some(&etag), change())
            .unwrap(),
        first
    );
    assert_eq!(
        store
            .mutate_as_integration(
                &space,
                &writer.token,
                "change",
                Some(&etag),
                patch(&note.info.id, "另一次修改")
            )
            .unwrap_err()
            .code,
        "IDEMPOTENCY_CONFLICT"
    );
    let source = store.revision_attributions(&space, &note.info.id).unwrap();
    assert_eq!(source[&2].integration_id, writer.integration.id);
    assert_eq!(source[&2].operation_id, first.operation_id);
    assert_eq!(store.history(&space, &note.info.id).unwrap().len(), 2);
    assert_eq!(operation_count(root.path(), &space), 1);

    let create = || Mutation::CreatePage {
        parent_id: folder.info.id.clone(),
        title: "创建测试".into(),
        icon: "file".into(),
    };
    let created = store
        .mutate_as_integration(&space, &writer.token, "create", None, create())
        .unwrap();
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "create", None, create())
            .unwrap(),
        created
    );
    store
        .move_page(&space, &created.page_id, created.revision, None)
        .unwrap();
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "create", None, create())
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );

    store
        .revoke_integration(&space, &writer.integration.id)
        .unwrap();
    assert_eq!(
        store
            .mutate_as_integration(&space, &writer.token, "change", Some(&etag), change())
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(store.history(&space, &note.info.id).unwrap().len(), 2);
    assert!(
        store
            .integrations(&space)
            .unwrap()
            .iter()
            .any(|entry| entry.revoked_at.is_some() && entry.last_used_at.is_some())
    );
}

#[test]
fn a_reference_moving_out_of_scope_changes_the_etag_without_rewriting_the_page() {
    let (_root, store, space) = setup();
    let root = store.create_page(&space, None, "允许范围", "file").unwrap();
    let before = store
        .create_page(&space, Some(&root.info.id), "正文", "file")
        .unwrap();
    let target = store
        .create_page(&space, Some(&root.info.id), "引用页", "file")
        .unwrap();
    let reference = block(
        "paragraph",
        1,
        json!({"inline": [
            {"type": "pageReference", "pageId": target.info.id, "label": "引用目标"}
        ]}),
        vec![],
    );
    store
        .save_page(
            &space,
            &before.info.id,
            before.info.revision,
            PageDraft {
                title: before.info.title,
                blocks: vec![reference.clone()],
            },
        )
        .unwrap();
    let grant = issue(
        &store,
        &space,
        &[Scope::Read, Scope::Update],
        vec![Grant::Subtree(root.info.id)],
    );
    let original = store
        .integration_page(&space, &grant.token, &before.info.id)
        .unwrap();
    store
        .move_page(&space, &target.info.id, target.info.revision, None)
        .unwrap();
    let filtered = store
        .integration_page(&space, &grant.token, &before.info.id)
        .unwrap();
    assert_eq!(filtered.page_revision, original.page_revision);
    assert_ne!(filtered.etag, original.etag);
    let append = || Mutation::AppendParagraphs {
        page_id: before.info.id.clone(),
        paragraphs: vec!["新笔记\n第二行".into()],
    };
    assert_eq!(
        store
            .mutate_as_integration(
                &space,
                &grant.token,
                "stale",
                Some(&original.etag),
                append()
            )
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    let operation = store
        .mutate_as_integration(
            &space,
            &grant.token,
            "append",
            Some(&filtered.etag),
            append(),
        )
        .unwrap();
    assert_eq!(operation.created_block_ids.len(), 1);
    let updated = store.page(&space, &before.info.id).unwrap();
    assert_eq!(updated.blocks[0], reference);
    assert_eq!(
        updated.blocks[1].payload["inline"][1],
        json!({"type": "hardBreak"})
    );
}

#[test]
fn concurrent_same_key_writes_commit_once() {
    let (root, store, space) = setup();
    let note = store.create_page(&space, None, "原始页", "file").unwrap();
    let issued = issue(
        &store,
        &space,
        &[Scope::Read, Scope::Update],
        vec![Grant::Page(note.info.id.clone())],
    );
    let etag = store
        .integration_page(&space, &issued.token, &note.info.id)
        .unwrap()
        .etag;
    let gate = Barrier::new(2);
    thread::scope(|threads| {
        let apply = || {
            gate.wait();
            store
                .mutate_as_integration(
                    &space,
                    &issued.token,
                    "one-write",
                    Some(&etag),
                    patch(&note.info.id, "并发修改"),
                )
                .unwrap()
        };
        let first = threads.spawn(apply);
        let second = threads.spawn(apply);
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    });
    assert_eq!(operation_count(root.path(), &space), 1);
    assert_eq!(store.history(&space, &note.info.id).unwrap().len(), 2);
}

#[test]
fn a_download_rechecks_revocation_and_subtree_scope_at_each_chunk() {
    let (_root, store, space) = setup();
    let folder = store.create_page(&space, None, "共享目录", "file").unwrap();
    let note = store
        .create_page(&space, Some(&folder.info.id), "下载页", "file")
        .unwrap();
    let asset = store
        .add_attachment(
            &space,
            &note.info.id,
            "资料.bin",
            Cursor::new(vec![33; 128 * 1024]),
        )
        .unwrap()
        .attachment;
    let grant = || {
        issue(
            &store,
            &space,
            &[Scope::Read, Scope::ReadAssets],
            vec![Grant::Subtree(folder.info.id.clone())],
        )
    };
    let first = grant();
    let second = grant();
    let mut download = store
        .open_integration_asset(&space, &first.token, &asset.id)
        .unwrap();
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, &first.token, &mut download)
            .unwrap()
            .unwrap()
            .len(),
        64 * 1024
    );
    store
        .revoke_integration(&space, &first.integration.id)
        .unwrap();
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, &first.token, &mut download)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );

    let mut download = store
        .open_integration_asset(&space, &second.token, &asset.id)
        .unwrap();
    store
        .move_page(&space, &note.info.id, note.info.revision, None)
        .unwrap();
    assert_eq!(
        store
            .read_integration_asset_chunk(&space, &second.token, &mut download)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
}

#[test]
fn v5_upgrade_snapshots_old_state_and_content_restore_excludes_credentials() {
    let (root, store, space) = setup();
    let note = store
        .create_page(&space, None, "升级前页面", "file")
        .unwrap();
    let old = database(root.path(), &space);
    support::remove_v7_schema(&old);
    old.execute_batch(
        "DROP TABLE integration_operations; DROP TABLE integration_revisions;
         DROP TABLE integration_grants; DROP TABLE integrations; PRAGMA user_version=5;",
    )
    .unwrap();
    drop(old);
    assert_eq!(store.page(&space, &note.info.id).unwrap(), note);
    let snapshot = root
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v5.sqlite"));
    assert_eq!(
        Connection::open(snapshot)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get::<_, u32>(0))
            .unwrap(),
        5
    );
    let integration = issue(
        &store,
        &space,
        &[Scope::Read],
        vec![Grant::Page(note.info.id.clone())],
    );
    let entry = serde_json::to_string(&store.integrations(&space).unwrap()).unwrap();
    assert!(!entry.contains(&integration.token));
    let db = database(root.path(), &space);
    let (kind, length): (String, u32) = db
        .query_row(
            "SELECT typeof(token_hash), length(token_hash) FROM integrations",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((kind.as_str(), length), ("blob", 32));

    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    archive.set_position(0);
    let restored = store.read_workspace_archive(archive).unwrap();
    assert!(store.integrations(&restored.id).unwrap().is_empty());
    assert_eq!(store.page(&restored.id, &note.info.id).unwrap(), note);
    assert_eq!(
        store
            .integration_page(&restored.id, &integration.token, &note.info.id)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
}
