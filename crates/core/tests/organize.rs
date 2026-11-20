// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use std::{collections::BTreeMap, io::Cursor};
use zhiye_core::{
    Collection, CsvDestination, FilterOperator, ImportFile, ImportStatus, PageDraft, PageInfo,
    PageVersion, PropertyDraft, PropertyKind, PropertyOptionDraft, RecordDraft, SortDirection,
    Store, ViewFilter, ViewKind, ViewSettings, ViewSort,
};

fn version(page: &PageInfo) -> PageVersion {
    PageVersion {
        id: page.id.clone(),
        revision: page.revision,
    }
}

fn property(
    store: &Store,
    workspace: &str,
    schema: &mut Collection,
    name: &str,
    kind: PropertyKind,
    options: &[&str],
) -> String {
    *schema = store
        .save_property(
            workspace,
            &schema.id,
            schema.schema_revision,
            PropertyDraft {
                id: None,
                name: name.into(),
                kind,
                options: options
                    .iter()
                    .map(|name| PropertyOptionDraft {
                        id: None,
                        name: (*name).into(),
                    })
                    .collect(),
            },
        )
        .unwrap();
    schema.properties.last().unwrap().id.clone()
}

#[test]
fn batch_move_subsumes_descendants_and_rolls_back_invalid_or_stale_requests() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("页面批处理").unwrap().id;
    let old = store
        .create_page(&workspace, None, "原目录", "folder")
        .unwrap()
        .info;
    let destination = store
        .create_page(&workspace, None, "目标目录", "folder")
        .unwrap()
        .info;
    let first = store
        .create_page(&workspace, Some(&old.id), "第一页", "file")
        .unwrap()
        .info;
    let child = store
        .create_page(&workspace, Some(&first.id), "子页", "file")
        .unwrap()
        .info;
    let second = store
        .create_page(&workspace, Some(&old.id), "第二页", "file")
        .unwrap()
        .info;
    let (collection, db_page) = store
        .create_collection(&workspace, Some(&old.id), "数据库")
        .unwrap();
    let record = store
        .create_page(&workspace, Some(&db_page.info.id), "记录", "file")
        .unwrap();
    let err = store
        .move_pages(
            &workspace,
            &[version(&first), version(&second)],
            Some(&child.id),
        )
        .unwrap_err();
    assert_eq!(err.code, "PAGE_CYCLE");
    let err = store
        .move_pages(
            &workspace,
            &[version(&first), version(&record.info)],
            Some(&destination.id),
        )
        .unwrap_err();
    assert_eq!(err.code, "COLLECTION_MOVE");
    assert_eq!(store.page(&workspace, &first.id).unwrap().info, first);
    assert_eq!(store.history(&workspace, &first.id).unwrap().len(), 1);

    let saved = store
        .save_page(
            &workspace,
            &child.id,
            child.revision,
            PageDraft {
                title: "子页已更新".into(),
                blocks: Vec::new(),
            },
        )
        .unwrap();
    let err = store
        .move_pages(
            &workspace,
            &[version(&first), version(&child)],
            Some(&destination.id),
        )
        .unwrap_err();
    assert_eq!(err.code, "REVISION_CONFLICT");
    let moved = store
        .move_pages(
            &workspace,
            &[
                version(&saved),
                version(&first),
                version(&first),
                version(&second),
                version(&db_page.info),
                version(&record.info),
            ],
            Some(&destination.id),
        )
        .unwrap();
    assert_eq!(moved.len(), 3);
    for page in moved {
        assert_eq!(page.parent_id.as_deref(), Some(destination.id.as_str()));
        assert_eq!(page.revision, 2);
    }
    assert_eq!(store.page(&workspace, &child.id).unwrap().info, saved);
    assert_eq!(
        store
            .record(&workspace, &collection.id, &record.info.id)
            .unwrap()
            .id,
        record.info.id
    );
}

