// SPDX-License-Identifier: AGPL-3.0-or-later

mod computed_changes;
mod content;
mod cursors;
pub(crate) mod derived;
mod edit_blocks;
pub(crate) mod feed;
mod feed_cursors;
mod mutations;
mod proposals;
mod queries;
mod uploads;

pub use content::{
    IntegrationAssetDownload, IntegrationBlock, IntegrationMarkdown, IntegrationPage,
    IntegrationReference, LocatedIntegrationBlock, integration_markdown,
};
pub(crate) use content::{reference_inline, source_anchors, visible_blocks};
pub use cursors::IntegrationQueryPage;
pub use edit_blocks::{IntegrationBlockDraft, IntegrationBlockOperation};
pub use feed::{CatalogItem, CatalogPage, ChangeEvent, ChangePage, MoveImpact};
pub use mutations::{
    IntegrationMutation, IntegrationOperation, IntegrationSource, PagePatch, RevisionAttribution,
};
pub use proposals::{
    ProposalAction, ProposalChanges, ProposalDraft, ProposalReceipt, ProposalReview,
    ProposalSnapshot, ProposalStatus, ProposalSummary,
};
pub use queries::{
    IntegrationCollection, IntegrationCollectionQuery, IntegrationCollectionRecords,
    IntegrationFilter, IntegrationRecord, IntegrationSearchHit, IntegrationSearchKind,
    IntegrationSearchRequest, IntegrationSearchSource, IntegrationSort,
};
pub use uploads::{IntegrationAssetUpload, IntegrationUploadResult};

use crate::assets::hex_sha256;
use crate::model::{name, new_id, valid_id};
use crate::store::now;
use crate::{Error, Result, Store};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashSet};

const TOKEN_PREFIX: &str = "zhiye_";
const LAST_USED_INTERVAL: i64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum IntegrationScope {
    #[serde(rename = "content:read")]
    Read,
    #[serde(rename = "content:create")]
    Create,
    #[serde(rename = "content:update")]
    Update,
    #[serde(rename = "content:archive")]
    Archive,
    #[serde(rename = "assets:read")]
    ReadAssets,
    #[serde(rename = "assets:write")]
    WriteAssets,
    #[serde(rename = "changes:read")]
    Changes,
    #[serde(rename = "proposals:create")]
    Propose,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "id",
    rename_all = "camelCase",
    deny_unknown_fields
)]
pub enum IntegrationGrant {
    Page(String),
    Subtree(String),
    Collection(String),
}

