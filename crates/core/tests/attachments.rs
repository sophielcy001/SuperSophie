use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    io::{Cursor, Read},
};
use zhiye_core::{PageDraft, Store, Workspace};

fn workspace(store: &Store) -> (Workspace, String) {
    let workspace = store.create_workspace("资料").unwrap();
    let page = store
        .create_page(&workspace.id, None, "页面", "file")
        .unwrap();
    (workspace, page.info.id)
}

#[test]
fn attaches_once_and_reuses_the_immutable_file_across_pages() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let other = store.create_page(&ws.id, None, "另一页", "file").unwrap();
    let bytes = b"%PDF-1.7\nattachment content\n";
    let first = store
        .add_attachment(&ws.id, &page, "原始资料.pdf", &bytes[..])
        .unwrap();
    assert_eq!(first.attachment.media_type, "application/pdf");
    assert!(!first.reused_blob && !first.already_attached);
    let retry = store
        .add_attachment(&ws.id, &page, "原始资料.pdf", &bytes[..])
        .unwrap();
    assert!(retry.reused_blob && retry.already_attached);
    assert_eq!(retry.attachment.id, first.attachment.id);
    let second = store
        .add_attachment(&ws.id, &other.info.id, "副本.pdf", &bytes[..])
        .unwrap();
    assert!(second.reused_blob && !second.already_attached);
    assert_ne!(second.attachment.id, first.attachment.id);
    assert_eq!(
        fs::read_dir(root.path().join(&ws.id).join("blobs"))
            .unwrap()
            .count(),
        1
    );

    let mut exported = Vec::new();
    store
        .copy_attachment(&ws.id, &page, &first.attachment.id, &mut exported)
        .unwrap();
    assert_eq!(exported, bytes);
    assert_eq!(
        store
            .copy_attachment(&ws.id, &other.info.id, &first.attachment.id, io::sink())
            .unwrap_err()
            .code,
        "ATTACHMENT_NOT_FOUND"
    );
    let (isolated, isolated_page) = workspace(&store);
    assert_eq!(
        store
            .copy_attachment(
                &isolated.id,
                &isolated_page,
                &first.attachment.id,
                io::sink()
            )
            .unwrap_err()
            .code,
        "ATTACHMENT_NOT_FOUND"
    );

    store.archive_page(&ws.id, &page, 1).unwrap();
    assert!(store.attachments(&ws.id, &page).is_err());
    store.restore_page(&ws.id, &page, 2).unwrap();
    assert_eq!(
        store.attachments(&ws.id, &page).unwrap(),
        vec![first.attachment]
    );
}

#[test]
fn interrupted_source_and_failed_metadata_leave_no_attachment() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let reader = Cursor::new(b"partial file").chain(BrokenReader);
    assert_eq!(
        store
            .add_attachment(&ws.id, &page, "中断.txt", reader)
            .unwrap_err()
            .code,
        "IO"
    );
    let blobs = root.path().join(&ws.id).join("blobs");
    assert_eq!(fs::read_dir(&blobs).unwrap().count(), 0);
    assert!(store.attachments(&ws.id, &page).unwrap().is_empty());

    let connection = Connection::open(root.path().join(&ws.id).join("workspace.sqlite")).unwrap();
    connection.execute_batch("CREATE TRIGGER stop_attachment BEFORE INSERT ON attachments BEGIN SELECT RAISE(ABORT, 'injected write failure'); END;").unwrap();
    drop(connection);
    assert!(
        store
            .add_attachment(&ws.id, &page, "失败.txt", &b"injected"[..])
            .is_err()
    );
    assert!(store.attachments(&ws.id, &page).unwrap().is_empty());
    assert_eq!(fs::read_dir(&blobs).unwrap().count(), 1);
    assert_eq!(store.page(&ws.id, &page).unwrap().info.revision, 1);
}

struct BrokenReader;

impl io::Read for BrokenReader {
    fn read(&mut self, _buf: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::other("source interrupted"))
    }
}

struct FailingOutput {
    kind: io::ErrorKind,
    position: Cursor<Vec<u8>>,
}

