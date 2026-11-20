// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        atomic::AtomicBool,
        mpsc::{self, Receiver, Sender},
    },
    time::Duration,
};
use zhiye_core::{
    Block, Calculation, ComputedDraft, IntegrationDraft, IntegrationGrant, IntegrationMutation,
    IntegrationScope, Page, PageDraft, RecordDraft, RelationDraft, RollupFunction, Store,
};

fn setup() -> (tempfile::TempDir, Store, String) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let space = store.create_workspace("清空回收站验收").unwrap().id;
    (directory, store, space)
}

fn database(directory: &Path, space: &str) -> Connection {
    Connection::open(directory.join(space).join("workspace.sqlite")).unwrap()
}

fn archive(store: &Store, space: &str, page: &str) {
    let current = store.page(space, page).unwrap();
    store
        .archive_page(space, page, current.info.revision)
        .unwrap();
}

#[test]
fn confirmation_rejects_changed_trash_and_a_delete_failure_rolls_back_the_batch() {
    let (directory, store, space) = setup();
    let branch = store.create_page(&space, None, "目录", "folder").unwrap();
    let child = store
        .create_page(&space, Some(&branch.info.id), "子页", "file")
        .unwrap();
    let unrelated = store.create_page(&space, None, "仍在使用", "file").unwrap();
    let asset = store
        .add_attachment(&space, &child.info.id, "原件.txt", &b"trash bytes"[..])
        .unwrap()
        .attachment;
    store.advance_search_index(&space).unwrap();
    assert_eq!(store.search(&space, "子页").unwrap()[0].id, child.info.id);
    archive(&store, &space, &child.info.id);
    archive(&store, &space, &branch.info.id);
    let preview = store.preview_empty_trash(&space).unwrap();
    assert_eq!((preview.pages, preview.branches), (2, 2));
    archive(&store, &space, &unrelated.info.id);
    assert_eq!(
        store
            .empty_trash(&space, &preview.confirmation)
            .unwrap_err()
            .code,
        "TRASH_CHANGED"
    );
    let moving = store.page(&space, &unrelated.info.id).unwrap();
    store
        .restore_page(&space, &moving.info.id, moving.info.revision)
        .unwrap();
    let before = store.pages(&space).unwrap();
    let child_history = store.history(&space, &child.info.id).unwrap();

    let db = database(directory.path(), &space);
    db.execute_batch(
        "CREATE TRIGGER stop_trash BEFORE DELETE ON pages
        WHEN old.title = '子页' BEGIN SELECT RAISE(ABORT, 'simulated failure'); END;",
    )
    .unwrap();
    let preview = store.preview_empty_trash(&space).unwrap();
    assert_eq!(
        store
            .empty_trash(&space, &preview.confirmation)
            .unwrap_err()
            .code,
        "DATABASE"
    );
    assert_eq!(store.pages(&space).unwrap(), before);
    assert_eq!(
        store.history(&space, &child.info.id).unwrap(),
        child_history
    );
    db.execute_batch("DROP TRIGGER stop_trash;").unwrap();

    let result = store.empty_trash(&space, &preview.confirmation).unwrap();
    assert_eq!(result.pages, 2);
    assert_eq!(store.preview_empty_trash(&space).unwrap().pages, 0);
    assert_eq!(store.pages(&space).unwrap().len(), 1);
    assert!(store.history(&space, &child.info.id).is_err());
    assert!(store.search(&space, "子页").unwrap().is_empty());
    assert!(
        directory
            .path()
            .join(&space)
            .join("blobs")
            .join(&asset.sha256)
            .is_file()
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, u32>(0)
        })
        .unwrap(),
        0
    );
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

fn save_body(store: &Store, space: &str, page: &Page, blocks: Vec<Block>) {
    store
        .save_page(
            space,
            &page.info.id,
            page.info.revision,
            PageDraft {
                title: page.info.title.clone(),
                blocks,
            },
        )
        .unwrap();
}

