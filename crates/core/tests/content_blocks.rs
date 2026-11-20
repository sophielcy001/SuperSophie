// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use std::{collections::HashMap, io::Cursor};
use zhiye_core::{
    Block, IntegrationDraft, IntegrationGrant, IntegrationMutation, IntegrationScope, PageDraft,
    SearchRequest, Store, integration_markdown, render_workspace_markdown,
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

fn text(text: &str) -> Block {
    block(
        "paragraph",
        json!({"inline":[{"type":"text","text":text}]}),
        vec![],
    )
}

#[test]
fn rich_content_keeps_history_originals_and_unknown_payloads() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let workspace = store.create_workspace("内容块验收").unwrap().id;
    let page = store
        .create_page(&workspace, None, "正文内容", "file")
        .unwrap()
        .info;
    let other = store
        .create_page(&workspace, None, "其他页面", "file")
        .unwrap()
        .info;
    let source = b"original content";
    let file = store
        .add_attachment(&workspace, &page.id, "来源.txt", source.as_slice())
        .unwrap()
        .attachment;
    let attachment = block("attachmentFile", json!({"attachmentId":file.id}), vec![]);
    let heading = block(
        "heading",
        json!({"level":3,"inline":[{"type":"text","text":"起始标题"}]}),
        vec![],
    );
    let formula = block("mathBlock", json!({"latex":"\\frac{a}{b}\n```"}), vec![]);
    let fold = block(
        "toggleBlock",
        json!({"title":"稀土批次","collapsed":true}),
        vec![
            heading,
            formula.clone(),
            block(
                "callout",
                json!({"tone":"warning"}),
                vec![attachment.clone()],
            ),
        ],
    );
    let outline = block("tableOfContents", json!({}), vec![]);
    let mut opaque = block("callout", json!({"tone":"custom"}), vec![text("未来数据")]);
    opaque.schema_version = 2;
    opaque.extra.insert("future".into(), json!({"keep":true}));
    let blocks = vec![outline, fold, opaque];
    let draft = |blocks| PageDraft {
        title: page.title.clone(),
        blocks,
    };

    assert_eq!(
        store
            .save_page(&workspace, &other.id, 1, draft(blocks.clone()))
            .unwrap_err()
            .code,
        "BLOCK_ATTACHMENT"
    );
    assert_eq!(store.page(&workspace, &other.id).unwrap().info.revision, 1);
    let saved = store
        .save_page(&workspace, &page.id, 1, draft(blocks.clone()))
        .unwrap();
    assert_eq!(
        store
            .body_attachments(&workspace, &page.id, &blocks)
            .unwrap(),
        [file.clone()]
    );
    assert!(
        store
            .image_attachments(&workspace, &page.id, &blocks)
            .unwrap()
            .is_empty()
    );
    while store.advance_search_index(&workspace).unwrap().pending > 0 {}
    for (query, expected_id) in [("稀土批次", &blocks[1].id), ("frac", &formula.id)] {
        let request: SearchRequest = serde_json::from_value(json!({"query":query})).unwrap();
        let hits = store.search_text(&workspace, &request).unwrap().hits;
        assert_eq!(hits[0].block_id.as_deref(), Some(expected_id.as_str()));
    }
    let result = render_workspace_markdown(
        &page.title,
        &blocks,
        &HashMap::from([(file.id.clone(), "exported/source.txt".into())]),
        Some(&workspace),
    );
    assert!(result.contains("**目录**\n\n- 起始标题"));
    assert!(result.contains("> **稀土批次**"));
    assert!(result.contains("> ````math\n> \\frac{a}{b}"));
    assert!(result.contains("> > [!WARNING]"));
    assert!(result.contains("[附件](<exported/source.txt>)"));

    let template = store
        .save_template(&workspace, &page.id, "内容模板", None)
        .unwrap();
    let copied = store
        .apply_template(&workspace, &template.id, None, Some("副本"))
        .unwrap();
    let new_file = &store
        .body_attachments(&workspace, &copied.info.id, &copied.blocks)
        .unwrap()[0];
    assert_ne!(new_file.id, file.id);
    assert_eq!(new_file.sha256, file.sha256);
    assert_eq!(copied.blocks[2].extra, blocks[2].extra);
    store
        .save_page(&workspace, &page.id, saved.revision, draft(vec![]))
        .unwrap();

    let mut archive = Cursor::new(Vec::new());
    store
        .write_workspace_archive(&workspace, &mut archive)
        .unwrap();
    let recovered = store.read_workspace_archive(archive).unwrap();
    let restored = store
        .restore_version(&recovered.id, &page.id, saved.revision + 1, saved.revision)
        .unwrap();
    assert_eq!(restored.blocks, blocks);
    for (page_id, file_id) in [(&page.id, &file.id), (&copied.info.id, &new_file.id)] {
        let mut bytes = Vec::new();
        store
            .copy_attachment(&recovered.id, page_id, file_id, &mut bytes)
            .unwrap();
        assert_eq!(bytes, source);
    }
}