impl FailingOutput {
    fn new(kind: io::ErrorKind) -> Self {
        Self {
            kind,
            position: Cursor::new(Vec::new()),
        }
    }
}

impl io::Write for FailingOutput {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(self.kind.into())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl io::Seek for FailingOutput {
    fn seek(&mut self, position: io::SeekFrom) -> io::Result<u64> {
        self.position.seek(position)
    }
}

#[test]
fn attachment_export_preserves_storage_quota_error_and_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let payload = b"original attachment";
    let attachment = store
        .add_attachment(&ws.id, &page, "原件.bin", &payload[..])
        .unwrap()
        .attachment;
    let original = store.page(&ws.id, &page).unwrap();
    let error = store
        .copy_attachment(
            &ws.id,
            &page,
            &attachment.id,
            FailingOutput::new(io::ErrorKind::QuotaExceeded),
        )
        .unwrap_err();
    assert_eq!(error.code, "STORAGE_FULL");
    assert!(error.message.contains("存储配额不足"));
    assert_eq!(store.page(&ws.id, &page).unwrap(), original);
    assert_eq!(
        store.attachments(&ws.id, &page).unwrap(),
        vec![attachment.clone()]
    );

    let mut retry = Vec::new();
    store
        .copy_attachment(&ws.id, &page, &attachment.id, &mut retry)
        .unwrap();
    assert_eq!(retry, payload);
}

#[test]
fn workspace_archive_preserves_capacity_errors_and_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let original = store.page(&ws.id, &page).unwrap();

    for (kind, expected) in [
        (io::ErrorKind::StorageFull, "存储空间不足"),
        (io::ErrorKind::QuotaExceeded, "存储配额不足"),
    ] {
        let error = store
            .write_workspace_archive_with_history(&ws.id, true, FailingOutput::new(kind))
            .unwrap_err();
        assert_eq!(error.code, "STORAGE_FULL");
        assert!(error.message.contains(expected));
        assert_eq!(store.page(&ws.id, &page).unwrap(), original);
    }

    let error = store
        .write_workspace_archive_with_history(
            &ws.id,
            true,
            FailingOutput::new(io::ErrorKind::PermissionDenied),
        )
        .unwrap_err();
    assert_eq!(error.code, "ARCHIVE");
    let mut archive = Cursor::new(Vec::new());
    store
        .write_workspace_archive_with_history(&ws.id, true, &mut archive)
        .unwrap();
    let restored = store
        .read_workspace_archive(Cursor::new(archive.into_inner()))
        .unwrap();
    assert_ne!(restored.id, ws.id);
    assert_eq!(store.page(&restored.id, &page).unwrap(), original);
}

#[test]
fn occupied_blob_path_reports_storage_error_and_can_retry() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let payload = b"fixture for an occupied attachment storage path";
    let blobs = root.path().join(&ws.id).join("blobs");
    let digest: String = Sha256::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let blocked = blobs.join(digest);
    fs::create_dir_all(&blocked).unwrap();

    let error = store
        .add_attachment(&ws.id, &page, "原件.bin", &payload[..])
        .unwrap_err();
    assert_eq!(error.code, "WORKSPACE_PATH");
    assert_eq!(error.message, "附件原件的存储位置不可用，无法保存。");
    assert!(store.attachments(&ws.id, &page).unwrap().is_empty());
    assert_eq!(fs::read_dir(&blobs).unwrap().count(), 1);
    assert!(blocked.is_dir());

    fs::remove_dir(&blocked).unwrap();
    let imported = store
        .add_attachment(&ws.id, &page, "原件.bin", &payload[..])
        .unwrap();
    assert_eq!(
        store.attachments(&ws.id, &page).unwrap(),
        vec![imported.attachment]
    );
    assert_eq!(fs::read(blocked).unwrap(), payload);
}