impl IntegrationGrant {
    fn fields(&self) -> (&'static str, &str) {
        match self {
            Self::Page(id) => ("page", id),
            Self::Subtree(id) => ("subtree", id),
            Self::Collection(id) => ("collection", id),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationDraft {
    pub name: String,
    pub scopes: BTreeSet<IntegrationScope>,
    pub grants: Vec<IntegrationGrant>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    pub scopes: BTreeSet<IntegrationScope>,
    pub grants: Vec<IntegrationGrant>,
    pub created_at: i64,
    pub expires_at: Option<i64>,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

// Only the creation command returns the credential.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IssuedIntegration {
    pub integration: Integration,
    pub token: String,
}

impl Store {
    pub fn integrations(&self, workspace: &str) -> Result<Vec<Integration>> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let rows = {
            let mut query = tx.prepare(
                "SELECT id, name, fingerprint, scopes, created_at, expires_at, last_used_at, revoked_at
                 FROM integrations
                 ORDER BY (revoked_at IS NOT NULL OR coalesce(expires_at <= ?1, 0)), created_at DESC, id",
            )?;
            query
                .query_map([now()], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get::<_, String>(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut result = Vec::with_capacity(rows.len());
        let mut grants = tx.prepare(
            "SELECT kind, resource_id FROM integration_grants
             WHERE integration_id = ?1 ORDER BY kind, resource_id",
        )?;
        for (id, name, fingerprint, scopes, created_at, expires_at, last_used_at, revoked_at) in
            rows
        {
            let resources = grants
                .query_map([&id], |row| {
                    let id = row.get(1)?;
                    match row.get::<_, String>(0)?.as_str() {
                        "page" => Ok(IntegrationGrant::Page(id)),
                        "subtree" => Ok(IntegrationGrant::Subtree(id)),
                        "collection" => Ok(IntegrationGrant::Collection(id)),
                        _ => Err(rusqlite::Error::InvalidQuery),
                    }
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            result.push(Integration {
                id,
                name,
                fingerprint,
                scopes: serde_json::from_str(&scopes)?,
                grants: resources,
                created_at,
                expires_at,
                last_used_at,
                revoked_at,
            });
        }
        Ok(result)
    }

    pub fn issue_integration(
        &self,
        workspace: &str,
        draft: IntegrationDraft,
    ) -> Result<IssuedIntegration> {
        self.issue_integration_with_expiry(workspace, draft, None)
    }

    pub fn issue_integration_with_expiry(
        &self,
        workspace: &str,
        draft: IntegrationDraft,
        expires_at: Option<i64>,
    ) -> Result<IssuedIntegration> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        validate_grants(&tx, &draft)?;
        let name = name(&draft.name, 64, None)?;
        let created_at = now();
        if expires_at.is_some_and(|deadline| {
            deadline <= created_at || chrono::DateTime::from_timestamp_millis(deadline).is_none()
        }) {
            return Err(Error::new(
                "INTEGRATION_EXPIRY",
                "请选择有效的将来到期时间。",
            ));
        }
        let mut secret = [0_u8; 32];
        getrandom::fill(&mut secret)
            .map_err(|_| Error::new("TOKEN_GENERATION", "无法生成集成令牌，请稍后重试。"))?;
        let token = format!("{TOKEN_PREFIX}{}", hex_sha256(&secret));
        let digest = Sha256::digest(token.as_bytes());
        let integration = Integration {
            id: new_id(),
            name,
            fingerprint: hex_sha256(&digest[..6]),
            scopes: draft.scopes,
            grants: draft.grants,
            created_at,
            expires_at,
            last_used_at: None,
            revoked_at: None,
        };
        tx.execute(
            "INSERT INTO integrations (id, name, token_hash, fingerprint, scopes, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                integration.id,
                integration.name,
                &digest[..],
                integration.fingerprint,
                serde_json::to_string(&integration.scopes)?,
                integration.created_at,
                integration.expires_at,
            ],
        )?;
        for grant in &integration.grants {
            let (kind, resource) = grant.fields();
            let (page, collection) = if kind == "collection" {
                (None, Some(resource))
            } else {
                (Some(resource), None)
            };
            tx.execute(
                "INSERT INTO integration_grants VALUES (?1, ?2, ?3, ?4, ?5)",
                params![integration.id, kind, resource, page, collection],
            )?;
        }
        tx.commit()?;
        Ok(IssuedIntegration { integration, token })
    }

    pub fn revoke_integration(&self, workspace: &str, id: &str) -> Result<()> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if tx.execute(
            "UPDATE integrations SET revoked_at = coalesce(revoked_at, ?2) WHERE id = ?1",
            params![id, now()],
        )? == 0
        {
            return Err(Error::new("INTEGRATION_NOT_FOUND", "找不到该集成。"));
        }
        tx.execute("DELETE FROM computed_cache WHERE reader=?1", [id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn integration_identity(&self, workspace: &str, token: &str) -> Result<String> {
        let db = self.integration_connection(workspace, token)?;
        Ok(Access::authenticate(&db, token)?.id)
    }

    pub fn integration_scopes(
        &self,
        workspace: &str,
        token: &str,
    ) -> Result<BTreeSet<IntegrationScope>> {
        self.integration_read(workspace, token, |_, access| Ok(access.scopes.clone()))
    }

    fn integration_connection(&self, workspace: &str, token: &str) -> Result<Connection> {
        if !token
            .strip_prefix(TOKEN_PREFIX)
            .is_some_and(crate::assets::valid_hash)
            || !valid_id(workspace)
        {
            return Err(unauthorized());
        }
        self.connection(workspace)
            .map_err(|error| match error.code {
                "WORKSPACE_NOT_FOUND"
                | "WORKSPACE_PATH"
                | "WORKSPACE_MISMATCH"
                | "FORMAT_VERSION" => unauthorized(),
                _ => error,
            })
    }

    fn integration_read<T>(
        &self,
        workspace: &str,
        token: &str,
        read: impl FnOnce(&Transaction<'_>, &Access) -> Result<T>,
    ) -> Result<T> {
        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction()?;
        let access = Access::authenticate(&tx, token)?;
        let value = read(&tx, &access)?;
        tx.commit()?;
        access.mark_read_use(&db);
        Ok(value)
    }
}

fn validate_grants(db: &Connection, draft: &IntegrationDraft) -> Result<()> {
    if !draft.scopes.contains(&IntegrationScope::Read) || draft.grants.is_empty() {
        return Err(Error::new("INTEGRATION_SCOPE", "请至少选择一个可读资源。"));
    }
    if draft.scopes.contains(&IntegrationScope::WriteAssets)
        && !draft.scopes.contains(&IntegrationScope::Update)
    {
        return Err(Error::new(
            "INTEGRATION_SCOPE",
            "上传附件需要页面修改权限。",
        ));
    }
    if draft.scopes.contains(&IntegrationScope::Create)
        && draft
            .grants
            .iter()
            .all(|grant| matches!(grant, IntegrationGrant::Page(_)))
    {
        return Err(Error::new(
            "INTEGRATION_SCOPE",
            "创建内容需要选择页面子树或整个数据库。",
        ));
    }
    let mut seen = HashSet::new();
    for grant in &draft.grants {
        let (kind, id) = grant.fields();
        let exists = valid_id(id)
            && db.query_row(
                "SELECT CASE WHEN ?1 = 'collection' THEN EXISTS (
                    SELECT 1 FROM collections c JOIN pages p ON p.id = c.container_page_id
                    WHERE c.id = ?2 AND p.trash_root_id IS NULL
                ) ELSE EXISTS (
                    SELECT 1 FROM pages p WHERE p.id = ?2 AND p.trash_root_id IS NULL
                ) END",
                params![kind, id],
                |row| row.get::<_, bool>(0),
            )?;
        if !exists || !seen.insert(grant) {
            return Err(Error::new(
                "INTEGRATION_RESOURCE",
                "请选择当前工作空间中有效且互不重复的授权资源。",
            ));
        }
    }
    Ok(())
}

fn unauthorized() -> Error {
    Error::new("UNAUTHORIZED", "集成令牌无效或无法用于此工作空间。")
}

fn missing() -> Error {
    Error::new("RESOURCE_NOT_FOUND", "找不到该资源。")
}

#[derive(Clone)]
pub(crate) struct Access {
    id: String,
    scopes: BTreeSet<IntegrationScope>,
    last_used_at: Option<i64>,
    grant_revision: i64,
    range_revision: i64,
}

impl Access {
    fn authenticate(db: &Connection, token: &str) -> Result<Self> {
        let digest = Sha256::digest(token.as_bytes());
        let access = Self::load(db, "token_hash", &digest[..])?.ok_or_else(unauthorized)?;
        access.require(IntegrationScope::Read)?;
        Ok(access)
    }

    fn by_id(db: &Connection, id: &str) -> Result<Option<Self>> {
        Self::load(db, "id", id)
    }

    fn load(db: &Connection, column: &str, value: impl rusqlite::ToSql) -> Result<Option<Self>> {
        let row = db
            .query_row(
                &format!(
                    "SELECT id, scopes, last_used_at, grant_revision, range_revision
                     FROM integrations WHERE {column} = ?1 AND revoked_at IS NULL
                     AND (expires_at IS NULL OR expires_at > ?2)"
                ),
                params![value, now()],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get::<_, String>(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .optional()?;
        row.map(
            |(id, scopes, last_used_at, grant_revision, range_revision)| {
                Ok(Self {
                    id,
                    scopes: serde_json::from_str(&scopes)?,
                    last_used_at,
                    grant_revision,
                    range_revision,
                })
            },
        )
        .transpose()
    }

    fn require(&self, scope: IntegrationScope) -> Result<()> {
        if self.scopes.contains(&scope) {
            Ok(())
        } else {
            Err(Error::new("FORBIDDEN", "集成没有执行此操作的权限。"))
        }
    }

    fn mark_use(&self, db: &Connection) -> Result<()> {
        let time = now();
        db.execute(
            "UPDATE integrations SET last_used_at = ?2
             WHERE id = ?1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?2)
             AND (last_used_at IS NULL OR last_used_at <= ?3)",
            params![self.id, time, time - LAST_USED_INTERVAL],
        )?;
        Ok(())
    }

    fn mark_read_use(&self, db: &Connection) {
        if self
            .last_used_at
            .is_none_or(|time| time <= now() - LAST_USED_INTERVAL)
        {
            // Display metadata must not prevent a read during a storage failure.
            let _ = self.mark_use(db);
        }
    }

    pub(crate) fn can_read(&self, db: &Connection, id: &str) -> Result<bool> {
        self.covers(db, id, false, false)
    }

    pub(crate) fn covers_archived_record(&self, db: &Connection, id: &str) -> Result<bool> {
        self.covers(db, id, false, true)
    }

    pub(crate) fn projection_scope(&self) -> (&str, i64, i64) {
        (&self.id, self.grant_revision, self.range_revision)
    }

    fn require_page(&self, db: &Connection, id: &str) -> Result<()> {
        if self.can_read(db, id)? {
            Ok(())
        } else {
            Err(missing())
        }
    }

    fn require_parent(&self, db: &Connection, id: &str) -> Result<()> {
        if self.covers(db, id, true, false)? {
            return Ok(());
        }
        self.require_page(db, id)?;
        Err(Error::new(
            "FORBIDDEN",
            "请选择允许创建内容的页面子树或数据库。",
        ))
    }

    pub(crate) fn readable_pages_cte(
        &self,
        arguments: &mut crate::collections::Arguments,
    ) -> String {
        self.covered_pages_cte(arguments, false)
    }

    pub(crate) fn reads_assets(&self) -> bool {
        self.scopes.contains(&IntegrationScope::ReadAssets)
    }

    fn covered_pages_cte(
        &self,
        arguments: &mut crate::collections::Arguments,
        include_archived: bool,
    ) -> String {
        let integration = arguments.bind(self.id.clone());
        let active = if include_archived {
            "1"
        } else {
            "p.trash_root_id IS NULL"
        };
        // Expand the same three resource kinds as the point lookup. Joining
        // this set before filtering, sorting or paging keeps the hidden set
        // from influencing indexes and counts returned to the caller.
        format!(
            "WITH RECURSIVE
               grants(kind, page_id, collection_id) AS (
                 SELECT kind, page_id, collection_id FROM integration_grants
                 WHERE integration_id = {integration}
               ),
               branches(id) AS (
                 SELECT p.id FROM grants g JOIN pages p ON p.id = g.page_id
                 WHERE g.kind = 'subtree' AND {active}
                 UNION
                 SELECT p.id FROM branches b JOIN pages p ON p.parent_id = b.id
                 WHERE {active}
               ),
               granted_collections(id, page_id) AS (
                 SELECT c.id, p.id FROM grants g JOIN collections c ON c.id = g.collection_id
                 JOIN pages p ON p.id = c.container_page_id WHERE {active}
               ),
               readable_pages(id) AS (
                 SELECT id FROM branches
                 UNION
                 SELECT p.id FROM grants g JOIN pages p ON p.id = g.page_id
                 WHERE g.kind = 'page' AND {active}
                 UNION
                 SELECT page_id FROM granted_collections
                 UNION
                 SELECT p.id FROM granted_collections c JOIN collection_records r ON r.collection_id = c.id
                 JOIN pages p ON p.id = r.page_id WHERE {active}
               )"
        )
    }

    fn covers(
        &self,
        db: &Connection,
        id: &str,
        creating: bool,
        include_archived: bool,
    ) -> Result<bool> {
        // Traverse the current tree for subtree grants. A database grant follows membership
        // and admits creation only at the container itself.
        let mut query = db.prepare_cached(
            "WITH RECURSIVE ancestors(id, parent_id) AS (
                SELECT id, parent_id FROM pages WHERE id = ?2 AND (?4 OR trash_root_id IS NULL)
                UNION
                SELECT p.id, p.parent_id FROM pages p JOIN ancestors a ON p.id = a.parent_id
                WHERE ?4 OR p.trash_root_id IS NULL
            )
            SELECT EXISTS (
                SELECT 1 FROM ancestors target JOIN integration_grants g ON g.integration_id = ?1
                WHERE target.id = ?2 AND (
                    (g.kind = 'page' AND NOT ?3 AND g.page_id = target.id)
                    OR (g.kind = 'subtree' AND g.page_id IN (SELECT id FROM ancestors))
                    OR (g.kind = 'collection' AND EXISTS (
                        SELECT 1 FROM collections c JOIN pages p ON p.id = c.container_page_id
                        WHERE c.id = g.collection_id AND (?4 OR p.trash_root_id IS NULL) AND (
                            p.id = target.id OR (NOT ?3 AND EXISTS (
                                SELECT 1 FROM collection_records r
                                WHERE r.collection_id = c.id AND r.page_id = target.id
                            ))
                        )
                    ))
                )
            )",
        )?;
        Ok(
            query.query_row(params![self.id, id, creating, include_archived], |row| {
                row.get(0)
            })?,
        )
    }
}
