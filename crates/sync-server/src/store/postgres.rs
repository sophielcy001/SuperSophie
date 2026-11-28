// SPDX-License-Identifier: AGPL-3.0-or-later

//! PostgreSQL backend for server deployments.
//!
//! One connection is shared behind a mutex and reopened after a failure, so
//! the service survives a database restart. Each push takes a transaction
//! scoped advisory lock on its workspace: pushes to one workspace run one at a
//! time, which keeps sequence numbers visible in commit order and lets pulls
//! advance their cursor without skipping changes.

use super::{Backend, Ops, remote_change};
use crate::engine::{ServiceError, ServiceResult};
use postgres::{Client, Config, NoTls, Transaction};
use std::sync::Mutex;
use std::time::Duration;
use zhiye_sync_protocol::{DeviceInfo, PushResult, RemoteChange, WorkspaceInfo};

const SCHEMA_VERSION: i32 = 1;
/// Key of the advisory lock that serializes schema setup across instances.
const SETUP_LOCK: i64 = 0x7a68_6979_6573_796e;
const SCHEMA: &str = "
    CREATE TABLE zhiye_sync_schema (version INTEGER NOT NULL);
    CREATE TABLE devices (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        token_hash BYTEA NOT NULL UNIQUE CHECK (length(token_hash) = 32),
        created_at BIGINT NOT NULL,
        last_seen_at BIGINT,
        revoked_at BIGINT
    );
    CREATE TABLE workspaces (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        created_by TEXT NOT NULL REFERENCES devices(id)
    );
    CREATE TABLE changes (
        seq BIGSERIAL PRIMARY KEY,
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        object_id TEXT NOT NULL,
        version BIGINT NOT NULL CHECK (version > 0),
        device_id TEXT NOT NULL REFERENCES devices(id),
        op_id TEXT NOT NULL,
        change TEXT NOT NULL,
        created_at BIGINT NOT NULL,
        UNIQUE (workspace_id, object_id, version)
    );
    CREATE INDEX changes_workspace ON changes(workspace_id, seq);
    CREATE TABLE objects (
        workspace_id TEXT NOT NULL REFERENCES workspaces(id),
        object_id TEXT NOT NULL,
        version BIGINT NOT NULL CHECK (version > 0),
        seq BIGINT NOT NULL REFERENCES changes(seq),
        PRIMARY KEY (workspace_id, object_id)
    );
    CREATE TABLE operations (
        device_id TEXT NOT NULL REFERENCES devices(id),
        op_id TEXT NOT NULL,
        request_hash BYTEA NOT NULL CHECK (length(request_hash) = 32),
        result TEXT NOT NULL,
        PRIMARY KEY (device_id, op_id)
    );
";

impl From<postgres::Error> for ServiceError {
    fn from(error: postgres::Error) -> Self {
        // The top-level message is generic; the cause says what went wrong.
        let mut detail = error.to_string();
        let mut source = std::error::Error::source(&error);
        while let Some(cause) = source {
            detail.push('：');
            detail.push_str(&cause.to_string());
            source = cause.source();
        }
        ServiceError::storage(detail)
    }
}

pub(crate) struct PostgresBackend {
    config: Config,
    client: Mutex<Option<Client>>,
}