#[test]
fn removed_database_preserves_live_schema_old_revisions_and_exported_backups() {
    let (_directory, store, space) = setup();
    let (surviving_db, surviving_container) =
        store.create_collection(&space, None, "业务库").unwrap();
    let (discarded_db, discarded_container) =
        store.create_collection(&space, None, "旧项目库").unwrap();
    let alive = store
        .create_page(
            &space,
            Some(&surviving_container.info.id),
            "工作笔记",
            "file",
        )
        .unwrap();
    let discarded = store
        .create_page(&space, Some(&discarded_container.info.id), "旧记录", "file")
        .unwrap();
    let anchor = block(
        "paragraph",
        json!({"inline":[{"type":"text","text":"永久删除的原文"}]}),
    );
    save_body(&store, &space, &discarded, vec![anchor.clone()]);
    save_body(
        &store,
        &space,
        &alive,
        vec![block(
            "blockReference",
            json!({
                "pageId": discarded.info.id, "blockId": anchor.id,
            }),
        )],
    );
    let schema = store
        .create_relation(
            &space,
            &surviving_db.id,
            1,
            RelationDraft {
                name: "关联项目".into(),
                collection_id: discarded_db.id.clone(),
                inverse_name: Some("任务".into()),
            },
        )
        .unwrap();
    let field = schema
        .properties
        .iter()
        .find(|property| property.name == "关联项目")
        .unwrap()
        .id
        .clone();
    let schema = store
        .save_computed_property(
            &space,
            &schema.id,
            schema.schema_revision,
            ComputedDraft {
                id: None,
                name: "项目数".into(),
                calculation: Calculation::Rollup {
                    relation_property_id: field.clone(),
                    function: RollupFunction::Count,
                    target_property_id: None,
                },
            },
        )
        .unwrap();
    let prior = store
        .save_record(
            &space,
            &schema.id,
            &alive.info.id,
            2,
            RecordDraft {
                title: alive.info.title.clone(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([(field.clone(), json!({"add":[discarded.info.id]}))]),
            },
        )
        .unwrap();
    let reusable = store
        .save_template(&space, &alive.info.id, "工作模板", None)
        .unwrap();
    let removed_template = store
        .save_template(&space, &discarded.info.id, "旧库专属模板", None)
        .unwrap();
    archive(&store, &space, &discarded_container.info.id);
    let outputs = tempfile::tempdir().unwrap();
    let before = outputs.path().join("before.zhiye-recovery.zip");
    store.save_recovery_package(&space, &before).unwrap();

    let preview = store.preview_empty_trash(&space).unwrap();
    assert_eq!(
        (preview.pages, preview.affected_records, preview.templates),
        (2, 1, 1)
    );
    let outcome = store.empty_trash(&space, &preview.confirmation).unwrap();
    assert_eq!(outcome.updated_records, 1);
    assert_eq!(store.collection(&space, &schema.id).unwrap(), schema);
    let cleaned = store.page(&space, &alive.info.id).unwrap();
    assert_eq!(cleaned.info.revision, prior.info.revision + 1);
    assert_eq!(cleaned.blocks, prior.blocks);
    assert!(cleaned.properties.as_ref().unwrap().is_empty());
    assert!(store.page(&space, &discarded.info.id).is_err());
    assert!(
        store
            .templates(&space)
            .unwrap()
            .iter()
            .all(|item| item.id != removed_template.id)
    );
    let history = store.history(&space, &alive.info.id).unwrap();
    assert!(history.contains(&prior));
    let restored_revision = store
        .restore_version(
            &space,
            &alive.info.id,
            cleaned.info.revision,
            prior.info.revision,
        )
        .unwrap();
    assert!(restored_revision.properties.unwrap().is_empty());
    let from_template = store
        .apply_template(
            &space,
            &reusable.id,
            Some(&surviving_container.info.id),
            None,
        )
        .unwrap();
    assert!(from_template.properties.unwrap().is_empty());

    let mut content = Cursor::new(Vec::new());
    store
        .write_workspace_archive_with_history(&space, true, &mut content)
        .unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(content.get_ref())).unwrap();
    let mut manifest = String::new();
    zip.by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    let manifest: Value = serde_json::from_str(&manifest).unwrap();
    assert_eq!(manifest["formatVersion"], 8);
    assert_eq!(
        manifest["data"]["databases"]["tombstones"]["collections"],
        json!([discarded_db.id])
    );
    assert_eq!(
        manifest["data"]["databases"]["tombstones"]["records"],
        json!([{
            "pageId": discarded.info.id, "collectionId": discarded_db.id,
        }])
    );
    let restored = store
        .read_workspace_archive(Cursor::new(content.get_ref()))
        .unwrap();
    assert!(
        store
            .history(&restored.id, &alive.info.id)
            .unwrap()
            .contains(&prior)
    );
    let merged = store
        .merge_workspace_package(
            &restored.id,
            "合并含历史.zip",
            Cursor::new(content.get_ref()),
            &AtomicBool::new(false),
        )
        .unwrap();
    let pages = store.pages(&restored.id).unwrap();
    let new_container = pages
        .iter()
        .find(|page| {
            page.parent_id.as_deref() == Some(merged.page_id.as_str()) && page.title == "业务库"
        })
        .unwrap();
    let new_note = pages
        .iter()
        .find(|page| {
            page.parent_id.as_deref() == Some(new_container.id.as_str())
                && page.title == alive.info.title
        })
        .unwrap();
    let new_history = store.history(&restored.id, &new_note.id).unwrap();
    let former = new_history
        .iter()
        .find(|page| page.info.revision == prior.info.revision)
        .unwrap();
    let missing_id = former.properties.as_ref().unwrap().values().next().unwrap()[0]
        .as_str()
        .unwrap();
    assert_ne!(missing_id, discarded.info.id);
    assert_eq!(former.blocks[0].payload["pageId"], missing_id);
    assert!(store.page(&restored.id, missing_id).is_err());

    let after = outputs.path().join("after.zhiye-recovery.zip");
    store.save_recovery_package(&space, &after).unwrap();
    let before = store
        .read_workspace_package(File::open(before).unwrap())
        .unwrap();
    let after = store
        .read_workspace_package(File::open(after).unwrap())
        .unwrap();
    assert_eq!(
        store.page(&before.id, &discarded.info.id).unwrap().blocks,
        vec![anchor]
    );
    assert!(store.page(&after.id, &discarded.info.id).is_err());
    assert!(
        store
            .history(&after.id, &alive.info.id)
            .unwrap()
            .contains(&prior)
    );
}

