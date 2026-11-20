// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::Connection;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashSet};
use std::io::{Cursor, Read, Write};
use tempfile::TempDir;
use zhiye_core::{
    Block, Collection, FilterOperator, PageDraft, PropertyDraft, PropertyKind, PropertyOptionDraft,
    RecordDraft, SortDirection, Store, ViewFilter, ViewKind, ViewSettings, ViewSort,
};

fn setup() -> (TempDir, Store, String, Collection) {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("测试资料").unwrap().id;
    let (database, _) = store.create_collection(&space, None, "阅读清单").unwrap();
    let schema = store.collection(&space, &database.id).unwrap();
    (root, store, space, schema)
}

fn property(
    store: &Store,
    space: &str,
    schema: &mut Collection,
    name: &str,
    kind: PropertyKind,
    options: &[&str],
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
                options: options
                    .iter()
                    .map(|label| PropertyOptionDraft {
                        id: None,
                        name: (*label).into(),
                    })
                    .collect(),
            },
        )
        .unwrap();
    schema.properties.last().unwrap().id.clone()
}

fn record_draft(
    schema: &Collection,
    title: &str,
    properties: BTreeMap<String, Value>,
) -> RecordDraft {
    RecordDraft {
        schema_revision: schema.schema_revision,
        title: title.into(),
        properties,
    }
}

#[test]
fn property_type_preview_binds_conversion_and_retains_original_history() {
    let (root, store, space, mut schema) = setup();
    let field = property(&store, &space, &mut schema, "评分", PropertyKind::Text, &[]);
    let save = |title: &str, value: &str| {
        let record = store
            .create_page(&space, Some(&schema.container_page_id), title, "file")
            .unwrap();
        store
            .save_record(
                &space,
                &schema.id,
                &record.info.id,
                record.info.revision,
                record_draft(
                    &schema,
                    title,
                    BTreeMap::from([(field.clone(), json!(value))]),
                ),
            )
            .unwrap()
    };
    let valid = save("数字记录", "0");
    let invalid = save("待确认记录", "待确认");
    let precision = save("高精度记录", "9007199254740990.4");
    let draft = || PropertyDraft {
        id: Some(field.clone()),
        name: "评分".into(),
        kind: PropertyKind::Number,
        options: vec![],
    };
    let preview = store
        .preview_property_conversion(&space, &schema.id, schema.schema_revision, draft())
        .unwrap();
    assert_eq!((preview.converted_records, preview.cleared_records), (1, 2));
    assert_eq!(store.collection(&space, &schema.id).unwrap(), schema);
    assert_eq!(store.page(&space, &invalid.info.id).unwrap(), invalid);
    let later = save("确认前新增", "34");
    assert_eq!(
        store
            .apply_property_conversion(
                &space,
                &schema.id,
                schema.schema_revision,
                draft(),
                &preview.fingerprint
            )
            .unwrap_err()
            .code,
        "PROPERTY_CONVERSION_CHANGED"
    );
    assert_eq!(store.collection(&space, &schema.id).unwrap(), schema);

    let preview = store
        .preview_property_conversion(&space, &schema.id, schema.schema_revision, draft())
        .unwrap();
    assert_eq!((preview.converted_records, preview.cleared_records), (2, 2));
    let converted = store
        .apply_property_conversion(
            &space,
            &schema.id,
            schema.schema_revision,
            draft(),
            &preview.fingerprint,
        )
        .unwrap();
    assert_eq!(converted.schema_revision, schema.schema_revision + 1);
    assert!(
        converted
            .properties
            .iter()
            .any(|property| property.id == field && property.kind == PropertyKind::Number)
    );
    for (original, expected) in [
        (&valid, Some(json!(0))),
        (&invalid, None),
        (&precision, None),
        (&later, Some(json!(34))),
    ] {
        let current = store.page(&space, &original.info.id).unwrap();
        assert_eq!(current.info.revision, original.info.revision + 1);
        assert_eq!(current.properties.unwrap().get(&field), expected.as_ref());
        let history = store.history(&space, &original.info.id).unwrap();
        assert_eq!(history[1].properties, original.properties);
        assert_eq!(history[1].historical_properties[0].kind, PropertyKind::Text);
    }

    let archive_path = root.path().join("properties.zhiye.zip");
    let archive = std::fs::File::create(&archive_path).unwrap();
    store
        .write_workspace_archive_with_history(&space, true, archive)
        .unwrap();
    let restored = store
        .read_workspace_archive(std::fs::File::open(&archive_path).unwrap())
        .unwrap();
    let history = store.history(&restored.id, &invalid.info.id).unwrap();
    assert_eq!(history[1].properties, invalid.properties);
    assert_eq!(history[1].historical_properties[0].kind, PropertyKind::Text);
    let kept = store
        .restore_version(
            &restored.id,
            &invalid.info.id,
            history[0].info.revision,
            history[1].info.revision,
        )
        .unwrap();
    assert!(!kept.properties.unwrap().contains_key(&field));
}

