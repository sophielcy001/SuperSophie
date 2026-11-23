// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{
    Access, IntegrationScope, IntegrationSource,
    mutations::{
        attribute, checked_page, precondition_required, remember, replay, validate_request_key,
    },
};
use crate::assets::{
    blob_directory, insert_attachment, matching_attachment, persist_blob, transfer,
    valid_attachment_name,
};
use crate::model::new_id;
use crate::store::{commit_revision, now};
use crate::{Attachment, Error, Result, Store};
use rusqlite::{Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;

const MIB: u64 = 1024 * 1024;

pub struct IntegrationAssetUpload<'a> {
    pub page_id: &'a str,
    pub name: &'a str,
    pub request_key: &'a str,
    pub if_match: Option<&'a str>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IntegrationUploadResult {
    pub operation_id: String,
    pub asset_id: String,
    pub page_id: String,
    pub revision: u32,
    pub source: IntegrationSource,
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
    pub media_type: String,
}

impl Store {
    pub fn api_attachment_limit_mib(&self, workspace: &str) -> Result<u32> {
        read_limit(&self.connection(workspace)?)
    }

    pub fn set_api_attachment_limit_mib(&self, workspace: &str, limit: u32) -> Result<()> {
        if !(1..=100).contains(&limit) {
            return Err(Error::new(
                "UPLOAD_LIMIT",
                "本机 API 附件大小上限需要在 1 到 100 MiB 之间。",
            ));
        }
        self.connection(workspace)?.execute(
            "UPDATE workspace_state SET max_attachment_mib = ?1 WHERE slot = 1",
            [limit],
        )?;
        Ok(())
    }

    pub fn integration_upload_limit(
        &self,
        workspace: &str,
        token: &str,
        request: &IntegrationAssetUpload<'_>,
    ) -> Result<u64> {
        self.integration_read(workspace, token, |db, access| {
            authorize(db, access, request)?;
            Ok(u64::from(read_limit(db)?) * MIB)
        })
    }

    pub fn upload_as_integration(
        &self,
        workspace: &str,
        token: &str,
        request: IntegrationAssetUpload<'_>,
        source: impl Read,
    ) -> Result<IntegrationUploadResult> {
        let limit = self.integration_upload_limit(workspace, token, &request)?;
        let directory = blob_directory(&self.data_root().join(workspace), true)?;
        let mut staged = tempfile::NamedTempFile::new_in(&directory)?;
        let content = transfer(source, &mut staged, limit)?;
        let hash = Sha256::digest(serde_json::to_vec(&(
            "upload_asset",
            request.page_id,
            request.name,
            request.if_match,
            &content.sha256,
        ))?);

        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let access = Access::authenticate(&tx, token)?;
        authorize(&tx, &access, &request)?;
        let time = now();
        if let Some(previous) =
            replay::<IntegrationUploadResult>(&tx, &access, request.request_key, &hash, time)?
        {
            access.mark_use(&tx)?;
            tx.commit()?;
            return Ok(previous);
        }
        if content.bytes > u64::from(read_limit(&tx)?) * MIB {
            return Err(Error::new("FILE_TOO_LARGE", "附件超过允许的文件大小。"));
        }
        let mut page = checked_page(&tx, &access, request.page_id, request.if_match)?;
        let candidate = Attachment {
            id: new_id(),
            page_id: request.page_id.into(),
            name: request.name.into(),
            sha256: content.sha256,
            bytes: content.bytes,
            media_type: content.media_type.into(),
            created_at: time,
        };
        let existing = matching_attachment(&tx, &candidate)?;
        let publish = existing.is_none();
        let attachment = existing.unwrap_or(candidate);
        let operation_id = new_id();
        if publish {
            insert_attachment(&tx, &attachment)?;
            commit_revision(&tx, &mut page)?;
            attribute(&tx, &access, &page.info, &operation_id)?;
        }
        let result = IntegrationUploadResult {
            operation_id,
            asset_id: attachment.id.clone(),
            page_id: page.info.id,
            revision: page.info.revision,
            source: IntegrationSource::Integration {
                id: access.id.clone(),
            },
            name: attachment.name.clone(),
            bytes: attachment.bytes,
            sha256: attachment.sha256.clone(),
            media_type: attachment.media_type.clone(),
        };
        remember(&tx, &access, request.request_key, &hash, &result, time)?;
        access.mark_use(&tx)?;
        persist_blob(staged, &directory, &attachment)?;
        tx.commit()?;
        Ok(result)
    }
}

fn read_limit(db: &Connection) -> Result<u32> {
    Ok(db.query_row(
        "SELECT max_attachment_mib FROM workspace_state WHERE slot = 1",
        [],
        |row| row.get(0),
    )?)
}

fn authorize(db: &Connection, access: &Access, request: &IntegrationAssetUpload<'_>) -> Result<()> {
    access.require(IntegrationScope::WriteAssets)?;
    access.require(IntegrationScope::Update)?;
    access.require_page(db, request.page_id)?;
    validate_request_key(request.request_key)?;
    if request.if_match.is_none() {
        return Err(precondition_required());
    }
    if !valid_attachment_name(request.name) {
        return Err(Error::new("INVALID_ATTACHMENT", "附件名称不受支持。"));
    }
    Ok(())
}
