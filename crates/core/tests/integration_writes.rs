// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, Read},
};
use zhiye_core::{
    Block, IntegrationAssetUpload, IntegrationDraft, IntegrationGrant as Grant,
    IntegrationMutation as Mutation, IntegrationScope as Scope, IssuedIntegration, PageDraft,
    PageExportFormat, PagePatch, PropertyDraft, PropertyKind, Store, integration_markdown,
    render_page_export,
};

fn fixture() -> (tempfile::TempDir, Store, String) {
    let folder = tempfile::tempdir().unwrap();
    let store = Store::open(folder.path()).unwrap();
    let workspace = store.create_workspace("API 写入验收").unwrap().id;
    (folder, store, workspace)
}

fn issue(store: &Store, workspace: &str, grant: Grant, scopes: &[Scope]) -> IssuedIntegration {
    store
        .issue_integration(
            workspace,
            IntegrationDraft {
                name: "写入验收".into(),
                scopes: std::iter::once(Scope::Read)
                    .chain(scopes.iter().copied())
                    .collect(),
                grants: vec![grant],
            },
        )
        .unwrap()
}

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

fn paragraph(text: &str) -> Block {
    block(
        "paragraph",
        json!({"inline":[{"type":"text","text":text}]}),
        vec![],
    )
}

fn edit(id: &str, operations: Value) -> Mutation {
    Mutation::EditBlocks {
        page_id: id.into(),
        operations: serde_json::from_value(operations).unwrap(),
    }
}

#[test]
fn explicit_base_revision_is_checked_after_idempotent_replay() {
    let (_root, store, workspace) = fixture();
    let page = store
        .create_page(&workspace, None, "审阅底稿", "file")
        .unwrap()
        .info;
    let writer = issue(
        &store,
        &workspace,
        Grant::Page(page.id.clone()),
        &[Scope::Update],
    );
    let token = &writer.token;
    let tag = store
        .integration_page(&workspace, token, &page.id)
        .unwrap()
        .etag;
    let apply = |revision| {
        store.mutate_as_integration_at(
            &workspace,
            token,
            "mcp-append",
            Some(&tag),
            Some(revision),
            Mutation::AppendParagraphs {
                page_id: page.id.clone(),
                paragraphs: vec!["同一段".into()],
            },
        )
    };
    assert_eq!(apply(2).unwrap_err().code, "REVISION_CONFLICT");
    let added = apply(1).unwrap();
    assert_eq!(added.revision, 2);
    assert_eq!(apply(1).unwrap(), added);
    assert_eq!(apply(2).unwrap_err().code, "IDEMPOTENCY_CONFLICT");
    assert_eq!(store.page(&workspace, &page.id).unwrap().blocks.len(), 1);
    assert_eq!(
        store.integration_scopes(&workspace, token).unwrap(),
        [Scope::Read, Scope::Update].into()
    );
    store
        .revoke_integration(&workspace, &writer.integration.id)
        .unwrap();
    assert_eq!(apply(1).unwrap_err().code, "UNAUTHORIZED");
    assert_eq!(
        store
            .integration_scopes(&workspace, token)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
}

fn before(store: &Store, workspace: &str, token: &str, id: &str) -> String {
    store.integration_page(workspace, token, id).unwrap().etag
}

#[test]
fn atomic_block_batches_preserve_ids_and_protect_unreadable_structure() {
    let (root, store, workspace) = fixture();
    let note = store.create_page(&workspace, None, "记录", "file").unwrap();
    let private = store
        .create_page(&workspace, None, "私有页", "file")
        .unwrap();
    let moved = paragraph("旧正文");
    let discarded = paragraph("将删除");
    let anchor = paragraph("引用框原文");
    let quote = block("blockquote", json!({}), vec![anchor.clone()]);
    let protected = block(
        "paragraph",
        json!({"inline":[
            {"type":"pageReference","pageId":private.info.id,"label":"私有引用"},
        ]}),
        vec![],
    );
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks: vec![
                    moved.clone(),
                    quote.clone(),
                    protected.clone(),
                    discarded.clone(),
                ],
            },
        )
        .unwrap();
    let writer = issue(
        &store,
        &workspace,
        Grant::Page(note.info.id.clone()),
        &[Scope::Update],
    );
    let etag = before(&store, &workspace, &writer.token, &note.info.id);
    let apply = |key, tag: &str, operations| {
        store.mutate_as_integration(
            &workspace,
            &writer.token,
            key,
            Some(tag),
            edit(&note.info.id, operations),
        )
    };

    let failed = apply(
        "rollback",
        &etag,
        json!([
            {"op":"append","block":{"type":"paragraph","payload":{"text":"临时改变"}}},
            {"op":"delete","block_id":protected.id},
        ]),
    )
    .unwrap_err();
    assert_eq!(failed.code, "PARTIAL_CONTENT");
    let db = Connection::open(root.path().join(&workspace).join("workspace.sqlite")).unwrap();
    let recorded: u32 = db
        .query_row("SELECT count(*) FROM integration_operations", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(recorded, 0);
    assert_eq!(
        before(&store, &workspace, &writer.token, &note.info.id),
        etag
    );
    assert_eq!(
        apply(
            "cycle",
            &etag,
            json!([{"op":"move","block_id":quote.id,"parent_block_id":anchor.id,"index":0}])
        )
        .unwrap_err()
        .code,
        "INVALID_BLOCK_OPERATION"
    );
    let operations = json!([
        {"op":"move", "block_id":moved.id, "parent_block_id":quote.id, "index":1},
        {"op":"update", "block_id":moved.id, "block":{"type":"paragraph","payload":{"text":"更新后"}}},
        {"op":"insert", "parent_block_id":quote.id,"index":0,"block":{"type":"heading","payload":{"level":1,"inline":[{"type":"text","text":"引用标题"}]}}},
        {"op":"delete","block_id":discarded.id},
        {"op":"append","block":{"type":"paragraph","payload":{"text":"最后一段"}}},
    ]);
    let result = apply("rollback", &etag, operations.clone()).unwrap();
    assert_eq!(result.revision, 3);
    assert_eq!(result.created_block_ids.len(), 2);
    assert_eq!(result, apply("rollback", &etag, operations).unwrap());
    let updated = store.page(&workspace, &note.info.id).unwrap();
    assert_eq!(updated.blocks[0].id, quote.id);
    assert_eq!(updated.blocks[0].children[1], anchor);
    assert_eq!(updated.blocks[0].children[2].id, moved.id);
    assert_eq!(
        updated.blocks[0].children[2].payload["inline"][0]["text"],
        "更新后"
    );
    assert_eq!(updated.blocks[1], protected);
    assert_eq!(updated.blocks[2].id, result.created_block_ids[1]);
}

