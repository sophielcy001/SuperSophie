// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::archive::{blob_entry, check_zip_file};
use crate::assets::{
    blob_directory, check_content, persist_blob, read_attachments, transfer, valid_attachment,
    valid_hash,
};
use crate::bundle;
use crate::collections::{snapshot_collections, validate_collections};
use crate::migrations;
use crate::model::{name, new_id, valid_id};
use crate::store::{
    configure, initialize_schema, now, recovered_workspace, workspace_info, workspace_snapshot,
};
use crate::templates::{snapshot_templates, template_attachments, validate_templates};
use crate::{Attachment, Error, MAX_ATTACHMENT_BYTES, Result, Store, Workspace, sync_export_file};
use rusqlite::config::DbConfig;
use rusqlite::{Connection, MAIN_DB, OpenFlags, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, Write},
    path::{Path, PathBuf},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

pub(super) const MANIFEST: &str = "recovery.json";
const DATABASE: &str = "workspace.sqlite";
const FORMAT: &str = "zhiye.recovery";
const VERSION: u32 = 1;
const MAX_DATABASE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_BLOBS: usize = 100_000;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryKind {
    Manual,
    Automatic,
    Upgrade,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Capture {
    pub id: String,
    pub kind: RecoveryKind,
    pub at: i64,
}

impl Capture {
    pub fn new(kind: RecoveryKind, at: i64) -> Self {
        Self {
            id: new_id(),
            kind,
            at,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoveryPoint {
    pub id: String,
    pub workspace_id: String,
    pub kind: RecoveryKind,
    pub created_at: i64,
    pub bytes: u64,
    pub path: PathBuf,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Manifest {
    format: String,
    format_version: u32,
    sqlite_version: u32,
    capture: Capture,
    workspace: Workspace,
    database: FileDigest,
    blobs: Vec<FileDigest>,
}

#[derive(Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileDigest {
    sha256: String,
    bytes: u64,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::new("RECOVERY_INVALID", message)
}

pub(super) fn create_package(
    store: &Store,
    live: &Connection,
    workspace: &str,
    destination: &Path,
    capture: Capture,
) -> Result<RecoveryPoint> {
    let _lease = store.pin_blobs()?;
    let directory = destination.parent().ok_or_else(super::bad_destination)?;
    if directory.canonicalize()? != directory {
        return Err(super::bad_destination());
    }
    let stage = tempfile::Builder::new()
        .prefix(&format!(".zhiye-backup-{}-", capture.id))
        .tempdir_in(directory)?;
    let snapshot = stage.path().join(DATABASE);
    // SQLite obtains its own consistent snapshot while live writers may continue.
    live.backup(MAIN_DB, &snapshot, None::<fn(rusqlite::backup::Progress)>)?;
    let snapshot_db = Connection::open_with_flags(&snapshot, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    let version = migrations::version(&snapshot_db)?;
    let origin = workspace_info(&snapshot_db)?;
    if origin.id != workspace || !(1..=migrations::VERSION).contains(&version) {
        return Err(invalid("备份源工作空间或数据格式不匹配。"));
    }
    protect_connection(&snapshot_db)?;
    if capture.kind != RecoveryKind::Upgrade {
        validate_schema(&snapshot_db, version)?;
        integrity(&snapshot_db, false)?;
    }
    scrub_credentials(&snapshot_db, version)?;
    let assets = assets(&snapshot_db, version)?;
    drop(snapshot_db);
    let hash = transfer(File::open(&snapshot)?, io::sink(), MAX_DATABASE_BYTES)?;
    let manifest = Manifest {
        format: FORMAT.into(),
        format_version: VERSION,
        sqlite_version: version,
        capture,
        workspace: origin,
        database: FileDigest {
            sha256: hash.sha256,
            bytes: hash.bytes,
        },
        blobs: assets
            .values()
            .map(|asset| FileDigest {
                sha256: asset.sha256.clone(),
                bytes: asset.bytes,
            })
            .collect(),
    };
    let descriptor = serde_json::to_vec(&manifest)?;
    if descriptor.len() as u64 > MAX_MANIFEST_BYTES {
        return Err(invalid("整库恢复清单超过 16 MiB。"));
    }
    let mut output = tempfile::NamedTempFile::new_in(stage.path())?;
    let mut zip = ZipWriter::new(output.as_file_mut());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    zip.start_file(MANIFEST, options)?;
    zip.write_all(&descriptor)?;
    zip.start_file(
        DATABASE,
        options.large_file(manifest.database.bytes >= u64::from(u32::MAX)),
    )?;
    let copied = transfer(File::open(&snapshot)?, &mut zip, manifest.database.bytes)?;
    check_digest(&copied, &manifest.database)?;
    // The lease protects the immutable files referenced by this snapshot,
    // even if live metadata changes before the copy completes.
    for asset in assets.values() {
        zip.start_file(blob_entry(&asset.sha256), options)?;
        store.copy_blob(workspace, asset, &mut zip)?;
    }
    zip.finish()?;
    sync_export_file(output.as_file())?;
    let bytes = output.as_file().metadata()?.len();
    output
        .persist(destination)
        .map_err(|error| Error::from(error.error))?;
    Ok(point(&manifest, destination, bytes))
}

pub(super) fn restore_package<R: Read + Seek>(
    store: &Store,
    zip: &mut ZipArchive<R>,
) -> Result<Workspace> {
    let manifest = read_manifest(zip)?;
    let stage = tempfile::Builder::new()
        .prefix(".staging-recovery-")
        .tempdir_in(store.data_root())?;
    let path = stage.path().join(DATABASE);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    let hash = transfer(zip.by_name(DATABASE)?, &file, manifest.database.bytes)?;
    check_digest(&hash, &manifest.database)?;
    file.sync_all()?;
    drop(file);

    let mut db = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    protect_connection(&db)?;
    if migrations::version(&db)? != manifest.sqlite_version {
        return Err(invalid("整库恢复包的数据版本与清单不符。"));
    }
    validate_schema(&db, manifest.sqlite_version)?;
    integrity(&db, true)?;
    if workspace_info(&db)? != manifest.workspace {
        return Err(invalid("整库恢复包的来源与清单不符。"));
    }
    let assets = assets(&db, manifest.sqlite_version)?;
    if assets.len() != manifest.blobs.len()
        || manifest.blobs.iter().any(|entry| {
            !assets
                .get(&entry.sha256)
                .is_some_and(|asset| asset.bytes == entry.bytes)
        })
    {
        return Err(invalid("整库恢复包的原件与快照不符。"));
    }
    if !assets.is_empty() {
        let directory = blob_directory(stage.path(), true)?;
        for asset in assets.values() {
            let mut file = tempfile::NamedTempFile::new_in(&directory)?;
            let hash = transfer(
                zip.by_name(&blob_entry(&asset.sha256))?,
                &mut file,
                asset.bytes,
            )?;
            check_content(&hash, asset)?;
            persist_blob(file, &directory, asset)?;
        }
    }

    // All DDL has been matched to the app's own schema. Only now allow the
    // bundled migrations to use the trusted schema and its FTS triggers.
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, true)?;
    let restored = recovered_workspace(&manifest.workspace);
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    migrations::apply(&tx, manifest.sqlite_version, migrations::VERSION)?;
    // Recovery carries referenced files only. A quarantine timestamp belongs
    // to an observed file in the original workspace, not to its hash alone.
    tx.execute("DELETE FROM blob_quarantine", [])?;
    let data = workspace_snapshot(&tx, true)?;
    bundle::validate(&data)?;
    let attachments = read_attachments(&tx, None)?;
    bundle::validate_attachments(&data, &attachments)?;
    let collections = snapshot_collections(&tx)?;
    validate_collections(&data, &collections, &attachments)?;
    validate_templates(&snapshot_templates(&tx)?, &collections.definitions)?;
    tx.execute(
        "UPDATE workspace SET id=?1, name=?2",
        params![restored.id, restored.name],
    )?;
    tx.commit()?;
    scrub_credentials(&db, migrations::VERSION)?;
    integrity(&db, true)?;
    drop(db);
    OpenOptions::new().write(true).open(&path)?.sync_all()?;
    store.publish_staging(&restored.id, stage.path())?;
    Ok(restored)
}

fn protect_connection(db: &Connection) -> Result<()> {
    configure(db)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_TRUSTED_SCHEMA, false)?;
    Ok(())
}

fn check_digest(actual: &crate::assets::Content, expected: &FileDigest) -> Result<()> {
    if actual.bytes != expected.bytes || actual.sha256 != expected.sha256 {
        return Err(Error::new("RECOVERY_CHECKSUM", "整库恢复包内容校验失败。"));
    }
    Ok(())
}

fn integrity(db: &Connection, full: bool) -> Result<()> {
    let command = if full {
        "PRAGMA integrity_check(1)"
    } else {
        "PRAGMA quick_check(1)"
    };
    let check: String = db.query_row(command, [], |row| row.get(0))?;
    if check != "ok"
        || db
            .prepare("PRAGMA foreign_key_check")?
            .query([])?
            .next()?
            .is_some()
    {
        return Err(invalid("SQLite 快照存在损坏或缺失的引用。"));
    }
    Ok(())
}

fn validate_schema(db: &Connection, version: u32) -> Result<()> {
    let mut example = Connection::open_in_memory()?;
    configure(&example)?;
    let tx = example.transaction()?;
    initialize_schema(&tx, version)?;
    if schema_objects(db)? != schema_objects(&tx)? {
        return Err(invalid("整库恢复包包含未知或被修改的 SQLite 结构。"));
    }
    Ok(())
}

fn schema_objects(db: &Connection) -> Result<Vec<(String, String, String, Option<String>)>> {
    Ok(db
        .prepare("SELECT type,name,tbl_name,sql FROM sqlite_schema ORDER BY type,name")?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn scrub_credentials(db: &Connection, version: u32) -> Result<()> {
    if version < 6 {
        return Ok(());
    }
    db.pragma_update(None, "secure_delete", true)?;
    let previous = db.db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER)?;
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, false)?;
    let result = (|| {
        let tx = db.unchecked_transaction()?;
        tx.execute(
            "UPDATE integrations SET revoked_at=coalesce(revoked_at,?1), token_hash=randomblob(32)",
            [now()],
        )?;
        if version >= 7 {
            tx.execute(
                "UPDATE workspace_state SET query_cursor_key=randomblob(32) WHERE slot=1",
                [],
            )?;
        }
        tx.commit()?;
        // Remove old digests or cursor secrets even from unused database pages.
        db.execute_batch("VACUUM;")?;
        Ok(())
    })();
    db.set_db_config(DbConfig::SQLITE_DBCONFIG_ENABLE_TRIGGER, previous)?;
    result
}

fn assets(db: &Connection, version: u32) -> Result<BTreeMap<String, Attachment>> {
    let mut attachments = if version >= 2 {
        read_attachments(db, None)?
    } else {
        vec![]
    };
    if version >= 5 {
        attachments.extend(template_attachments(&snapshot_templates(db)?).cloned());
    }
    let mut assets: BTreeMap<String, Attachment> = BTreeMap::new();
    for attachment in attachments {
        if !valid_attachment(&attachment) {
            return Err(invalid("快照中的附件元数据无效。"));
        }
        if let Some(existing) = assets.get(&attachment.sha256) {
            if existing.bytes != attachment.bytes || existing.media_type != attachment.media_type {
                return Err(invalid("快照中同一个原件的元数据不一致。"));
            }
        } else {
            assets.insert(attachment.sha256.clone(), attachment);
            if assets.len() > MAX_BLOBS {
                return Err(invalid("整库恢复包最多包含 100,000 份不同原件。"));
            }
        }
    }
    Ok(assets)
}

fn read_manifest<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<Manifest> {
    if zip.len() > MAX_BLOBS + 2 {
        return Err(invalid("整库恢复包包含过多文件。"));
    }
    let mut data = Vec::new();
    let entry = zip.by_name(MANIFEST)?;
    check_zip_file(&entry)?;
    if entry.size() > MAX_MANIFEST_BYTES {
        return Err(invalid("整库恢复清单超过 16 MiB。"));
    }
    entry.take(MAX_MANIFEST_BYTES + 1).read_to_end(&mut data)?;
    let manifest: Manifest = serde_json::from_slice(&data)?;
    if manifest.format != FORMAT
        || manifest.format_version != VERSION
        || !(1..=migrations::VERSION).contains(&manifest.sqlite_version)
        || !valid_id(&manifest.workspace.id)
        || !valid_id(&manifest.capture.id)
        || manifest.capture.at < 0
        || name(&manifest.workspace.name, 64, None)? != manifest.workspace.name
        || !valid_hash(&manifest.database.sha256)
        || !(512..=MAX_DATABASE_BYTES).contains(&manifest.database.bytes)
        || manifest.blobs.len() > MAX_BLOBS
    {
        return Err(invalid("整库恢复清单中的格式或字段无效。"));
    }
    let mut expected = BTreeMap::from([
        (MANIFEST.into(), data.len() as u64),
        (DATABASE.into(), manifest.database.bytes),
    ]);
    let mut hashes = HashSet::new();
    for asset in &manifest.blobs {
        if !valid_hash(&asset.sha256)
            || asset.bytes > MAX_ATTACHMENT_BYTES
            || !hashes.insert(&asset.sha256)
        {
            return Err(invalid("整库恢复清单中的原件无效或重复。"));
        }
        expected.insert(blob_entry(&asset.sha256), asset.bytes);
    }
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index)?;
        check_zip_file(&entry)?;
        if expected.remove(entry.name()?.as_ref()) != Some(entry.size()) {
            return Err(invalid("整库恢复包包含未声明、重复或大小不符的条目。"));
        }
    }
    if !expected.is_empty() {
        return Err(invalid("整库恢复包缺少数据库或原件。"));
    }
    Ok(manifest)
}

pub(super) fn inspect_automatic(
    path: &Path,
    workspace: &str,
    capture_id: &str,
    verify: bool,
) -> Result<RecoveryPoint> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(invalid("自动备份目标已被替换。"));
    }
    let mut zip = ZipArchive::new(File::open(path)?)?;
    let manifest = read_manifest(&mut zip)?;
    if manifest.capture.kind != RecoveryKind::Automatic
        || manifest.capture.id != capture_id
        || manifest.workspace.id != workspace
    {
        return Err(invalid("文件与自动备份记录不匹配。"));
    }
    if verify {
        let hash = transfer(zip.by_name(DATABASE)?, io::sink(), manifest.database.bytes)?;
        check_digest(&hash, &manifest.database)?;
        for asset in &manifest.blobs {
            let hash = transfer(
                zip.by_name(&blob_entry(&asset.sha256))?,
                io::sink(),
                asset.bytes,
            )?;
            check_digest(&hash, asset)?;
        }
    }
    Ok(point(&manifest, path, metadata.len()))
}

fn point(manifest: &Manifest, file: &Path, bytes: u64) -> RecoveryPoint {
    RecoveryPoint {
        id: manifest.capture.id.clone(),
        workspace_id: manifest.workspace.id.clone(),
        kind: manifest.capture.kind,
        created_at: manifest.capture.at,
        bytes,
        path: file.to_path_buf(),
    }
}
