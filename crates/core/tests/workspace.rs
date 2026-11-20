use serde_json::{Value, json};
use zhiye_core::{
    Block, PageDraft, Store, decode_desktop_blocks, encode_desktop_blocks, render_markdown,
};

fn paragraph() -> Block {
    serde_json::from_value(json!({
        "id": "5e84b82c-55de-410a-aa56-b1573e61fe99",
        "type": "paragraph",
        "schemaVersion": 1,
        "payload": {"inline": [{"type":"text","text":"中文阅读笔记"}]},
        "children": []
    }))
    .unwrap()
}

fn draft(title: &str) -> PageDraft {
    PageDraft {
        title: title.into(),
        blocks: vec![paragraph()],
    }
}

#[test]
fn saves_and_reopens_the_same_page_and_tree() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let workspace = store.create_workspace("个人空间").unwrap();
    let other = store.create_workspace("独立空间").unwrap();
    let parent = store
        .create_page(&workspace.id, None, "资料", "folder")
        .unwrap();
    let page = store
        .create_page(&workspace.id, Some(&parent.info.id), "新建", "file")
        .unwrap();
    store
        .save_page(&workspace.id, &page.info.id, 1, draft("改名后"))
        .unwrap();
    assert!(store.page(&other.id, &page.info.id).is_err());
    drop(store);

    let store = Store::open(directory.path()).unwrap();
    let saved = store.page(&workspace.id, &page.info.id).unwrap();
    assert_eq!(saved.info.title, "改名后");
    assert_eq!(
        saved.info.parent_id.as_deref(),
        Some(parent.info.id.as_str())
    );
    assert_eq!(saved.info.revision, 2);
    assert_eq!(saved.blocks, vec![paragraph()]);
    assert_eq!(
        store.search(&workspace.id, "阅读").unwrap()[0].id,
        page.info.id
    );
}

#[test]
fn stale_or_invalid_writes_leave_current_revision_and_history_intact() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let workspace = store.create_workspace("资料").unwrap();
    let page = store
        .create_page(&workspace.id, None, "初稿", "file")
        .unwrap();
    let id = &page.info.id;
    store
        .save_page(&workspace.id, id, 1, draft("第二稿"))
        .unwrap();
    assert_eq!(
        store
            .save_page(&workspace.id, id, 1, draft("旧稿"))
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );

    let invalid = PageDraft {
        title: "不应写入".into(),
        blocks: vec![paragraph(), paragraph()],
    };
    assert_eq!(
        store
            .save_page(&workspace.id, id, 2, invalid)
            .unwrap_err()
            .code,
        "INVALID_BLOCK"
    );
    assert_eq!(store.page(&workspace.id, id).unwrap().info.title, "第二稿");
    assert_eq!(store.history(&workspace.id, id).unwrap().len(), 2);
    let restored = store.restore_version(&workspace.id, id, 2, 1).unwrap();
    assert_eq!(restored.info.title, "初稿");
    assert_eq!(restored.info.revision, 3);
    assert_eq!(
        store.history(&workspace.id, id).unwrap()[1].info.title,
        "第二稿"
    );
}

#[test]
fn rejects_page_cycles_and_restores_only_the_selected_trash_batch() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let ws = store.create_workspace("树").unwrap().id;
    let root = store
        .create_page(&ws, None, "根", "folder")
        .unwrap()
        .info
        .id;
    let child = store
        .create_page(&ws, Some(&root), "子页", "file")
        .unwrap()
        .info
        .id;
    let archived = store
        .create_page(&ws, Some(&root), "先删除", "file")
        .unwrap()
        .info
        .id;
    assert_eq!(
        store
            .move_page(&ws, &root, 1, Some(&child))
            .unwrap_err()
            .code,
        "PAGE_CYCLE"
    );
    store.archive_page(&ws, &archived, 1).unwrap();
    store.archive_page(&ws, &root, 1).unwrap();
    assert!(store.search(&ws, "子页").unwrap().is_empty());
    store.restore_page(&ws, &root, 2).unwrap();
    assert_eq!(store.page(&ws, &child).unwrap().info.trash_root_id, None);
    assert_eq!(
        store.page(&ws, &archived).unwrap().info.trash_root_id,
        Some(archived)
    );
}

