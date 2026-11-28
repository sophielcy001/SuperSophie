// SPDX-License-Identifier: AGPL-3.0-or-later

//! Client side of personal sync (M6, protocol 1).
//!
//! Core never opens a network connection. A caller asks for the next push
//! request, sends it to the sync service, and hands the response back; pulls
//! work the same way. Each step commits in one local transaction, so an
//! interrupted exchange can simply be repeated: unsent operations keep their
//! IDs and the service replays results for operations it already accepted.
//!
//! Protocol 1 synchronizes ordinary pages: title, icon, parent, blocks, trash
//! state, attachments and cover. Attachment originals travel separately by
//! SHA-256. Databases and records stay local and are listed as skipped.

use crate::assets::{blob_directory, insert_attachment, read_attachments, valid_hash};
use crate::collections::{
    Collection, all_collections, check_synced_values, insert_synced_record, record_created_at,
    relation_fields, store_synced_collection,
};
use crate::integrations::derived::{ComputedChange, ReferenceChange, enqueue};
use crate::integrations::feed::{RangeSnapshot, record_archive};
use crate::model::new_id;
use crate::store::{commit_removed_relations, insert_page, insert_revision, read_page};
use crate::{
    Attachment, Block, Error, MAX_ATTACHMENT_BYTES, Page, PageCover, PageInfo, Result, Store,
    Workspace,
};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::io::Read;
use std::path::Path;
use zhiye_sync_protocol::{
    MAX_PUSH_OPERATIONS, MAX_TITLE_CHARS, ObjectChange, PROTOCOL_VERSION, PullResponse,
    PushOperation, PushRequest, PushResponse, PushResult, RemoteChange, SyncAttachment,
    SyncCollection, SyncCover, SyncPage, SyncRecord,
};

const CONFLICT_SUFFIX: &str = "（冲突副本）";

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<()> {
    // Re-running is harmless; SQLite stores the statements without IF NOT EXISTS.
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_state (
             slot INTEGER PRIMARY KEY CHECK (slot = 1),
             cursor INTEGER NOT NULL DEFAULT 0 CHECK (cursor >= 0),
             enabled INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),
             server_url TEXT,
             last_attempt_at INTEGER,
             last_success_at INTEGER,
             last_error TEXT
         );
         INSERT OR IGNORE INTO sync_state (slot) VALUES (1);
         CREATE TABLE IF NOT EXISTS sync_objects (
             object_id TEXT PRIMARY KEY NOT NULL,
             server_version INTEGER NOT NULL CHECK (server_version > 0),
             synced_hash TEXT,
             deleted INTEGER NOT NULL DEFAULT 0 CHECK (deleted IN (0, 1))
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS sync_outbox (
             object_id TEXT PRIMARY KEY NOT NULL,
             op_id TEXT NOT NULL UNIQUE,
             base_version INTEGER NOT NULL CHECK (base_version >= 0),
             sent_hash TEXT,
             payload_hash TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS sync_pending (
             object_id TEXT PRIMARY KEY NOT NULL,
             seq INTEGER NOT NULL,
             version INTEGER NOT NULL CHECK (version > 0),
             change TEXT NOT NULL CHECK (json_valid(change)),
             reason TEXT NOT NULL
         ) WITHOUT ROWID;",
    )?;
    tx.pragma_update(None, "user_version", 22)?;
    Ok(())
}

/// Forgets the sync stream, e.g. for a recovered copy with a new identity.
pub(crate) fn reset(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "DELETE FROM sync_objects; DELETE FROM sync_outbox; DELETE FROM sync_pending;
         UPDATE sync_state SET cursor = 0, enabled = 0, server_url = NULL,
             last_attempt_at = NULL, last_success_at = NULL, last_error = NULL WHERE slot = 1;",
    )?;
    Ok(())
}

