// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use zhiye_core::{
    Block, DesktopReference, IntegrationDraft, IntegrationGrant as Grant, IntegrationMutation,
    IntegrationScope as Scope, Page, PageDraft, Store, integration_markdown,
};

fn block(kind: &str, payload: Value, children: Vec<Block>) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children,
        extra: Default::default(),
    }
}

fn text(value: &str) -> Block {
    block(
        "paragraph",
        json!({"inline":[{"type":"text","text":value}]}),
        vec![],
    )
}

fn reference(page: &Page, target: &Block) -> Block {
    block(
        "blockReference",
        json!({"pageId":page.info.id,"blockId":target.id}),
        vec![],
    )
}

fn save(store: &Store, workspace: &str, page: &Page, blocks: Vec<Block>) -> Page {
    let info = store
        .save_page(
            workspace,
            &page.info.id,
            page.info.revision,
            PageDraft {
                title: page.info.title.clone(),
                blocks: blocks.clone(),
            },
        )
        .unwrap();
    Page {
        info,
        blocks,
        properties: None,
        historical_properties: Vec::new(),
        cover: None,
    }
}

#[test]
fn indexes_old_links_and_restores_reference_identity_through_history_templates_and_zip() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let space = store.create_workspace("引用保留验收").unwrap().id;
    let root = store.create_page(&space, None, "引用页", "file").unwrap();
    let source = store
        .create_page(&space, Some(&root.info.id), "源页", "file")
        .unwrap();
    let original = text("来源段落");
    let source = save(
        &store,
        &space,
        &source,
        vec![block("blockquote", json!({}), vec![original.clone()])],
    );
    let inline = block(
        "paragraph",
        json!({"inline":[
            {"type":"pageReference","pageId":source.info.id,"label":"源页"},
            {"type":"pageReference","pageId":source.info.id,"label":"同一源页"},
        ]}),
        vec![],
    );
    let reference = reference(&source, &original);
    let root = save(
        &store,
        &space,
        &root,
        vec![inline.clone(), reference.clone()],
    );
    let database = folder.path().join(&space).join("workspace.sqlite");
    support::remove_v9_schema(&Connection::open(&database).unwrap());
    let links = store.backlinks(&space, &source.info.id, None).unwrap();
    assert_eq!(links.items.len(), 2);
    assert_eq!(store.page(&space, &root.info.id).unwrap(), root);
    let backup = folder
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v8.sqlite"));
    let backup = Connection::open(backup).unwrap();
    assert_eq!(
        backup
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        8
    );

    let preview = |revision| {
        store
            .desktop_reference(&space, &source.info.id, &original.id, revision)
            .unwrap()
    };
    assert!(
        matches!(preview(None), DesktopReference::Available { block, .. } if block == original)
    );
    assert!(matches!(
        preview(Some(source.info.revision)),
        DesktopReference::Unchanged
    ));
    let removed = save(&store, &space, &source, vec![]);
    assert!(matches!(
        preview(Some(source.info.revision)),
        DesktopReference::Unavailable { .. }
    ));
    assert_eq!(
        store
            .backlinks(&space, &source.info.id, None)
            .unwrap()
            .items[0]
            .block_id,
        inline.id
    );
    let source = store
        .restore_version(
            &space,
            &source.info.id,
            removed.info.revision,
            source.info.revision,
        )
        .unwrap();
    assert!(
        matches!(preview(None), DesktopReference::Available { block, .. } if block == original)
    );
    store
        .archive_page(&space, &source.info.id, source.info.revision)
        .unwrap();
    assert!(matches!(
        preview(Some(source.info.revision)),
        DesktopReference::Unavailable { revision: None }
    ));
    store
        .restore_page(&space, &source.info.id, source.info.revision + 1)
        .unwrap();
    assert!(
        matches!(preview(None), DesktopReference::Available { block, .. } if block == original)
    );
    assert_eq!(store.page(&space, &root.info.id).unwrap(), root);

    let template = store
        .save_template(&space, &root.info.id, "引用子树样本", None)
        .unwrap();
    let copied = store
        .apply_template(&space, &template.id, None, None)
        .unwrap();
    let child = store
        .pages(&space)
        .unwrap()
        .into_iter()
        .find(|page| page.parent_id.as_deref() == Some(&copied.info.id))
        .unwrap();
    let cloned = store.page(&space, &child.id).unwrap().blocks[0].children[0].clone();
    assert_ne!(cloned.id, original.id);
    assert_eq!(
        copied.blocks[1].payload,
        json!({"pageId":child.id,"blockId":cloned.id})
    );
    assert_eq!(
        store
            .backlinks(&space, &child.id, None)
            .unwrap()
            .items
            .len(),
        2
    );

    let mut archive = std::io::Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    let recovered = store.read_workspace_archive(archive).unwrap();
    let restored_root = store.page(&recovered.id, &root.info.id).unwrap();
    assert_eq!(restored_root.blocks[1], reference);
    assert_eq!(
        store
            .backlinks(&recovered.id, &source.info.id, None)
            .unwrap()
            .items
            .len(),
        2
    );
    let before = store.page(&space, &source.info.id).unwrap();
    save(&store, &space, &root, vec![inline]);
    assert_eq!(
        store
            .backlinks(&space, &source.info.id, None)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(store.page(&space, &source.info.id).unwrap(), before);
}

