// SPDX-License-Identifier: AGPL-3.0-or-later

//! Service rules: device registry, ordered change log, conflict checks and
//! idempotent replay. Rows live in a [`Backend`]: SQLite or PostgreSQL.

use crate::blobs::BlobStore;
use crate::store::postgres::PostgresBackend;
use crate::store::sqlite::SqliteBackend;
use crate::store::{Backend, Ops};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};
use zhiye_sync_protocol::{
    BlobQuery, DEFAULT_PULL_LIMIT, DeviceInfo, IssuedDevice, MAX_BLOB_QUERY, MAX_NAME_CHARS,
    MAX_PULL_LIMIT, MissingBlobs, ObjectChange, PullResponse, PushOperation, PushRequest,
    PushResponse, PushResult, WorkspaceInfo, valid_id, valid_name,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceError {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
}

impl ServiceError {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(401, "UNAUTHORIZED", "设备凭据无效或已被撤销。")
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(400, "INVALID_REQUEST", message)
    }

    pub(crate) fn storage(error: impl std::fmt::Display) -> Self {
        Self::new(500, "STORAGE", format!("同步服务的存储操作失败。{error}"))
    }

    pub(crate) fn state() -> Self {
        Self::new(500, "SERVICE_STATE", "同步服务需要重新启动。")
    }

    pub(crate) fn format_version() -> Self {
        Self::new(500, "FORMAT_VERSION", "此版本的同步服务不能打开该数据库。")
    }
}

impl std::fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for ServiceError {}

impl From<serde_json::Error> for ServiceError {
    fn from(error: serde_json::Error) -> Self {
        Self::storage(error)
    }
}

pub type ServiceResult<T> = std::result::Result<T, ServiceError>;

/// An authenticated, non-revoked device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
}

pub struct Engine {
    backend: Box<dyn Backend>,
    blobs: BlobStore,
}

impl Engine {
    /// Opens or creates `sync.sqlite` and `blobs/` in `directory`.
    pub fn open(directory: impl AsRef<Path>) -> ServiceResult<Self> {
        std::fs::create_dir_all(directory.as_ref()).map_err(ServiceError::storage)?;
        let db = rusqlite::Connection::open(directory.as_ref().join("sync.sqlite"))?;
        Ok(Self {
            backend: Box::new(SqliteBackend::new(db)?),
            blobs: BlobStore::directory(directory.as_ref().join("blobs"))?,
        })
    }

    pub fn open_in_memory() -> ServiceResult<Self> {
        Ok(Self {
            backend: Box::new(SqliteBackend::new(rusqlite::Connection::open_in_memory()?)?),
            blobs: BlobStore::memory(),
        })
    }

    /// Uses PostgreSQL for the change log and `blobs` for attachment originals.
    /// `database` is a connection URL or key/value string; a separate
    /// `password` keeps the secret out of the command line.
    pub fn open_postgres(
        database: &str,
        password: Option<&str>,
        blobs: impl AsRef<Path>,
    ) -> ServiceResult<Self> {
        let mut config: postgres::Config = database.parse().map_err(|error| {
            ServiceError::new(
                500,
                "DATABASE_URL",
                format!("PostgreSQL 连接参数无效。{error}"),
            )
        })?;
        if let Some(password) = password {
            config.password(password);
        }
        Self::open_postgres_config(config, blobs)
    }

    #[doc(hidden)]
    pub fn open_postgres_config(
        config: postgres::Config,
        blobs: impl AsRef<Path>,
    ) -> ServiceResult<Self> {
        Ok(Self {
            backend: Box::new(PostgresBackend::connect(config)?),
            blobs: BlobStore::directory(blobs.as_ref().to_path_buf())?,
        })
    }

