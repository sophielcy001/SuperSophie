// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::{Connection, params};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};
use zhiye_core::{
    Block, Calculation, Collection, ComputedDraft, ComputedValue, FilterOperator, ImportFile,
    ImportStatus, IntegrationDraft, IntegrationGrant, IntegrationScope, Page, PageDraft,
    PropertyDefinition, PropertyDraft, PropertyKind, PropertyOptionDraft, RecordDraft,
    RelationDraft, RollupFunction, SortDirection, Store, ViewDraft, ViewFilter, ViewKind,
    ViewLayout, ViewSettings, ViewSort,
};

fn add_field(
    store: &Store,
    space: &str,
    collection: &str,
    name: &str,
    kind: PropertyKind,
    option: Option<&str>,
) -> PropertyDefinition {
    let schema = store.collection(space, collection).unwrap();
    store
        .save_property(
            space,
            collection,
            schema.schema_revision,
            PropertyDraft {
                id: None,
                name: name.into(),
                kind,
                options: option
                    .into_iter()
                    .map(|name| PropertyOptionDraft {
                        id: None,
                        name: name.into(),
                    })
                    .collect(),
            },
        )
        .unwrap()
        .properties
        .pop()
        .unwrap()
}

fn field<'a>(collection: &'a Collection, label: &str) -> &'a PropertyDefinition {
    collection
        .properties
        .iter()
        .find(|property| property.name == label)
        .unwrap()
}

fn block(kind: &str, payload: Value) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children: Vec::new(),
        extra: BTreeMap::new(),
    }
}

fn props(
    store: &Store,
    space: &str,
    collection: &str,
    page: &Page,
    properties: BTreeMap<String, Value>,
) -> Page {
    let schema = store.collection(space, collection).unwrap();
    store
        .save_record(
            space,
            collection,
            &page.info.id,
            page.info.revision,
            RecordDraft {
                title: page.info.title.clone(),
                schema_revision: schema.schema_revision,
                properties,
            },
        )
        .unwrap()
}

fn add_computed(
    store: &Store,
    space: &str,
    collection: &str,
    name: &str,
    calculation: Calculation,
) {
    let schema = store.collection(space, collection).unwrap();
    store
        .save_computed_property(
            space,
            collection,
            schema.schema_revision,
            ComputedDraft {
                id: None,
                name: name.into(),
                calculation,
            },
        )
        .unwrap();
}

