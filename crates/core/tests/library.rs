// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};
use tempfile::TempDir;
use zhiye_core::{
    Block, Collection, CsvDestination, ImportFile, ImportStatus, Page, PageDraft, PropertyDraft,
    PropertyKind, RecordDraft, Store,
};

fn setup() -> (TempDir, Store, String) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let space = store.create_workspace("资料").unwrap().id;
    (directory, store, space)
}

fn file(name: &str, bytes: impl AsRef<[u8]>) -> ImportFile {
    ImportFile {
        name: name.into(),
        bytes: bytes.as_ref().to_vec(),
    }
}

fn draft(title: &str, inline: Value) -> PageDraft {
    PageDraft {
        title: title.into(),
        blocks: vec![Block {
            id: uuid::Uuid::new_v4().to_string(),
            kind: "paragraph".into(),
            schema_version: 1,
            payload: json!({"inline": inline}),
            children: vec![],
            extra: Default::default(),
        }],
    }
}

fn original(store: &Store, space: &str, page: &str) -> Vec<u8> {
    let items = store.attachments(space, page).unwrap();
    assert_eq!(items.len(), 1);
    let mut bytes = Vec::new();
    store
        .copy_attachment(space, page, &items[0].id, &mut bytes)
        .unwrap();
    bytes
}

fn add_property(
    store: &Store,
    space: &str,
    schema: &mut Collection,
    name: &str,
    kind: PropertyKind,
) -> String {
    *schema = store
        .save_property(
            space,
            &schema.id,
            schema.schema_revision,
            PropertyDraft {
                id: None,
                name: name.into(),
                kind,
                options: Vec::new(),
            },
        )
        .unwrap();
    schema.properties.last().unwrap().id.clone()
}

fn child(store: &Store, space: &str, root: &Page) -> Page {
    let child = store
        .pages(space)
        .unwrap()
        .into_iter()
        .find(|page| page.parent_id.as_deref() == Some(root.info.id.as_str()))
        .unwrap();
    store.page(space, &child.id).unwrap()
}

#[test]
fn markdown_import_maps_forward_and_wiki_links_with_originals_and_losses() {
    let (_directory, store, space) = setup();
    let first = "\u{feff}# 中文标题\n\n[向前](%E7%AC%AC%E4%BA%8C%E7%AB%A0.md#段落)\n\n- [x] 完成\n  - 子项\n- [ ] 待办\n\n#### 降级标题\n\n|名称|值|\n|---|---|\n|甲|1|\n\n![图片](https://example.test/图.png)";
    let second = "[[第一章|返回]]\n\n**中文定位词**  \n下一行\n\n```rust\nlet zero = 0;\n```";
    let source = || file("第一章.md", first);
    let report = store
        .import_markdown(
            &space,
            None,
            vec![
                source(),
                file("第二章.md", second),
                file("损坏.md", [0xff]),
                source(),
            ],
        )
        .unwrap();
    assert_eq!(
        report
            .iter()
            .map(|result| result.status)
            .collect::<Vec<_>>(),
        [
            ImportStatus::Imported,
            ImportStatus::Imported,
            ImportStatus::Failed,
            ImportStatus::Duplicate
        ]
    );
    let id1 = report[0].page_id.as_deref().unwrap();
    let id2 = report[1].page_id.as_deref().unwrap();
    assert_eq!(report[3].page_id.as_deref(), Some(id1));
    assert_eq!(report[0].notes.len(), 3);
    assert!(report[0].notes.iter().any(|note| note.contains("锚点")));
    assert!(report[2].notes.iter().any(|note| note.contains("UTF-8")));
    let page1 = store.page(&space, id1).unwrap();
    let page2 = store.page(&space, id2).unwrap();
    assert_eq!(page1.info.title, "第一章");
    assert_eq!(page1.blocks[1].payload["inline"][0]["pageId"], id2);
    assert_eq!(page2.blocks[0].payload["inline"][0]["pageId"], id1);
    assert_eq!(page2.blocks[0].payload["inline"][0]["label"], "返回");
    assert_eq!(page1.blocks[2].kind, "taskList");
    assert_eq!(page1.blocks[2].children[0].payload["checked"], true);
    assert_eq!(page1.blocks[2].children[0].children[1].kind, "bulletList");
    assert_eq!(page1.blocks[3].payload["level"], 3);
    assert_eq!(page1.blocks[4].kind, "table");
    let cell = &page1.blocks[4].children[1].children[0];
    assert_eq!(cell.children[0].payload["inline"][0]["text"], "甲");
    assert_eq!(page2.blocks[2].payload["language"], "rust");
    assert_eq!(original(&store, &space, id1), first.as_bytes());
    while store.advance_search_index(&space).unwrap().pending > 0 {}
    assert_eq!(store.search(&space, "中文定位词").unwrap()[0].id, id2);
    let result = store.import_markdown(&space, None, vec![source()]).unwrap();
    assert_eq!(result[0].status, ImportStatus::Duplicate);
    assert_eq!(store.pages(&space).unwrap().len(), 2);
}

