// SPDX-License-Identifier: AGPL-3.0-or-later

//! Storage backends for the service. The engine keeps the rules (validation,
//! version checks, idempotent replay); a backend only stores and reads rows.

pub(crate) mod postgres;
pub(crate) mod sqlite;

use crate::engine::ServiceResult;
use zhiye_sync_protocol::{DeviceInfo, PushResult, RemoteChange, WorkspaceInfo};

pub(crate) trait Backend: Send + Sync {
    fn insert_device(&self, device: &DeviceInfo, token_hash: &[u8]) -> ServiceResult<()>;
    fn devices(&self) -> ServiceResult<Vec<DeviceInfo>>;
    /// Returns whether the device exists.
    fn revoke_device(&self, id: &str, now: i64) -> ServiceResult<bool>;
    /// Returns the ID of the active device with this token digest and records the use.
    fn authenticate(&self, token_hash: &[u8], now: i64) -> ServiceResult<Option<String>>;
    fn workspaces(&self) -> ServiceResult<Vec<WorkspaceInfo>>;
    fn put_workspace(
        &self,
        id: &str,
        name: &str,
        now: i64,
        device: &str,
    ) -> ServiceResult<WorkspaceInfo>;
    fn workspace_exists(&self, id: &str) -> ServiceResult<bool>;
    fn changes_after(
        &self,
        workspace: &str,
        since: i64,
        limit: i64,
    ) -> ServiceResult<Vec<RemoteChange>>;
    /// Runs `work` in one transaction. Pushes to the same workspace are
    /// serialized, so sequence numbers become visible in commit order.
    fn transact(
        &self,
        workspace: &str,
        work: &mut dyn FnMut(&mut dyn Ops) -> ServiceResult<Vec<PushResult>>,
    ) -> ServiceResult<Vec<PushResult>>;
}

/// Row operations available inside a push transaction.
pub(crate) trait Ops {
    fn workspace_exists(&mut self, id: &str) -> ServiceResult<bool>;
    fn stored_operation(
        &mut self,
        device: &str,
        op_id: &str,
    ) -> ServiceResult<Option<(Vec<u8>, String)>>;
    /// Current `(version, seq)` of an object.
    fn object(&mut self, workspace: &str, object_id: &str) -> ServiceResult<Option<(i64, i64)>>;
    fn change(&mut self, seq: i64) -> ServiceResult<RemoteChange>;
    #[allow(clippy::too_many_arguments)]
    fn insert_change(
        &mut self,
        workspace: &str,
        object_id: &str,
        version: i64,
        device: &str,
        op_id: &str,
        change: &str,
        now: i64,
    ) -> ServiceResult<i64>;
    fn upsert_object(
        &mut self,
        workspace: &str,
        object_id: &str,
        version: i64,
        seq: i64,
    ) -> ServiceResult<()>;
    fn insert_operation(
        &mut self,
        device: &str,
        op_id: &str,
        request_hash: &[u8],
        result: &str,
    ) -> ServiceResult<()>;
}

pub(crate) fn remote_change(
    seq: i64,
    object_id: String,
    version: i64,
    device_id: String,
    op_id: String,
    change: &str,
) -> ServiceResult<RemoteChange> {
    Ok(RemoteChange {
        seq: seq as u64,
        object_id,
        version: version as u64,
        device_id,
        op_id,
        change: serde_json::from_str(change)?,
    })
}