#[test]
fn scopes_filter_descriptors_previews_writes_and_backlinks_at_current_revision() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let space = store.create_workspace("引用权限验收").unwrap().id;
    let root = store.create_page(&space, None, "授权范围", "file").unwrap();
    let new_child = |title| {
        store
            .create_page(&space, Some(&root.info.id), title, "file")
            .unwrap()
    };
    let hidden = store.create_page(&space, None, "保密样本", "file").unwrap();
    let source = new_child("原文页面");
    let shared = new_child("引用位置");
    let first = block(
        "paragraph",
        json!({"inline":[
            {"type":"text","text":"源内容初版"},
            {"type":"pageReference","pageId":hidden.info.id,"label":"保密标题"},
        ]}),
        vec![],
    );
    let repeated = text("同一页面的其他段");
    let opaque_child = text("不应出现在派生读取中的子块");
    let source = save(
        &store,
        &space,
        &source,
        vec![
            first.clone(),
            repeated.clone(),
            block("futureBox", json!({}), vec![opaque_child.clone()]),
        ],
    );
    let reference_one = reference(&source, &first);
    let reference_two = reference(&source, &repeated);
    let opaque = reference(&source, &opaque_child);
    let shared = save(
        &store,
        &space,
        &shared,
        vec![reference_one.clone(), reference_two, opaque.clone()],
    );
    save(&store, &space, &hidden, vec![reference(&source, &first)]);
    let issue = |grant| {
        store
            .issue_integration(
                &space,
                IntegrationDraft {
                    name: "引用验收".into(),
                    scopes: [Scope::Read, Scope::Update].into(),
                    grants: vec![grant],
                },
            )
            .unwrap()
    };
    let limited = issue(Grant::Page(shared.info.id.clone()));
    let reader = issue(Grant::Subtree(root.info.id));
    let raw = || {
        store
            .integration_page(&space, &reader.token, &shared.info.id)
            .unwrap()
    };
    let read =
        || store.integration_reference(&space, &reader.token, &shared.info.id, &reference_one.id);
    let page = raw();
    assert_eq!(page.blocks[0].payload, reference_one.payload);
    assert_eq!(page.blocks[2].kind, "restricted");
    assert!(
        !serde_json::to_string(&page)
            .unwrap()
            .contains(&opaque_child.id)
    );
    let markdown = integration_markdown(&space, None, &page.blocks);
    assert!(markdown.markdown.contains(&format!("/blocks/{}", first.id)));
    assert!(!markdown.markdown.contains("源内容初版"));
    assert!(markdown.losses.contains(&"markdown_approximation"));
    let before = read().unwrap();
    assert!(!before.stale);
    let encoded = serde_json::to_string(&before).unwrap();
    assert!(encoded.contains("源内容初版"));
    assert!(!encoded.contains("保密标题"));
    assert!(!encoded.contains(&hidden.info.id));

    let mut revised = first.clone();
    revised.payload["inline"][1]["label"] = json!("只改隐藏标题");
    let source = save(
        &store,
        &space,
        &source,
        vec![revised.clone(), repeated.clone()],
    );
    assert_eq!(
        before.representation_version,
        read().unwrap().representation_version
    );
    revised.payload["inline"][0]["text"] = json!("源内容新版");
    let source = save(&store, &space, &source, vec![revised, repeated]);
    assert_ne!(
        before.representation_version,
        read().unwrap().representation_version
    );
    assert_eq!(page.etag, raw().etag);
    assert_eq!(shared.info.revision, raw().page_revision);

    let restricted = store
        .integration_page(&space, &limited.token, &shared.info.id)
        .unwrap();
    assert_eq!(restricted.blocks[0].kind, "restricted");
    let encoded = serde_json::to_string(&restricted).unwrap();
    assert!(!encoded.contains(&source.info.id));
    assert!(!encoded.contains(&first.id));
    assert_eq!(
        store
            .integration_reference(&space, &limited.token, &shared.info.id, &reference_one.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    for (key, operation) in [
        (
            "hidden-delete",
            json!({"op":"delete","block_id":reference_one.id}),
        ),
        (
            "hidden-create",
            json!({"op":"append","block":{"type":"blockReference","payload":reference_one.payload}}),
        ),
    ] {
        let error = store
            .mutate_as_integration_at(
                &space,
                &limited.token,
                key,
                Some(&restricted.etag),
                None,
                IntegrationMutation::EditBlocks {
                    page_id: shared.info.id.clone(),
                    operations: vec![serde_json::from_value(operation).unwrap()],
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "PARTIAL_CONTENT");
    }

    let links = |cursor: Option<&str>| {
        store
            .integration_backlinks(&space, &reader.token, &source.info.id, cursor, Some(1))
            .unwrap()
    };
    let first_page = links(None);
    let next = links(first_page.next_cursor.as_deref());
    assert_eq!(first_page.items[0].page_id, shared.info.id);
    assert_eq!(next.items[0].page_id, shared.info.id);
    assert_ne!(first_page.items[0].block_id, next.items[0].block_id);
    assert!(next.next_cursor.is_none());
    store
        .move_page(&space, &source.info.id, source.info.revision, None)
        .unwrap();
    assert_eq!(read().unwrap_err().code, "RESOURCE_NOT_FOUND");
    assert_eq!(raw().blocks[0].kind, "restricted");
    assert_eq!(raw().page_revision, shared.info.revision);
    assert_eq!(
        store
            .integration_backlinks(&space, &reader.token, &source.info.id, None, None)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
}

#[test]
fn image_reference_requires_asset_scope_on_each_read() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let space = store.create_workspace("图片引用").unwrap().id;
    let source = store.create_page(&space, None, "图片原文", "file").unwrap();
    let referring = store.create_page(&space, None, "图片引用", "file").unwrap();
    let asset = store
        .add_image(
            &space,
            &source.info.id,
            "样本.png",
            b"\x89PNG\r\n\x1a\nfixture".as_slice(),
        )
        .unwrap()
        .attachment;
    let image = block(
        "attachmentImage",
        json!({
            "attachmentId": asset.id, "mediaType": asset.media_type, "alt": "图片说明样本",
        }),
        vec![],
    );
    let source = save(&store, &space, &source, vec![image.clone()]);
    let link = reference(&source, &image);
    let referring = save(&store, &space, &referring, vec![link.clone()]);
    let issue = |read_assets| {
        let mut scopes = vec![Scope::Read];
        if read_assets {
            scopes.push(Scope::ReadAssets);
        }
        store
            .issue_integration(
                &space,
                IntegrationDraft {
                    name: "图片读取验收".into(),
                    scopes: scopes.into_iter().collect(),
                    grants: vec![
                        Grant::Page(source.info.id.clone()),
                        Grant::Page(referring.info.id.clone()),
                    ],
                },
            )
            .unwrap()
            .token
    };
    let ungranted = issue(false);
    let granted = issue(true);
    let read = |token| store.integration_reference(&space, token, &referring.info.id, &link.id);
    assert_eq!(read(&ungranted).unwrap_err().code, "RESOURCE_NOT_FOUND");
    assert_eq!(
        store
            .integration_page(&space, &ungranted, &referring.info.id)
            .unwrap()
            .blocks[0]
            .kind,
        "restricted"
    );
    assert_eq!(read(&granted).unwrap().block.payload, image.payload);
    assert!(
        store
            .integration_backlinks(&space, &ungranted, &source.info.id, None, None)
            .unwrap()
            .items
            .is_empty()
    );
    assert_eq!(
        store
            .integration_backlinks(&space, &granted, &source.info.id, None, None)
            .unwrap()
            .items[0]
            .block_id,
        link.id
    );
}