#[test]
fn property_conversion_preserves_choice_history_rolls_back_on_write_failure_and_updates_templates()
{
    let (root, store, space, mut schema) = setup();
    let field = property(
        &store,
        &space,
        &mut schema,
        "进展",
        PropertyKind::Select,
        &["待处理", "结案"],
    );
    let old_option = schema.properties.last().unwrap().options[1].id.clone();
    let record = store
        .create_page(&space, Some(&schema.container_page_id), "记录样例", "file")
        .unwrap();
    let saved = store
        .save_record(
            &space,
            &schema.id,
            &record.info.id,
            record.info.revision,
            record_draft(
                &schema,
                "记录样例",
                BTreeMap::from([(field.clone(), json!(old_option))]),
            ),
        )
        .unwrap();
    let template = store
        .save_template(&space, &saved.info.id, "进展模板", None)
        .unwrap();
    let board = schema
        .views
        .iter()
        .find(|view| view.kind == ViewKind::Board)
        .unwrap();
    let board = store
        .save_view(
            &space,
            &schema.id,
            &board.id,
            board.revision,
            ViewSettings {
                filter: Some(ViewFilter {
                    field_id: field.clone(),
                    operator: FilterOperator::Equal,
                    value: Some(json!(old_option)),
                }),
                group_by: Some(field.clone()),
                sort: Some(ViewSort {
                    field_id: field.clone(),
                    direction: SortDirection::Asc,
                }),
            },
        )
        .unwrap();
    let draft = || PropertyDraft {
        id: Some(field.clone()),
        name: "进展".into(),
        kind: PropertyKind::Text,
        options: vec![],
    };
    let preview = store
        .preview_property_conversion(&space, &schema.id, schema.schema_revision, draft())
        .unwrap();
    assert_eq!(preview.converted_templates, ["进展模板"]);
    assert!(preview.cleared_templates.is_empty());
    assert_eq!(preview.view_changes, ["「看板」将清除筛选、分组。"]);
    let db = Connection::open(root.path().join(&space).join("workspace.sqlite")).unwrap();
    db.execute_batch(
        "CREATE TRIGGER reject_conversion BEFORE UPDATE OF properties ON collection_records
        BEGIN SELECT RAISE(ABORT, 'simulated record write failure'); END;",
    )
    .unwrap();
    assert!(
        store
            .apply_property_conversion(
                &space,
                &schema.id,
                schema.schema_revision,
                draft(),
                &preview.fingerprint
            )
            .is_err()
    );
    assert_eq!(store.page(&space, &record.info.id).unwrap(), saved);
    assert_eq!(store.history(&space, &record.info.id).unwrap()[0], saved);
    assert_eq!(
        store.collection(&space, &schema.id).unwrap().properties,
        schema.properties
    );
    db.execute_batch("DROP TRIGGER reject_conversion;").unwrap();

    let converted = store
        .apply_property_conversion(
            &space,
            &schema.id,
            schema.schema_revision,
            draft(),
            &preview.fingerprint,
        )
        .unwrap();
    let updated_board = converted
        .views
        .iter()
        .find(|view| view.id == board.id)
        .unwrap();
    assert_eq!(updated_board.revision, board.revision + 1);
    assert_eq!(
        updated_board.settings,
        ViewSettings {
            filter: None,
            group_by: None,
            sort: board.settings.sort
        }
    );
    let from_template = store
        .apply_template(&space, &template.id, Some(&schema.container_page_id), None)
        .unwrap();
    assert_eq!(from_template.properties.unwrap()[&field], json!("结案"));

    let mut package = Cursor::new(Vec::new());
    store
        .write_workspace_archive_with_history(&space, true, &mut package)
        .unwrap();
    package.set_position(0);
    let target = store.create_workspace("合并结果").unwrap();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    store
        .merge_workspace_package(&target.id, "types.zhiye.zip", package, &cancel)
        .unwrap();
    let copied = store
        .pages(&target.id)
        .unwrap()
        .into_iter()
        .find(|page| page.title == "记录样例")
        .unwrap();
    let history = store.history(&target.id, &copied.id).unwrap();
    let original_field = &history[1].historical_properties[0];
    assert_ne!(original_field.id, field);
    let copied_option = original_field
        .options
        .iter()
        .find(|option| option.name == "结案")
        .unwrap();
    assert_ne!(copied_option.id, old_option);
    assert_eq!(
        history[1].properties.as_ref().unwrap()[&original_field.id],
        json!(copied_option.id)
    );
    assert_eq!(
        history[0].properties.as_ref().unwrap()[&original_field.id],
        json!("结案")
    );
}