#[test]
fn record_writes_validate_the_live_schema_and_keep_unsubmitted_fields() {
    let (_root, store, workspace) = fixture();
    let (collection, page) = store
        .create_collection(&workspace, None, "实际记录")
        .unwrap();
    let mut schema = store.collection(&workspace, &collection.id).unwrap();
    for (label, kind) in [
        ("文本", PropertyKind::Text),
        ("数字", PropertyKind::Number),
        ("附件", PropertyKind::Attachment),
    ] {
        schema = store
            .save_property(
                &workspace,
                &collection.id,
                schema.schema_revision,
                PropertyDraft {
                    id: None,
                    name: label.into(),
                    kind,
                    options: vec![],
                },
            )
            .unwrap();
    }
    let text = &schema.properties[1].id;
    let number = &schema.properties[2].id;
    let file = &schema.properties[3].id;
    let writer = issue(
        &store,
        &workspace,
        Grant::Collection(collection.id.clone()),
        &[Scope::Create, Scope::Update],
    );
    let created = store
        .mutate_as_integration(
            &workspace,
            &writer.token,
            "create",
            None,
            Mutation::CreateRecord {
                collection_id: collection.id.clone(),
                title: "外部记录".into(),
                icon: "file".into(),
                properties: BTreeMap::from([
                    (text.clone(), json!("保留")),
                    (number.clone(), json!(9)),
                ]),
            },
        )
        .unwrap();
    let record = store.page(&workspace, &created.page_id).unwrap();
    assert_eq!(
        record.info.parent_id.as_deref(),
        Some(page.info.id.as_str())
    );
    let make_patch = |properties| Mutation::PatchProperties {
        page_id: created.page_id.clone(),
        patch: PagePatch {
            title: None,
            icon: None,
        },
        properties,
    };
    let etag = before(&store, &workspace, &writer.token, &created.page_id);
    let failed = store
        .mutate_as_integration(
            &workspace,
            &writer.token,
            "hidden-field",
            Some(&etag),
            make_patch(BTreeMap::from([
                (number.clone(), json!(7)),
                (file.clone(), Value::Null),
            ])),
        )
        .unwrap_err();
    assert_eq!(failed.code, "PARTIAL_CONTENT");
    assert_eq!(store.page(&workspace, &created.page_id).unwrap(), record);
    let other = issue(
        &store,
        &workspace,
        Grant::Page(created.page_id.clone()),
        &[Scope::Update],
    );
    let other_etag = before(&store, &workspace, &other.token, &created.page_id);
    assert_eq!(
        store
            .mutate_as_integration(
                &workspace,
                &other.token,
                "hidden-schema",
                Some(&other_etag),
                make_patch(BTreeMap::from([(number.clone(), json!(7))]))
            )
            .unwrap_err()
            .code,
        "PARTIAL_CONTENT"
    );

    let changed = store
        .mutate_as_integration(
            &workspace,
            &writer.token,
            "ordinary-field",
            Some(&etag),
            make_patch(BTreeMap::from([(number.clone(), Value::Null)])),
        )
        .unwrap();
    assert_eq!(changed.revision, 2);
    let persisted = store
        .page(&workspace, &created.page_id)
        .unwrap()
        .properties
        .unwrap();
    assert_eq!(persisted, BTreeMap::from([(text.clone(), json!("保留"))]));

    let before_schema = before(&store, &workspace, &writer.token, &created.page_id);
    store
        .save_property(
            &workspace,
            &collection.id,
            schema.schema_revision,
            PropertyDraft {
                id: Some(number.clone()),
                name: "新名称".into(),
                kind: PropertyKind::Number,
                options: vec![],
            },
        )
        .unwrap();
    let again = store
        .integration_page(&workspace, &writer.token, &created.page_id)
        .unwrap();
    assert_eq!(again.page_revision, 2);
    assert_ne!(again.etag, before_schema);
}