    /// Test helper: an in-memory SQLite engine, or a fresh schema in the
    /// PostgreSQL database named by `ZHIYE_SYNC_TEST_DATABASE_URL`.
    #[doc(hidden)]
    pub fn open_for_tests() -> ServiceResult<Self> {
        let Ok(url) = std::env::var("ZHIYE_SYNC_TEST_DATABASE_URL") else {
            return Self::open_in_memory();
        };
        let mut config: postgres::Config = url
            .parse()
            .map_err(|error| ServiceError::new(500, "DATABASE_URL", format!("{error}")))?;
        let schema = format!("zhiye_test_{}", uuid::Uuid::new_v4().simple());
        config
            .connect(postgres::NoTls)?
            .batch_execute(&format!("CREATE SCHEMA {schema}"))?;
        config.options(&format!("-c search_path={schema}"));
        let blobs = std::env::temp_dir().join(&schema);
        Self::open_postgres_config(config, blobs)
    }

    pub fn register_device(&self, name: &str) -> ServiceResult<IssuedDevice> {
        let name = checked_name(name, "设备名称")?;
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|_| ServiceError::new(500, "RANDOM", "无法生成设备凭据。"))?;
        let token = format!("zsd_{}", hex(&secret));
        let device = DeviceInfo {
            id: uuid::Uuid::new_v4().to_string(),
            name,
            created_at: now(),
            last_seen_at: None,
            revoked_at: None,
        };
        self.backend
            .insert_device(&device, &digest(token.as_bytes()))?;
        Ok(IssuedDevice { device, token })
    }

    pub fn devices(&self) -> ServiceResult<Vec<DeviceInfo>> {
        self.backend.devices()
    }

    /// Revoking is permanent; the device must be registered again.
    pub fn revoke_device(&self, id: &str) -> ServiceResult<()> {
        if !self.backend.revoke_device(id, now())? {
            return Err(ServiceError::new(404, "DEVICE_NOT_FOUND", "找不到该设备。"));
        }
        Ok(())
    }

    pub fn authenticate(&self, token: &str) -> ServiceResult<Device> {
        if !token.starts_with("zsd_") || token.len() != 68 {
            return Err(ServiceError::unauthorized());
        }
        let id = self
            .backend
            .authenticate(&digest(token.as_bytes()), now())?
            .ok_or_else(ServiceError::unauthorized)?;
        Ok(Device { id })
    }

    pub fn workspaces(&self, _device: &Device) -> ServiceResult<Vec<WorkspaceInfo>> {
        self.backend.workspaces()
    }

    /// Registers a workspace, or renames an existing one.
    pub fn put_workspace(
        &self,
        device: &Device,
        id: &str,
        name: &str,
    ) -> ServiceResult<WorkspaceInfo> {
        if !valid_id(id) {
            return Err(workspace_not_found());
        }
        let name = checked_name(name, "工作空间名称")?;
        self.backend.put_workspace(id, &name, now(), &device.id)
    }

    pub fn push(
        &self,
        device: &Device,
        workspace: &str,
        request: &PushRequest,
    ) -> ServiceResult<PushResponse> {
        request
            .validate()
            .map_err(|error| ServiceError::invalid(error.0))?;
        // Other devices must be able to fetch every original a page refers to.
        let missing = request
            .blob_hashes()
            .into_iter()
            .filter(|hash| !self.blobs.contains(hash).unwrap_or(false))
            .count();
        if missing > 0 {
            return Err(ServiceError::new(
                409,
                "BLOB_MISSING",
                format!("请先上传 {missing} 个附件原件。"),
            ));
        }
        let results = self.backend.transact(workspace, &mut |ops| {
            if !ops.workspace_exists(workspace)? {
                return Err(workspace_not_found());
            }
            request
                .operations
                .iter()
                .map(|operation| apply(ops, device, workspace, operation))
                .collect()
        })?;
        Ok(PushResponse { results })
    }

    pub fn missing_blobs(
        &self,
        _device: &Device,
        workspace: &str,
        query: &BlobQuery,
    ) -> ServiceResult<MissingBlobs> {
        if query.hashes.len() > MAX_BLOB_QUERY {
            return Err(ServiceError::invalid("每次最多查询 1000 个原件。"));
        }
        self.ensure_workspace(workspace)?;
        let mut missing = Vec::new();
        for hash in &query.hashes {
            if !self.blobs.contains(hash)? && !missing.contains(hash) {
                missing.push(hash.clone());
            }
        }
        Ok(MissingBlobs { missing })
    }

    pub fn put_blob(
        &self,
        _device: &Device,
        workspace: &str,
        hash: &str,
        bytes: &[u8],
    ) -> ServiceResult<()> {
        self.ensure_workspace(workspace)?;
        self.blobs.put(hash, bytes)
    }

    pub fn blob(&self, _device: &Device, workspace: &str, hash: &str) -> ServiceResult<Vec<u8>> {
        self.ensure_workspace(workspace)?;
        self.blobs
            .get(hash)?
            .ok_or_else(|| ServiceError::new(404, "BLOB_NOT_FOUND", "同步服务中没有这个附件原件。"))
    }

    pub fn pull(
        &self,
        _device: &Device,
        workspace: &str,
        since: u64,
        limit: Option<u32>,
    ) -> ServiceResult<PullResponse> {
        let limit = limit.unwrap_or(DEFAULT_PULL_LIMIT);
        if limit == 0 || limit > MAX_PULL_LIMIT {
            return Err(ServiceError::invalid("每页变更数量需要在 1 至 500 之间。"));
        }
        let since = i64::try_from(since).map_err(|_| ServiceError::invalid("同步游标无效。"))?;
        self.ensure_workspace(workspace)?;
        let mut changes = self
            .backend
            .changes_after(workspace, since, i64::from(limit) + 1)?;
        let has_more = changes.len() > limit as usize;
        changes.truncate(limit as usize);
        let next_seq = changes.last().map_or(since as u64, |change| change.seq);
        Ok(PullResponse {
            changes,
            next_seq,
            has_more,
        })
    }

    fn ensure_workspace(&self, workspace: &str) -> ServiceResult<()> {
        if !self.backend.workspace_exists(workspace)? {
            return Err(workspace_not_found());
        }
        Ok(())
    }
}