#[test]
fn record_properties_share_the_page_revision_and_keep_ids_when_schema_is_renamed() {
    let (_root, store, space, mut schema) = setup();
    let status = schema.properties[0].id.clone();
    let option = schema.properties[0].options[1].id.clone();
    let notes = property(&store, &space, &mut schema, "作者", PropertyKind::Text, &[]);
    let number = property(
        &store,
        &space,
        &mut schema,
        "评分",
        PropertyKind::Number,
        &[],
    );
    let checkbox = property(
        &store,
        &space,
        &mut schema,
        "已归档",
        PropertyKind::Checkbox,
        &[],
    );
    let date = property(
        &store,
        &space,
        &mut schema,
        "阅读日期",
        PropertyKind::Date,
        &[],
    );
    let link = property(&store, &space, &mut schema, "原文", PropertyKind::Url, &[]);
    let tags = property(
        &store,
        &space,
        &mut schema,
        "标签",
        PropertyKind::MultiSelect,
        &["研究", "工程"],
    );
    let file = property(
        &store,
        &space,
        &mut schema,
        "资料",
        PropertyKind::Attachment,
        &[],
    );
    let created = property(
        &store,
        &space,
        &mut schema,
        "创建于",
        PropertyKind::CreatedAt,
        &[],
    );
    let tag = schema
        .properties
        .iter()
        .find(|field| field.id == tags)
        .unwrap()
        .options[1]
        .id
        .clone();
    let page = store
        .create_page(&space, Some(&schema.container_page_id), "第一条", "file")
        .unwrap();
    let id = &page.info.id;
    let original = store
        .add_attachment(&space, id, "资料.txt", &b"content"[..])
        .unwrap()
        .attachment;
    let other = store.create_page(&space, None, "普通页面", "file").unwrap();
    let foreign = store
        .add_attachment(&space, &other.info.id, "其他页.txt", &b"different"[..])
        .unwrap()
        .attachment;
    let valid = BTreeMap::from([
        (status.clone(), json!(option)),
        (notes.clone(), json!("张三")),
        (number.clone(), json!(0)),
        (checkbox.clone(), json!(false)),
        (date.clone(), json!("2028-02-29")),
        (link.clone(), json!("https://example.com/资料")),
        (tags.clone(), json!([tag])),
        (file.clone(), json!([original.id])),
    ]);
    for (field, value) in [
        (&date, json!("2026-02-29")),
        (&checkbox, json!("false")),
        (&number, json!(9_007_199_254_740_993_i64)),
        (&link, json!("file:///private/path")),
        (&tags, json!([tag, tag])),
        (&file, json!([foreign.id])),
        (&created, json!(123)),
    ] {
        let mut invalid = valid.clone();
        invalid.insert(field.clone(), value);
        assert_eq!(
            store
                .save_record(
                    &space,
                    &schema.id,
                    id,
                    1,
                    record_draft(&schema, "未保存", invalid)
                )
                .unwrap_err()
                .code,
            "PROPERTY_VALUE"
        );
    }
    assert_eq!(store.page(&space, id).unwrap(), page);

    let changed = store
        .save_record(
            &space,
            &schema.id,
            id,
            1,
            record_draft(&schema, "我的记录", valid.clone()),
        )
        .unwrap();
    assert_eq!(changed.info.revision, 2);
    assert_eq!(changed.properties.as_ref(), Some(&valid));
    let record = store.record(&space, &schema.id, id).unwrap();
    assert_eq!(record.created_at, page.info.updated_at);
    assert_eq!(record.id, *id);
    assert_eq!(record.title, changed.info.title);
    assert_eq!(record.properties, valid);
    assert_eq!(
        store
            .save_page(
                &space,
                id,
                1,
                PageDraft {
                    title: "旧编辑".into(),
                    blocks: vec![]
                }
            )
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    let block: Block = serde_json::from_value(json!({
        "id": "923ae4bd-f07d-41fd-ab2b-04dff5ef017a", "type": "paragraph",
        "schemaVersion": 1, "payload": {"inline":[{"type":"text","text":"正文与属性共用页面"}]}, "children": []
    })).unwrap();
    let updated = store
        .save_page(
            &space,
            id,
            2,
            PageDraft {
                title: "正文修改".into(),
                blocks: vec![block],
            },
        )
        .unwrap();
    assert_eq!(
        store.page(&space, id).unwrap().properties.as_ref(),
        Some(&valid)
    );

    let previous = schema.properties[0].clone();
    let renamed = store
        .save_property(
            &space,
            &schema.id,
            schema.schema_revision,
            PropertyDraft {
                id: Some(status.clone()),
                name: "阅读状态".into(),
                kind: PropertyKind::Status,
                options: previous
                    .options
                    .into_iter()
                    .map(|old| PropertyOptionDraft {
                        name: if old.id == option {
                            "正在读".into()
                        } else {
                            old.name
                        },
                        id: Some(old.id),
                    })
                    .collect(),
            },
        )
        .unwrap();
    assert_eq!(renamed.properties[0].id, status);
    assert_eq!(renamed.properties[0].options[1].id, option);
    assert_eq!(
        store
            .save_record(
                &space,
                &schema.id,
                id,
                updated.revision,
                record_draft(&schema, "旧属性", valid.clone())
            )
            .unwrap_err()
            .code,
        "SCHEMA_CONFLICT"
    );
    let blank = store
        .save_record(
            &space,
            &schema.id,
            id,
            updated.revision,
            record_draft(
                &renamed,
                "清空",
                BTreeMap::from([(notes, Value::Null), (file, json!([]))]),
            ),
        )
        .unwrap();
    assert_eq!(blank.properties, Some(BTreeMap::new()));
    assert_eq!(blank.blocks.len(), 1);
    let restored = store
        .restore_version(&space, id, blank.info.revision, 2)
        .unwrap();
    assert_eq!(restored.properties, Some(valid));
    assert_eq!(restored.info.title, "我的记录");
    assert!(restored.blocks.is_empty());
    assert_eq!(store.search(&space, "我的记录").unwrap()[0].id, *id);
}

#[test]
fn persisted_filter_and_sort_page_through_one_set_of_records() {
    let (root, store, space, mut schema) = setup();
    let priority = property(
        &store,
        &space,
        &mut schema,
        "优先级",
        PropertyKind::Number,
        &[],
    );
    let table = schema
        .views
        .iter()
        .find(|view| view.kind == ViewKind::Table)
        .unwrap()
        .clone();
    let board = schema
        .views
        .iter()
        .find(|view| view.kind == ViewKind::Board)
        .unwrap();
    let list = schema
        .views
        .iter()
        .find(|view| view.kind == ViewKind::List)
        .unwrap();
    for index in 0..63 {
        let title = if index == 62 {
            "其他".into()
        } else {
            format!("读书 {index:02}")
        };
        let page = store
            .create_page(&space, Some(&schema.container_page_id), &title, "file")
            .unwrap();
        if index > 0 {
            store
                .save_record(
                    &space,
                    &schema.id,
                    &page.info.id,
                    1,
                    record_draft(
                        &schema,
                        &title,
                        BTreeMap::from([(priority.clone(), json!(index))]),
                    ),
                )
                .unwrap();
        }
    }
    let settings = ViewSettings {
        filter: Some(ViewFilter {
            field_id: "title".into(),
            operator: FilterOperator::Contains,
            value: Some(json!("读书")),
        }),
        sort: Some(ViewSort {
            field_id: priority.clone(),
            direction: SortDirection::Desc,
        }),
        group_by: Some(schema.properties[0].id.clone()),
    };
    store
        .save_view(
            &space,
            &schema.id,
            &table.id,
            table.revision,
            settings.clone(),
        )
        .unwrap();
    assert_eq!(
        store
            .save_view(
                &space,
                &schema.id,
                &table.id,
                table.revision,
                ViewSettings::default()
            )
            .unwrap_err()
            .code,
        "VIEW_CONFLICT"
    );
    drop(store);
    let store = Store::open(root.path()).unwrap();
    let first = store
        .query_records(&space, &schema.id, &table.id, 0)
        .unwrap();
    let next = store
        .query_records(&space, &schema.id, &table.id, first.page_size)
        .unwrap();
    assert_eq!(
        (
            first.total,
            first.records.len(),
            next.offset,
            next.records.len()
        ),
        (62, 50, 50, 12)
    );
    assert_eq!(first.records[0].properties[&priority], json!(61));
    assert_eq!(next.records.last().unwrap().title, "读书 00");
    assert_eq!(
        first
            .records
            .iter()
            .chain(&next.records)
            .map(|item| &item.id)
            .collect::<HashSet<_>>()
            .len(),
        62
    );
    assert_eq!(
        store
            .query_records(&space, &schema.id, &board.id, 0)
            .unwrap()
            .total,
        63
    );
    assert_eq!(
        store
            .query_records(&space, &schema.id, &list.id, 95_000)
            .unwrap()
            .offset,
        50
    );
    assert_eq!(
        store.collection(&space, &schema.id).unwrap().views[0].settings,
        settings
    );
}

#[test]
fn record_membership_survives_trash_and_rejects_cross_database_moves() {
    let (_root, store, space, schema) = setup();
    let page = store
        .create_page(&space, Some(&schema.container_page_id), "阅读笔记", "file")
        .unwrap();
    let normal = store.create_page(&space, None, "目录", "file").unwrap();
    let (other, _) = store.create_collection(&space, None, "其他库").unwrap();
    assert_eq!(
        store
            .move_page(&space, &page.info.id, 1, None)
            .unwrap_err()
            .code,
        "COLLECTION_MOVE"
    );
    assert_eq!(
        store
            .move_page(&space, &normal.info.id, 1, Some(&schema.container_page_id))
            .unwrap_err()
            .code,
        "COLLECTION_MOVE"
    );
    assert_eq!(
        store
            .record(&space, &other.id, &page.info.id)
            .unwrap_err()
            .code,
        "RECORD_NOT_FOUND"
    );
    assert_eq!(
        store
            .save_record(
                &space,
                &other.id,
                &page.info.id,
                1,
                record_draft(&schema, "不应保存", BTreeMap::new())
            )
            .unwrap_err()
            .code,
        "RECORD_NOT_FOUND"
    );
    store.archive_page(&space, &page.info.id, 1).unwrap();
    store
        .archive_page(&space, &schema.container_page_id, 1)
        .unwrap();
    assert_eq!(
        store
            .restore_page(&space, &page.info.id, 2)
            .unwrap_err()
            .code,
        "COLLECTION_TRASHED"
    );
    store
        .restore_page(&space, &schema.container_page_id, 2)
        .unwrap();
    store.restore_page(&space, &page.info.id, 2).unwrap();
    assert_eq!(
        store
            .page(&space, &page.info.id)
            .unwrap()
            .info
            .parent_id
            .as_deref(),
        Some(schema.container_page_id.as_str())
    );
    assert_eq!(
        store
            .query_records(&space, &schema.id, &schema.views[0].id, 0)
            .unwrap()
            .total,
        1
    );
}

#[test]
fn native_archive_restores_the_schema_views_record_history_and_attachments() {
    let (_root, store, space, mut schema) = setup();
    let page = store
        .create_page(&space, Some(&schema.container_page_id), "参考资料", "file")
        .unwrap();
    let attachment_property = property(
        &store,
        &space,
        &mut schema,
        "文档",
        PropertyKind::Attachment,
        &[],
    );
    let bytes = b"original article";
    let attachment = store
        .add_attachment(&space, &page.info.id, "文档.txt", &bytes[..])
        .unwrap()
        .attachment;
    let saved = store
        .save_record(
            &space,
            &schema.id,
            &page.info.id,
            1,
            record_draft(
                &schema,
                "新标题",
                BTreeMap::from([(attachment_property, json!([attachment.id]))]),
            ),
        )
        .unwrap();
    let view = &schema.views[0];
    schema.views[0] = store
        .save_view(
            &space,
            &schema.id,
            &view.id,
            view.revision,
            ViewSettings {
                sort: Some(ViewSort {
                    field_id: "title".into(),
                    direction: SortDirection::Asc,
                }),
                ..ViewSettings::default()
            },
        )
        .unwrap();
    assert_eq!(
        store.export_workspace(&space).unwrap_err().code,
        "ARCHIVE_REQUIRED"
    );
    let mut buffer = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut buffer).unwrap();
    let restored = store
        .read_workspace_archive(Cursor::new(buffer.into_inner()))
        .unwrap();
    assert_ne!(restored.id, space);
    assert_eq!(store.collection(&restored.id, &schema.id).unwrap(), schema);
    assert_eq!(store.page(&restored.id, &page.info.id).unwrap(), saved);
    assert_eq!(
        store.history(&restored.id, &page.info.id).unwrap(),
        vec![saved, page.clone()]
    );
    let mut original = Vec::new();
    store
        .copy_attachment(&restored.id, &page.info.id, &attachment.id, &mut original)
        .unwrap();
    assert_eq!(original, bytes);
}