/// A page that protocol 1 does not synchronize.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncSkip {
    pub page_id: String,
    pub title: String,
    /// `COLLECTION`, `RECORD`, `ORIGINAL_MISSING` (an attachment original is
    /// absent here) or `PARENT_SKIPPED`.
    pub reason: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPush {
    /// `None` when nothing is waiting to be sent.
    pub request: Option<PushRequest>,
    /// Further changes that did not fit into this request.
    pub remaining: usize,
    pub skipped: Vec<SyncSkip>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub page_id: String,
    /// `EDIT_EDIT`: both sides edited; the local version was kept as `copy_page_id`.
    /// `DELETE_EDIT`: the other device deleted a page edited here; the local page is kept.
    /// `EDIT_DELETE`: a page deleted here was edited elsewhere; the edited page is restored.
    /// `PARENT_CYCLE`: two moves formed a cycle; the page was moved to the top level.
    /// `REJECTED`: the service had a newer version; it arrives with the next pull.
    pub kind: &'static str,
    pub copy_page_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncPendingItem {
    pub object_id: String,
    pub version: u64,
    pub reason: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncReport {
    pub accepted: usize,
    pub applied: usize,
    pub conflicts: Vec<SyncConflict>,
    /// Remote changes that cannot be applied yet, retried on every pull.
    pub pending: Vec<SyncPendingItem>,
    pub cursor: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    /// Service this workspace's sync state belongs to.
    pub server_url: Option<String>,
    pub last_attempt_at: Option<i64>,
    pub last_success_at: Option<i64>,
    pub last_error: Option<String>,
    pub cursor: u64,
    pub tracked: usize,
    pub unsent: usize,
    pub pending: Vec<SyncPendingItem>,
    pub skipped: Vec<SyncSkip>,
}

impl Store {
    /// Creates an empty local workspace that will receive an existing synced workspace.
    pub fn create_synced_workspace(&self, id: &str, name: &str) -> Result<Workspace> {
        if !crate::model::valid_id(id) {
            return Err(Error::new("WORKSPACE_ID", "同步工作空间的标识无效。"));
        }
        let workspace = Workspace {
            id: id.into(),
            name: crate::model::name(name, 64, None)?,
        };
        self.publish_workspace(&workspace, |_, _| Ok(()))?;
        Ok(workspace)
    }

    pub fn sync_status(&self, workspace: &str) -> Result<SyncStatus> {
        let mut db = self.connection(workspace)?;
        let blobs = self.sync_blob_directory(workspace)?;
        let tx = db.transaction()?;
        let scan = Scan::read(&tx, &blobs)?;
        let unsent = scan.changes()?.len();
        let settings = settings(&tx)?;
        Ok(SyncStatus {
            enabled: settings.enabled,
            server_url: settings.server_url,
            last_attempt_at: settings.last_attempt_at,
            last_success_at: settings.last_success_at,
            last_error: settings.last_error,
            cursor: cursor(&tx)?,
            tracked: tx.query_row("SELECT count(*) FROM sync_objects", [], |row| {
                row.get::<_, i64>(0)
            })? as usize,
            unsent,
            pending: pending(&tx)?,
            skipped: scan.skipped,
        })
    }

    /// Cheap cursor read for paging through pulls.
    pub fn sync_cursor(&self, workspace: &str) -> Result<u64> {
        cursor(&self.connection(workspace)?)
    }

    /// Enables sync with `server_url`. Switching to another service forgets
    /// the previous stream, so every local page is sent to the new service.
    pub fn enable_sync(&self, workspace: &str, server_url: &str) -> Result<SyncStatus> {
        if server_url.is_empty() || server_url.chars().count() > 2048 {
            return Err(Error::new("SYNC_SERVER_URL", "同步服务地址无效。"));
        }
        {
            let mut db = self.connection(workspace)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let previous = settings(&tx)?.server_url;
            if previous
                .as_deref()
                .is_some_and(|previous| previous != server_url)
            {
                reset(&tx)?;
            }
            tx.execute(
                "UPDATE sync_state SET enabled = 1, server_url = ?1 WHERE slot = 1",
                [server_url],
            )?;
            tx.commit()?;
        }
        self.sync_status(workspace)
    }

    /// Stops automatic sync. The stream is kept, so enabling it again with the
    /// same service continues where it stopped.
    pub fn disable_sync(&self, workspace: &str) -> Result<()> {
        self.connection(workspace)?
            .execute("UPDATE sync_state SET enabled = 0 WHERE slot = 1", [])?;
        Ok(())
    }

    /// Records the outcome of one sync run for display.
    pub fn record_sync_attempt(&self, workspace: &str, error: Option<&str>) -> Result<()> {
        let now = crate::store::now();
        let error: Option<String> = error.map(|message| message.chars().take(500).collect());
        self.connection(workspace)?.execute(
            "UPDATE sync_state SET last_attempt_at = ?1,
                 last_success_at = CASE WHEN ?2 IS NULL THEN ?1 ELSE last_success_at END,
                 last_error = ?2 WHERE slot = 1",
            params![now, error],
        )?;
        Ok(())
    }

    /// Builds the next push request. Repeating the call before the response is
    /// applied returns the same operation IDs for unchanged content.
    pub fn sync_prepare_push(&self, workspace: &str) -> Result<SyncPush> {
        let mut db = self.connection(workspace)?;
        let blobs = self.sync_blob_directory(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let scan = Scan::read(&tx, &blobs)?;
        let changes = scan.changes()?;
        let remaining = changes.len().saturating_sub(MAX_PUSH_OPERATIONS);
        let mut operations = Vec::new();
        for change in changes.into_iter().take(MAX_PUSH_OPERATIONS) {
            operations.push(outbox_operation(&tx, change)?);
        }
        tx.commit()?;
        Ok(SyncPush {
            request: (!operations.is_empty()).then_some(PushRequest {
                protocol: PROTOCOL_VERSION,
                operations,
            }),
            remaining,
            skipped: scan.skipped,
        })
    }

    /// Records the service's answer to a request from [`Store::sync_prepare_push`].
    pub fn sync_apply_push(&self, workspace: &str, response: &PushResponse) -> Result<SyncReport> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = SyncReport::default();
        for result in &response.results {
            let sent: Option<(String, Option<String>)> = tx
                .query_row(
                    "SELECT object_id, sent_hash FROM sync_outbox WHERE op_id = ?1",
                    [result.op_id()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((object_id, sent_hash)) = sent else {
                continue; // Already applied by an earlier, interrupted attempt.
            };
            tx.execute("DELETE FROM sync_outbox WHERE op_id = ?1", [result.op_id()])?;
            match result {
                PushResult::Accepted {
                    object_id: accepted,
                    version,
                    ..
                } if *accepted == object_id => {
                    if *version > known_version(&tx, &object_id)?.unwrap_or(0) {
                        let deleted = sent_hash.is_none();
                        set_object(&tx, &object_id, *version, sent_hash, deleted)?;
                    }
                    report.accepted += 1;
                }
                PushResult::Accepted { .. } => {
                    return Err(Error::new(
                        "SYNC_RESPONSE",
                        "同步服务的回执与发送内容不一致。",
                    ));
                }
                PushResult::Conflict { .. } => report.conflicts.push(SyncConflict {
                    page_id: object_id,
                    kind: "REJECTED",
                    copy_page_id: None,
                }),
            }
        }
        report.cursor = cursor(&tx)?;
        report.pending = pending(&tx)?;
        tx.commit()?;
        Ok(report)
    }

    /// Applies one page of remote changes and advances the cursor.
    pub fn sync_apply_pull(&self, workspace: &str, response: &PullResponse) -> Result<SyncReport> {
        let mut db = self.connection(workspace)?;
        let blobs = self.sync_blob_directory(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut report = SyncReport::default();
        let mut queue: Vec<RemoteChange> = Vec::new();
        {
            let mut query = tx
                .prepare("SELECT seq, object_id, version, change FROM sync_pending ORDER BY seq")?;
            for row in query.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })? {
                let (seq, object_id, version, change) = row?;
                queue.push(RemoteChange {
                    seq: seq as u64,
                    object_id,
                    version: version as u64,
                    device_id: String::new(),
                    op_id: String::new(),
                    change: serde_json::from_str(&change)?,
                });
            }
        }
        queue.extend(response.changes.iter().cloned());
        // Keep only the newest change per object, in service order.
        let mut newest = HashMap::new();
        for (index, change) in queue.iter().enumerate() {
            let entry = newest.entry(change.object_id.clone()).or_insert(index);
            if queue[*entry].version < change.version {
                *entry = index;
            }
        }
        let mut waiting: Vec<RemoteChange> = queue
            .into_iter()
            .enumerate()
            .filter(|(index, change)| newest.get(&change.object_id) == Some(index))
            .map(|(_, change)| change)
            .collect();
        waiting.sort_by_key(|change| change.seq);

        // Records arriving in this batch may relate to each other in both
        // directions. Accept them as relation targets up front; if any of them
        // fails to land, its referrers would point nowhere, so the batch is
        // redone without trusting that target.
        let mut arriving: HashSet<String> = waiting
            .iter()
            .filter(|change| {
                matches!(&change.change, ObjectChange::Upsert { page } if page.record.is_some())
            })
            .map(|change| change.object_id.clone())
            .collect();
        let (batch, waiting, failures) = loop {
            tx.execute_batch("SAVEPOINT sync_batch")?;
            let attempt = apply_batch(&tx, waiting.clone(), &blobs, &arriving)?;
            let dangling: HashSet<String> = tx
                .prepare(
                    "SELECT DISTINCT e.target_page_id FROM relation_edges e
                     LEFT JOIN collection_records r ON r.page_id = e.target_page_id
                     WHERE r.page_id IS NULL",
                )?
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            if dangling.is_empty() {
                tx.execute_batch("RELEASE sync_batch")?;
                break attempt;
            }
            tx.execute_batch("ROLLBACK TO sync_batch; RELEASE sync_batch")?;
            let before = arriving.len();
            arriving.retain(|id| !dangling.contains(id));
            if arriving.len() == before {
                return Err(Error::new(
                    "SYNC_CHANGE",
                    "同步后的数据库关联指向不存在的记录。",
                ));
            }
        };
        report.applied = batch.applied;
        report.conflicts = batch.conflicts;
        for change in &waiting {
            tx.execute(
                "INSERT INTO sync_pending (object_id, seq, version, change, reason)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (object_id) DO UPDATE SET seq = excluded.seq,
                     version = excluded.version, change = excluded.change, reason = excluded.reason",
                params![
                    change.object_id,
                    change.seq as i64,
                    change.version as i64,
                    serde_json::to_string(&change.change)?,
                    failures.get(&change.object_id).map_or("", String::as_str)
                ],
            )?;
        }
        let next = response.next_seq.max(cursor(&tx)?);
        tx.execute(
            "UPDATE sync_state SET cursor = ?1 WHERE slot = 1",
            [next as i64],
        )?;
        report.cursor = next;
        report.pending = pending(&tx)?;
        tx.commit()?;
        Ok(report)
    }

    fn sync_blob_directory(&self, workspace: &str) -> Result<std::path::PathBuf> {
        blob_directory(&self.data_root().join(workspace), true)
    }

    /// Originals among `hashes` that are not stored in this workspace yet.
    pub fn sync_missing_blobs(&self, workspace: &str, hashes: &[String]) -> Result<Vec<String>> {
        self.connection(workspace)?;
        let blobs = self.sync_blob_directory(workspace)?;
        let mut missing = Vec::new();
        for hash in hashes {
            if !valid_hash(hash) {
                return Err(invalid_blob_hash());
            }
            if !blobs.join(hash).is_file() && !missing.contains(hash) {
                missing.push(hash.clone());
            }
        }
        Ok(missing)
    }

    /// Stores a downloaded original after checking its SHA-256.
    pub fn sync_store_blob(&self, workspace: &str, hash: &str, source: impl Read) -> Result<()> {
        if !valid_hash(hash) {
            return Err(invalid_blob_hash());
        }
        self.connection(workspace)?;
        let _lease = self.pin_blobs()?;
        let blobs = self.sync_blob_directory(workspace)?;
        let target = blobs.join(hash);
        if target.is_file() {
            return Ok(());
        }
        let mut staging = tempfile::NamedTempFile::new_in(&blobs)?;
        let content = crate::assets::transfer(source, &mut staging, MAX_ATTACHMENT_BYTES)?;
        if content.sha256 != hash {
            return Err(Error::new(
                "SYNC_BLOB",
                "下载的附件原件与标识不一致，已丢弃。",
            ));
        }
        staging.as_file().sync_all()?;
        match staging.persist_noclobber(&target) {
            Ok(_) => Ok(()),
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error.into()),
        }
    }

    /// Opens a stored original for upload.
    pub fn sync_open_blob(&self, workspace: &str, hash: &str) -> Result<File> {
        if !valid_hash(hash) {
            return Err(invalid_blob_hash());
        }
        self.connection(workspace)?;
        let path = self.sync_blob_directory(workspace)?.join(hash);
        File::open(&path).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                Error::new("ATTACHMENT_MISSING", "找不到附件原件。")
            } else {
                error.into()
            }
        })
    }
}