#[test]
fn batch_archive_is_atomic_and_bulk_restore_keeps_independent_archive_roots_linked() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("回收恢复").unwrap().id;
    let (collection, database) = store.create_collection(&workspace, None, "任务").unwrap();
    let record = store
        .create_page(&workspace, Some(&database.info.id), "曾独立归档", "file")
        .unwrap()
        .info;
    let other = store
        .create_page(&workspace, Some(&database.info.id), "同批归档", "file")
        .unwrap()
        .info;
    let spare = store
        .create_page(&workspace, None, "另一分支", "file")
        .unwrap()
        .info;
    store.set_favorite(&workspace, &record.id, true).unwrap();
    store
        .archive_page(&workspace, &record.id, record.revision)
        .unwrap();
    let record = store.page(&workspace, &record.id).unwrap().info;

    let sqlite = root.path().join(&workspace).join("workspace.sqlite");
    let db = rusqlite::Connection::open(sqlite).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_archive BEFORE UPDATE OF trash_root_id ON pages
         WHEN OLD.title = '另一分支' AND NEW.trash_root_id IS NOT NULL
         BEGIN SELECT RAISE(ABORT, 'injected archive failure'); END;",
    )
    .unwrap();
    let selection = || [version(&other), version(&database.info), version(&spare)];
    assert_eq!(
        store
            .archive_pages(&workspace, &selection())
            .unwrap_err()
            .code,
        "DATABASE"
    );
    assert_eq!(
        store.page(&workspace, &database.info.id).unwrap().info,
        database.info
    );
    assert_eq!(store.history(&workspace, &other.id).unwrap().len(), 1);
    db.execute_batch("DROP TRIGGER fail_archive").unwrap();
    assert_eq!(store.archive_pages(&workspace, &selection()).unwrap(), 3);
    assert!(store.record(&workspace, &collection.id, &other.id).is_err());
    assert!(store.navigation(&workspace).unwrap().favorites.is_empty());
    let database = store.page(&workspace, &database.info.id).unwrap().info;
    let spare = store.page(&workspace, &spare.id).unwrap().info;
    assert_eq!(store.page(&workspace, &record.id).unwrap().info, record);
    assert_eq!(
        store
            .restore_pages(&workspace, &[version(&spare), version(&record)])
            .unwrap_err()
            .code,
        "COLLECTION_TRASHED"
    );
    assert_eq!(store.page(&workspace, &spare.id).unwrap().info, spare);
    let mut zip = Cursor::new(Vec::new());
    store.write_workspace_archive(&workspace, &mut zip).unwrap();
    let restored = store.read_workspace_archive(zip).unwrap().id;
    assert_eq!(
        store
            .restore_pages(
                &restored,
                &[version(&record), version(&database), version(&spare)]
            )
            .unwrap(),
        4
    );
    let reopened_record = store.page(&restored, &record.id).unwrap();
    assert_eq!(
        reopened_record.info.parent_id.as_deref(),
        Some(database.id.as_str())
    );
    assert!(reopened_record.info.trash_root_id.is_none());
    assert!(reopened_record.properties.is_some());
    assert_eq!(store.navigation(&restored).unwrap().favorites, [record.id]);
    assert!(
        store
            .page(&restored, &other.id)
            .unwrap()
            .info
            .trash_root_id
            .is_none()
    );
    assert!(
        store
            .page(&workspace, &other.id)
            .unwrap()
            .info
            .trash_root_id
            .is_some()
    );
}