#[test]
fn markdown_reports_ambiguous_paths_and_rolls_back_on_a_receipt_write_failure() {
    let (directory, store, space) = setup();
    let db = Connection::open(directory.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER stop_import BEFORE INSERT ON import_receipts
        WHEN new.name = '最后.md' BEGIN SELECT RAISE(ABORT, 'receipt failure'); END;",
    )
    .unwrap();
    let batch = || {
        vec![
            file("同名.md", "甲"),
            file("同名.md", "乙"),
            file("最后.md", "[同名](同名.md)"),
        ]
    };
    assert_eq!(
        store
            .import_markdown(&space, None, batch())
            .unwrap_err()
            .code,
        "DATABASE"
    );
    assert!(store.pages(&space).unwrap().is_empty());
    let rows: u32 = db
        .query_row("SELECT count(*) FROM import_receipts", [], |row| row.get(0))
        .unwrap();
    assert_eq!(rows, 0);
    db.execute_batch("DROP TRIGGER stop_import").unwrap();
    let report = store.import_markdown(&space, None, batch()).unwrap();
    let id = report[2].page_id.as_deref().unwrap();
    let page = store.page(&space, id).unwrap();
    assert_eq!(
        page.blocks[0].payload["inline"][0]["text"],
        "同名（同名.md）"
    );
    assert!(report[2].notes[0].contains("唯一匹配"));
    assert_eq!(original(&store, &space, id), "[同名](同名.md)".as_bytes());
}

#[test]
fn csv_import_creates_a_database_and_retains_original_quoted_data() {
    let (_directory, store, space) = setup();
    let input = "\u{feff}备注,标题,编号\r\n\"第一行，带逗号,\r\n第二行含\"\"引号\"\"\",甲,001\r\n缺列,乙\r\n标题为空,,003\r\n正常,丙,0004\r\n";
    let destination = || CsvDestination::New { parent_id: None };
    let result = store
        .import_csv(&space, destination(), file("测试.csv", input))
        .unwrap();
    assert_eq!(result.status, ImportStatus::Imported);
    assert_eq!(result.records, 2);
    assert_eq!(result.notes.len(), 2);
    let page = result.page_id.as_deref().unwrap();
    let collection = store.collections(&space).unwrap().pop().unwrap();
    assert_eq!(collection.container_page_id, page);
    let schema = store.collection(&space, &collection.id).unwrap();
    assert_eq!(
        schema
            .properties
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["备注", "编号"]
    );
    let records = store
        .query_records(&space, &collection.id, &schema.views[0].id, 0)
        .unwrap();
    let first = records
        .records
        .iter()
        .find(|item| item.title == "甲")
        .unwrap();
    assert_eq!(
        first.properties[&schema.properties[0].id],
        "第一行，带逗号,\r\n第二行含\"引号\""
    );
    assert_eq!(first.properties[&schema.properties[1].id], "001");
    assert_eq!(original(&store, &space, page), input.as_bytes());
    let asset = store.attachments(&space, page).unwrap().pop().unwrap();
    let reported = result
        .original
        .expect("successful CSV report points to the original");
    assert_eq!(reported.page_id, page);
    assert_eq!(reported.attachment_id, asset.id);
    assert_eq!(reported.name, "测试.csv");
    assert_eq!(
        store
            .import_csv(&space, destination(), file("测试.csv", input))
            .unwrap()
            .status,
        ImportStatus::Duplicate
    );
    let malformed = store
        .import_csv(
            &space,
            destination(),
            file("损坏.csv", "标题,正文\n示例,\"未闭合\n另一个,会被吞入"),
        )
        .unwrap();
    assert_eq!(malformed.status, ImportStatus::Failed);
    assert!(malformed.notes[0].contains("未闭合"));
    assert_eq!(store.collections(&space).unwrap().len(), 1);
}