#[test]
fn merge_remaps_a_full_package_into_an_existing_copy_without_rebinding_old_ids() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let space = store.create_workspace("源资料").unwrap().id;
    let (jobs, job_page) = store.create_collection(&space, None, "任务").unwrap();
    let (projects, _) = store.create_collection(&space, None, "项目").unwrap();
    let state = add_field(
        &store,
        &space,
        &jobs.id,
        "进度",
        PropertyKind::Status,
        Some("开始"),
    );
    let cost = add_field(
        &store,
        &space,
        &projects.id,
        "工时",
        PropertyKind::Number,
        None,
    );
    let asset = add_field(
        &store,
        &space,
        &jobs.id,
        "原件",
        PropertyKind::Attachment,
        None,
    );
    let start = add_field(
        &store,
        &space,
        &jobs.id,
        "开始日期",
        PropertyKind::Date,
        None,
    );
    let end = add_field(
        &store,
        &space,
        &jobs.id,
        "结束日期",
        PropertyKind::Date,
        None,
    );
    let schema = store.collection(&space, &jobs.id).unwrap();
    let with_relation = store
        .create_relation(
            &space,
            &jobs.id,
            schema.schema_revision,
            RelationDraft {
                name: "所属项目".into(),
                collection_id: projects.id.clone(),
                inverse_name: Some("任务回链".into()),
            },
        )
        .unwrap();
    let link = field(&with_relation, "所属项目");
    add_computed(
        &store,
        &space,
        &projects.id,
        "复核工时",
        Calculation::Formula {
            expression: "prop(\"工时\") * 2".into(),
        },
    );
    add_computed(
        &store,
        &space,
        &jobs.id,
        "项目工时",
        Calculation::Rollup {
            relation_property_id: link.id.clone(),
            target_property_id: Some(cost.id.clone()),
            function: RollupFunction::Sum,
        },
    );
    let schema = store.collection(&space, &jobs.id).unwrap();
    let table = schema
        .views
        .iter()
        .find(|view| view.kind == ViewKind::Table)
        .unwrap();
    let saved_view = store
        .save_view(
            &space,
            &jobs.id,
            &table.id,
            table.revision,
            ViewSettings {
                filter: Some(ViewFilter {
                    field_id: state.id.clone(),
                    operator: FilterOperator::Equal,
                    value: Some(json!(state.options[0].id)),
                }),
                sort: Some(ViewSort {
                    field_id: "title".into(),
                    direction: SortDirection::Asc,
                }),
                group_by: Some(state.id.clone()),
            },
        )
        .unwrap();
    let scheduled = store
        .upsert_view(
            &space,
            &jobs.id,
            ViewDraft {
                id: None,
                revision: None,
                kind: ViewKind::Timeline,
                name: "排期副本".into(),
                settings: ViewSettings::default(),
                layout: Some(ViewLayout::Timeline {
                    start_field: start.id.clone(),
                    end_field: end.id.clone(),
                    zoom: Default::default(),
                }),
            },
        )
        .unwrap();
    let schedule = scheduled
        .views
        .iter()
        .find(|view| view.name.as_deref() == Some("排期副本"))
        .unwrap();
    let task = store
        .create_page(&space, Some(&job_page.info.id), "样本任务", "file")
        .unwrap();
    let project = store
        .create_page(
            &space,
            Some(&projects.container_page_id),
            "样本项目",
            "file",
        )
        .unwrap();
    let project = props(
        &store,
        &space,
        &projects.id,
        &project,
        BTreeMap::from([(cost.id.clone(), json!(7))]),
    );
    let other_projects: Vec<_> = (1..=5)
        .map(|number| {
            store
                .create_page(
                    &space,
                    Some(&projects.container_page_id),
                    &format!("空工时项目 {number}"),
                    "file",
                )
                .unwrap()
        })
        .collect();
    let project_ids: Vec<_> = std::iter::once(&project)
        .chain(&other_projects)
        .map(|page| &page.info.id)
        .collect();
    let original = b"portable source bytes";
    let file = store
        .add_attachment(&space, &task.info.id, "结果.txt", original.as_slice())
        .unwrap()
        .attachment;
    let task = props(
        &store,
        &space,
        &jobs.id,
        &task,
        BTreeMap::from([
            (state.id.clone(), json!(state.options[0].id)),
            (asset.id.clone(), json!([file.id])),
            (link.id.clone(), json!({"add":project_ids})),
        ]),
    );
    let memo = store.create_page(&space, None, "项目笔记", "book").unwrap();
    let anchor = block(
        "paragraph",
        json!({"inline":[{"type":"text","text":"源段落"}]}),
    );
    let historical = store.save_page(&space, &memo.info.id, 1, PageDraft {
        title: memo.info.title.clone(), blocks: vec![anchor.clone(),
            block("blockReference", json!({"pageId":memo.info.id,"blockId":anchor.id})),
            block("paragraph", json!({"inline":[{"type":"pageReference","pageId":project.info.id,"label":"项目"}]})),
        ],
    }).unwrap();
    let png = b"\x89PNG\r\n\x1a\nfixture";
    let image = store
        .add_attachment(&space, &memo.info.id, "封面.png", png.as_slice())
        .unwrap()
        .attachment;
    let styled = store
        .set_page_appearance(
            &space,
            &memo.info.id,
            historical.revision,
            "star",
            Some(&image.id),
        )
        .unwrap();
    let archived = store
        .create_page(&space, None, "回收站样本", "file")
        .unwrap();
    store
        .archive_page(&space, &archived.info.id, archived.info.revision)
        .unwrap();
    let missing = uuid::Uuid::new_v4().to_string();
    let mut opaque = block(
        "futureWidget",
        json!({"wide":9_007_199_254_740_993_u64,"literal":task.info.id}),
    );
    opaque.schema_version = 3;
    let task_info = store
        .save_page(
            &space,
            &task.info.id,
            task.info.revision,
            PageDraft {
                title: task.info.title.clone(),
                blocks: vec![
                    block(
                        "blockReference",
                        json!({"pageId":memo.info.id,"blockId":anchor.id}),
                    ),
                    block(
                        "paragraph",
                        json!({"inline":[
                            {"type":"pageReference","pageId":project.info.id,"label":"项目"},
                            {"type":"pageReference","pageId":missing,"label":"失配引用"},
                        ]}),
                    ),
                    block(
                        "collectionView",
                        json!({"collectionId":jobs.id,"viewId":saved_view.id}),
                    ),
                    block("attachmentFile", json!({"attachmentId":file.id})),
                    opaque.clone(),
                ],
            },
        )
        .unwrap();
    store.set_favorite(&space, &task.info.id, true).unwrap();
    store.set_inbox(&space, &memo.info.id).unwrap();
    store.visit_page(&space, &memo.info.id).unwrap();
    store
        .save_template(&space, &memo.info.id, "说明模板", None)
        .unwrap();
    let receipt = || ImportFile {
        name: "旧稿.md".into(),
        bytes: b"# Markdown note\n".to_vec(),
    };
    assert_eq!(
        store
            .import_markdown(&space, None, vec![receipt()])
            .unwrap()[0]
            .status,
        ImportStatus::Imported
    );
    let mut output = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut output).unwrap();
    let archive = output.into_inner();
    let target = store
        .read_workspace_archive(Cursor::new(&archive))
        .unwrap()
        .id;
    let existing = store.page(&target, &task.info.id).unwrap();
    let outsider = store
        .issue_integration(
            &target,
            IntegrationDraft {
                name: "旧分支读取".into(),
                scopes: BTreeSet::from([IntegrationScope::Read]),
                grants: vec![IntegrationGrant::Page(task.info.id.clone())],
            },
        )
        .unwrap();
    let merged = store
        .merge_workspace_package(
            &target,
            "样本.zhiye.zip",
            Cursor::new(&archive),
            &AtomicBool::new(false),
        )
        .unwrap();
    let folder = &merged.page_id;
    assert_eq!(merged.collections, 2);
    assert_eq!(merged.attachments, 4);
    assert!(
        merged
            .notices
            .iter()
            .any(|notice| notice.contains("引用目标缺失"))
    );
    assert!(
        merged
            .notices
            .iter()
            .any(|notice| notice.contains("未知块"))
    );
    assert!(
        merged
            .notices
            .iter()
            .any(|notice| notice.contains("重名模板"))
    );
    let pages = store.pages(&target).unwrap();
    let imported = |title: &str, parent: &str| {
        pages
            .iter()
            .find(|page| page.title == title && page.parent_id.as_deref() == Some(parent))
            .unwrap()
    };
    let copied_jobs = imported("任务", folder);
    let copied_projects = imported("项目", folder);
    let copied_task = imported("样本任务", &copied_jobs.id);
    let copied_project = imported("样本项目", &copied_projects.id);
    let copied_memo = imported("项目笔记", folder);
    let copied_trash = imported("回收站样本", folder);
    assert_eq!(
        copied_trash.trash_root_id.as_deref(),
        Some(copied_trash.id.as_str())
    );
    assert_ne!(copied_task.id, existing.info.id);
    assert_eq!(store.page(&target, &task.info.id).unwrap(), existing);
    assert_eq!(
        store.navigation(&target).unwrap().inbox_page_id,
        Some(memo.info.id.clone())
    );
    assert!(
        store
            .navigation(&target)
            .unwrap()
            .favorites
            .contains(&copied_task.id)
    );
    assert!(
        store
            .integration_page(&target, &outsider.token, &copied_task.id)
            .is_err()
    );
    let collection = |page_id: &str| {
        let id = store
            .collections(&target)
            .unwrap()
            .into_iter()
            .find(|c| c.container_page_id == page_id)
            .unwrap()
            .id;
        store.collection(&target, &id).unwrap()
    };
    let new_jobs = collection(&copied_jobs.id);
    let new_projects = collection(&copied_projects.id);
    let new_state = field(&new_jobs, "进度");
    let new_link = field(&new_jobs, "所属项目");
    let new_inverse = field(&new_projects, "任务回链");
    assert_ne!(new_jobs.id, jobs.id);
    assert_ne!(new_state.id, state.id);
    assert_ne!(new_state.options[0].id, state.options[0].id);
    assert_eq!(
        new_link
            .relation
            .as_ref()
            .unwrap()
            .inverse_property_id
            .as_ref(),
        Some(&new_inverse.id)
    );
    let imported_task = store.page(&target, &copied_task.id).unwrap();
    assert_eq!(imported_task.info.revision, task_info.revision);
    let new_values = imported_task.properties.as_ref().unwrap();
    let copied_ids: BTreeSet<_> = std::iter::once(&copied_project.id)
        .chain(
            other_projects
                .iter()
                .map(|project| &imported(&project.info.title, &copied_projects.id).id),
        )
        .collect();
    assert_eq!(new_values[&new_link.id], json!(copied_ids));
    assert_eq!(new_values[&new_state.id], json!(new_state.options[0].id));
    let new_project = store
        .record(&target, &new_projects.id, &copied_project.id)
        .unwrap();
    assert_eq!(
        new_project.properties[&new_inverse.id],
        json!([copied_task.id])
    );
    assert_eq!(
        new_project.computed[&field(&new_projects, "复核工时").id],
        ComputedValue::Value { value: json!(14.0) }
    );
    let calculated = store
        .record(&target, &new_jobs.id, &copied_task.id)
        .unwrap();
    assert_eq!(
        calculated.computed[&field(&new_jobs, "项目工时").id],
        ComputedValue::Value { value: json!(7.0) }
    );
    let new_view = new_jobs
        .views
        .iter()
        .find(|view| view.kind == ViewKind::Table)
        .unwrap();
    assert_ne!(new_view.id, saved_view.id);
    assert_eq!(new_view.settings.group_by.as_ref(), Some(&new_state.id));
    assert_eq!(
        store
            .query_records(&target, &new_jobs.id, &new_view.id, 0)
            .unwrap()
            .records[0]
            .id,
        copied_task.id
    );
    let new_schedule = new_jobs
        .views
        .iter()
        .find(|view| view.name.as_deref() == schedule.name.as_deref())
        .unwrap();
    assert_ne!(new_schedule.id, schedule.id);
    assert_eq!(
        new_schedule.layout,
        Some(ViewLayout::Timeline {
            start_field: field(&new_jobs, "开始日期").id.clone(),
            end_field: field(&new_jobs, "结束日期").id.clone(),
            zoom: Default::default(),
        })
    );
    let copied_asset = store
        .attachments(&target, &copied_task.id)
        .unwrap()
        .remove(0);
    assert_ne!(copied_asset.id, file.id);
    let mut bytes = Vec::new();
    store
        .copy_attachment(&target, &copied_task.id, &copied_asset.id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, original);
    assert_eq!(
        new_values[&field(&new_jobs, "原件").id],
        json!([copied_asset.id])
    );
    let blocks = &imported_task.blocks;
    let new_anchor = store
        .page(&target, &copied_memo.id)
        .unwrap()
        .blocks
        .remove(0);
    assert_ne!(new_anchor.id, anchor.id);
    assert_eq!(
        blocks[0].payload,
        json!({"pageId":copied_memo.id,"blockId":new_anchor.id})
    );
    assert_eq!(blocks[1].payload["inline"][0]["pageId"], copied_project.id);
    let stale = blocks[1].payload["inline"][1]["pageId"].as_str().unwrap();
    assert_ne!(stale, missing);
    assert!(store.page(&target, stale).is_err());
    assert_eq!(
        blocks[2].payload,
        json!({"collectionId":new_jobs.id,"viewId":new_view.id})
    );
    assert_eq!(blocks[3].payload["attachmentId"], copied_asset.id);
    assert_ne!(blocks[4].id, opaque.id);
    assert_eq!(blocks[4].payload, opaque.payload);
    let history = store.history(&target, &copied_memo.id).unwrap();
    assert_eq!(history[0].info.revision, styled.info.revision);
    assert_eq!(history[0].blocks[0].id, new_anchor.id);
    assert_eq!(history[1].info.revision, historical.revision);
    assert!(history[1].cover.is_none());
    let copied_cover = &history[0].cover.as_ref().unwrap().attachment_id;
    assert_ne!(copied_cover, &image.id);
    let templates = store.templates(&target).unwrap();
    assert_eq!(templates.len(), 2);
    let template = templates
        .iter()
        .find(|template| template.name == "说明模板（2）")
        .unwrap();
    let applied = store
        .apply_template(&target, &template.id, Some(folder), Some("模板新建"))
        .unwrap();
    assert_eq!(
        applied.blocks[1].payload,
        json!({"pageId":applied.info.id,"blockId":applied.blocks[0].id})
    );
    assert_eq!(
        applied.blocks[2].payload["inline"][0]["pageId"],
        copied_project.id
    );
    let applied_cover = &applied.cover.unwrap().attachment_id;
    assert_ne!(applied_cover, copied_cover);
    let mut image_bytes = Vec::new();
    store
        .copy_attachment(&target, &applied.info.id, applied_cover, &mut image_bytes)
        .unwrap();
    assert_eq!(image_bytes, png);
    let duplicate = store
        .import_markdown(&target, Some(folder), vec![receipt()])
        .unwrap();
    assert_eq!(duplicate[0].status, ImportStatus::Duplicate);
    let batch = store.latest_migration_for(&target, true).unwrap().unwrap();
    assert_eq!(batch.status, "completed");
    assert_eq!(batch.page_id.as_deref(), Some(folder.as_str()));
}