impl PostgresBackend {
    /// Connects, then creates or checks the schema.
    pub(crate) fn connect(mut config: Config) -> ServiceResult<Self> {
        if config.get_connect_timeout().is_none() {
            config.connect_timeout(Duration::from_secs(10));
        }
        config.application_name("zhiye-sync-server");
        let mut client = config.connect(NoTls)?;
        let mut tx = client.transaction()?;
        tx.execute("SELECT pg_advisory_xact_lock($1)", &[&SETUP_LOCK])?;
        let installed: bool = tx
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM pg_tables
                 WHERE schemaname = current_schema() AND tablename = 'zhiye_sync_schema')",
                &[],
            )?
            .get(0);
        if installed {
            let version: i32 = tx
                .query_one("SELECT version FROM zhiye_sync_schema", &[])?
                .get(0);
            if version != SCHEMA_VERSION {
                return Err(ServiceError::format_version());
            }
        } else {
            tx.batch_execute(SCHEMA)?;
            tx.execute(
                "INSERT INTO zhiye_sync_schema (version) VALUES ($1)",
                &[&SCHEMA_VERSION],
            )?;
        }
        tx.commit()?;
        Ok(Self {
            config,
            client: Mutex::new(Some(client)),
        })
    }

    /// Runs `work` on the shared connection. A connection found closed is
    /// reopened, and work that failed because the connection had dropped (for
    /// example after a database restart) runs once more on a new one. Pushes
    /// are idempotent by operation ID and the other writes are idempotent or
    /// harmless to repeat, so the retry cannot duplicate content.
    fn with<T>(&self, mut work: impl FnMut(&mut Client) -> ServiceResult<T>) -> ServiceResult<T> {
        let mut slot = self.client.lock().map_err(|_| ServiceError::state())?;
        let mut attempt = 0;
        loop {
            if slot.as_ref().is_none_or(Client::is_closed) {
                *slot = None;
                *slot = Some(self.config.connect(NoTls)?);
            }
            let client = slot.as_mut().expect("connection was opened");
            let result = work(client);
            if client.is_closed() {
                *slot = None;
                if result.is_err() && attempt == 0 {
                    attempt += 1;
                    continue;
                }
            }
            return result;
        }
    }
}

fn read_device(row: &postgres::Row) -> DeviceInfo {
    DeviceInfo {
        id: row.get(0),
        name: row.get(1),
        created_at: row.get(2),
        last_seen_at: row.get(3),
        revoked_at: row.get(4),
    }
}

fn read_workspace(row: &postgres::Row) -> WorkspaceInfo {
    WorkspaceInfo {
        id: row.get(0),
        name: row.get(1),
        created_at: row.get(2),
    }
}

fn read_change(row: &postgres::Row) -> ServiceResult<RemoteChange> {
    remote_change(
        row.get(0),
        row.get(1),
        row.get(2),
        row.get(3),
        row.get(4),
        row.get::<_, &str>(5),
    )
}

impl Backend for PostgresBackend {
    fn insert_device(&self, device: &DeviceInfo, token_hash: &[u8]) -> ServiceResult<()> {
        self.with(|client| {
            client.execute(
                "INSERT INTO devices (id, name, token_hash, created_at) VALUES ($1, $2, $3, $4)",
                &[&device.id, &device.name, &token_hash, &device.created_at],
            )?;
            Ok(())
        })
    }

    fn devices(&self) -> ServiceResult<Vec<DeviceInfo>> {
        self.with(|client| {
            Ok(client
                .query(
                    "SELECT id, name, created_at, last_seen_at, revoked_at FROM devices
                     ORDER BY created_at, id",
                    &[],
                )?
                .iter()
                .map(read_device)
                .collect())
        })
    }

    fn revoke_device(&self, id: &str, now: i64) -> ServiceResult<bool> {
        self.with(|client| {
            Ok(client.execute(
                "UPDATE devices SET revoked_at = coalesce(revoked_at, $2) WHERE id = $1",
                &[&id, &now],
            )? > 0)
        })
    }

    fn authenticate(&self, token_hash: &[u8], now: i64) -> ServiceResult<Option<String>> {
        self.with(|client| {
            Ok(client
                .query_opt(
                    "UPDATE devices SET last_seen_at = $2 WHERE token_hash = $1 AND revoked_at IS NULL
                     RETURNING id",
                    &[&token_hash, &now],
                )?
                .map(|row| row.get(0)))
        })
    }

    fn workspaces(&self) -> ServiceResult<Vec<WorkspaceInfo>> {
        self.with(|client| {
            Ok(client
                .query(
                    "SELECT id, name, created_at FROM workspaces ORDER BY created_at, id",
                    &[],
                )?
                .iter()
                .map(read_workspace)
                .collect())
        })
    }