#[test]
fn csv_append_uses_property_types_and_reports_unmapped_or_invalid_values() {
    let (_directory, store, space) = setup();
    let (database, _) = store.create_collection(&space, None, "任务").unwrap();
    let mut schema = store.collection(&space, &database.id).unwrap();
    let number = add_property(&store, &space, &mut schema, "价格", PropertyKind::Number);
    let done = add_property(&store, &space, &mut schema, "确认", PropertyKind::Checkbox);
    let date = add_property(&store, &space, &mut schema, "日期", PropertyKind::Date);
    let status = &schema.properties[0];
    let input = "标题,价格,确认,日期,状态,额外\n第一项,0,false,2024-02-29,进行中,保留\n第二项,无效,是,2025-02-29,未定义,原件\n";
    let source_attachment = store
        .add_attachment(
            &space,
            &database.container_page_id,
            "追加.csv",
            input.as_bytes(),
        )
        .unwrap()
        .attachment;
    let result = store
        .import_csv(
            &space,
            CsvDestination::Existing {
                collection_id: database.id.clone(),
            },
            file("追加.csv", input),
        )
        .unwrap();
    assert_eq!(result.status, ImportStatus::Imported);
    assert_eq!(result.records, 2);
    assert_eq!(result.notes.len(), 4);
    let records = store
        .query_records(&space, &database.id, &schema.views[0].id, 0)
        .unwrap();
    let one = records
        .records
        .iter()
        .find(|item| item.title == "第一项")
        .unwrap();
    assert_eq!(one.properties[&number], 0);
    assert_eq!(one.properties[&done], false);
    assert_eq!(one.properties[&date], "2024-02-29");
    assert_eq!(one.properties[&status.id], status.options[1].id);
    let two = records
        .records
        .iter()
        .find(|item| item.title == "第二项")
        .unwrap();
    assert_eq!(two.properties, BTreeMap::from([(done, json!(true))]));
    assert_eq!(
        original(&store, &space, &database.container_page_id),
        input.as_bytes()
    );
    let reported = result
        .original
        .expect("CSV report points to the reused original");
    assert_eq!(reported.attachment_id, source_attachment.id);
    assert_eq!(reported.page_id, database.container_page_id);
    assert_eq!(reported.name, "追加.csv");
}

#[test]
fn csv_report_uses_source_lines_with_empty_lines_and_quoted_newlines() {
    let (_directory, store, space) = setup();
    let (database, _) = store.create_collection(&space, None, "行号验收").unwrap();
    let mut schema = store.collection(&space, &database.id).unwrap();
    add_property(&store, &space, &mut schema, "分数", PropertyKind::Number);
    let lines = [
        "\u{feff}标题,分数,未映射列",
        "",
        "  首条记录  ,12.5,\"备注",
        "跨行\"",
        "",
        "错误数值,非数字,原值甲",
        ",3,空标题",
        "列数不足,4",
    ];
    for (name, newline) in [("LF", "\n"), ("CRLF", "\r\n"), ("CR", "\r")] {
        let result = store
            .import_csv(
                &space,
                CsvDestination::Existing {
                    collection_id: database.id.clone(),
                },
                file(&format!("{name}.csv"), lines.join(newline)),
            )
            .unwrap();
        assert_eq!(result.status, ImportStatus::Imported, "{name}");
        assert_eq!(result.records, 2, "{name}");
        assert_eq!(
            result.notes[1..],
            [
                "第 3 行标题的首尾空白已去除。",
                "第 6 行「分数」无法转换，原值保存在 CSV 原件中。",
                "第 7 行的标题无效，此行已跳过。",
                "第 8 行有 2 列，预期 3 列，此行已跳过。",
            ],
            "{name}"
        );
    }
}