#[test]
fn attachment_children_preserve_scope_and_partial_edit_protection() {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let workspace = store.create_workspace("内容块授权").unwrap().id;
    let page = store
        .create_page(&workspace, None, "只给正文", "file")
        .unwrap()
        .info;
    let file = store
        .add_attachment(&workspace, &page.id, "保密原件.txt", b"verified".as_slice())
        .unwrap()
        .attachment;
    let issue = |name: &str, assets: bool| {
        store
            .issue_integration(
                &workspace,
                IntegrationDraft {
                    name: name.into(),
                    grants: vec![IntegrationGrant::Page(page.id.clone())],
                    scopes: [IntegrationScope::Read, IntegrationScope::Update]
                        .into_iter()
                        .chain(assets.then_some(IntegrationScope::ReadAssets))
                        .collect(),
                },
            )
            .unwrap()
            .token
    };
    let limited = issue("正文读写", false);
    let full = issue("正文与原件", true);
    let get = |token| store.integration_page(&workspace, token, &page.id).unwrap();
    let change = |token, key, tag: &str, operations| {
        store.mutate_as_integration(
            &workspace,
            token,
            key,
            Some(tag),
            IntegrationMutation::EditBlocks {
                page_id: page.id.clone(),
                operations: serde_json::from_value(operations).unwrap(),
            },
        )
    };
    change(&full, "append", &get(&full).etag, json!([
        {"op":"append","block":{"type":"toggleBlock","payload":{"title":"调查正文","collapsed":true},"children":[
            {"type":"callout","payload":{"tone":"tip"},"children":[
                {"type":"paragraph","payload":{"text":"可编辑子块"}},
                {"type":"attachmentFile","payload":{"attachmentId":file.id}},
                {"type":"mathBlock","payload":{"latex":"x^2"}}
            ]}
        ]}},
        {"op":"append","block":{"type":"tableOfContents","payload":{}}}
    ])).unwrap();
    let masked = get(&limited);
    let fold = &masked.blocks[0];
    let callout = &fold.children[0];
    assert!(fold.restricted && callout.restricted);
    assert_eq!(callout.children[1].kind, "restricted");
    assert_eq!(callout.children[1].payload, json!({}));
    assert!(!serde_json::to_string(&masked).unwrap().contains(&file.id));
    assert!(!serde_json::to_string(&masked).unwrap().contains(&file.name));
    assert!(
        store
            .integration_asset(&workspace, &limited, &file.id)
            .is_err()
    );
    assert_eq!(
        store
            .integration_asset(&workspace, &full, &file.id)
            .unwrap(),
        file
    );
    let public = integration_markdown(&workspace, None, &masked.blocks);
    assert_eq!(
        public.losses,
        ["restricted_content", "markdown_approximation"]
    );
    assert!(!public.markdown.contains(&file.id));
    assert!(public.markdown.contains("目录按当前页面标题生成"));
    let projected = integration_markdown(&workspace, None, &get(&full).blocks);
    assert_eq!(projected.losses, ["markdown_approximation"]);
    assert!(
        projected
            .markdown
            .contains(&format!("/assets/{}/content", file.id))
    );

    let error = change(
        &limited,
        "deny-delete",
        &masked.etag,
        json!([
            {"op":"delete","block_id":fold.id}
        ]),
    )
    .unwrap_err();
    assert_eq!(error.code, "PARTIAL_CONTENT");
    assert_eq!(get(&limited).etag, masked.etag);
    change(&limited, "update-visible", &masked.etag, json!([
        {"op":"update","block_id":callout.children[0].id,"block":{"type":"paragraph","payload":{"text":"局部更新"}}}
    ])).unwrap();
    let refreshed = get(&full);
    let children = &refreshed.blocks[0].children[0].children;
    assert_eq!(children[0].payload["inline"][0]["text"], "局部更新");
    assert_eq!(children[1].payload["attachmentId"], file.id);
}