#[test]
fn declared_manifest_scope_and_rollback_preserve_destination_on_corruption_and_cancel() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let source = store.create_workspace("原件来源").unwrap().id;
    let target = store.create_workspace("目标空间").unwrap().id;
    let present = store
        .create_page(&target, None, "既有记录", "file")
        .unwrap();
    let page = store
        .create_page(&source, None, "原始记录", "file")
        .unwrap();
    let raw = b"distinct archive blob data".repeat(32_000);
    store
        .add_attachment(&source, &page.info.id, "资料.txt", raw.as_slice())
        .unwrap();
    let recent = store
        .save_page(
            &source,
            &page.info.id,
            page.info.revision,
            PageDraft {
                title: page.info.title,
                blocks: vec![],
            },
        )
        .unwrap();
    let mut output = Cursor::new(Vec::new());
    store
        .write_workspace_archive_with_history(&source, false, &mut output)
        .unwrap();
    let data = output.into_inner();
    let mut zip = zip::ZipArchive::new(Cursor::new(&data)).unwrap();
    let mut text = String::new();
    zip.by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    let manifest: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(manifest["formatVersion"], 7);
    assert_eq!(manifest["data"]["export"]["scope"], "workspace");
    assert_eq!(manifest["data"]["export"]["history"], "current");
    assert_eq!(manifest["data"]["export"]["downgrades"], json!([]));
    assert!(
        manifest["data"]["export"]["omissions"]
            .as_array()
            .unwrap()
            .contains(&json!("priorRevisions"))
    );
    let mut damaged = data.clone();
    let marker = &raw[..64];
    let start = damaged
        .windows(marker.len())
        .position(|slice| slice == marker)
        .unwrap();
    damaged[start] ^= 1;
    let db = Connection::open(temp.path().join(&target).join("workspace.sqlite")).unwrap();
    let count = |table: &str| {
        db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get::<_, u32>(0)
        })
        .unwrap()
    };
    let feed_before = count("feed_events");
    let preserve = || {
        assert_eq!(store.pages(&target).unwrap(), vec![present.info.clone()]);
        assert_eq!(count("attachments"), 0);
        assert_eq!(count("revisions"), 1);
        assert_eq!(count("feed_events"), feed_before);
        let directory = temp.path().join(&target).join("blobs");
        assert!(!directory.exists() || fs::read_dir(directory).unwrap().next().is_none());
    };
    assert!(
        store
            .merge_workspace_package(
                &target,
                "损坏.zhiye.zip",
                Cursor::new(damaged),
                &AtomicBool::new(false)
            )
            .is_err()
    );
    preserve();
    assert_eq!(
        store
            .latest_migration_for(&target, true)
            .unwrap()
            .unwrap()
            .status,
        "failed"
    );
    let cancellation = AtomicBool::new(false);
    let input = CancelOnBlob {
        cursor: Cursor::new(data.as_slice()),
        offset: start as u64,
        cancel: &cancellation,
    };
    assert_eq!(
        store
            .merge_workspace_package(&target, "取消.zhiye.zip", input, &cancellation)
            .unwrap_err()
            .code,
        "IMPORT_CANCELLED"
    );
    preserve();
    assert_eq!(
        store
            .latest_migration_for(&target, true)
            .unwrap()
            .unwrap()
            .status,
        "cancelled"
    );
    db.execute_batch(
        "CREATE TRIGGER stop_import BEFORE INSERT ON attachments BEGIN
        SELECT RAISE(ABORT, 'simulated write failure'); END;",
    )
    .unwrap();
    assert_eq!(
        store
            .merge_workspace_package(
                &target,
                "写入失败.zip",
                Cursor::new(&data),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
        "DATABASE"
    );
    preserve();
    db.execute_batch("DROP TRIGGER stop_import;").unwrap();
    let merged = store
        .merge_workspace_package(
            &target,
            "重试.zip",
            Cursor::new(&data),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert_eq!(merged.revisions, 1);
    let records = store.pages(&target).unwrap();
    let copied = records
        .iter()
        .find(|item| item.parent_id.as_deref() == Some(merged.page_id.as_str()))
        .unwrap();
    assert_eq!(
        store.history(&target, &copied.id).unwrap()[0].info.revision,
        recent.revision
    );
    assert_eq!(count("revisions"), 3);
    assert_eq!(count("attachments"), 1);
    assert!(count("feed_events") > feed_before);
}

struct CancelOnBlob<'a> {
    cursor: Cursor<&'a [u8]>,
    offset: u64,
    cancel: &'a AtomicBool,
}