fn apply(
    ops: &mut dyn Ops,
    device: &Device,
    workspace: &str,
    operation: &PushOperation,
) -> ServiceResult<PushResult> {
    let request_hash = digest(&serde_json::to_vec(&(workspace, operation))?);
    if let Some((hash, result)) = ops.stored_operation(&device.id, &operation.op_id)? {
        if hash != request_hash {
            return Err(ServiceError::new(
                409,
                "OPERATION_REUSED",
                "该操作标识已用于不同的内容。",
            ));
        }
        return Ok(serde_json::from_str(&result)?);
    }

    let current = ops.object(workspace, &operation.object_id)?;
    let current_version = current.map_or(0, |(version, _)| version as u64);
    if operation.base_version != current_version {
        let current = current
            .map(|(_, seq)| ops.change(seq).map(Box::new))
            .transpose()?;
        return Ok(PushResult::Conflict {
            op_id: operation.op_id.clone(),
            object_id: operation.object_id.clone(),
            current,
        });
    }
    if current.is_none() && matches!(operation.change, ObjectChange::Delete) {
        return Err(ServiceError::invalid("不能删除尚未同步的对象。"));
    }

    let version = current_version + 1;
    let stored_version = i64::try_from(version)
        .map_err(|_| ServiceError::new(409, "VERSION_LIMIT", "对象版本号已达上限。"))?;
    let seq = ops.insert_change(
        workspace,
        &operation.object_id,
        stored_version,
        &device.id,
        &operation.op_id,
        &serde_json::to_string(&operation.change)?,
        now(),
    )?;
    ops.upsert_object(workspace, &operation.object_id, stored_version, seq)?;
    let result = PushResult::Accepted {
        op_id: operation.op_id.clone(),
        object_id: operation.object_id.clone(),
        version,
        seq: seq as u64,
    };
    ops.insert_operation(
        &device.id,
        &operation.op_id,
        &request_hash,
        &serde_json::to_string(&result)?,
    )?;
    Ok(result)
}