#[test]
fn archive_requires_the_scope_and_full_branch_then_attributes_every_revision() {
    let (_root, store, workspace) = fixture();
    let page = store.create_page(&workspace, None, "分支", "file").unwrap();
    let child = store
        .create_page(&workspace, Some(&page.info.id), "子页", "file")
        .unwrap();
    let limited = issue(
        &store,
        &workspace,
        Grant::Page(page.info.id.clone()),
        &[Scope::Archive],
    );
    let full = issue(
        &store,
        &workspace,
        Grant::Subtree(page.info.id.clone()),
        &[Scope::Archive],
    );
    let updater = issue(
        &store,
        &workspace,
        Grant::Subtree(page.info.id.clone()),
        &[Scope::Update],
    );
    let archive = |subject: &IssuedIntegration| {
        store.mutate_as_integration(
            &workspace,
            &subject.token,
            "archive",
            Some(&before(&store, &workspace, &subject.token, &page.info.id)),
            Mutation::ArchivePage {
                page_id: page.info.id.clone(),
            },
        )
    };
    assert_eq!(archive(&limited).unwrap_err().code, "PARTIAL_CONTENT");
    assert_eq!(archive(&updater).unwrap_err().code, "FORBIDDEN");
    assert_eq!(
        store
            .page(&workspace, &child.info.id)
            .unwrap()
            .info
            .revision,
        1
    );
    let done = archive(&full).unwrap();
    assert_eq!(done.revision, 2);
    for id in [&page.info.id, &child.info.id] {
        let item = store.page(&workspace, id).unwrap();
        assert_eq!(
            item.info.trash_root_id.as_deref(),
            Some(page.info.id.as_str())
        );
        assert_eq!(
            store.revision_attributions(&workspace, id).unwrap()[&2].operation_id,
            done.operation_id
        );
    }
    assert_eq!(
        store
            .integration_page(&workspace, &full.token, &page.info.id)
            .unwrap_err()
            .code,
        "RESOURCE_NOT_FOUND"
    );
    store.restore_page(&workspace, &page.info.id, 2).unwrap();
    assert!(
        store
            .integration_page(&workspace, &full.token, &child.info.id)
            .is_ok()
    );
}