fn invalid_blob_hash() -> Error {
    Error::new("SYNC_BLOB", "附件原件标识无效。")
}

/// One attempt at applying remote changes: retries what failed until a
/// round makes no progress, so a child that arrived before its parent still
/// lands in the same pull. Returns the outcome, what is still waiting and why.
fn apply_batch(
    tx: &Transaction<'_>,
    mut waiting: Vec<RemoteChange>,
    blobs: &Path,
    arriving: &HashSet<String>,
) -> Result<(SyncReport, Vec<RemoteChange>, HashMap<String, String>)> {
    let mut report = SyncReport::default();
    let mut failures = HashMap::new();
    loop {
        let before = waiting.len();
        let mut retry = Vec::new();
        for change in waiting {
            tx.execute_batch("SAVEPOINT sync_change")?;
            match apply_remote(tx, &change, blobs, arriving) {
                Ok(outcome) => {
                    tx.execute(
                        "DELETE FROM sync_pending WHERE object_id = ?1",
                        [&change.object_id],
                    )?;
                    tx.execute_batch("RELEASE sync_change")?;
                    failures.remove(&change.object_id);
                    if outcome.applied {
                        report.applied += 1;
                    }
                    report.conflicts.extend(outcome.conflict);
                }
                Err(error) => {
                    tx.execute_batch("ROLLBACK TO sync_change; RELEASE sync_change")?;
                    if fatal(&error) {
                        return Err(error);
                    }
                    failures.insert(change.object_id.clone(), error.message);
                    retry.push(change);
                }
            }
        }
        waiting = retry;
        if waiting.is_empty() || waiting.len() == before {
            return Ok((report, waiting, failures));
        }
    }
}

/// Storage and I/O failures abort the pull; content problems park the change.
fn fatal(error: &Error) -> bool {
    matches!(
        error.code,
        "DATABASE" | "DATABASE_OPEN" | "IO" | "STORAGE_FULL" | "FORMAT"
    )
}

struct Tracked {
    server_version: u64,
    /// Fingerprint of the object as last agreed with the service.
    synced_hash: Option<String>,
    deleted: bool,
}

/// Local pages, database definitions and their sync bookkeeping.
struct Scan {
    pages: Vec<Page>,
    attachments: HashMap<String, Vec<Attachment>>,
    collections: Vec<Collection>,
    record_created: BTreeMap<String, i64>,
    tracked: HashMap<String, Tracked>,
    excluded: HashSet<String>,
    skipped: Vec<SyncSkip>,
}

enum LocalChange {
    Upsert {
        object_id: String,
        change: Box<ObjectChange>,
        hash: String,
        base_version: u64,
    },
    Delete {
        object_id: String,
        base_version: u64,
    },
}