#[test]
fn csv_exports_all_filtered_rows_and_reimports_typed_values() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let workspace = store.create_workspace("CSV 数据交换").unwrap().id;
    let (database, _) = store.create_collection(&workspace, None, "资料").unwrap();
    let mut schema = store.collection(&workspace, &database.id).unwrap();
    let score = property(
        &store,
        &workspace,
        &mut schema,
        "评分",
        PropertyKind::Number,
        &[],
    );
    let checked = property(
        &store,
        &workspace,
        &mut schema,
        "Checked",
        PropertyKind::Checkbox,
        &[],
    );
    let note = property(
        &store,
        &workspace,
        &mut schema,
        "Title",
        PropertyKind::Text,
        &[],
    );
    let labels = property(
        &store,
        &workspace,
        &mut schema,
        "标签",
        PropertyKind::MultiSelect,
        &["甲;乙", "[\"甲;乙\"]"],
    );
    let created = property(
        &store,
        &workspace,
        &mut schema,
        "创建",
        PropertyKind::CreatedAt,
        &[],
    );
    let asset = property(
        &store,
        &workspace,
        &mut schema,
        "原件",
        PropertyKind::Attachment,
        &[],
    );
    let extra_asset = property(
        &store,
        &workspace,
        &mut schema,
        "副引用",
        PropertyKind::Attachment,
        &[],
    );
    let label_id = schema
        .properties
        .iter()
        .find(|item| item.id == labels)
        .unwrap()
        .options[0]
        .id
        .clone();
    let note_text = "逗号, \"引号\"\r\n下一行";
    for n in 0..55 {
        let title = format!("第{n:02}条");
        let record = store
            .create_page(&workspace, Some(&schema.container_page_id), &title, "file")
            .unwrap();
        let mut fields = BTreeMap::from([
            (score.clone(), json!(n)),
            (checked.clone(), json!(n % 54 == 53)),
            (note.clone(), json!(note_text)),
            (labels.clone(), json!([label_id])),
        ]);
        if n == 0 {
            let image = store
                .add_attachment(&workspace, &record.info.id, "样例.txt", "原件".as_bytes())
                .unwrap()
                .attachment;
            fields.insert(asset.clone(), json!([image.id]));
            fields.insert(extra_asset.clone(), json!([image.id]));
        }
        store
            .save_record(
                &workspace,
                &database.id,
                &record.info.id,
                1,
                RecordDraft {
                    title,
                    schema_revision: schema.schema_revision,
                    properties: fields,
                },
            )
            .unwrap();
    }
    let view = schema
        .views
        .iter()
        .find(|item| item.kind == ViewKind::Table)
        .unwrap();
    store
        .save_view(
            &workspace,
            &database.id,
            &view.id,
            view.revision,
            ViewSettings {
                filter: Some(ViewFilter {
                    field_id: checked.clone(),
                    operator: FilterOperator::Equal,
                    value: Some(json!(false)),
                }),
                sort: Some(ViewSort {
                    field_id: score.clone(),
                    direction: SortDirection::Asc,
                }),
                group_by: Some(checked.clone()),
            },
        )
        .unwrap();
    let query = store
        .query_records(&workspace, &database.id, &view.id, 0)
        .unwrap();
    assert_eq!(query.records.len(), 50);
    assert_eq!(query.total, 54);
    let mut csv = Vec::new();
    let mut copied = 0;
    let report = store
        .write_collection_csv(
            &workspace,
            &database.id,
            Some(&view.id),
            &mut csv,
            |asset| {
                copied += 1;
                let mut bytes = Vec::new();
                store.copy_attachment(&workspace, &asset.page_id, &asset.id, &mut bytes)?;
                assert_eq!(bytes, "原件".as_bytes());
                Ok("files/sample.txt".into())
            },
        )
        .unwrap();
    assert_eq!(report.rows, 54);
    assert_eq!(copied, 1);
    assert_eq!(report.view, Some(ViewKind::Table));
    assert!(
        report
            .limitations
            .iter()
            .any(|note| note.contains("系统时间"))
    );
    assert!(
        report
            .limitations
            .iter()
            .any(|note| note.contains("视图分组"))
    );
    assert!(csv.starts_with(b"\xef\xbb\xbf"));
    let mut reader = csv::Reader::from_reader(csv.as_slice());
    let headers = reader.headers().unwrap().clone();
    let rows = reader
        .records()
        .collect::<std::result::Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(rows.len(), 54);
    let index = |name: &str| headers.iter().position(|field| field == name).unwrap();
    assert_eq!(&rows[0][index("标题")], "第00条");
    assert_eq!(&rows[0][index("Title")], note_text);
    assert_eq!(&rows[0][index("Checked")], "false");
    assert_eq!(&rows[0][index("评分")], "0");
    assert_eq!(&rows[0][index("原件")], r#"["files/sample.txt"]"#);
    assert_eq!(&rows[0][index("标签")], r#"["甲;乙"]"#);
    assert!(rows[0][index("创建")].parse::<i64>().unwrap() > 0);
    assert_eq!(&rows[53][index("标题")], "第54条");
    assert_eq!(
        store
            .write_collection_csv(&workspace, &database.id, None, Vec::new(), |_| Ok(
                "file".into()
            ))
            .unwrap()
            .rows,
        55
    );

    let copied = store
        .import_csv(
            &workspace,
            CsvDestination::Existing {
                collection_id: database.id.clone(),
            },
            ImportFile {
                name: "导回.csv".into(),
                bytes: csv,
            },
        )
        .unwrap();
    assert_eq!(copied.status, ImportStatus::Imported);
    assert_eq!(copied.records, 54);
    let all = store
        .query_records(&workspace, &database.id, &view.id, 0)
        .unwrap();
    let zeros = all
        .records
        .iter()
        .filter(|row| row.properties.get(&score) == Some(&json!(0)))
        .collect::<Vec<_>>();
    assert_eq!(zeros.len(), 2);
    for row in zeros {
        assert_eq!(row.properties.get(&note), Some(&json!(note_text)));
        assert_eq!(row.properties.get(&labels), Some(&json!([label_id])));
        assert_eq!(row.properties.get(&checked), Some(&Value::Bool(false)));
        assert!(row.properties.get(&created).is_none());
    }
}
