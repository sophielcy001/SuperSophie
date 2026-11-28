// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single-file SQLite backend for small deployments and tests.

use super::{Backend, Ops, remote_change};
use crate::engine::{ServiceError, ServiceResult};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use std::sync::Mutex;
use std::time::Duration;
use zhiye_sync_protocol::{DeviceInfo, PushResult, RemoteChange, WorkspaceInfo};

const SCHEMA_VERSION: u32 = 1;
const SCHEMA: &str = "
    CREATE TABLE devices (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        token_hash BLOB NOT NULL UNIQUE CHECK (length(token_hash) = 32),
        created_at INTEGER NOT NULL,
        last_seen_at INTEGER,
        revoked_at INTEGER
    );
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY NOT NULL,
        name TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        created_by TEXT NOT NULL REFERENCES devices(id)
    );
    CREATE TABLE changes (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        object_id TEXT NOT NULL,
        version INTEGER NOT NULL CHECK (version > 0),
        device_id TEXT NOT NULL REFERENCES devices(id),
        op_id TEXT NOT NULL,
        change TEXT NOT NULL CHECK (json_valid(change)),
        created_at INTEGER NOT NULL,
        UNIQUE (workspace_id, object_id, version)
    );
    CREATE INDEX changes_workspace ON changes(workspace_id, seq);
    CREATE TABLE objects (
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        object_id TEXT NOT NULL,
        version INTEGER NOT NULL CHECK (version > 0),
        seq INTEGER NOT NULL REFERENCES changes(seq),
        PRIMARY KEY (workspace_id, object_id)
    ) WITHOUT ROWID;
    CREATE TABLE operations (
        device_id TEXT NOT NULL REFERENCES devices(id),
        op_id TEXT NOT NULL,
        request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
        result TEXT NOT NULL CHECK (json_valid(result)),
        PRIMARY KEY (device_id, op_id)
    ) WITHOUT ROWID;
";

impl From<rusqlite::Error> for ServiceError {
    fn from(error: rusqlite::Error) -> Self {
        ServiceError::storage(error)
    }
}

pub(crate) struct SqliteBackend {
    db: Mutex<Connection>,
}

impl SqliteBackend {
    pub(crate) fn new(mut db: Connection) -> ServiceResult<Self> {
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch(
            "PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;",
        )?;
        let version: u32 = db.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        match version {
            0 => {
                let tx = db.transaction_with_behavior(TransactionBehavior::Exclusive)?;
                tx.execute_batch(SCHEMA)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                tx.commit()?;
            }
            SCHEMA_VERSION => {}
            _ => return Err(ServiceError::format_version()),
        }
        Ok(Self { db: Mutex::new(db) })
    }

    fn lock(&self) -> ServiceResult<std::sync::MutexGuard<'_, Connection>> {
        self.db.lock().map_err(|_| ServiceError::state())
    }
}

fn read_device(row: &rusqlite::Row<'_>) -> rusqlite::Result<DeviceInfo> {
    Ok(DeviceInfo {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
        last_seen_at: row.get(3)?,
        revoked_at: row.get(4)?,
    })
}

fn read_workspace(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceInfo> {
    Ok(WorkspaceInfo {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at: row.get(2)?,
    })
}

type ChangeRow = (i64, String, i64, String, String, String);

fn change_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ChangeRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
        row.get(5)?,
    ))
}

fn into_change(
    (seq, object, version, device, op, change): ChangeRow,
) -> ServiceResult<RemoteChange> {
    remote_change(seq, object, version, device, op, &change)
}

fn exists(db: &Connection, id: &str) -> ServiceResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = ?1)",
        [id],
        |row| row.get(0),
    )?)
}

impl Backend for SqliteBackend {
    fn insert_device(&self, device: &DeviceInfo, token_hash: &[u8]) -> ServiceResult<()> {
        self.lock()?.execute(
            "INSERT INTO devices (id, name, token_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![device.id, device.name, token_hash, device.created_at],
        )?;
        Ok(())
    }