fn workspace_not_found() -> ServiceError {
    ServiceError::new(404, "WORKSPACE_NOT_FOUND", "同步服务中没有该工作空间。")
}

fn checked_name(name: &str, label: &str) -> ServiceResult<String> {
    let name = name.trim();
    if !valid_name(name, MAX_NAME_CHARS) {
        return Err(ServiceError::invalid(format!(
            "{label}需要 1 至 64 个字符，且不能包含换行。"
        )));
    }
    Ok(name.to_owned())
}

pub(crate) fn digest(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zhiye_sync_protocol::{PROTOCOL_VERSION, SyncPage};

    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    fn upsert(object_id: &str, base_version: u64, title: &str) -> PushOperation {
        PushOperation {
            op_id: id(),
            object_id: object_id.into(),
            base_version,
            change: ObjectChange::Upsert {
                page: SyncPage {
                    parent_id: None,
                    title: title.into(),
                    icon: "📄".into(),
                    blocks: vec![json!({"id": id(), "type": "paragraph"})],
                    trash_root_id: None,
                    attachments: vec![],
                    cover: None,
                    record: None,
                },
            },
        }
    }

    fn push(operations: Vec<PushOperation>) -> PushRequest {
        PushRequest {
            protocol: PROTOCOL_VERSION,
            operations,
        }
    }

    fn setup() -> (Engine, Device, Device, String) {
        let engine = Engine::open_for_tests().unwrap();
        let first = engine.register_device("书房电脑").unwrap();
        let second = engine.register_device("笔记本").unwrap();
        let first = engine.authenticate(&first.token).unwrap();
        let second = engine.authenticate(&second.token).unwrap();
        let workspace = id();
        engine
            .put_workspace(&first, &workspace, "个人资料")
            .unwrap();
        (engine, first, second, workspace)
    }

    #[test]
    fn orders_changes_and_rejects_stale_versions() {
        let (engine, first, second, workspace) = setup();
        let page = id();
        let created = engine
            .push(&first, &workspace, &push(vec![upsert(&page, 0, "初稿")]))
            .unwrap();
        assert!(matches!(
            created.results[0],
            PushResult::Accepted { version: 1, .. }
        ));

        let edited = engine
            .push(
                &second,
                &workspace,
                &push(vec![upsert(&page, 1, "笔记本修改")]),
            )
            .unwrap();
        assert!(matches!(
            edited.results[0],
            PushResult::Accepted { version: 2, .. }
        ));

        let stale = engine
            .push(
                &first,
                &workspace,
                &push(vec![upsert(&page, 1, "书房修改")]),
            )
            .unwrap();
        let PushResult::Conflict { current, .. } = &stale.results[0] else {
            panic!("stale base must conflict: {stale:?}");
        };
        let current = current.as_ref().unwrap();
        assert_eq!(current.version, 2);
        assert_eq!(current.device_id, second.id);

        let recreate = engine
            .push(
                &first,
                &workspace,
                &push(vec![upsert(&page, 0, "重复创建")]),
            )
            .unwrap();
        assert!(matches!(recreate.results[0], PushResult::Conflict { .. }));

        let changes = engine.pull(&first, &workspace, 0, None).unwrap();
        assert_eq!(changes.changes.len(), 2);
        assert!(!changes.has_more);
        let versions: Vec<_> = changes.changes.iter().map(|c| c.version).collect();
        assert_eq!(versions, [1, 2]);
        let page_one = engine.pull(&first, &workspace, 0, Some(1)).unwrap();
        assert!(page_one.has_more);
        let rest = engine
            .pull(&first, &workspace, page_one.next_seq, Some(1))
            .unwrap();
        assert_eq!(rest.changes[0].version, 2);
        assert!(!rest.has_more);
        let empty = engine
            .pull(&first, &workspace, rest.next_seq, None)
            .unwrap();
        assert!(empty.changes.is_empty());
        assert_eq!(empty.next_seq, rest.next_seq);
    }

    #[test]
    fn replays_operations_idempotently() {
        let (engine, first, second, workspace) = setup();
        let operation = upsert(&id(), 0, "离线创建");
        let request = push(vec![operation.clone()]);
        let one = engine.push(&first, &workspace, &request).unwrap();
        let two = engine.push(&first, &workspace, &request).unwrap();
        assert_eq!(one, two);
        assert_eq!(
            engine
                .pull(&first, &workspace, 0, None)
                .unwrap()
                .changes
                .len(),
            1
        );

        let mut changed = operation.clone();
        changed.change = upsert(&operation.object_id, 0, "不同内容").change;
        let error = engine
            .push(&first, &workspace, &push(vec![changed]))
            .unwrap_err();
        assert_eq!(error.code, "OPERATION_REUSED");
        assert_eq!(error.status, 409);

        // The same operation ID from another device is a different operation.
        let other = engine.push(&second, &workspace, &request).unwrap();
        assert!(matches!(other.results[0], PushResult::Conflict { .. }));
    }

    #[test]
    fn deletes_and_resurrects_by_version() {
        let (engine, first, second, workspace) = setup();
        let page = id();
        engine
            .push(&first, &workspace, &push(vec![upsert(&page, 0, "页面")]))
            .unwrap();
        let delete = PushOperation {
            op_id: id(),
            object_id: page.clone(),
            base_version: 1,
            change: ObjectChange::Delete,
        };
        let deleted = engine
            .push(&second, &workspace, &push(vec![delete]))
            .unwrap();
        assert!(matches!(
            deleted.results[0],
            PushResult::Accepted { version: 2, .. }
        ));
        let restored = engine
            .push(
                &first,
                &workspace,
                &push(vec![upsert(&page, 2, "保留的修改")]),
            )
            .unwrap();
        assert!(matches!(
            restored.results[0],
            PushResult::Accepted { version: 3, .. }
        ));
        let missing = PushOperation {
            op_id: id(),
            object_id: id(),
            base_version: 1,
            change: ObjectChange::Delete,
        };
        let conflict = engine
            .push(&first, &workspace, &push(vec![missing]))
            .unwrap();
        assert!(matches!(
            &conflict.results[0],
            PushResult::Conflict { current: None, .. }
        ));
    }

    #[test]
    fn revoked_devices_and_unknown_workspaces_are_rejected() {
        let engine = Engine::open_for_tests().unwrap();
        let issued = engine.register_device("旧手机").unwrap();
        let device = engine.authenticate(&issued.token).unwrap();
        assert_eq!(
            engine.pull(&device, &id(), 0, None).unwrap_err().code,
            "WORKSPACE_NOT_FOUND"
        );
        assert!(engine.devices().unwrap()[0].last_seen_at.is_some());
        engine.revoke_device(&device.id).unwrap();
        assert_eq!(
            engine.authenticate(&issued.token).unwrap_err().code,
            "UNAUTHORIZED"
        );
        assert!(engine.devices().unwrap()[0].revoked_at.is_some());
        assert_eq!(engine.revoke_device(&id()).unwrap_err().status, 404);
        assert_eq!(engine.authenticate("zsd_short").unwrap_err().status, 401);
        assert_eq!(engine.register_device(" ").unwrap_err().status, 400);
    }

    #[test]
    fn stores_originals_and_requires_them_before_push() {
        use sha2::{Digest, Sha256};
        use zhiye_sync_protocol::{BlobQuery, SyncAttachment};
        let (engine, first, second, workspace) = setup();
        let bytes = b"\x89PNG fixture";
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let query = BlobQuery {
            hashes: vec![hash.clone(), hash.clone()],
        };
        assert_eq!(
            engine
                .missing_blobs(&first, &workspace, &query)
                .unwrap()
                .missing,
            std::slice::from_ref(&hash)
        );

        let mut operation = upsert(&id(), 0, "带图页面");
        if let ObjectChange::Upsert { page } = &mut operation.change {
            page.attachments.push(SyncAttachment {
                id: id(),
                name: "海湾.png".into(),
                sha256: hash.clone(),
                bytes: bytes.len() as u64,
                media_type: "image/png".into(),
                created_at: 1,
            });
        }
        let error = engine
            .push(&first, &workspace, &push(vec![operation.clone()]))
            .unwrap_err();
        assert_eq!((error.status, error.code), (409, "BLOB_MISSING"));

        let wrong = engine
            .put_blob(&first, &workspace, &hash, b"other")
            .unwrap_err();
        assert_eq!(wrong.code, "BLOB_MISMATCH");
        assert_eq!(
            engine
                .put_blob(&first, &workspace, "ABC", bytes)
                .unwrap_err()
                .status,
            400
        );
        engine.put_blob(&first, &workspace, &hash, bytes).unwrap();
        engine.put_blob(&first, &workspace, &hash, bytes).unwrap();
        assert!(
            engine
                .missing_blobs(&first, &workspace, &query)
                .unwrap()
                .missing
                .is_empty()
        );
        assert!(matches!(
            engine
                .push(&first, &workspace, &push(vec![operation]))
                .unwrap()
                .results[0],
            PushResult::Accepted { .. }
        ));
        assert_eq!(engine.blob(&second, &workspace, &hash).unwrap(), bytes);
        let absent = "0".repeat(64);
        assert_eq!(
            engine
                .blob(&second, &workspace, &absent)
                .unwrap_err()
                .status,
            404
        );
        assert_eq!(
            engine.blob(&second, &id(), &hash).unwrap_err().code,
            "WORKSPACE_NOT_FOUND"
        );
    }

    /// Runs only with `ZHIYE_SYNC_TEST_DATABASE_URL`: concurrent pushes to the
    /// same object from two connections accept exactly one, and a second
    /// instance reopens the existing schema.
    #[test]
    fn postgres_serializes_pushes_and_reopens() {
        let Ok(url) = std::env::var("ZHIYE_SYNC_TEST_DATABASE_URL") else {
            return;
        };
        let mut config: postgres::Config = url.parse().unwrap();
        let schema = format!("zhiye_test_{}", uuid::Uuid::new_v4().simple());
        config
            .connect(postgres::NoTls)
            .unwrap()
            .batch_execute(&format!("CREATE SCHEMA {schema}"))
            .unwrap();
        config.options(&format!("-c search_path={schema}"));
        let blobs = tempfile::tempdir().unwrap();
        let first = Engine::open_postgres_config(config.clone(), blobs.path()).unwrap();
        let issued = first.register_device("书房电脑").unwrap();
        let device = first.authenticate(&issued.token).unwrap();
        let workspace = id();
        first
            .put_workspace(&device, &workspace, "个人资料")
            .unwrap();
        let second = Engine::open_postgres_config(config, blobs.path()).unwrap();
        assert_eq!(second.devices().unwrap().len(), 1, "the schema is reused");

        let page = id();
        let engines = [&first, &second];
        let results: Vec<PushResult> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|index| {
                    let engine = engines[index % 2];
                    let (device, workspace, page) = (&device, &workspace, &page);
                    scope.spawn(move || {
                        engine
                            .push(device, workspace, &push(vec![upsert(page, 0, "同时创建")]))
                            .unwrap()
                            .results
                            .remove(0)
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|handle| handle.join().unwrap())
                .collect()
        });
        let accepted = results
            .iter()
            .filter(|result| matches!(result, PushResult::Accepted { .. }))
            .count();
        assert_eq!(accepted, 1, "{results:?}");
        let pulled = second.pull(&device, &workspace, 0, None).unwrap();
        assert_eq!(pulled.changes.len(), 1);
    }

    #[test]
    fn reopens_existing_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let token = {
            let engine = Engine::open(directory.path()).unwrap();
            engine.register_device("NAS").unwrap().token
        };
        let engine = Engine::open(directory.path()).unwrap();
        assert!(engine.authenticate(&token).is_ok());
        assert!(directory.path().join("blobs").is_dir());
    }
}