impl Read for CancelOnBlob<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let start = self.cursor.position();
        let size = self.cursor.read(buffer)?;
        if start == self.offset && size > 0 {
            self.cancel.store(true, Ordering::Relaxed);
        }
        Ok(size)
    }
}

impl Seek for CancelOnBlob<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.cursor.seek(position)
    }
}

#[test]
fn legacy_content_is_mergeable_after_v17_upgrade_but_recovery_packages_are_separate() {
    let temp = tempfile::tempdir().unwrap();
    let store = Store::open(temp.path()).unwrap();
    let source = store.create_workspace("旧内容").unwrap().id;
    let target = store.create_workspace("合并目标").unwrap().id;
    let page = store
        .create_page(&source, None, "旧 JSON 页面", "file")
        .unwrap();
    let db = Connection::open(temp.path().join(&target).join("workspace.sqlite")).unwrap();
    support::remove_v18_schema(&db);
    db.execute(
        "INSERT INTO migration_batches (id, session_id, kind, source, status, detail)
         VALUES (?1, ?2, 'notionZip', '旧批次.zip', 'failed', '旧记录')",
        params![
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string()
        ],
    )
    .unwrap();
    let legacy = store.export_workspace(&source).unwrap();
    let cancel = AtomicBool::new(false);
    let merged = store
        .merge_workspace_package(&target, "内容.json", Cursor::new(legacy), &cancel)
        .unwrap();
    let copy = store
        .pages(&target)
        .unwrap()
        .into_iter()
        .find(|item| item.title == page.info.title)
        .unwrap();
    assert_eq!(copy.parent_id.as_deref(), Some(merged.page_id.as_str()));
    assert_ne!(copy.id, page.info.id);
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        22
    );
    assert!(
        temp.path()
            .join("upgrade-backups")
            .join(format!("{target}-v17.zhiye-recovery.zip"))
            .is_file()
    );
    assert_eq!(
        store
            .latest_migration_for(&target, false)
            .unwrap()
            .unwrap()
            .detail
            .as_deref(),
        Some("旧记录")
    );
    let exports = tempfile::tempdir().unwrap();
    let recovery = exports.path().join("完整恢复.zhiye-recovery.zip");
    store.save_recovery_package(&source, &recovery).unwrap();
    let input = fs::File::open(&recovery).unwrap();
    assert_eq!(
        store
            .merge_workspace_package(&target, "完整恢复.zip", input, &cancel)
            .unwrap_err()
            .code,
        "ARCHIVE_FORMAT"
    );
    assert_eq!(store.pages(&target).unwrap().len(), 2);
    drop(store);
    db.execute(
        "INSERT INTO migration_batches (id, session_id, kind, source, status)
         VALUES (?1, ?2, 'nativeContent', '断电批次.zip', 'preparing')",
        params![
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string()
        ],
    )
    .unwrap();
    let store = Store::open(temp.path()).unwrap();
    let batch = store.latest_migration_for(&target, true).unwrap().unwrap();
    assert_eq!(batch.status, "interrupted");
    assert_eq!(batch.page_id, None);
}