    fn devices(&self) -> ServiceResult<Vec<DeviceInfo>> {
        let db = self.lock()?;
        let mut query = db.prepare(
            "SELECT id, name, created_at, last_seen_at, revoked_at FROM devices ORDER BY created_at, id",
        )?;
        Ok(query
            .query_map([], read_device)?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn revoke_device(&self, id: &str, now: i64) -> ServiceResult<bool> {
        Ok(self.lock()?.execute(
            "UPDATE devices SET revoked_at = coalesce(revoked_at, ?2) WHERE id = ?1",
            params![id, now],
        )? > 0)
    }

    fn authenticate(&self, token_hash: &[u8], now: i64) -> ServiceResult<Option<String>> {
        Ok(self
            .lock()?
            .query_row(
                "UPDATE devices SET last_seen_at = ?2 WHERE token_hash = ?1 AND revoked_at IS NULL
                 RETURNING id",
                params![token_hash, now],
                |row| row.get(0),
            )
            .optional()?)
    }

    fn workspaces(&self) -> ServiceResult<Vec<WorkspaceInfo>> {
        let db = self.lock()?;
        let mut query =
            db.prepare("SELECT id, name, created_at FROM workspaces ORDER BY created_at, id")?;
        Ok(query
            .query_map([], read_workspace)?
            .collect::<rusqlite::Result<_>>()?)
    }

    fn put_workspace(
        &self,
        id: &str,
        name: &str,
        now: i64,
        device: &str,
    ) -> ServiceResult<WorkspaceInfo> {
        Ok(self.lock()?.query_row(
            "INSERT INTO workspaces (id, name, created_at, created_by) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name
             RETURNING id, name, created_at",
            params![id, name, now, device],
            read_workspace,
        )?)
    }

    fn workspace_exists(&self, id: &str) -> ServiceResult<bool> {
        exists(&*self.lock()?, id)
    }

    fn changes_after(
        &self,
        workspace: &str,
        since: i64,
        limit: i64,
    ) -> ServiceResult<Vec<RemoteChange>> {
        let db = self.lock()?;
        let mut query = db.prepare(
            "SELECT seq, object_id, version, device_id, op_id, change FROM changes
             WHERE workspace_id = ?1 AND seq > ?2 ORDER BY seq LIMIT ?3",
        )?;
        let rows = query
            .query_map(params![workspace, since, limit], change_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows.into_iter().map(into_change).collect()
    }

    fn transact(
        &self,
        _workspace: &str,
        work: &mut dyn FnMut(&mut dyn Ops) -> ServiceResult<Vec<PushResult>>,
    ) -> ServiceResult<Vec<PushResult>> {
        let mut db = self.lock()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let results = work(&mut SqliteOps(&tx))?;
        tx.commit()?;
        Ok(results)
    }
}

struct SqliteOps<'a, 'b>(&'a Transaction<'b>);

impl Ops for SqliteOps<'_, '_> {
    fn workspace_exists(&mut self, id: &str) -> ServiceResult<bool> {
        exists(self.0, id)
    }

    fn stored_operation(
        &mut self,
        device: &str,
        op_id: &str,
    ) -> ServiceResult<Option<(Vec<u8>, String)>> {
        Ok(self
            .0
            .query_row(
                "SELECT request_hash, result FROM operations WHERE device_id = ?1 AND op_id = ?2",
                params![device, op_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    fn object(&mut self, workspace: &str, object_id: &str) -> ServiceResult<Option<(i64, i64)>> {
        Ok(self
            .0
            .query_row(
                "SELECT version, seq FROM objects WHERE workspace_id = ?1 AND object_id = ?2",
                params![workspace, object_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    fn change(&mut self, seq: i64) -> ServiceResult<RemoteChange> {
        into_change(self.0.query_row(
            "SELECT seq, object_id, version, device_id, op_id, change FROM changes WHERE seq = ?1",
            [seq],
            change_row,
        )?)
    }

    fn insert_change(
        &mut self,
        workspace: &str,
        object_id: &str,
        version: i64,
        device: &str,
        op_id: &str,
        change: &str,
        now: i64,
    ) -> ServiceResult<i64> {
        self.0.execute(
            "INSERT INTO changes (workspace_id, object_id, version, device_id, op_id, change, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![workspace, object_id, version, device, op_id, change, now],
        )?;
        Ok(self.0.last_insert_rowid())
    }

    fn upsert_object(
        &mut self,
        workspace: &str,
        object_id: &str,
        version: i64,
        seq: i64,
    ) -> ServiceResult<()> {
        self.0.execute(
            "INSERT INTO objects (workspace_id, object_id, version, seq) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, object_id) DO UPDATE SET version = excluded.version, seq = excluded.seq",
            params![workspace, object_id, version, seq],
        )?;
        Ok(())
    }

    fn insert_operation(
        &mut self,
        device: &str,
        op_id: &str,
        request_hash: &[u8],
        result: &str,
    ) -> ServiceResult<()> {
        self.0.execute(
            "INSERT INTO operations (device_id, op_id, request_hash, result) VALUES (?1, ?2, ?3, ?4)",
            params![device, op_id, request_hash, result],
        )?;
        Ok(())
    }
}