#[test]
fn original_format_preserves_links_ids_and_unknown_blocks_in_an_independent_store() {
    let original = tempfile::tempdir().unwrap();
    let restored = tempfile::tempdir().unwrap();
    let store = Store::open(original.path()).unwrap();
    let ws = store.create_workspace("资料库").unwrap().id;
    let root = store.create_page(&ws, None, "主页面", "folder").unwrap();
    let child = store
        .create_page(&ws, Some(&root.info.id), "子页面", "file")
        .unwrap();
    let mut linked = paragraph();
    linked.payload =
        json!({"inline": [{"type":"pageReference","pageId":root.info.id,"label":"主页面"}]});
    let unknown: Block = serde_json::from_value(json!({
        "id": "8207e258-dacf-4f70-97dc-d86940c456ad", "type": "futureWidget", "schemaVersion": 4,
        "payload": {"rows": [1, 2]}, "children": [], "extensionMetadata": "retain"
    }))
    .unwrap();
    let draft = PageDraft {
        title: "子页面".into(),
        blocks: vec![linked, unknown],
    };
    store.save_page(&ws, &child.info.id, 1, draft).unwrap();
    let expected = store.page(&ws, &child.info.id).unwrap();
    let bytes = store.export_workspace(&ws).unwrap();

    let recovery = Store::open(restored.path()).unwrap();
    let imported = recovery.import_workspace(&bytes).unwrap();
    assert_ne!(imported.id, ws);
    assert_eq!(
        recovery.page(&imported.id, &child.info.id).unwrap(),
        expected
    );
    let markdown = render_markdown(&expected);
    assert!(markdown.contains("# 子页面\n"));
    assert!(markdown.contains("[[主页面]]"));
    assert!(markdown.contains("futureWidget"));

    let mut corrupted: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    corrupted["data"]["workspace"]["name"] = json!("被改写的工作空间");
    assert_eq!(
        recovery
            .import_workspace(&serde_json::to_vec(&corrupted).unwrap())
            .unwrap_err()
            .code,
        "CHECKSUM"
    );
    assert_eq!(recovery.workspaces().unwrap().len(), 1);
}

#[test]
fn concurrent_store_handles_cannot_write_to_the_same_root() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let second = Store::open(directory.path());
    assert!(matches!(second, Err(error) if error.code == "WORKSPACE_BUSY"));
    drop(store);
    assert!(Store::open(directory.path()).is_ok());
}

#[test]
fn a_damaged_workspace_does_not_block_other_workspaces_or_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let healthy = store.create_workspace("正常库").unwrap();
    let damaged = store.create_workspace("坏库").unwrap();
    std::fs::write(
        directory.path().join(&damaged.id).join("workspace.sqlite"),
        b"invalid sqlite data",
    )
    .unwrap();
    let overview = store.workspace_overview().unwrap();
    assert_eq!(overview.workspaces, vec![healthy.clone()]);
    assert_eq!(overview.issues.len(), 1);
    assert_eq!(overview.issues[0].id, damaged.id);
    assert_eq!(store.workspace(&healthy.id).unwrap(), healthy);
    let backup = store.export_workspace(&healthy.id).unwrap();
    assert!(store.import_workspace(&backup).is_ok());
}

#[test]
fn shared_desktop_draft_preserves_opaque_blocks_and_revisions() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/desktop-page-draft.json")).unwrap();
    let mut wire: PageDraft = serde_json::from_value(fixture["draft"].clone()).unwrap();
    let initial = PageDraft {
        title: wire.title.clone(),
        blocks: decode_desktop_blocks(wire.blocks.clone()).unwrap(),
    };
    let opaque = &initial.blocks[1].children[0];
    assert_eq!(
        opaque.payload["value"].as_u64(),
        Some(9_007_199_254_740_993)
    );
    assert_eq!(opaque.extra["extensionMetadata"]["label"], "保留此扩展字段");
    assert_eq!(
        opaque.children[0].payload["inline"][0]["text"],
        "未知块内的正文"
    );
    let encoded = encode_desktop_blocks(initial.blocks.clone()).unwrap();
    let placeholder = &encoded[1].children[0];
    assert_eq!(placeholder.payload, json!({}));
    assert!(placeholder.children.is_empty());
    assert!(placeholder.extra["$zhiyeOpaque"].is_string());
    assert_eq!(decode_desktop_blocks(encoded).unwrap(), initial.blocks);

    wire.blocks[0].payload["inline"][0]["text"] = fixture["editedText"].clone();
    let edited = PageDraft {
        title: wire.title,
        blocks: decode_desktop_blocks(wire.blocks).unwrap(),
    };
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let workspace = store.create_workspace("正文契约复验").unwrap().id;
    let page = store.create_page(&workspace, None, "初稿", "file").unwrap();
    let id = &page.info.id;
    let first = store.save_page(&workspace, id, 1, initial.clone()).unwrap();
    let second = store.save_page(&workspace, id, 2, edited.clone()).unwrap();
    assert_eq!((first.revision, second.revision), (2, 3));
    assert_eq!(
        store
            .save_page(&workspace, id, 2, initial.clone())
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    drop(store);

    let reopened = Store::open(directory.path()).unwrap();
    let current = reopened.page(&workspace, id).unwrap();
    assert_eq!(current.info, second);
    assert_eq!(current.blocks, edited.blocks);
    let revisions = reopened.history(&workspace, id).unwrap();
    assert_eq!(revisions.len(), 3);
    assert_eq!(revisions[0], current);
    assert_eq!(revisions[1].info, first);
    assert_eq!(revisions[1].blocks, initial.blocks);
}
