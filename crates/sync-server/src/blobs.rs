// SPDX-License-Identifier: AGPL-3.0-or-later

//! Content-addressed storage for attachment originals.

use crate::engine::{ServiceError, ServiceResult};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use zhiye_sync_protocol::{MAX_BLOB_BYTES, valid_sha256};

pub(crate) enum BlobStore {
    Directory(PathBuf),
    Memory(Mutex<HashMap<String, Vec<u8>>>),
}

fn invalid_hash() -> ServiceError {
    ServiceError::new(400, "INVALID_REQUEST", "原件标识需要是小写的 SHA-256。")
}

fn storage(error: impl std::fmt::Display) -> ServiceError {
    ServiceError::new(500, "STORAGE", format!("同步服务无法读写附件原件。{error}"))
}

impl BlobStore {
    pub(crate) fn directory(root: PathBuf) -> ServiceResult<Self> {
        std::fs::create_dir_all(&root).map_err(storage)?;
        Ok(Self::Directory(root))
    }

    pub(crate) fn memory() -> Self {
        Self::Memory(Mutex::new(HashMap::new()))
    }

    pub(crate) fn contains(&self, hash: &str) -> ServiceResult<bool> {
        if !valid_sha256(hash) {
            return Err(invalid_hash());
        }
        match self {
            Self::Directory(root) => Ok(root.join(hash).is_file()),
            Self::Memory(map) => Ok(map.lock().map_err(storage_state)?.contains_key(hash)),
        }
    }

    /// Stores `bytes` under `hash` after checking the digest.
    pub(crate) fn put(&self, hash: &str, bytes: &[u8]) -> ServiceResult<()> {
        if !valid_sha256(hash) {
            return Err(invalid_hash());
        }
        if bytes.len() as u64 > MAX_BLOB_BYTES {
            return Err(ServiceError::new(
                413,
                "BLOB_TOO_LARGE",
                "附件原件最大为 100 MiB。",
            ));
        }
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != hash {
            return Err(ServiceError::new(
                400,
                "BLOB_MISMATCH",
                "上传的内容与原件标识不一致。",
            ));
        }
        match self {
            Self::Directory(root) => {
                let target = root.join(hash);
                if target.is_file() {
                    return Ok(());
                }
                let mut staging = tempfile::NamedTempFile::new_in(root).map_err(storage)?;
                staging.write_all(bytes).map_err(storage)?;
                staging.as_file().sync_all().map_err(storage)?;
                match staging.persist_noclobber(&target) {
                    Ok(_) => Ok(()),
                    Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
                    Err(error) => Err(storage(error.error)),
                }
            }
            Self::Memory(map) => {
                map.lock()
                    .map_err(storage_state)?
                    .entry(hash.to_owned())
                    .or_insert_with(|| bytes.to_vec());
                Ok(())
            }
        }
    }

    pub(crate) fn get(&self, hash: &str) -> ServiceResult<Option<Vec<u8>>> {
        if !valid_sha256(hash) {
            return Err(invalid_hash());
        }
        match self {
            Self::Directory(root) => match std::fs::read(root.join(hash)) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(storage(error)),
            },
            Self::Memory(map) => Ok(map.lock().map_err(storage_state)?.get(hash).cloned()),
        }
    }
}

fn storage_state<T>(_: T) -> ServiceError {
    ServiceError::new(500, "SERVICE_STATE", "同步服务需要重新启动。")
}