#[test]
fn v2_sqlite_and_zip_open_with_the_original_page_and_a_pre_migration_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("旧工作空间").unwrap();
    let page = store
        .create_page(&workspace.id, None, "旧笔记", "file")
        .unwrap();
    let mut buffer = Cursor::new(Vec::new());
    store
        .write_workspace_archive(&workspace.id, &mut buffer)
        .unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(buffer.into_inner())).unwrap();
    let mut bytes = Vec::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let mut manifest: Value = serde_json::from_slice(&bytes).unwrap();
    manifest["formatVersion"] = json!(2);
    manifest["data"]
        .as_object_mut()
        .unwrap()
        .remove("databases");
    manifest["data"].as_object_mut().unwrap().remove("library");
    manifest["data"].sort_all_objects();
    let digest = Sha256::digest(serde_json::to_vec(&manifest["data"]).unwrap());
    manifest["sha256"] = json!(
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    );
    let v2 = {
        let mut output = zip::ZipWriter::new(Cursor::new(Vec::new()));
        output
            .start_file(
                "manifest.json",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored),
            )
            .unwrap();
        output
            .write_all(&serde_json::to_vec(&manifest).unwrap())
            .unwrap();
        output.finish().unwrap()
    };
    assert_eq!(
        store
            .page(&store.read_workspace_archive(v2).unwrap().id, &page.info.id)
            .unwrap(),
        page
    );
    drop(store);

    let connection =
        Connection::open(root.path().join(&workspace.id).join("workspace.sqlite")).unwrap();
    support::remove_v7_schema(&connection);
    connection
        .execute_batch(
            "DROP TABLE integration_operations; DROP TABLE integration_revisions;
             DROP TABLE integration_grants; DROP TABLE integrations;
             DROP TRIGGER search_page_insert; DROP TRIGGER search_page_revision;
             DROP TABLE search_pending; DROP TABLE search_entries; DROP TABLE search_fts;
             DROP TABLE templates; DROP TABLE import_receipts; DROP TABLE favorites;
             DROP TABLE recent_pages; DROP TABLE workspace_inbox;
             DROP TABLE collection_records; DROP TABLE collections; PRAGMA user_version = 2;",
        )
        .unwrap();
    drop(connection);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(store.page(&workspace.id, &page.info.id).unwrap(), page);
    let backup = root
        .path()
        .join("upgrade-backups")
        .join(format!("{}-v2.sqlite", workspace.id));
    let version: u32 = Connection::open(backup)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version, 2);
    assert!(store.collections(&workspace.id).unwrap().is_empty());
}