impl Scan {
    fn read(db: &Connection, blobs: &Path) -> Result<Self> {
        let ids: Vec<String> = db
            .prepare("SELECT id FROM pages ORDER BY rowid")?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let pages = ids
            .iter()
            .map(|id| read_page(db, id))
            .collect::<Result<Vec<_>>>()?;
        let mut attachments: HashMap<String, Vec<Attachment>> = HashMap::new();
        for attachment in read_attachments(db, None)? {
            attachments
                .entry(attachment.page_id.clone())
                .or_default()
                .push(attachment);
        }
        let mut tracked = HashMap::new();
        for row in db
            .prepare("SELECT object_id, server_version, synced_hash, deleted FROM sync_objects")?
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    Tracked {
                        server_version: row.get::<_, i64>(1)? as u64,
                        synced_hash: row.get(2)?,
                        deleted: row.get(3)?,
                    },
                ))
            })?
        {
            let (id, entry) = row?;
            tracked.insert(id, entry);
        }

        let mut reasons: HashMap<&str, &'static str> = HashMap::new();
        for page in &pages {
            let originals_missing = attachments
                .get(&page.info.id)
                .is_some_and(|items| items.iter().any(|item| !blobs.join(&item.sha256).is_file()));
            if originals_missing {
                reasons.insert(&page.info.id, "ORIGINAL_MISSING");
            }
        }
        // Children of skipped pages cannot be placed on another device.
        let parents: HashMap<&str, Option<&str>> = pages
            .iter()
            .map(|page| (page.info.id.as_str(), page.info.parent_id.as_deref()))
            .collect();
        for page in &pages {
            let mut parent = page.info.parent_id.as_deref();
            let mut steps = 0;
            while let Some(id) = parent {
                if reasons.contains_key(id) && !reasons.contains_key(page.info.id.as_str()) {
                    reasons.insert(&page.info.id, "PARENT_SKIPPED");
                    break;
                }
                steps += 1;
                if steps > pages.len() {
                    break;
                }
                parent = parents.get(id).copied().flatten();
            }
        }
        let skipped = pages
            .iter()
            .filter_map(|page| {
                reasons.get(page.info.id.as_str()).map(|reason| SyncSkip {
                    page_id: page.info.id.clone(),
                    title: page.info.title.clone(),
                    reason,
                })
            })
            .collect();
        let excluded = reasons.keys().map(|id| (*id).to_owned()).collect();
        Ok(Self {
            pages,
            attachments,
            collections: all_collections(db)?,
            record_created: record_created_at(db)?,
            tracked,
            excluded,
            skipped,
        })
    }

    fn unsent(&self, id: &str, hash: &str) -> bool {
        !self
            .tracked
            .get(id)
            .is_some_and(|entry| entry.synced_hash.as_deref() == Some(hash))
    }

    fn base_version(&self, id: &str) -> u64 {
        self.tracked.get(id).map_or(0, |entry| entry.server_version)
    }

    /// Unsent local changes. Pages precede their children, and a database
    /// definition follows its container page and precedes its records, so the
    /// service order lets other devices place each object on arrival.
    fn changes(&self) -> Result<Vec<LocalChange>> {
        let depth = depths(&self.pages);
        let mut upserts = Vec::new();
        for page in self
            .pages
            .iter()
            .filter(|page| !self.excluded.contains(&page.info.id))
        {
            let snapshot = sync_page(
                page,
                self.attachments
                    .get(&page.info.id)
                    .map_or(&[], Vec::as_slice),
                self.record_created.get(&page.info.id).copied(),
            )?;
            let hash = fingerprint(&snapshot)?;
            if !self.unsent(&page.info.id, &hash) {
                continue;
            }
            upserts.push((
                depth.get(page.info.id.as_str()).copied().unwrap_or(0) * 2,
                LocalChange::Upsert {
                    object_id: page.info.id.clone(),
                    change: Box::new(ObjectChange::Upsert { page: snapshot }),
                    hash,
                    base_version: self.base_version(&page.info.id),
                },
            ));
        }
        for collection in self
            .collections
            .iter()
            .filter(|collection| !self.excluded.contains(&collection.container_page_id))
        {
            let snapshot = sync_collection(collection)?;
            let hash = fingerprint(&snapshot)?;
            if !self.unsent(&collection.id, &hash) {
                continue;
            }
            let container = depth
                .get(collection.container_page_id.as_str())
                .copied()
                .unwrap_or(0);
            upserts.push((
                container * 2 + 1,
                LocalChange::Upsert {
                    object_id: collection.id.clone(),
                    change: Box::new(ObjectChange::Collection {
                        collection: snapshot,
                    }),
                    hash,
                    base_version: self.base_version(&collection.id),
                },
            ));
        }
        upserts.sort_by_key(|(order, _)| *order);
        let present: HashSet<&str> = self
            .pages
            .iter()
            .map(|page| page.info.id.as_str())
            .chain(self.collections.iter().map(|item| item.id.as_str()))
            .collect();
        let mut changes: Vec<LocalChange> = upserts.into_iter().map(|(_, change)| change).collect();
        let mut deletes: Vec<_> = self
            .tracked
            .iter()
            .filter(|(id, entry)| !entry.deleted && !present.contains(id.as_str()))
            .map(|(id, entry)| LocalChange::Delete {
                object_id: id.clone(),
                base_version: entry.server_version,
            })
            .collect();
        deletes.sort_by(|left, right| object_id(left).cmp(object_id(right)));
        changes.extend(deletes);
        Ok(changes)
    }
}

fn object_id(change: &LocalChange) -> &str {
    match change {
        LocalChange::Upsert { object_id, .. } | LocalChange::Delete { object_id, .. } => object_id,
    }
}

fn depths(pages: &[Page]) -> HashMap<&str, usize> {
    let parents: HashMap<&str, Option<&str>> = pages
        .iter()
        .map(|page| (page.info.id.as_str(), page.info.parent_id.as_deref()))
        .collect();
    pages
        .iter()
        .map(|page| {
            let mut depth = 0;
            let mut parent = page.info.parent_id.as_deref();
            while let Some(id) = parent {
                depth += 1;
                if depth > pages.len() {
                    break;
                }
                parent = parents.get(id).copied().flatten();
            }
            (page.info.id.as_str(), depth)
        })
        .collect()
}

fn sync_page(page: &Page, attachments: &[Attachment], created_at: Option<i64>) -> Result<SyncPage> {
    Ok(SyncPage {
        parent_id: page.info.parent_id.clone(),
        title: page.info.title.clone(),
        icon: page.info.icon.clone(),
        blocks: page
            .blocks
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<_>>()?,
        trash_root_id: page.info.trash_root_id.clone(),
        attachments: attachments
            .iter()
            .map(|item| SyncAttachment {
                id: item.id.clone(),
                name: item.name.clone(),
                sha256: item.sha256.clone(),
                bytes: item.bytes,
                media_type: item.media_type.clone(),
                created_at: item.created_at,
            })
            .collect(),
        cover: page.cover.as_ref().map(|cover| SyncCover {
            attachment_id: cover.attachment_id.clone(),
            media_type: cover.media_type.clone(),
        }),
        record: page.properties.as_ref().map(|values| SyncRecord {
            created_at: created_at.unwrap_or(page.info.updated_at),
            properties: values.clone().into_iter().collect(),
        }),
    })
}