#[test]
fn upload_checks_limits_revision_and_live_scope_and_commits_once() {
    let (root, store, workspace) = fixture();
    let page = store
        .create_page(&workspace, None, "附件页", "file")
        .unwrap();
    let grant = Grant::Page(page.info.id.clone());
    let uploader = issue(
        &store,
        &workspace,
        grant.clone(),
        &[Scope::Update, Scope::WriteAssets],
    );
    let viewer = issue(
        &store,
        &workspace,
        grant.clone(),
        &[Scope::Update, Scope::ReadAssets],
    );
    let revoked = issue(
        &store,
        &workspace,
        grant,
        &[Scope::Update, Scope::WriteAssets],
    );
    let etag = before(&store, &workspace, &uploader.token, &page.info.id);
    let viewer_etag = before(&store, &workspace, &viewer.token, &page.info.id);
    let revoke_etag = before(&store, &workspace, &revoked.token, &page.info.id);
    let request = |key, tag| IntegrationAssetUpload {
        page_id: &page.info.id,
        name: "实验原件.txt",
        request_key: key,
        if_match: Some(tag),
    };
    assert_eq!(store.api_attachment_limit_mib(&workspace).unwrap(), 100);
    store.set_api_attachment_limit_mib(&workspace, 1).unwrap();
    assert_eq!(
        store
            .upload_as_integration(
                &workspace,
                &viewer.token,
                request("readonly", &viewer_etag),
                &b"abcd"[..]
            )
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    let large = vec![0_u8; 1024 * 1024 + 1];
    assert_eq!(
        store
            .upload_as_integration(
                &workspace,
                &uploader.token,
                request("large", &etag),
                &large[..]
            )
            .unwrap_err()
            .code,
        "FILE_TOO_LARGE"
    );
    let interrupted = CallbackReader {
        read: "未发布".as_bytes(),
        callback: Some(|| {
            store
                .revoke_integration(&workspace, &revoked.integration.id)
                .unwrap()
        }),
    };
    assert_eq!(
        store
            .upload_as_integration(
                &workspace,
                &revoked.token,
                request("revoked", &revoke_etag),
                interrupted
            )
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert!(
        store
            .attachments(&workspace, &page.info.id)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        std::fs::read_dir(root.path().join(&workspace).join("blobs"))
            .unwrap()
            .count(),
        0
    );
    assert_eq!(
        store.page(&workspace, &page.info.id).unwrap().info.revision,
        1
    );

    let contents = b"real fixture bytes";
    let save = |data: &[u8]| {
        store.upload_as_integration(&workspace, &uploader.token, request("done", &etag), data)
    };
    let receipt = save(contents).unwrap();
    assert_eq!(receipt.revision, 2);
    assert_eq!(receipt, save(contents).unwrap());
    assert_eq!(
        save(b"different bytes").unwrap_err().code,
        "IDEMPOTENCY_CONFLICT"
    );
    assert_eq!(
        store.attachments(&workspace, &page.info.id).unwrap().len(),
        1
    );
    assert_eq!(
        store
            .integration_asset(&workspace, &uploader.token, &receipt.asset_id)
            .unwrap_err()
            .code,
        "FORBIDDEN"
    );
    let mut bytes = Vec::new();
    store
        .copy_attachment(&workspace, &page.info.id, &receipt.asset_id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, contents);
    assert_eq!(
        store
            .revision_attributions(&workspace, &page.info.id)
            .unwrap()[&2]
            .operation_id,
        receipt.operation_id
    );
}

struct CallbackReader<'a, F> {
    read: &'a [u8],
    callback: Option<F>,
}

impl<F: FnOnce()> Read for CallbackReader<'_, F> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if let Some(callback) = self.callback.take() {
            callback();
        }
        self.read.read(buffer)
    }
}