#[test]
fn feed_cursor_invalidation_is_limited_to_subscriptions_that_lost_scope() {
    let (_directory, store, space) = setup();
    let folder = store
        .create_page(&space, None, "授权目录", "folder")
        .unwrap();
    let victim = store
        .create_page(&space, Some(&folder.info.id), "待清空", "file")
        .unwrap();
    let other = store.create_page(&space, None, "独立页面", "file").unwrap();
    let token = |grant| {
        store
            .issue_integration(
                &space,
                IntegrationDraft {
                    name: "同步客户端".into(),
                    scopes: [IntegrationScope::Read, IntegrationScope::Changes]
                        .into_iter()
                        .collect(),
                    grants: vec![grant],
                },
            )
            .unwrap()
            .token
    };
    let affected = token(IntegrationGrant::Subtree(folder.info.id));
    let unaffected = token(IntegrationGrant::Page(other.info.id));
    let cursor = |token| {
        store
            .integration_catalog(&space, token, None, None)
            .unwrap()
            .change_cursor
            .unwrap()
    };
    let before_affected = cursor(&affected);
    let before_unaffected = cursor(&unaffected);
    archive(&store, &space, &victim.info.id);
    let preview = store.preview_empty_trash(&space).unwrap();
    store.empty_trash(&space, &preview.confirmation).unwrap();
    assert_eq!(
        store
            .integration_changes(&space, &affected, None, &before_affected)
            .unwrap_err()
            .code,
        "CURSOR_INVALID"
    );
    assert!(
        store
            .integration_changes(&space, &unaffected, None, &before_unaffected)
            .unwrap()
            .items
            .is_empty()
    );
    assert!(
        store
            .integration_changes(&space, &affected, None, &cursor(&affected))
            .unwrap()
            .items
            .is_empty()
    );
}