fn sync_collection(collection: &Collection) -> Result<SyncCollection> {
    Ok(SyncCollection {
        container_page_id: collection.container_page_id.clone(),
        properties: collection
            .properties
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<_>>()?,
        views: collection
            .views
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<_>>()?,
    })
}

fn local_snapshot(db: &Connection, page: &Page) -> Result<SyncPage> {
    let created_at = db
        .query_row(
            "SELECT created_at FROM collection_records WHERE page_id = ?1",
            [&page.info.id],
            |row| row.get(0),
        )
        .optional()?;
    sync_page(
        page,
        &read_attachments(db, Some(&page.info.id))?,
        created_at,
    )
}

fn local_collection(db: &Connection, id: &str) -> Result<Option<Collection>> {
    Ok(all_collections(db)?
        .into_iter()
        .find(|collection| collection.id == id))
}

/// Stable digest of an object's synchronized state.
fn fingerprint(value: &impl Serialize) -> Result<String> {
    Ok(hex(&Sha256::digest(serde_json::to_vec(value)?)))
}

fn outbox_operation(tx: &Transaction<'_>, change: LocalChange) -> Result<PushOperation> {
    let (object_id, base_version, sent_hash, change) = match change {
        LocalChange::Upsert {
            object_id,
            change,
            hash,
            base_version,
        } => (object_id, base_version, Some(hash), *change),
        LocalChange::Delete {
            object_id,
            base_version,
        } => (object_id, base_version, None, ObjectChange::Delete),
    };
    let hash = hex(&Sha256::digest(serde_json::to_vec(&(
        &object_id,
        base_version,
        &change,
    ))?));
    let existing: Option<(String, String)> = tx
        .query_row(
            "SELECT op_id, payload_hash FROM sync_outbox WHERE object_id = ?1",
            [&object_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let op_id = match existing {
        Some((op_id, existing_hash)) if existing_hash == hash => op_id,
        _ => {
            let op_id = new_id();
            tx.execute(
                "INSERT INTO sync_outbox (object_id, op_id, base_version, sent_hash, payload_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (object_id) DO UPDATE SET op_id = excluded.op_id,
                     base_version = excluded.base_version, sent_hash = excluded.sent_hash,
                     payload_hash = excluded.payload_hash",
                params![object_id, op_id, base_version as i64, sent_hash, hash],
            )?;
            op_id
        }
    };
    Ok(PushOperation {
        op_id,
        object_id,
        base_version,
        change,
    })
}

struct Outcome {
    applied: bool,
    conflict: Option<SyncConflict>,
}

impl Outcome {
    fn applied() -> Self {
        Self {
            applied: true,
            conflict: None,
        }
    }
}

/// What this device holds under an object ID.
enum Local {
    Page(Box<Page>),
    Collection(Box<Collection>),
    Missing,
}

fn apply_remote(
    tx: &Transaction<'_>,
    change: &RemoteChange,
    blobs: &Path,
    arriving: &HashSet<String>,
) -> Result<Outcome> {
    let id = change.object_id.as_str();
    let known = tracked(tx, id)?;
    if known
        .as_ref()
        .is_some_and(|entry| entry.server_version >= change.version)
    {
        return Ok(Outcome {
            applied: false,
            conflict: None,
        });
    }
    let local = match read_optional_page(tx, id)? {
        Some(page) => Local::Page(Box::new(page)),
        None => match local_collection(tx, id)? {
            Some(collection) => Local::Collection(Box::new(collection)),
            None => Local::Missing,
        },
    };
    let local_hash = match &local {
        Local::Page(page) => Some(fingerprint(&local_snapshot(tx, page)?)?),
        Local::Collection(collection) => Some(fingerprint(&sync_collection(collection)?)?),
        Local::Missing => None,
    };
    let dirty = match (&local_hash, &known) {
        (Some(hash), Some(entry)) => entry.synced_hash.as_deref() != Some(hash.as_str()),
        (Some(_), None) => true,
        (None, _) => false,
    };
    // A delete prepared here is superseded by whatever the service now holds.
    tx.execute("DELETE FROM sync_outbox WHERE object_id = ?1", [id])?;
    let locally_deleted = known.as_ref().is_some_and(|entry| !entry.deleted);

    match &change.change {
        ObjectChange::Upsert { page: remote } => {
            remote
                .validate(id)
                .map_err(|error| Error::new("SYNC_CHANGE", error.0))?;
            let blocks: Vec<Block> = remote
                .blocks
                .iter()
                .cloned()
                .map(serde_json::from_value)
                .collect::<serde_json::Result<_>>()
                .map_err(|_| Error::new("SYNC_CHANGE", "远端页面的正文块格式无效。"))?;
            let remote_hash = fingerprint(remote)?;
            match local {
                Local::Collection(_) => Err(Error::new(
                    "SYNC_CHANGE",
                    "该标识在本机属于数据库定义，不能写入页面。",
                )),
                Local::Missing => {
                    let page = insert_remote(tx, id, remote, blocks, blobs, arriving)?;
                    let synced = agreed_hash(tx, &page, remote, &remote_hash, false)?;
                    set_object(tx, id, change.version, Some(synced), false)?;
                    Ok(Outcome {
                        applied: true,
                        conflict: locally_deleted.then(|| SyncConflict {
                            page_id: id.into(),
                            kind: "EDIT_DELETE",
                            copy_page_id: None,
                        }),
                    })
                }
                Local::Page(before) => {
                    let copy = if dirty && local_hash.as_deref() != Some(remote_hash.as_str()) {
                        Some(conflict_copy(tx, &before)?)
                    } else {
                        None
                    };
                    let (after, cycle) =
                        update_local(tx, *before, remote, blocks, blobs, arriving)?;
                    let synced = agreed_hash(tx, &after, remote, &remote_hash, cycle)?;
                    set_object(tx, id, change.version, Some(synced), false)?;
                    let conflict = match (copy, cycle) {
                        (Some(copy), _) => Some(SyncConflict {
                            page_id: id.into(),
                            kind: "EDIT_EDIT",
                            copy_page_id: Some(copy),
                        }),
                        (None, true) => Some(SyncConflict {
                            page_id: id.into(),
                            kind: "PARENT_CYCLE",
                            copy_page_id: None,
                        }),
                        (None, false) => None,
                    };
                    Ok(Outcome {
                        applied: true,
                        conflict,
                    })
                }
            }
        }
        ObjectChange::Collection { collection: remote } => {
            remote
                .validate(id)
                .map_err(|error| Error::new("SYNC_CHANGE", error.0))?;
            let remote_hash = fingerprint(remote)?;
            let incoming = Collection {
                id: id.into(),
                container_page_id: remote.container_page_id.clone(),
                schema_revision: 1,
                properties: remote
                    .properties
                    .iter()
                    .cloned()
                    .map(serde_json::from_value)
                    .collect::<serde_json::Result<_>>()
                    .map_err(|_| Error::new("SYNC_CHANGE", "远端数据库的属性定义无法识别。"))?,
                views: remote
                    .views
                    .iter()
                    .cloned()
                    .map(serde_json::from_value)
                    .collect::<serde_json::Result<_>>()
                    .map_err(|_| Error::new("SYNC_CHANGE", "远端数据库的视图无法识别。"))?,
            };
            if read_optional_page(tx, &incoming.container_page_id)?.is_none() {
                return Err(Error::new(
                    "SYNC_PARENT",
                    "数据库的容器页面尚未同步到本机。",
                ));
            }
            let container = incoming.container_page_id.clone();
            let (merged, extra) = match local {
                Local::Page(_) => {
                    return Err(Error::new(
                        "SYNC_CHANGE",
                        "该标识在本机属于页面，不能写入数据库定义。",
                    ));
                }
                Local::Collection(current)
                    if dirty && local_hash.as_deref() != Some(remote_hash.as_str()) =>
                {
                    merge_collection(incoming, &current)
                }
                _ => (incoming, false),
            };
            store_synced_collection(tx, merged)?;
            let after = local_collection(tx, id)?
                .ok_or_else(|| Error::new("SYNC_CHANGE", "数据库定义未能写入。"))?;
            let synced = if extra {
                remote_hash
            } else {
                fingerprint(&sync_collection(&after)?)?
            };
            set_object(tx, id, change.version, Some(synced), false)?;
            Ok(Outcome {
                applied: true,
                conflict: extra.then_some(SyncConflict {
                    page_id: container,
                    kind: "SCHEMA_MERGED",
                    copy_page_id: None,
                }),
            })
        }
        ObjectChange::Delete => match local {
            Local::Missing => {
                set_object(tx, id, change.version, None, true)?;
                Ok(Outcome::applied())
            }
            Local::Page(_) | Local::Collection(_) if dirty => {
                // Keep the local edit; the next push restores it on the service.
                set_object(tx, id, change.version, None, true)?;
                Ok(Outcome {
                    applied: false,
                    conflict: Some(SyncConflict {
                        page_id: id.into(),
                        kind: "DELETE_EDIT",
                        copy_page_id: None,
                    }),
                })
            }
            Local::Collection(collection) => {
                // The container page carries the deletion into the trash.
                let synced = fingerprint(&sync_collection(&collection)?)?;
                set_object(tx, id, change.version, Some(synced), true)?;
                Ok(Outcome::applied())
            }
            Local::Page(before) => {
                // Deleted elsewhere: move to the local trash rather than erase.
                let after = if before.info.trash_root_id.is_none() {
                    let mut remote = local_snapshot(tx, &before)?;
                    remote.trash_root_id = Some(id.into());
                    let blocks = before.blocks.clone();
                    update_local(tx, *before, &remote, blocks, blobs, arriving)?.0
                } else {
                    *before
                };
                let synced = fingerprint(&local_snapshot(tx, &after)?)?;
                set_object(tx, id, change.version, Some(synced), true)?;
                Ok(Outcome::applied())
            }
        },
    }
}

/// Merges a definition changed on both sides: the service's properties and
/// views win where both have the same ID; items only known here are kept.
/// Returns whether anything local was kept.
fn merge_collection(mut incoming: Collection, local: &Collection) -> (Collection, bool) {
    let mut extra = false;
    for field in &local.properties {
        if !incoming.properties.iter().any(|item| item.id == field.id) {
            incoming.properties.push(field.clone());
            extra = true;
        }
    }
    for view in &local.views {
        if !incoming.views.iter().any(|item| item.id == view.id) {
            incoming.views.push(view.clone());
            extra = true;
        }
    }
    (incoming, extra)
}

/// The fingerprint to record as agreed with the service after applying
/// `remote`. Normally the local result, so formatting differences never cause
/// a resend. When the local page keeps something the service lacks (an
/// attachment only known here, or a parent changed to break a cycle) the
/// service's fingerprint is kept, so the next push sends the local state.
fn agreed_hash(
    tx: &Transaction<'_>,
    page: &Page,
    remote: &SyncPage,
    remote_hash: &str,
    cycle: bool,
) -> Result<String> {
    let local = local_snapshot(tx, page)?;
    let extra = local
        .attachments
        .iter()
        .any(|item| !remote.attachments.iter().any(|known| known.id == item.id));
    if cycle || extra {
        Ok(remote_hash.to_owned())
    } else {
        fingerprint(&local)
    }
}

/// Checks that the parent can hold the incoming page and returns the database
/// a record belongs to.
fn ensure_parent(
    tx: &Transaction<'_>,
    parent: Option<&str>,
    record: bool,
) -> Result<Option<String>> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    read_page(tx, parent).map_err(|_| Error::new("SYNC_PARENT", "上级页面尚未同步到本机。"))?;
    let collection = crate::collections::for_parent(tx, Some(parent))?;
    match (collection, record) {
        (Some(collection), true) => Ok(Some(collection)),
        (None, true) => Err(Error::new(
            "SYNC_SCHEMA",
            "记录所属的数据库定义尚未同步到本机。",
        )),
        (Some(_), false) => Err(Error::new(
            "SYNC_CHANGE",
            "数据库中只能放记录，远端页面却不是记录。",
        )),
        (None, false) => Ok(None),
    }
}

fn ensure_trash_root(tx: &Transaction<'_>, id: &str, root: Option<&str>) -> Result<()> {
    match root {
        Some(root) if root != id => read_page(tx, root)
            .map(drop)
            .map_err(|_| Error::new("SYNC_PARENT", "回收站根页面尚未同步到本机。")),
        _ => Ok(()),
    }
}

fn page_cover(remote: &SyncPage) -> Option<PageCover> {
    remote.cover.as_ref().map(|cover| PageCover {
        attachment_id: cover.attachment_id.clone(),
        media_type: cover.media_type.clone(),
    })
}

fn record_values(remote: &SyncPage) -> Option<BTreeMap<String, serde_json::Value>> {
    remote
        .record
        .as_ref()
        .map(|record| record.properties.clone().into_iter().collect())
}

/// Adds remote attachments and applies renames. Attachments are never removed
/// here: earlier revisions may still refer to them.
fn merge_attachments(
    tx: &Transaction<'_>,
    page: &str,
    remote: &[SyncAttachment],
    blobs: &Path,
) -> Result<()> {
    for item in remote {
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT page_id, name FROM attachments WHERE id = ?1",
                [&item.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match existing {
            Some((owner, _)) if owner != page => {
                return Err(Error::new(
                    "SYNC_CHANGE",
                    "远端附件的标识已属于本机的其他页面。",
                ));
            }
            Some((_, name)) if name == item.name => {}
            Some(_) => {
                let name = free_attachment_name(tx, page, &item.id, &item.name, &item.sha256)?;
                tx.execute(
                    "UPDATE attachments SET name = ?1 WHERE id = ?2",
                    params![name, item.id],
                )?;
            }
            None => {
                if !blobs.join(&item.sha256).is_file() {
                    return Err(Error::new("SYNC_BLOB", "附件原件尚未下载到本机。"));
                }
                let attachment = Attachment {
                    id: item.id.clone(),
                    page_id: page.into(),
                    name: free_attachment_name(tx, page, &item.id, &item.name, &item.sha256)?,
                    sha256: item.sha256.clone(),
                    bytes: item.bytes,
                    media_type: item.media_type.clone(),
                    created_at: item.created_at,
                };
                insert_attachment(tx, &attachment)?;
            }
        }
    }
    Ok(())
}

/// Keeps (page, name, content) unique when both devices added the same file.
fn free_attachment_name(
    tx: &Transaction<'_>,
    page: &str,
    id: &str,
    name: &str,
    sha256: &str,
) -> Result<String> {
    let taken = |candidate: &str| -> Result<bool> {
        Ok(tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM attachments WHERE page_id = ?1 AND name = ?2
             AND sha256 = ?3 AND id <> ?4)",
            params![page, candidate, sha256, id],
            |row| row.get(0),
        )?)
    };
    if !taken(name)? {
        return Ok(name.into());
    }
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    for index in 2..1000 {
        let candidate = format!("{stem} ({index}){extension}");
        if candidate.chars().count() <= 255 && !taken(&candidate)? {
            return Ok(candidate);
        }
    }
    Err(Error::new("SYNC_CHANGE", "无法为同名附件分配名称。"))
}