#[test]
fn notion_colors_stay_visible_and_export_as_plain_text() {
    let (_root, store, workspace) = fixture();
    let note = store.create_page(&workspace, None, "颜色", "file").unwrap();
    let target = store
        .create_page(&workspace, Some(&note.info.id), "被引用", "file")
        .unwrap();
    let colored = block(
        "paragraph",
        json!({"color": "blue_background", "inline": [
            {"type": "pageReference", "pageId": target.info.id, "label": "被引用"},
            {"type": "text", "text": "红字黄底", "marks": [
                {"type": "textColor", "color": "red"}, {"type": "backgroundColor", "color": "yellow"},
            ]},
        ]}),
        vec![],
    );
    let unknown = block(
        "paragraph",
        json!({"inline": [{"type": "text", "text": "未知颜色", "marks": [{"type": "textColor", "color": "#f00"}]}]}),
        vec![],
    );
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks: vec![colored, unknown],
            },
        )
        .unwrap();
    let viewer = issue(
        &store,
        &workspace,
        Grant::Subtree(note.info.id.clone()),
        &[],
    );
    let page = store
        .integration_page(&workspace, &viewer.token, &note.info.id)
        .unwrap();
    let exported = integration_markdown(&workspace, None, &page.blocks[..1]);
    assert!(
        exported.markdown.contains("红字黄底"),
        "{}",
        exported.markdown
    );
    assert!(exported.losses.is_empty(), "{:?}", exported.losses);
    // Colors outside Notion's palette are not readable inline content for integrations.
    let all = integration_markdown(&workspace, None, &page.blocks);
    assert!(!all.markdown.contains("未知颜色"));
    // A colored paragraph still counts as a backlink to the page it mentions.
    let backlinks = store.backlinks(&workspace, &target.info.id, None).unwrap();
    assert_eq!(backlinks.items.len(), 1);
}

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 56, 150, 102, 246, 31, 0, 5,
    186, 2, 98, 77, 127, 216, 92, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[test]
fn notion_layout_blocks_bookmarks_and_comments_stay_readable() {
    let (_root, store, workspace) = fixture();
    let note = store.create_page(&workspace, None, "布局", "file").unwrap();
    let preview = store
        .add_image(&workspace, &note.info.id, "书签预览.png", PNG)
        .unwrap()
        .attachment;
    let commented = block(
        "paragraph",
        json!({"inline": [{"type": "text", "text": "有评论", "marks": [{"type": "comment", "text": "补充来源", "replies": ["已补充"]}]}]}),
        vec![],
    );
    let blocks = vec![
        commented,
        block("indentBlock", json!({}), vec![paragraph("缩进子块")]),
        block(
            "columnList",
            json!({}),
            vec![
                block("column", json!({}), vec![paragraph("左栏内容")]),
                block("column", json!({}), vec![paragraph("右栏内容")]),
            ],
        ),
        block(
            "bookmark",
            json!({"url": "https://example.com/a", "title": "示例书签", "description": "网页摘要",
                "attachmentId": preview.id, "mediaType": "image/png"}),
            vec![],
        ),
    ];
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks,
            },
        )
        .unwrap();
    let viewer = issue(&store, &workspace, Grant::Page(note.info.id.clone()), &[]);
    let page = store
        .integration_page(&workspace, &viewer.token, &note.info.id)
        .unwrap();
    let exported = integration_markdown(&workspace, None, &page.blocks);
    for text in [
        "有评论",
        "缩进子块",
        "左栏内容",
        "右栏内容",
        "[示例书签](<https://example.com/a>)",
    ] {
        assert!(
            exported.markdown.contains(text),
            "{text}: {}",
            exported.markdown
        );
    }
    assert!(!exported.markdown.contains("补充来源") && !exported.markdown.contains("已补充"));
    // Without asset access the card keeps its text but not its preview image.
    let card = page
        .blocks
        .iter()
        .find(|block| block.kind == "bookmark")
        .unwrap();
    assert_eq!(card.payload["title"], "示例书签");
    assert!(card.payload.get("attachmentId").is_none() && card.payload.get("mediaType").is_none());
    assert!(!exported.markdown.contains("不可访问"));
}