#[test]
fn replaying_a_create_after_emptying_trash_cannot_recreate_the_page() {
    let (_directory, store, space) = setup();
    let parent = store
        .create_page(&space, None, "保留的授权目录", "folder")
        .unwrap();
    let issued = store
        .issue_integration(
            &space,
            IntegrationDraft {
                name: "可创建内容的测试集成".into(),
                scopes: [IntegrationScope::Read, IntegrationScope::Create]
                    .into_iter()
                    .collect(),
                grants: vec![IntegrationGrant::Subtree(parent.info.id.clone())],
            },
        )
        .unwrap();
    let create = |title: &str| {
        store.mutate_as_integration(
            &space,
            &issued.token,
            "original-create-request",
            None,
            IntegrationMutation::CreatePage {
                parent_id: parent.info.id.clone(),
                title: title.into(),
                icon: "file".into(),
            },
        )
    };
    let made = create("一次创建").unwrap();
    archive(&store, &space, &made.page_id);
    let preview = store.preview_empty_trash(&space).unwrap();
    store.empty_trash(&space, &preview.confirmation).unwrap();
    assert_eq!(create("一次创建").unwrap_err().code, "RESOURCE_NOT_FOUND");
    assert_eq!(
        create("不同请求内容").unwrap_err().code,
        "IDEMPOTENCY_CONFLICT"
    );
    assert_eq!(store.pages(&space).unwrap(), vec![parent.info]);
}