fn insert_remote(
    tx: &Transaction<'_>,
    id: &str,
    remote: &SyncPage,
    blocks: Vec<Block>,
    blobs: &Path,
    arriving: &HashSet<String>,
) -> Result<Page> {
    let values = record_values(remote);
    let collection = ensure_parent(tx, remote.parent_id.as_deref(), values.is_some())?;
    ensure_trash_root(tx, id, remote.trash_root_id.as_deref())?;
    if let (Some(collection), Some(values)) = (&collection, &values) {
        check_synced_values(tx, collection, values, arriving)?;
    }
    let page = Page {
        info: PageInfo {
            id: id.into(),
            parent_id: remote.parent_id.clone(),
            title: remote.title.clone(),
            revision: 1,
            updated_at: crate::store::now(),
            icon: remote.icon.clone(),
            trash_root_id: remote.trash_root_id.clone(),
        },
        blocks,
        properties: values,
        historical_properties: Vec::new(),
        cover: page_cover(remote),
    };
    insert_fresh(tx, page, |tx, page| {
        merge_attachments(tx, id, &remote.attachments, blobs)?;
        if let (Some(collection), Some(record)) = (&collection, &remote.record) {
            insert_synced_record(tx, page, collection, record.created_at)?;
        }
        Ok(())
    })
}