    fn put_workspace(
        &self,
        id: &str,
        name: &str,
        now: i64,
        device: &str,
    ) -> ServiceResult<WorkspaceInfo> {
        self.with(|client| {
            Ok(read_workspace(&client.query_one(
                "INSERT INTO workspaces (id, name, created_at, created_by) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (id) DO UPDATE SET name = EXCLUDED.name
                 RETURNING id, name, created_at",
                &[&id, &name, &now, &device],
            )?))
        })
    }

    fn workspace_exists(&self, id: &str) -> ServiceResult<bool> {
        self.with(|client| {
            Ok(client
                .query_one(
                    "SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = $1)",
                    &[&id],
                )?
                .get(0))
        })
    }

    fn changes_after(
        &self,
        workspace: &str,
        since: i64,
        limit: i64,
    ) -> ServiceResult<Vec<RemoteChange>> {
        self.with(|client| {
            client
                .query(
                    "SELECT seq, object_id, version, device_id, op_id, change FROM changes
                     WHERE workspace_id = $1 AND seq > $2 ORDER BY seq LIMIT $3",
                    &[&workspace, &since, &limit],
                )?
                .iter()
                .map(read_change)
                .collect()
        })
    }

    fn transact(
        &self,
        workspace: &str,
        work: &mut dyn FnMut(&mut dyn Ops) -> ServiceResult<Vec<PushResult>>,
    ) -> ServiceResult<Vec<PushResult>> {
        self.with(|client| {
            let mut tx = client.transaction()?;
            tx.execute(
                "SELECT pg_advisory_xact_lock(hashtextextended($1, 0))",
                &[&workspace],
            )?;
            let results = work(&mut PostgresOps(&mut tx))?;
            tx.commit()?;
            Ok(results)
        })
    }
}

struct PostgresOps<'a, 'b>(&'a mut Transaction<'b>);

impl Ops for PostgresOps<'_, '_> {
    fn workspace_exists(&mut self, id: &str) -> ServiceResult<bool> {
        Ok(self
            .0
            .query_one(
                "SELECT EXISTS (SELECT 1 FROM workspaces WHERE id = $1)",
                &[&id],
            )?
            .get(0))
    }

    fn stored_operation(
        &mut self,
        device: &str,
        op_id: &str,
    ) -> ServiceResult<Option<(Vec<u8>, String)>> {
        Ok(self
            .0
            .query_opt(
                "SELECT request_hash, result FROM operations WHERE device_id = $1 AND op_id = $2",
                &[&device, &op_id],
            )?
            .map(|row| (row.get(0), row.get(1))))
    }

    fn object(&mut self, workspace: &str, object_id: &str) -> ServiceResult<Option<(i64, i64)>> {
        Ok(self
            .0
            .query_opt(
                "SELECT version, seq FROM objects WHERE workspace_id = $1 AND object_id = $2",
                &[&workspace, &object_id],
            )?
            .map(|row| (row.get(0), row.get(1))))
    }

    fn change(&mut self, seq: i64) -> ServiceResult<RemoteChange> {
        read_change(&self.0.query_one(
            "SELECT seq, object_id, version, device_id, op_id, change FROM changes WHERE seq = $1",
            &[&seq],
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
        Ok(self
            .0
            .query_one(
                "INSERT INTO changes (workspace_id, object_id, version, device_id, op_id, change, created_at)
                 VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING seq",
                &[&workspace, &object_id, &version, &device, &op_id, &change, &now],
            )?
            .get(0))
    }

    fn upsert_object(
        &mut self,
        workspace: &str,
        object_id: &str,
        version: i64,
        seq: i64,
    ) -> ServiceResult<()> {
        self.0.execute(
            "INSERT INTO objects (workspace_id, object_id, version, seq) VALUES ($1, $2, $3, $4)
             ON CONFLICT (workspace_id, object_id)
             DO UPDATE SET version = EXCLUDED.version, seq = EXCLUDED.seq",
            &[&workspace, &object_id, &version, &seq],
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
            "INSERT INTO operations (device_id, op_id, request_hash, result) VALUES ($1, $2, $3, $4)",
            &[&device, &op_id, &request_hash, &result],
        )?;
        Ok(())
    }
}