#[test]
fn blob_cleanup_delays_deletion_and_rechecks_live_and_template_references() {
    let (directory, store, space) = setup();
    let to_delete = store
        .create_page(&space, None, "将永久删除", "file")
        .unwrap();
    let in_trash = store.create_page(&space, None, "等待恢复", "file").unwrap();
    let active = store.create_page(&space, None, "活动页", "file").unwrap();
    let original = |page: &Page, label, data| {
        store
            .add_attachment(&space, &page.info.id, label, data)
            .unwrap()
            .attachment
    };
    let candidate = original(&to_delete, "孤立.txt", &b"isolated"[..]);
    let shared = original(&to_delete, "共享.txt", &b"shared"[..]);
    let template_only = original(&to_delete, "模板.txt", &b"template"[..]);
    let rescued = original(&to_delete, "再次引用.txt", &b"rescue"[..]);
    original(&active, "共享副本.txt", &b"shared"[..]);
    let protected = original(&in_trash, "待恢复.txt", &b"restore"[..]);
    let folder = directory.path().join(&space).join("blobs");
    fs::write(folder.join(".unpublished-temp"), b"staged bytes").unwrap();
    let db = database(directory.path(), &space);
    let template_page = store.create_page(&space, None, "模板来源", "file").unwrap();
    original(&template_page, "模板.txt", &b"template"[..]);
    store
        .save_template(&space, &template_page.info.id, "保留原件的模板", None)
        .unwrap();
    archive(&store, &space, &to_delete.info.id);
    archive(&store, &space, &template_page.info.id);
    let backup_dir = tempfile::tempdir().unwrap();
    let backup = backup_dir.path().join("清空前.zhiye-recovery.zip");
    store.save_recovery_package(&space, &backup).unwrap();
    let preview = store.preview_empty_trash(&space).unwrap();
    store.empty_trash(&space, &preview.confirmation).unwrap();
    archive(&store, &space, &in_trash.info.id);

    let pending = store.preview_blob_cleanup(&space).unwrap();
    assert_eq!((pending.waiting, pending.files), (2, 0));
    db.execute("UPDATE blob_quarantine SET unreferenced_since = 0", [])
        .unwrap();
    let eligible = store.preview_blob_cleanup(&space).unwrap();
    assert_eq!(eligible.files, 2);
    let after_purge = backup_dir.path().join("清空后.zhiye-recovery.zip");
    store.save_recovery_package(&space, &after_purge).unwrap();
    original(&active, "找回.txt", &b"rescue"[..]);
    assert_eq!(
        store
            .clean_blobs(&space, &eligible.confirmation)
            .unwrap_err()
            .code,
        "BLOBS_CHANGED"
    );
    let eligible = store.preview_blob_cleanup(&space).unwrap();
    assert_eq!(eligible.files, 1);
    drop(store);
    let store = Store::open(directory.path()).unwrap();
    let result = store.clean_blobs(&space, &eligible.confirmation).unwrap();
    assert_eq!((result.removed, result.failed), (1, 0));
    assert!(!folder.join(&candidate.sha256).exists());
    for kept in [shared, template_only, rescued, protected] {
        assert!(folder.join(kept.sha256).is_file());
    }
    assert!(folder.join(".unpublished-temp").is_file());
    let recovered = store
        .read_workspace_package(File::open(backup).unwrap())
        .unwrap();
    let saved = store.page(&recovered.id, &to_delete.info.id).unwrap();
    store
        .restore_page(&recovered.id, &saved.info.id, saved.info.revision)
        .unwrap();
    let mut copied = Vec::new();
    store
        .copy_attachment(&recovered.id, &saved.info.id, &candidate.id, &mut copied)
        .unwrap();
    assert_eq!(copied, b"isolated");

    let recovered = store
        .read_workspace_package(File::open(after_purge).unwrap())
        .unwrap();
    let restored_blobs = directory.path().join(&recovered.id).join("blobs");
    assert!(!restored_blobs.join(&candidate.sha256).exists());
    // Simulate this hash being published again by an import that aborts
    // before it can commit attachment metadata in the restored workspace.
    fs::write(restored_blobs.join(&candidate.sha256), b"isolated").unwrap();
    let preview = store.preview_blob_cleanup(&recovered.id).unwrap();
    assert_eq!((preview.waiting, preview.files), (1, 0));
}

#[test]
fn blob_cleanup_defers_while_a_content_export_is_using_a_snapshot() {
    let (_directory, store, space) = setup();
    let page = store.create_page(&space, None, "正在打包", "file").unwrap();
    store
        .add_attachment(&space, &page.info.id, "仍引用的原件.txt", &b"snapshot"[..])
        .unwrap();
    let (entered, started) = mpsc::channel();
    let (resume, release) = mpsc::channel();
    std::thread::scope(|scope| {
        let export = scope.spawn(|| {
            store.write_workspace_archive(
                &space,
                PausedOutput {
                    output: Cursor::new(Vec::new()),
                    entered: Some(entered),
                    release,
                },
            )
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let busy = store.preview_blob_cleanup(&space).unwrap_err();
        resume.send(()).unwrap();
        export.join().unwrap().unwrap();
        assert_eq!(busy.code, "BLOBS_BUSY");
    });
    assert_eq!(store.preview_blob_cleanup(&space).unwrap().files, 0);
}

struct PausedOutput {
    output: Cursor<Vec<u8>>,
    entered: Option<Sender<()>>,
    release: Receiver<()>,
}

impl Write for PausedOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            entered.send(()).map_err(io::Error::other)?;
            self.release
                .recv_timeout(Duration::from_secs(5))
                .map_err(io::Error::other)?;
        }
        self.output.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

impl Seek for PausedOutput {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.output.seek(position)
    }
}