#[test]
fn archive_restores_originals_and_rejects_corrupted_binary_content() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (ws, page) = workspace(&store);
    let payload = b"isolated binary fixture 7345985";
    let saved = store
        .add_attachment(&ws.id, &page, "原件.bin", &payload[..])
        .unwrap()
        .attachment;
    let mut file = Cursor::new(Vec::new());
    store.write_workspace_archive(&ws.id, &mut file).unwrap();
    let data = file.into_inner();
    let restored = store.read_workspace_archive(Cursor::new(&data)).unwrap();
    assert_ne!(restored.id, ws.id);
    assert_eq!(
        store.attachments(&restored.id, &page).unwrap(),
        vec![saved.clone()]
    );
    let mut bytes = Vec::new();
    store
        .copy_attachment(&restored.id, &page, &saved.id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, payload);

    let mut corrupted = data;
    let start = corrupted
        .windows(payload.len())
        .position(|slice| slice == payload)
        .unwrap();
    corrupted[start] ^= 1;
    assert!(
        store
            .read_workspace_archive(Cursor::new(&corrupted))
            .is_err()
    );
    assert_eq!(store.workspaces().unwrap().len(), 2);
    assert!(fs::read_dir(root.path()).unwrap().all(|item| {
        !item
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".staging-")
    }));
}

#[test]
fn content_archive_can_keep_only_each_current_revision() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (space, id) = workspace(&store);
    let revise = |space: &str, revision, title: &str| {
        store
            .save_page(
                space,
                &id,
                revision,
                PageDraft {
                    title: title.into(),
                    blocks: Vec::new(),
                },
            )
            .unwrap();
    };
    revise(&space.id, 1, "旧草稿");
    revise(&space.id, 2, "迁移用页面");
    let current = store.page(&space.id, &id).unwrap();
    let original = b"portable original";
    let attachment = store
        .add_attachment(&space.id, &id, "原件.txt", original.as_slice())
        .unwrap()
        .attachment;

    let mut no_history = Cursor::new(Vec::new());
    store
        .write_workspace_archive_with_history(&space.id, false, &mut no_history)
        .unwrap();
    no_history.set_position(0);
    let recovered = store.read_workspace_archive(no_history).unwrap();
    assert_eq!(store.page(&recovered.id, &id).unwrap(), current);
    assert_eq!(store.history(&recovered.id, &id).unwrap(), vec![current]);
    let mut bytes = Vec::new();
    store
        .copy_attachment(&recovered.id, &id, &attachment.id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, original);
    revise(&recovered.id, 3, "恢复后修改");
    assert_eq!(
        store
            .history(&recovered.id, &id)
            .unwrap()
            .iter()
            .map(|page| page.info.revision)
            .collect::<Vec<_>>(),
        vec![4, 3]
    );

    let mut full = Cursor::new(Vec::new());
    store.write_workspace_archive(&space.id, &mut full).unwrap();
    full.set_position(0);
    let other_copy = store.read_workspace_archive(full).unwrap();
    let history = store.history(&space.id, &id).unwrap();
    assert_eq!(history.len(), 3);
    assert_eq!(store.history(&other_copy.id, &id).unwrap(), history);
}

#[test]
fn renaming_attachment_keeps_identity_and_rejects_an_exact_duplicate() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let (workspace, page) = workspace(&store);
    let bytes = b"immutable content";
    let first = store
        .add_attachment(&workspace.id, &page, "原件.txt", &bytes[..])
        .unwrap()
        .attachment;
    let renamed = store
        .rename_attachment(&workspace.id, &page, &first.id, "改名后.txt")
        .unwrap();
    assert_eq!(renamed.id, first.id);
    assert_eq!(renamed.sha256, first.sha256);
    assert_eq!(renamed.name, "改名后.txt");
    let second = store
        .add_attachment(&workspace.id, &page, "另一份.txt", &bytes[..])
        .unwrap()
        .attachment;
    let error = store
        .rename_attachment(&workspace.id, &page, &second.id, &renamed.name)
        .unwrap_err();
    assert_eq!(error.code, "ATTACHMENT_DUPLICATE");
    assert_eq!(
        store
            .attachment(&workspace.id, &page, &second.id)
            .unwrap()
            .name,
        "另一份.txt"
    );
    let mut copied = Vec::new();
    store
        .copy_attachment(&workspace.id, &page, &first.id, &mut copied)
        .unwrap();
    assert_eq!(copied, bytes);
}