#[test]
fn page_templates_remap_subtree_links_and_keep_previous_creations_after_updates() {
    let (_directory, store, space) = setup();
    let source = store.create_page(&space, None, "计划", "file").unwrap();
    let sub = store
        .create_page(&space, Some(&source.info.id), "执行", "file")
        .unwrap();
    let outside = store.create_page(&space, None, "外部参考", "file").unwrap();
    store
        .save_page(
            &space,
            &source.info.id,
            1,
            draft(
                "计划",
                json!([
                    {"type":"pageReference","pageId":sub.info.id,"label":"执行"},
                    {"type":"pageReference","pageId":outside.info.id,"label":"参考"}
                ]),
            ),
        )
        .unwrap();
    store
        .save_page(
            &space,
            &sub.info.id,
            1,
            draft(
                "执行",
                json!([
                    {"type":"pageReference","pageId":source.info.id,"label":"计划"}
                ]),
            ),
        )
        .unwrap();
    store
        .add_attachment(&space, &sub.info.id, "清单.txt", "原件".as_bytes())
        .unwrap();
    let template = store
        .save_template(&space, &source.info.id, "项目模板", None)
        .unwrap();
    assert_eq!(template.page_count, 2);
    let copy1 = store
        .apply_template(&space, &template.id, None, Some("项目 A"))
        .unwrap();
    let sub1 = child(&store, &space, &copy1);
    assert_ne!(copy1.info.id, source.info.id);
    assert_ne!(sub1.info.id, sub.info.id);
    assert_eq!(copy1.info.revision, 1);
    assert_ne!(
        sub1.blocks[0].id,
        store.page(&space, &sub.info.id).unwrap().blocks[0].id
    );
    assert_eq!(sub1.blocks[0].payload["inline"][0]["pageId"], copy1.info.id);
    assert_eq!(copy1.blocks[0].payload["inline"][0]["pageId"], sub1.info.id);
    assert_eq!(
        copy1.blocks[0].payload["inline"][1]["pageId"],
        outside.info.id
    );
    assert_eq!(original(&store, &space, &sub1.info.id), "原件".as_bytes());
    store
        .save_page(
            &space,
            &source.info.id,
            2,
            draft("新版本", json!([{"type":"text","text":"修订后"}])),
        )
        .unwrap();
    store
        .save_template(&space, &source.info.id, "项目模板", Some(&template.id))
        .unwrap();
    let copy2 = store
        .apply_template(&space, &template.id, None, None)
        .unwrap();
    assert_eq!(copy2.info.title, "新版本");
    assert_eq!(copy2.blocks[0].payload["inline"][0]["text"], "修订后");
    assert_eq!(store.page(&space, &copy1.info.id).unwrap(), copy1);
    store
        .create_collection(&space, Some(&source.info.id), "子数据库")
        .unwrap();
    assert_eq!(
        store
            .save_template(&space, &source.info.id, "有数据库", None)
            .unwrap_err()
            .code,
        "TEMPLATE_DATABASE"
    );
    store.delete_template(&space, &template.id).unwrap();
    assert_eq!(store.page(&space, &copy2.info.id).unwrap(), copy2);
}

