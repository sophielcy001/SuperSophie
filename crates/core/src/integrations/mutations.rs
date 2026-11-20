// SPDX-License-Identifier: AGPL-3.0-or-later

use super::edit_blocks::apply_operations;
use super::{
    Access, IntegrationBlockOperation, IntegrationScope,
    content::{can_read_property, project_page},
    missing,
    queries::authorized_collection,
};
use crate::collections::{field_kind, for_parent, normalize_values, relations::apply_delta};
use crate::model::{name, new_id, valid_icon};
use crate::page_management::{active_subtree, archive_subtrees};
use crate::store::{
    create_page_in, ensure_revision, insert_new_page_with_attachments, new_page, now, read_page,
    read_page_info, stage_edit, stage_edit_with_related,
};
use crate::{Block, Collection, Error, Page, PageInfo, PropertyKind, Result, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;
pub(super) const MAX_OPERATIONS: usize = 100;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PagePatch {
    pub title: Option<String>,
    pub icon: Option<String>,
}

#[derive(Serialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum IntegrationMutation {
    CreatePage {
        parent_id: String,
        title: String,
        icon: String,
    },
    CreateRecord {
        collection_id: String,
        title: String,
        icon: String,
        properties: BTreeMap<String, Value>,
    },
    PatchPage {
        page_id: String,
        patch: PagePatch,
    },
    PatchProperties {
        page_id: String,
        patch: PagePatch,
        properties: BTreeMap<String, Value>,
    },
    AppendParagraphs {
        page_id: String,
        paragraphs: Vec<String>,
    },
    EditBlocks {
        page_id: String,
        operations: Vec<IntegrationBlockOperation>,
    },
    ArchivePage {
        page_id: String,
    },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IntegrationSource {
    Integration { id: String },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntegrationOperation {
    pub operation_id: String,
    pub page_id: String,
    pub revision: u32,
    pub created_block_ids: Vec<String>,
    pub source: IntegrationSource,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionAttribution {
    pub integration_id: String,
    pub integration_name: String,
    pub operation_id: String,
}

struct AppliedMutation {
    page: PageInfo,
    revisions: Vec<PageInfo>,
    created_block_ids: Vec<String>,
}

impl AppliedMutation {
    fn page(page: PageInfo, changed: bool) -> Self {
        let revisions = if changed {
            vec![page.clone()]
        } else {
            Vec::new()
        };
        Self {
            page,
            revisions,
            created_block_ids: Vec::new(),
        }
    }
}

impl IntegrationMutation {
    fn authorize(&self, tx: &Transaction<'_>, access: &Access) -> Result<Option<String>> {
        match self {
            Self::CreatePage { parent_id, .. } => {
                access.require(IntegrationScope::Create)?;
                access.require_parent(tx, parent_id)?;
                Ok(None)
            }
            Self::CreateRecord { collection_id, .. } => {
                access.require(IntegrationScope::Create)?;
                let collection = authorized_collection(tx, access, collection_id)?;
                access.require_parent(tx, &collection.container_page_id)?;
                Ok(None)
            }
            Self::PatchPage { page_id, .. }
            | Self::PatchProperties { page_id, .. }
            | Self::EditBlocks { page_id, .. }
            | Self::AppendParagraphs { page_id, .. } => {
                access.require(IntegrationScope::Update)?;
                access.require_page(tx, page_id)?;
                Ok(Some(page_id.clone()))
            }
            Self::ArchivePage { page_id } => {
                access.require(IntegrationScope::Archive)?;
                access.require_page(tx, page_id)?;
                Ok(Some(page_id.clone()))
            }
        }
    }

    fn apply(
        self,
        tx: &Transaction<'_>,
        access: &Access,
        base: Option<u32>,
    ) -> Result<AppliedMutation> {
        match self {
            Self::CreatePage {
                parent_id,
                title,
                icon,
            } => {
                let page = create_page_in(tx, Some(&parent_id), &title, &icon)?;
                Ok(AppliedMutation::page(page.info, true))
            }
            Self::CreateRecord {
                collection_id,
                title,
                icon,
                properties,
            } => {
                let collection = authorized_collection(tx, access, &collection_id)?;
                let mut record = new_page(Some(&collection.container_page_id), &title, &icon)?;
                let mut values = BTreeMap::new();
                apply_properties(tx, access, &collection, &mut values, properties, true)?;
                record.properties = Some(values);
                let (page, related) = insert_new_page_with_attachments(tx, record, &[])?;
                let mut result = AppliedMutation::page(page.info, true);
                result.revisions.extend(related);
                Ok(result)
            }
            Self::PatchPage { page_id, patch } => {
                patch_page(tx, access, &page_id, base, patch, BTreeMap::new())
            }
            Self::PatchProperties {
                page_id,
                patch,
                properties,
            } => patch_page(tx, access, &page_id, base, patch, properties),
            Self::AppendParagraphs {
                page_id,
                paragraphs,
            } => {
                let base = base.ok_or_else(precondition_required)?;
                if paragraphs.len() > MAX_OPERATIONS {
                    return Err(Error::new(
                        "TOO_MANY_OPERATIONS",
                        "每批最多可以追加 100 个正文块。",
                    ));
                }
                if paragraphs.is_empty() || paragraphs.iter().any(|text| text.contains('\0')) {
                    return Err(Error::new("INVALID_BLOCK", "请提供有效的段落正文。"));
                }
                let blocks: Vec<_> = paragraphs.iter().map(|text| paragraph(text)).collect();
                let created = blocks.iter().map(|block| block.id.clone()).collect();
                let after = stage_edit(tx, &page_id, base, |_, page| {
                    page.blocks.extend(blocks);
                    Ok(())
                })?;
                let mut result = AppliedMutation::page(after.info, true);
                result.created_block_ids = created;
                Ok(result)
            }
            Self::EditBlocks {
                page_id,
                operations,
            } => {
                let base = base.ok_or_else(precondition_required)?;
                let mut created = Vec::new();
                let after = stage_edit(tx, &page_id, base, |tx, page| {
                    created = apply_operations(tx, access, &page_id, &mut page.blocks, operations)?;
                    Ok(())
                })?;
                let changed = after.info.revision != base;
                let mut result = AppliedMutation::page(after.info, changed);
                result.created_block_ids = created;
                Ok(result)
            }
            Self::ArchivePage { page_id } => {
                let ids = active_subtree(tx, &page_id)?;
                for id in &ids {
                    if !access.can_read(tx, id)? {
                        return Err(partial_content());
                    }
                }
                let revisions = archive_subtrees(tx, &[(page_id.clone(), ids)])?;
                let page = revisions
                    .iter()
                    .find(|page| page.id == page_id)
                    .ok_or_else(missing)?
                    .clone();
                Ok(AppliedMutation {
                    page,
                    revisions,
                    created_block_ids: Vec::new(),
                })
            }
        }
    }
}

fn patch_page(
    tx: &Transaction<'_>,
    access: &Access,
    page_id: &str,
    base: Option<u32>,
    patch: PagePatch,
    properties: BTreeMap<String, Value>,
) -> Result<AppliedMutation> {
    let base = base.ok_or_else(precondition_required)?;
    if patch.title.is_none() && patch.icon.is_none() && properties.is_empty() {
        return Err(Error::new(
            "INVALID_PATCH",
            "请选择要修改的标题、图标或属性。",
        ));
    }
    let (after, related) = stage_edit_with_related(tx, page_id, base, |tx, page| {
        apply_page_patch(tx, access, page, patch, properties)
    })?;
    let changed = after.info.revision != base;
    let mut result = AppliedMutation::page(after.info, changed);
    result.revisions.extend(related);
    Ok(result)
}

pub(super) fn apply_page_patch(
    db: &Connection,
    access: &Access,
    page: &mut Page,
    patch: PagePatch,
    properties: BTreeMap<String, Value>,
) -> Result<()> {
    if let Some(title) = patch.title {
        page.info.title = name(&title, 200, Some("无标题"))?;
    }
    if let Some(icon) = patch.icon {
        if !valid_icon(&icon) {
            return Err(Error::new("INVALID_ICON", "页面图标无效。"));
        }
        page.info.icon = icon;
    }
    if !properties.is_empty() {
        let collection_id = for_parent(db, page.info.parent_id.as_deref())?
            .ok_or_else(|| Error::new("RECORD_NOT_FOUND", "普通属性需要数据库记录。"))?;
        let collection = authorized_collection(db, access, &collection_id).map_err(|error| {
            if error.code == "RESOURCE_NOT_FOUND" {
                partial_content()
            } else {
                error
            }
        })?;
        let current = page.properties.as_mut().ok_or_else(missing)?;
        apply_properties(db, access, &collection, current, properties, false)?;
    }
    Ok(())
}

fn apply_properties(
    db: &Connection,
    access: &Access,
    collection: &Collection,
    current: &mut BTreeMap<String, Value>,
    properties: BTreeMap<String, Value>,
    creating: bool,
) -> Result<()> {
    let mut ordinary = BTreeMap::new();
    for (id, value) in properties {
        let kind = field_kind(&collection.properties, &id)?;
        if !can_read_property(access, kind) {
            return Err(partial_content());
        }
        if kind == PropertyKind::Attachment {
            return Err(Error::new("FORBIDDEN", "请通过专用附件操作管理附件。"));
        }
        if kind == PropertyKind::Relation {
            let field = collection
                .properties
                .iter()
                .find(|field| field.id == id)
                .ok_or_else(missing)?;
            apply_delta(db, field, current, value, |page| access.can_read(db, page))?;
            if creating
                && current.contains_key(&id)
                && field
                    .relation
                    .as_ref()
                    .is_some_and(|relation| relation.inverse_property_id.is_some())
            {
                access.require(IntegrationScope::Update)?;
            }
        } else {
            current.remove(&id);
            ordinary.insert(id, value);
        }
    }
    current.extend(normalize_values(&collection.properties, ordinary, |_| {
        Ok(false)
    })?);
    Ok(())
}

pub(super) fn partial_content() -> Error {
    Error::new("PARTIAL_CONTENT", "目标包含当前授权范围外的内容。")
}

impl Store {
    pub fn mutate_as_integration(
        &self,
        workspace: &str,
        token: &str,
        request_key: &str,
        if_match: Option<&str>,
        mutation: IntegrationMutation,
    ) -> Result<IntegrationOperation> {
        self.mutate_as_integration_at(workspace, token, request_key, if_match, None, mutation)
    }

    pub fn mutate_as_integration_at(
        &self,
        workspace: &str,
        token: &str,
        request_key: &str,
        if_match: Option<&str>,
        expected_revision: Option<u32>,
        mutation: IntegrationMutation,
    ) -> Result<IntegrationOperation> {
        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let access = Access::authenticate(&tx, token)?;
        let target = mutation.authorize(&tx, &access)?;
        if target.is_some() && if_match.is_none() {
            return Err(precondition_required());
        }
        if expected_revision.is_some() && target.is_none() {
            return Err(Error::new("INVALID_REQUEST", "创建页面时无需基准修订。"));
        }
        validate_request_key(request_key)?;
        let request = match expected_revision {
            Some(revision) => serde_json::to_vec(&(if_match, &mutation, revision))?,
            None => serde_json::to_vec(&(if_match, &mutation))?,
        };
        let digest = Sha256::digest(request);
        let time = now();
        if let Some(result) =
            replay::<IntegrationOperation>(&tx, &access, request_key, &digest, time)?
        {
            access.require_page(&tx, &result.page_id)?;
            access.mark_use(&tx)?;
            tx.commit()?;
            return Ok(result);
        }

        let base = target
            .as_deref()
            .map(|id| {
                let page = checked_page(&tx, &access, id, if_match)?;
                if let Some(revision) = expected_revision {
                    ensure_revision(&page.info, revision)?;
                }
                Ok::<_, Error>(page.info.revision)
            })
            .transpose()?;
        let archived = matches!(mutation, IntegrationMutation::ArchivePage { .. });
        let applied = mutation.apply(&tx, &access, base)?;
        if !archived {
            access.require_page(&tx, &applied.page.id)?;
        }

        let result = IntegrationOperation {
            operation_id: new_id(),
            page_id: applied.page.id,
            revision: applied.page.revision,
            created_block_ids: applied.created_block_ids,
            source: IntegrationSource::Integration {
                id: access.id.clone(),
            },
        };
        for page in applied.revisions {
            attribute(&tx, &access, &page, &result.operation_id)?;
        }
        remember(&tx, &access, request_key, &digest, &result, time)?;
        access.mark_use(&tx)?;
        tx.commit()?;
        Ok(result)
    }

    pub fn revision_attributions(
        &self,
        workspace: &str,
        page_id: &str,
    ) -> Result<BTreeMap<u32, RevisionAttribution>> {
        let db = self.connection(workspace)?;
        read_page_info(&db, page_id)?;
        let mut query = db.prepare(
            "SELECT r.revision, r.integration_id, i.name, r.operation_id
             FROM integration_revisions r JOIN integrations i ON i.id = r.integration_id
             WHERE r.page_id = ?1",
        )?;
        Ok(query
            .query_map([page_id], |row| {
                Ok((
                    row.get(0)?,
                    RevisionAttribution {
                        integration_id: row.get(1)?,
                        integration_name: row.get(2)?,
                        operation_id: row.get(3)?,
                    },
                ))
            })?
            .collect::<rusqlite::Result<_>>()?)
    }
}

pub(super) fn paragraph(text: &str) -> Block {
    let mut inline = Vec::<Value>::new();
    for (index, line) in text.split('\n').enumerate() {
        if index != 0 {
            inline.push(json!({"type": "hardBreak"}));
        }
        let line = line.trim_end_matches('\r');
        if !line.is_empty() {
            inline.push(json!({"type": "text", "text": line}));
        }
    }
    Block {
        id: new_id(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline": inline}),
        children: vec![],
        extra: Default::default(),
    }
}

pub(super) fn precondition_required() -> Error {
    Error::new("PRECONDITION_REQUIRED", "修改页面需要最近读取的 If-Match。")
}

pub(super) fn validate_request_key(key: &str) -> Result<()> {
    if key.is_empty() || key.len() > 128 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(Error::new(
            "IDEMPOTENCY_KEY",
            "请提供 1 至 128 个可见 ASCII 字符作为幂等键。",
        ));
    }
    Ok(())
}

pub(super) fn checked_page(
    tx: &Transaction<'_>,
    access: &Access,
    page_id: &str,
    if_match: Option<&str>,
) -> Result<Page> {
    let page = read_page(tx, page_id)?;
    let visible = project_page(tx, access, &page)?;
    if if_match != Some(visible.etag.as_str()) {
        return Err(Error::new(
            "REVISION_CONFLICT",
            "页面内容或授权表示已更新，请重新读取。",
        ));
    }
    Ok(page)
}

pub(super) fn replay<T: DeserializeOwned>(
    tx: &Transaction<'_>,
    access: &Access,
    key: &str,
    digest: &[u8],
    time: i64,
) -> Result<Option<T>> {
    let saved = tx
        .query_row(
            "SELECT request_hash, response FROM integration_operations
         WHERE integration_id = ?1 AND request_key = ?2 AND expires_at > ?3",
            params![access.id, key, time],
            |row| Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((hash, response)) = saved else {
        return Ok(None);
    };
    if hash != digest {
        return Err(Error::new(
            "IDEMPOTENCY_CONFLICT",
            "此幂等键已用于另一项请求。",
        ));
    }
    Ok(Some(serde_json::from_str(&response)?))
}

pub(super) fn remember(
    tx: &Transaction<'_>,
    access: &Access,
    key: &str,
    digest: &[u8],
    result: &impl Serialize,
    time: i64,
) -> Result<()> {
    tx.execute(
        "DELETE FROM integration_operations WHERE expires_at <= ?1",
        [time],
    )?;
    tx.execute(
        "INSERT INTO integration_operations VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            access.id,
            key,
            digest,
            serde_json::to_string(&result)?,
            time + RETENTION_MS
        ],
    )?;
    Ok(())
}

pub(super) fn attribute(
    tx: &Transaction<'_>,
    access: &Access,
    page: &PageInfo,
    operation: &str,
) -> Result<()> {
    tx.execute(
        "INSERT INTO integration_revisions VALUES (?1, ?2, ?3, ?4)",
        params![page.id, page.revision, access.id, operation],
    )?;
    Ok(())
}