#[test]
fn migrates_a_real_v1_schema_with_a_readable_pre_upgrade_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (ws, page) = v1_fixture(root.path(), false);
    let store = Store::open(root.path()).unwrap();
    assert_eq!(store.page(&ws, &page).unwrap().info.title, "旧页面");
    assert!(store.attachments(&ws, &page).unwrap().is_empty());
    let current = root.path().join(&ws).join("workspace.sqlite");
    let snapshot = root
        .path()
        .join("upgrade-backups")
        .join(format!("{ws}-v1.sqlite"));
    assert_eq!(version(&current), 22);
    assert_eq!(version(&snapshot), 1);
    let snapshot_db = Connection::open(snapshot).unwrap();
    assert_eq!(
        snapshot_db
            .query_row("SELECT title FROM pages", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "旧页面"
    );
    assert_eq!(
        snapshot_db
            .query_row("SELECT count(*) FROM revisions", [], |row| row
                .get::<_, u32>(0))
            .unwrap(),
        1
    );
}

#[test]
fn migration_failure_rolls_back_the_ddl_and_keeps_the_original_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (ws, _) = v1_fixture(root.path(), true);
    let store = Store::open(root.path()).unwrap();
    let overview = store.workspace_overview().unwrap();
    assert_eq!(overview.issues.len(), 1);
    assert_eq!(overview.issues[0].id, ws);
    let current = root.path().join(&ws).join("workspace.sqlite");
    let snapshot = root
        .path()
        .join("upgrade-backups")
        .join(format!("{ws}-v1.sqlite"));
    assert_eq!(version(&current), 1);
    assert_eq!(version(&snapshot), 1);
    let package = snapshot.with_extension("zhiye-recovery.zip");
    assert!(package.is_file());
    assert_eq!(
        store
            .read_workspace_package(fs::File::open(package).unwrap())
            .unwrap_err()
            .code,
        "RECOVERY_INVALID"
    );
    let db = Connection::open(current).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='attachments'",
            [],
            |row| row.get::<_, u32>(0)
        )
        .unwrap(),
        0
    );
    assert_eq!(
        db.query_row("SELECT title FROM pages", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "旧页面"
    );
}

fn version(path: &std::path::Path) -> u32 {
    Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap()
}

fn v1_fixture(root: &std::path::Path, block_migration: bool) -> (String, String) {
    let workspace = "424b640f-482b-427d-9009-596ff3cb742e";
    let page = "658af098-a4c2-4124-a5a8-548bd321a308";
    let directory = root.join(workspace);
    fs::create_dir(&directory).unwrap();
    let db = Connection::open(directory.join("workspace.sqlite")).unwrap();
    db.execute_batch("CREATE TABLE workspace(id TEXT PRIMARY KEY, name TEXT NOT NULL);
        CREATE TABLE pages(id TEXT PRIMARY KEY, parent_id TEXT, title TEXT NOT NULL, revision INTEGER NOT NULL,
          updated_at INTEGER NOT NULL, icon TEXT NOT NULL, trash_root_id TEXT, blocks TEXT NOT NULL, body_text TEXT NOT NULL);
        CREATE TABLE revisions(page_id TEXT NOT NULL, revision INTEGER NOT NULL, snapshot TEXT NOT NULL, PRIMARY KEY(page_id, revision));
        CREATE TABLE block_ids(id TEXT PRIMARY KEY, page_id TEXT NOT NULL);
        PRAGMA user_version=1;").unwrap();
    db.execute("INSERT INTO workspace VALUES (?1, '旧空间')", [workspace])
        .unwrap();
    db.execute(
        "INSERT INTO pages VALUES (?1, NULL, '旧页面', 1, 1, 'file', NULL, '[]', '')",
        [page],
    )
    .unwrap();
    let snapshot = serde_json::json!({ "id": page, "parentId": null, "title": "旧页面", "revision": 1,
        "updatedAt": 1, "icon": "file", "trashRootId": null, "blocks": [] });
    db.execute(
        "INSERT INTO revisions VALUES (?1, 1, ?2)",
        params![page, snapshot.to_string()],
    )
    .unwrap();
    if block_migration {
        db.execute_batch("CREATE TABLE attachment_page(placeholder INTEGER);")
            .unwrap();
    }
    (workspace.into(), page.into())
}