/// Inserts a page, then its attachments and record membership, then checks
/// the references.
fn insert_fresh(
    tx: &Transaction<'_>,
    page: Page,
    attach: impl FnOnce(&Transaction<'_>, &Page) -> Result<()>,
) -> Result<Page> {
    let computed = ComputedChange::edit(tx, std::iter::once(&page))?;
    insert_page(tx, &page)?;
    attach(tx, &page)?;
    crate::assets::validate_page_attachments(tx, &page)?;
    insert_revision(tx, &page)?;
    enqueue(tx, None, computed)?;
    Ok(page)
}

/// Writes remote fields as a new local revision. Returns whether the remote
/// parent had to be dropped to avoid a cycle. Paired relations are not
/// mirrored here: the other side arrives as its own change.
fn update_local(
    tx: &Transaction<'_>,
    before: Page,
    remote: &SyncPage,
    blocks: Vec<Block>,
    blobs: &Path,
    arriving: &HashSet<String>,
) -> Result<(Page, bool)> {
    let id = before.info.id.clone();
    let values = record_values(remote);
    if before.properties.is_some() != values.is_some() {
        return Err(Error::new(
            "SYNC_CHANGE",
            "远端与本机对这个页面是否为数据库记录的判断不一致。",
        ));
    }
    if values.is_some() && remote.parent_id != before.info.parent_id {
        return Err(Error::new("SYNC_CHANGE", "数据库记录不能移到其他位置。"));
    }
    let collection = ensure_parent(tx, remote.parent_id.as_deref(), values.is_some())?;
    ensure_trash_root(tx, &id, remote.trash_root_id.as_deref())?;
    if let (Some(collection), Some(values)) = (&collection, &values) {
        check_synced_values(tx, collection, values, arriving)?;
    }
    merge_attachments(tx, &id, &remote.attachments, blobs)?;
    let mut parent = remote.parent_id.clone();
    let cycle = match parent.as_deref() {
        Some(target) => creates_cycle(tx, &id, target)?,
        None => false,
    };
    if cycle {
        parent = None;
    }
    let mut after = before.clone();
    after.info.parent_id = parent;
    after.info.title = remote.title.clone();
    after.info.icon = remote.icon.clone();
    after.info.trash_root_id = remote.trash_root_id.clone();
    after.blocks = blocks;
    after.cover = page_cover(remote);
    after.properties = values;
    if after == before {
        return Ok((after, cycle));
    }
    let moved = after.info.parent_id != before.info.parent_id;
    let visibility = after.info.trash_root_id.is_some() != before.info.trash_root_id.is_some();
    let ranges = if moved || visibility {
        Some(RangeSnapshot::capture(tx)?)
    } else {
        None
    };
    let (derived, computed) = if visibility {
        let ids = [id.clone()];
        let derived = ReferenceChange::visibility(tx, &ids)?;
        let computed = ComputedChange::visibility(tx, &ids)?;
        if after.info.trash_root_id.is_some() {
            record_archive(tx, &ids)?;
        }
        (derived, computed)
    } else {
        (None, None)
    };
    commit_removed_relations(tx, &mut after)?;
    if let Some(ranges) = ranges {
        ranges.finish(tx)?;
    }
    enqueue(tx, derived, computed)?;
    Ok((after, cycle))
}

fn creates_cycle(tx: &Transaction<'_>, id: &str, parent: &str) -> Result<bool> {
    let mut current = Some(parent.to_owned());
    let mut steps = 0_usize;
    while let Some(node) = current {
        if node == id {
            return Ok(true);
        }
        steps += 1;
        if steps > 100_000 {
            return Ok(true);
        }
        current = tx
            .query_row(
                "SELECT parent_id FROM pages WHERE id = ?1",
                [&node],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
    }
    Ok(false)
}

/// Saves the local side of an edit conflict as a new, unsynced page with its
/// own copies of the attachments. A record copy stays in its database without
/// relation values, which would otherwise be paired on one side only.
fn conflict_copy(tx: &Transaction<'_>, local: &Page) -> Result<String> {
    fn renumber(blocks: &mut [Block], attachments: &HashMap<String, String>) {
        for block in blocks {
            block.id = new_id();
            let reference = crate::rich_blocks::attachment_reference(block)
                .map(|reference| reference.id.to_owned());
            if let Some(id) = reference.and_then(|id| attachments.get(&id)) {
                block.payload["attachmentId"] = serde_json::Value::String(id.clone());
            }
            renumber(&mut block.children, attachments);
        }
    }
    let copy_id = new_id();
    let originals = read_attachments(tx, Some(&local.info.id))?;
    let ids: HashMap<String, String> = originals
        .iter()
        .map(|item| (item.id.clone(), new_id()))
        .collect();
    let mut blocks = local.blocks.clone();
    renumber(&mut blocks, &ids);
    let keep = MAX_TITLE_CHARS - CONFLICT_SUFFIX.chars().count();
    let title: String = local.info.title.chars().take(keep).collect();
    let parent = match local.info.parent_id.as_deref() {
        Some(parent) => read_optional_page(tx, parent)?
            .filter(|page| page.info.trash_root_id.is_none() && page.properties.is_none())
            .map(|page| page.info.id),
        None => None,
    };
    let collection = crate::collections::for_parent(tx, parent.as_deref())?;
    let properties = match (&collection, &local.properties) {
        (Some(collection), Some(values)) => {
            let relations = relation_fields(tx, collection)?;
            let mut values = values.clone();
            values.retain(|key, _| !relations.contains(key));
            for value in values.values_mut() {
                // File values refer to attachment IDs, which the copy renumbers.
                if let Some(items) = value.as_array_mut() {
                    for item in items {
                        if let Some(id) = item.as_str().and_then(|id| ids.get(id)) {
                            *item = serde_json::Value::String(id.clone());
                        }
                    }
                }
            }
            Some(values)
        }
        _ => None,
    };
    // A record whose database is unavailable here is kept as an ordinary page.
    let parent = if collection.is_some() && properties.is_none() {
        None
    } else {
        parent
    };
    let page = Page {
        info: PageInfo {
            id: copy_id.clone(),
            parent_id: parent,
            title: format!("{title}{CONFLICT_SUFFIX}"),
            revision: 1,
            updated_at: crate::store::now(),
            icon: local.info.icon.clone(),
            trash_root_id: None,
        },
        blocks,
        properties,
        historical_properties: Vec::new(),
        cover: local.cover.as_ref().and_then(|cover| {
            ids.get(&cover.attachment_id).map(|id| PageCover {
                attachment_id: id.clone(),
                media_type: cover.media_type.clone(),
            })
        }),
    };
    Ok(insert_fresh(tx, page, |tx, page| {
        for original in &originals {
            insert_attachment(
                tx,
                &Attachment {
                    id: ids[&original.id].clone(),
                    page_id: copy_id.clone(),
                    ..original.clone()
                },
            )?;
        }
        if let (Some(collection), Some(_)) = (&collection, &page.properties) {
            insert_synced_record(tx, page, collection, crate::store::now())?;
        }
        Ok(())
    })?
    .info
    .id)
}

fn read_optional_page(db: &Connection, id: &str) -> Result<Option<Page>> {
    match read_page(db, id) {
        Ok(page) => Ok(Some(page)),
        Err(error) if error.code == "PAGE_NOT_FOUND" => Ok(None),
        Err(error) => Err(error),
    }
}

fn tracked(db: &Connection, id: &str) -> Result<Option<Tracked>> {
    Ok(db
        .query_row(
            "SELECT server_version, synced_hash, deleted FROM sync_objects WHERE object_id = ?1",
            [id],
            |row| {
                Ok(Tracked {
                    server_version: row.get::<_, i64>(0)? as u64,
                    synced_hash: row.get(1)?,
                    deleted: row.get(2)?,
                })
            },
        )
        .optional()?)
}

fn known_version(db: &Connection, id: &str) -> Result<Option<u64>> {
    Ok(tracked(db, id)?.map(|entry| entry.server_version))
}

fn set_object(
    tx: &Transaction<'_>,
    id: &str,
    version: u64,
    synced_hash: Option<String>,
    deleted: bool,
) -> Result<()> {
    tx.execute(
        "INSERT INTO sync_objects (object_id, server_version, synced_hash, deleted)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (object_id) DO UPDATE SET server_version = excluded.server_version,
             synced_hash = excluded.synced_hash, deleted = excluded.deleted",
        params![id, version as i64, synced_hash, deleted],
    )?;
    Ok(())
}

fn cursor(db: &Connection) -> Result<u64> {
    Ok(
        db.query_row("SELECT cursor FROM sync_state WHERE slot = 1", [], |row| {
            row.get::<_, i64>(0)
        })? as u64,
    )
}

struct Settings {
    enabled: bool,
    server_url: Option<String>,
    last_attempt_at: Option<i64>,
    last_success_at: Option<i64>,
    last_error: Option<String>,
}

fn settings(db: &Connection) -> Result<Settings> {
    Ok(db.query_row(
        "SELECT enabled, server_url, last_attempt_at, last_success_at, last_error
         FROM sync_state WHERE slot = 1",
        [],
        |row| {
            Ok(Settings {
                enabled: row.get(0)?,
                server_url: row.get(1)?,
                last_attempt_at: row.get(2)?,
                last_success_at: row.get(3)?,
                last_error: row.get(4)?,
            })
        },
    )?)
}

fn pending(db: &Connection) -> Result<Vec<SyncPendingItem>> {
    Ok(db
        .prepare("SELECT object_id, version, reason FROM sync_pending ORDER BY seq")?
        .query_map([], |row| {
            Ok(SyncPendingItem {
                object_id: row.get(0)?,
                version: row.get::<_, i64>(1)? as u64,
                reason: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
