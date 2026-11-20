// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use std::{collections::HashMap, io::Cursor};
use zhiye_core::{
    Block, ImportFile, ImportStatus, PageDraft, SearchRequest, Store, render_markdown_with_images,
};

fn block(kind: &str, payload: Value) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        children: vec![],
        extra: Default::default(),
        payload,
    }
}

#[test]
fn inline_image_identity_is_checked_and_survives_rename_history_templates_and_zip() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("图片验证").unwrap();
    let first = store.create_page(&space.id, None, "图文", "file").unwrap();
    let second = store
        .create_page(&space.id, None, "另一页", "file")
        .unwrap();
    let fake = store.add_image(
        &space.id,
        &first.info.id,
        "伪装.png",
        b"%PDF-1.7".as_slice(),
    );
    assert_eq!(fake.unwrap_err().code, "IMAGE_TYPE");
    assert!(
        store
            .attachments(&space.id, &first.info.id)
            .unwrap()
            .is_empty()
    );

    let source = b"\x89PNG\r\n\x1a\nfixture";
    let image = store
        .add_image(&space.id, &first.info.id, "原件.png", source.as_slice())
        .unwrap()
        .attachment;
    let body = block(
        "attachmentImage",
        json!({
            "attachmentId": image.id, "mediaType": image.media_type, "alt": "蓝鲸插图"
        }),
    );
    let draft = |blocks| PageDraft {
        title: first.info.title.clone(),
        blocks,
    };
    let err = store
        .save_page(&space.id, &second.info.id, 1, draft(vec![body.clone()]))
        .unwrap_err();
    assert_eq!(err.code, "IMAGE_ATTACHMENT");
    let mut wrong_type = body.clone();
    wrong_type.payload["mediaType"] = json!("image/jpeg");
    assert_eq!(
        store
            .save_page(&space.id, &first.info.id, 1, draft(vec![wrong_type]))
            .unwrap_err()
            .code,
        "IMAGE_ATTACHMENT"
    );
    assert_eq!(store.history(&space.id, &first.info.id).unwrap().len(), 1);

    // Future and foreign shapes continue to round-trip through the opaque format.
    let opaque = block("attachmentImage", json!({"legacy": "keep"}));
    let version = store
        .save_page(
            &space.id,
            &first.info.id,
            1,
            draft(vec![body.clone(), opaque.clone()]),
        )
        .unwrap();
    assert_eq!(version.revision, 2);
    store
        .rename_attachment(&space.id, &first.info.id, &image.id, "重命名.png")
        .unwrap();
    while store.advance_search_index(&space.id).unwrap().pending > 0 {}
    let query: SearchRequest = serde_json::from_value(json!({"query": "蓝鲸"})).unwrap();
    let search = store.search_text(&space.id, &query).unwrap();
    assert_eq!(search.hits[0].block_id.as_deref(), Some(body.id.as_str()));
    let copies = store
        .image_attachments(&space.id, &first.info.id, &[body.clone(), body.clone()])
        .unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].name, "重命名.png");
    let mapping = HashMap::from([(image.id.clone(), "exported/one.png".to_owned())]);
    assert!(
        render_markdown_with_images("图文", std::slice::from_ref(&body), &mapping)
            .contains("![蓝鲸插图](exported/one.png)")
    );

    let empty = store
        .save_page(&space.id, &first.info.id, 2, draft(Vec::new()))
        .unwrap();
    let restored = store
        .restore_version(&space.id, &first.info.id, empty.revision, version.revision)
        .unwrap();
    assert_eq!(restored.blocks, [body.clone(), opaque.clone()]);
    let template = store
        .save_template(&space.id, &first.info.id, "图文模板", None)
        .unwrap();
    let copy = store
        .apply_template(&space.id, &template.id, None, Some("副本"))
        .unwrap();
    let copied = store
        .image_attachments(&space.id, &copy.info.id, &copy.blocks)
        .unwrap()
        .remove(0);
    assert_ne!(copy.blocks[0].id, body.id);
    assert_ne!(copied.id, image.id);
    assert_eq!(copied.sha256, image.sha256);
    assert_eq!(copy.blocks[1].payload, opaque.payload);

    let mut archive = Cursor::new(Vec::new());
    store
        .write_workspace_archive(&space.id, &mut archive)
        .unwrap();
    let recovery = store.read_workspace_archive(archive).unwrap();
    let restored = store.page(&recovery.id, &first.info.id).unwrap();
    assert_eq!(restored.blocks, [body, opaque]);
    let mut bytes = Vec::new();
    store
        .copy_attachment(&recovery.id, &copy.info.id, &copied.id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, source);
}

#[test]
fn markdown_uses_editable_tables_with_alignment_and_http_links() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("表格验证").unwrap();
    let input = "| 名称 | 来源 |\n| :---: | ---: |\n| 含\\|竖线 | [参考](https://example.test/页面) |\n\n[禁止](javascript:example)";
    let file = ImportFile {
        name: "例.md".into(),
        bytes: input.as_bytes().to_vec(),
    };
    let result = store
        .import_markdown(&space.id, None, vec![file])
        .unwrap()
        .remove(0);
    assert_eq!(result.status, ImportStatus::Imported);
    assert_eq!(result.notes.len(), 1);
    let page = store
        .page(&space.id, result.page_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(page.blocks[0].kind, "table");
    let rows = &page.blocks[0].children;
    assert_eq!(rows[0].children[0].kind, "tableHeader");
    assert_eq!(rows[1].children[0].payload["align"], "center");
    let link = &rows[1].children[1].children[0].payload["inline"][0]["marks"][0];
    assert_eq!(link["type"], "link");
    assert_eq!(link["href"], "https://example.test/%E9%A1%B5%E9%9D%A2");
    let markdown = zhiye_core::render_markdown(&page);
    assert!(markdown.contains("| :---: | ---: |"));
    assert!(markdown.contains("含\\|竖线"));
    assert!(markdown.contains("[参考](<https://example.test/"));
    assert!(!markdown.contains("](<javascript:"));
}
