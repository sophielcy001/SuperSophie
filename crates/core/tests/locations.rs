// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::json;
use zhiye_core::{Block, PageDraft, PageLocation, Store};

#[test]
fn only_canonical_page_and_block_links_are_accepted() {
    let workspace = "10000000-0000-4000-8000-000000000000";
    let page = "20000000-0000-4000-8000-000000000000";
    let block = "30000000-0000-4000-8000-000000000000";
    let target = PageLocation::page(workspace, page);
    let url = target.to_string();
    assert_eq!(PageLocation::parse(&url).unwrap(), target);
    let target_with_block = target.block(block);
    let block_url = target_with_block.to_string();
    assert_eq!(PageLocation::parse(&block_url).unwrap(), target_with_block);
    for bad in [
        url.replace("zhiye://", "https://"),
        url.replace("//open/", "//localhost/"),
        url.replace("/pages/", "/../pages/"),
        url.replace(workspace, "saved.sqlite"),
        format!("{url}?block={block}"),
        format!("{url}#fragment"),
        format!("{url}/blocks/"),
        format!("{block_url}/extra"),
        format!("{block_url}\n"),
    ] {
        assert_eq!(
            PageLocation::parse(&bad).unwrap_err().code,
            "INVALID_LOCATION"
        );
    }
}

#[test]
fn resolution_checks_the_workspace_owner_page_and_trash_as_one_read() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let workspace = store.create_workspace("定位链接").unwrap();
    let other = store.create_workspace("隔离工作空间").unwrap();
    let parent = store
        .create_page(&workspace.id, None, "原位置", "file")
        .unwrap();
    let ancestor = store
        .create_page(&workspace.id, None, "当前位置", "file")
        .unwrap();
    let page = store
        .create_page(&workspace.id, Some(&parent.info.id), "初始标题", "file")
        .unwrap();
    let block = Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline":[{"type":"text","text":"可定位正文"}]}),
        children: vec![],
        extra: Default::default(),
    };
    let target = PageLocation::page(&workspace.id, &page.info.id);
    let link = target.block(&block.id).to_string();
    let revised = store
        .save_page(
            &workspace.id,
            &page.info.id,
            1,
            PageDraft {
                title: "改名仍可定位".into(),
                blocks: vec![block.clone()],
            },
        )
        .unwrap();
    let moved = store
        .move_page(
            &workspace.id,
            &page.info.id,
            revised.revision,
            Some(&ancestor.info.id),
        )
        .unwrap();
    let located = store.resolve_location(&link).unwrap();
    assert_eq!(located.workspace, workspace);
    assert_eq!(located.page.info, moved);
    assert_eq!(located.block_id.as_deref(), Some(block.id.as_str()));
    assert!(!located.missing_block);

    let mismatched = PageLocation::page(&workspace.id, &parent.info.id)
        .block(&block.id)
        .to_string();
    let resolved = store.resolve_location(&mismatched).unwrap();
    assert_eq!(resolved.page.info.id, parent.info.id);
    assert_eq!(resolved.block_id, None);
    assert!(resolved.missing_block);
    let wrong_workspace = PageLocation::page(&other.id, &page.info.id).to_string();
    assert_eq!(
        store.resolve_location(&wrong_workspace).unwrap_err().code,
        "PAGE_NOT_FOUND"
    );

    store
        .archive_page(&workspace.id, &page.info.id, moved.revision)
        .unwrap();
    assert_eq!(
        store.resolve_location(&link).unwrap_err().code,
        "PAGE_TRASHED"
    );
    store
        .restore_page(&workspace.id, &page.info.id, moved.revision + 1)
        .unwrap();
    assert_eq!(
        store.resolve_location(&link).unwrap().block_id,
        Some(block.id.clone())
    );
    store
        .save_page(
            &workspace.id,
            &page.info.id,
            moved.revision + 2,
            PageDraft {
                title: "已删正文块".into(),
                blocks: vec![],
            },
        )
        .unwrap();
    let fallback = store.resolve_location(&link).unwrap();
    assert!(fallback.missing_block && fallback.block_id.is_none());
}