#[test]
fn zip_restores_record_templates_navigation_receipts_and_attachment_properties() {
    let (directory, store, space) = setup();
    let (database, _) = store.create_collection(&space, None, "任务").unwrap();
    let mut schema = store.collection(&space, &database.id).unwrap();
    let file_field = add_property(
        &store,
        &space,
        &mut schema,
        "原件",
        PropertyKind::Attachment,
    );
    let record = store
        .create_page(&space, Some(&database.container_page_id), "需处理", "file")
        .unwrap();
    let attachment = store
        .add_attachment(&space, &record.info.id, "材料.txt", "资料".as_bytes())
        .unwrap()
        .attachment;
    store
        .save_record(
            &space,
            &database.id,
            &record.info.id,
            1,
            RecordDraft {
                title: record.info.title,
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([(file_field.clone(), json!([attachment.id]))]),
            },
        )
        .unwrap();
    let template = store
        .save_template(&space, &record.info.id, "任务模板", None)
        .unwrap();
    assert_eq!(
        store
            .apply_template(&space, &template.id, None, None)
            .unwrap_err()
            .code,
        "TEMPLATE_SCOPE"
    );
    store
        .set_inbox(&space, &database.container_page_id)
        .unwrap();
    let captured = store.capture_page(&space, "收集项").unwrap();
    assert_eq!(
        captured.info.parent_id.as_deref(),
        Some(database.container_page_id.as_str())
    );
    assert!(captured.properties.is_some());
    store.set_favorite(&space, &record.info.id, true).unwrap();
    store.visit_page(&space, &record.info.id).unwrap();
    store.visit_page(&space, &captured.info.id).unwrap();
    let source = || file("重复验证.md", "# 正文");
    let imported = store
        .import_markdown(&space, None, vec![source()])
        .unwrap()
        .remove(0);
    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    let restored = store.read_workspace_archive(archive).unwrap();
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let nav = store.navigation(&restored.id).unwrap();
    assert_eq!(
        nav.inbox_page_id.as_deref(),
        Some(database.container_page_id.as_str())
    );
    assert_eq!(nav.favorites, [record.info.id.clone()]);
    assert_eq!(nav.recent_pages, [captured.info.id, record.info.id.clone()]);
    let duplicate = store
        .import_markdown(&restored.id, None, vec![source()])
        .unwrap()
        .remove(0);
    assert_eq!(duplicate.status, ImportStatus::Duplicate);
    assert_eq!(duplicate.page_id, imported.page_id);
    let cloned = store
        .apply_template(
            &restored.id,
            &template.id,
            Some(&database.container_page_id),
            None,
        )
        .unwrap();
    let files = store.attachments(&restored.id, &cloned.info.id).unwrap();
    assert_eq!(files[0].sha256, attachment.sha256);
    assert_ne!(files[0].id, attachment.id);
    assert_eq!(
        cloned.properties.unwrap()[&file_field],
        json!([files[0].id])
    );
    assert_eq!(
        original(&store, &restored.id, &cloned.info.id),
        "资料".as_bytes()
    );
    store
        .archive_page(&restored.id, &record.info.id, 2)
        .unwrap();
    assert!(store.navigation(&restored.id).unwrap().favorites.is_empty());
    store
        .restore_page(&restored.id, &record.info.id, 3)
        .unwrap();
    assert_eq!(
        store.navigation(&restored.id).unwrap().favorites,
        [record.info.id]
    );
}

#[test]
fn version_four_inbox_is_adopted_and_pre_upgrade_snapshot_remains_readable() {
    let (directory, store, space) = setup();
    let inbox = store
        .create_page(&space, None, "旧收集页", "inbox")
        .unwrap();
    drop(store);
    let db = Connection::open(directory.path().join(&space).join("workspace.sqlite")).unwrap();
    support::remove_v7_schema(&db);
    db.execute_batch("DROP TABLE integration_operations; DROP TABLE integration_revisions;
                      DROP TABLE integration_grants; DROP TABLE integrations;
                      DROP TABLE templates; DROP TABLE import_receipts; DROP TABLE favorites;
                      DROP TABLE recent_pages; DROP TABLE workspace_inbox; PRAGMA user_version = 4;").unwrap();
    drop(db);
    let store = Store::open(directory.path()).unwrap();
    assert_eq!(
        store.navigation(&space).unwrap().inbox_page_id.as_deref(),
        Some(inbox.info.id.as_str())
    );
    let snapshot = directory
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v4.sqlite"));
    let version: u32 = Connection::open(snapshot)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 4);
    store.archive_page(&space, &inbox.info.id, 1).unwrap();
    let capture = store.capture_page(&space, "快速收集").unwrap();
    let replacement = store.navigation(&space).unwrap().inbox_page_id.unwrap();
    assert_ne!(replacement, inbox.info.id);
    assert_eq!(capture.info.parent_id, Some(replacement));
}