#[test]
fn child_blocks_and_comment_mentions_stay_readable_and_link_back() {
    let (_root, store, workspace) = fixture();
    let note = store.create_page(&workspace, None, "子块", "file").unwrap();
    let mentioned = store
        .create_page(&workspace, Some(&note.info.id), "被提及", "file")
        .unwrap();
    let mention = format!("看看 @[被提及](page:{})", mentioned.info.id);
    let parent = block(
        "paragraph",
        json!({"inline": [{"type": "text", "text": "上级段落", "marks": [
            {"type": "comment", "text": "备注", "replies": [mention]},
        ]}]}),
        vec![paragraph("缩进的子块")],
    );
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks: vec![parent],
            },
        )
        .unwrap();
    let viewer = issue(
        &store,
        &workspace,
        Grant::Subtree(note.info.id.clone()),
        &[],
    );
    let page = store
        .integration_page(&workspace, &viewer.token, &note.info.id)
        .unwrap();
    let exported = integration_markdown(&workspace, None, &page.blocks);
    assert!(
        exported.markdown.contains("上级段落"),
        "{}",
        exported.markdown
    );
    assert!(
        exported.markdown.contains("缩进的子块"),
        "{}",
        exported.markdown
    );
    // A page mentioned in a comment reply lists the commented block as a backlink.
    let backlinks = store
        .backlinks(&workspace, &mentioned.info.id, None)
        .unwrap();
    assert_eq!(backlinks.items.len(), 1);
}

#[test]
fn html_export_keeps_notion_colors_and_markdown_keeps_text() {
    let (_root, store, workspace) = fixture();
    let note = store
        .create_page(&workspace, None, "颜色导出", "file")
        .unwrap();
    let colored = block(
        "paragraph",
        json!({"color": "red_background", "inline": [
            {"type": "text", "text": "提醒", "marks": [{"type": "textColor", "color": "blue"}]},
            {"type": "text", "text": "<span data-text-color=\"red\">伪造</span>"},
        ]}),
        vec![],
    );
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks: vec![colored],
            },
        )
        .unwrap();
    let page = store.page(&workspace, &note.info.id).unwrap();
    let html = render_page_export(&page, &[], &workspace, PageExportFormat::Html);
    assert!(
        html.document
            .contains("<span data-block-color=\"red_background\">")
    );
    assert!(
        html.document
            .contains("<span data-text-color=\"blue\">提醒</span>")
    );
    assert!(html.document.contains("[data-text-color=blue]"));
    // Text that only looks like a color span stays text.
    assert!(!html.document.contains("<span data-text-color=\"red\">"));
    let markdown = render_page_export(&page, &[], &workspace, PageExportFormat::Markdown);
    assert!(markdown.document.contains("提醒"));
    assert!(!markdown.document.contains("data-block-color"));
    assert!(!markdown.document.contains("data-text-color=\"blue\""));
}

#[test]
fn markdown_retains_readable_links_marks_losses_and_omits_hidden_targets() {
    let (_root, store, workspace) = fixture();
    let parent = store
        .create_page(&workspace, None, "公开区", "file")
        .unwrap();
    let hidden = store
        .create_page(&workspace, None, "暗号原名", "file")
        .unwrap();
    let note = store
        .create_page(&workspace, Some(&parent.info.id), "Markdown 分片", "file")
        .unwrap();
    let link = block(
        "paragraph",
        json!({"inline":[
            {"type":"pageReference","pageId":parent.info.id,"label":"可读链接"},
            {"type":"pageReference","pageId":hidden.info.id,"label":"暗号标签"},
        ]}),
        vec![],
    );
    let original = block(
        "orderedList",
        json!({"type":"a"}),
        vec![block("listItem", json!({}), vec![paragraph("字母序号")])],
    );
    store
        .save_page(
            &workspace,
            &note.info.id,
            1,
            PageDraft {
                title: note.info.title,
                blocks: vec![link, original],
            },
        )
        .unwrap();
    let viewer = issue(
        &store,
        &workspace,
        Grant::Subtree(parent.info.id.clone()),
        &[],
    );
    let safe = store
        .integration_page(&workspace, &viewer.token, &note.info.id)
        .unwrap();
    let first = integration_markdown(&workspace, Some(&safe.title), &safe.blocks[..1]);
    assert!(
        first
            .markdown
            .contains(&format!("/pages/{}", parent.info.id))
    );
    assert!(first.markdown.contains("不可访问"));
    assert!(!first.markdown.contains(&hidden.info.id));
    assert!(!first.markdown.contains("暗号标签"));
    assert_eq!(first.losses, ["restricted_content"]);
    let last = integration_markdown(&workspace, None, &safe.blocks[1..]);
    assert!(last.markdown.contains("字母序号"));
    assert!(!last.markdown.contains(&safe.title));
    assert_eq!(last.losses, ["markdown_approximation"]);
}
