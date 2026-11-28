// SPDX-License-Identifier: AGPL-3.0-or-later

use super::content::project_page;
use super::edit_blocks::apply_operations;
use super::mutations::{apply_page_patch, attribute, remember, replay, validate_request_key};
use super::{Access, IntegrationBlockOperation, IntegrationScope, PagePatch, missing};
use crate::collections::{for_parent, read_collection_schema};
use crate::model::{new_id, validate_blocks};
use crate::store::{commit_revision, ensure_revision, now, read_page};
use crate::{Block, Error, Page, PropertyDefinition, Result, Store, render_markdown};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

const REVIEW_TTL: i64 = 30 * 24 * 60 * 60 * 1_000;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalDraft {
    pub page_id: String,
    pub base_revision: u32,
    pub changes: ProposalChanges,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProposalChanges {
    pub title: Option<String>,
    pub icon: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, Value>,
    #[serde(default)]
    pub operations: Vec<IntegrationBlockOperation>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    Pending,
    Conflict,
    Accepted,
    Rejected,
    Expired,
}

impl ProposalStatus {
    fn parse(value: &str, expires_at: i64, time: i64) -> rusqlite::Result<Self> {
        Ok(match value {
            "pending" | "conflict" if expires_at <= time => Self::Expired,
            "pending" => Self::Pending,
            "conflict" => Self::Conflict,
            "accepted" => Self::Accepted,
            "rejected" => Self::Rejected,
            _ => return Err(rusqlite::Error::InvalidQuery),
        })
    }

    fn reviewable(self) -> bool {
        matches!(self, Self::Pending | Self::Conflict)
    }
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ProposalReceipt {
    pub id: String,
    pub page_id: String,
    pub base_revision: u32,
    pub status: ProposalStatus,
    pub created_at: i64,
    pub expires_at: i64,
    pub accepted_revision: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalSummary {
    pub id: String,
    pub page_id: String,
    pub page_title: String,
    pub integration_name: String,
    pub base_revision: u32,
    pub review_revision: u32,
    pub status: ProposalStatus,
    pub conflict: Option<String>,
    pub created_at: i64,
    pub expires_at: i64,
    pub accepted_revision: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalSnapshot {
    pub title: String,
    pub icon: String,
    pub markdown: String,
    pub blocks: Vec<Block>,
    pub properties: Option<BTreeMap<String, Value>>,
}

impl From<Page> for ProposalSnapshot {
    fn from(page: Page) -> Self {
        let markdown = render_markdown(&page);
        Self {
            title: page.info.title,
            icon: page.info.icon,
            markdown,
            blocks: page.blocks,
            properties: page.properties,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposalReview {
    pub summary: ProposalSummary,
    pub base: ProposalSnapshot,
    pub candidate: ProposalSnapshot,
    pub current: Option<ProposalSnapshot>,
    pub current_revision: u32,
    pub fields: Vec<PropertyDefinition>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalAction {
    Accept,
    Reject,
    Rebase,
}

const OWNER_SUMMARY: &str =
    "r.id, r.page_id, p.title, i.name, r.base_revision, r.review_revision, r.status,
     r.conflict, r.created_at, r.expires_at, r.accepted_revision";
const OWNER_JOIN: &str =
    "FROM integration_proposals r JOIN integrations i ON i.id = r.integration_id
     JOIN pages p ON p.id = r.page_id";

impl Store {
    pub fn submit_integration_proposal(
        &self,
        workspace: &str,
        token: &str,
        request_key: &str,
        draft: ProposalDraft,
    ) -> Result<ProposalReceipt> {
        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let access = Access::authenticate(&tx, token)?;
        access.require(IntegrationScope::Propose)?;
        access.require_page(&tx, &draft.page_id)?;
        validate_request_key(request_key)?;
        let digest = Sha256::digest(serde_json::to_vec(&("proposal", &draft))?);
        let time = now();
        if let Some(receipt) = replay(&tx, &access, request_key, &digest, time)? {
            access.mark_use(&tx)?;
            tx.commit()?;
            return Ok(receipt);
        }
        let before = read_page(&tx, &draft.page_id)?;
        ensure_revision(&before.info, draft.base_revision)?;
        let etag = project_page(&tx, &access, &before)?.etag;
        let operations = serde_json::to_string(&draft.changes)?;
        let candidate = build_candidate(&tx, &access, &before, draft.changes)?;
        let receipt = ProposalReceipt {
            id: new_id(),
            page_id: draft.page_id,
            base_revision: draft.base_revision,
            status: ProposalStatus::Pending,
            created_at: time,
            expires_at: time + REVIEW_TTL,
            accepted_revision: None,
        };
        tx.execute(
            "INSERT INTO integration_proposals
             (id, integration_id, page_id, base_revision, base_etag, operations, candidate, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![receipt.id, access.id, receipt.page_id, receipt.base_revision, etag,
                operations, serde_json::to_string(&candidate)?, time, receipt.expires_at],
        )?;
        remember(&tx, &access, request_key, &digest, &receipt, time)?;
        access.mark_use(&tx)?;
        tx.commit()?;
        Ok(receipt)
    }

    pub fn integration_proposal(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<ProposalReceipt> {
        self.integration_read(workspace, token, |db, access| {
            access.require(IntegrationScope::Propose)?;
            let time = now();
            let receipt = db.query_row(
                "SELECT id, page_id, base_revision, status, created_at, expires_at, accepted_revision
                 FROM integration_proposals WHERE id = ?1 AND integration_id = ?2",
                params![id, access.id],
                |row| {
                    let expires_at = row.get(5)?;
                    Ok(ProposalReceipt {
                        id: row.get(0)?, page_id: row.get(1)?, base_revision: row.get(2)?,
                        status: ProposalStatus::parse(&row.get::<_, String>(3)?, expires_at, time)?,
                        created_at: row.get(4)?, expires_at, accepted_revision: row.get(6)?,
                    })
                },
            ).optional()?.ok_or_else(missing)?;
            access.require_page(db, &receipt.page_id)?;
            Ok(receipt)
        })
    }

    pub fn proposals(
        &self,
        workspace: &str,
        page_id: Option<&str>,
    ) -> Result<Vec<ProposalSummary>> {
        let db = self.connection(workspace)?;
        let time = now();
        let mut query = db.prepare(&format!(
            "SELECT {OWNER_SUMMARY} {OWNER_JOIN}
             WHERE ?1 IS NULL OR r.page_id = ?1
             ORDER BY (r.status IN ('pending', 'conflict') AND r.expires_at > ?2) DESC,
                      r.created_at DESC, r.id",
        ))?;
        Ok(query
            .query_map(params![page_id, time], |row| owner_summary(row, time))?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn proposal_review(&self, workspace: &str, id: &str) -> Result<ProposalReview> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        read_review(&tx, id)
    }

    pub fn review_proposal(
        &self,
        workspace: &str,
        id: &str,
        reviewed: u32,
        action: ProposalAction,
    ) -> Result<ProposalReview> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let summary = read_summary(&tx, id)?;
        if summary.review_revision != reviewed {
            return Err(Error::new(
                "PROPOSAL_REVIEW_CHANGED",
                "审阅版本已更新，请重新打开建议。",
            ));
        }
        if summary.status.reviewable() {
            match action {
                ProposalAction::Reject => {
                    tx.execute(
                        "UPDATE integration_proposals SET status = 'rejected', conflict = NULL WHERE id = ?1",
                        [id],
                    )?;
                }
                ProposalAction::Accept if summary.status == ProposalStatus::Conflict => {}
                ProposalAction::Accept | ProposalAction::Rebase => {
                    change_review(&tx, &summary, action)?;
                }
            }
        }
        let review = read_review(&tx, id)?;
        tx.commit()?;
        Ok(review)
    }
}

fn change_review(
    tx: &Transaction<'_>,
    summary: &ProposalSummary,
    action: ProposalAction,
) -> Result<()> {
    let (subject, etag, operations, stored): (String, String, String, String) = tx.query_row(
        "SELECT integration_id, base_etag, operations, candidate FROM integration_proposals WHERE id = ?1",
        [&summary.id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let access = match Access::by_id(tx, &subject)? {
        Some(access)
            if access.require(IntegrationScope::Read).is_ok()
                && access.require(IntegrationScope::Propose).is_ok()
                && access.can_read(tx, &summary.page_id)? =>
        {
            access
        }
        _ => return conflict(tx, &summary.id, "authorization_changed"),
    };
    let before = read_page(tx, &summary.page_id)?;
    let current_etag = project_page(tx, &access, &before)?.etag;
    let rebase = matches!(action, ProposalAction::Rebase);
    if !rebase && (before.info.revision != summary.base_revision || current_etag != etag) {
        return conflict(tx, &summary.id, "revision_changed");
    }
    // Recheck each operation against current reference, attachment and property access.
    // Acceptance uses the reviewed snapshot so newly inserted block IDs stay stable.
    let candidate = match build_candidate(tx, &access, &before, serde_json::from_str(&operations)?)
    {
        Ok(candidate) => candidate,
        Err(error) if rebase => return Err(error),
        Err(error) if invalid_candidate(&error) => {
            return conflict(tx, &summary.id, "candidate_changed");
        }
        Err(error) => return Err(error),
    };
    if summary.expires_at <= now() {
        return Ok(());
    }
    if rebase {
        tx.execute(
            "UPDATE integration_proposals
             SET base_revision = ?2, base_etag = ?3, review_revision = review_revision + 1,
                 candidate = ?4, status = 'pending', conflict = NULL
             WHERE id = ?1",
            params![
                summary.id,
                before.info.revision,
                current_etag,
                serde_json::to_string(&candidate)?
            ],
        )?;
    } else {
        let mut reviewed: Page = serde_json::from_str(&stored)?;
        let related = commit_revision(tx, &mut reviewed)?;
        attribute(tx, &access, &reviewed.info, &summary.id)?;
        for page in related {
            attribute(tx, &access, &page, &summary.id)?;
        }
        tx.execute(
            "UPDATE integration_proposals SET status = 'accepted', accepted_revision = ?2 WHERE id = ?1",
            params![summary.id, reviewed.info.revision],
        )?;
    }
    Ok(())
}

fn conflict(tx: &Transaction<'_>, id: &str, reason: &str) -> Result<()> {
    tx.execute(
        "UPDATE integration_proposals SET status = 'conflict', conflict = ?2 WHERE id = ?1",
        params![id, reason],
    )?;
    Ok(())
}

fn invalid_candidate(error: &Error) -> bool {
    matches!(
        error.code,
        "FORBIDDEN"
            | "PARTIAL_CONTENT"
            | "RESOURCE_NOT_FOUND"
            | "RECORD_NOT_FOUND"
            | "PROPERTY_NOT_FOUND"
            | "PROPERTY_VALUE"
            | "RELATION_DELTA"
            | "INVALID_PATCH"
            | "INVALID_BLOCK"
            | "INVALID_BLOCK_OPERATION"
            | "INVALID_ATTACHMENT"
            | "ATTACHMENT_NOT_FOUND"
            | "PAGE_TOO_LARGE"
    )
}

fn build_candidate(
    db: &Connection,
    access: &Access,
    before: &Page,
    changes: ProposalChanges,
) -> Result<Page> {
    let mut candidate = before.clone();
    apply_page_patch(
        db,
        access,
        &mut candidate,
        PagePatch {
            title: changes.title,
            icon: changes.icon,
        },
        changes.properties,
    )?;
    if !changes.operations.is_empty() {
        apply_operations(
            db,
            access,
            &before.info.id,
            &mut candidate.blocks,
            changes.operations,
        )?;
    }
    if candidate == *before {
        return Err(Error::new(
            "INVALID_PATCH",
            "建议需要包含至少一项有效修改。",
        ));
    }
    validate_blocks(&candidate.blocks)?;
    crate::assets::validate_page_attachments(db, &candidate)?;
    Ok(candidate)
}

fn owner_summary(row: &Row<'_>, time: i64) -> rusqlite::Result<ProposalSummary> {
    let expires_at = row.get(9)?;
    Ok(ProposalSummary {
        id: row.get(0)?,
        page_id: row.get(1)?,
        page_title: row.get(2)?,
        integration_name: row.get(3)?,
        base_revision: row.get(4)?,
        review_revision: row.get(5)?,
        status: ProposalStatus::parse(&row.get::<_, String>(6)?, expires_at, time)?,
        conflict: row.get(7)?,
        created_at: row.get(8)?,
        expires_at,
        accepted_revision: row.get(10)?,
    })
}

fn read_summary(db: &Connection, id: &str) -> Result<ProposalSummary> {
    let time = now();
    db.query_row(
        &format!("SELECT {OWNER_SUMMARY} {OWNER_JOIN} WHERE r.id = ?1"),
        [id],
        |row| owner_summary(row, time),
    )
    .optional()?
    .ok_or_else(missing)
}

fn read_review(db: &Connection, id: &str) -> Result<ProposalReview> {
    let summary = read_summary(db, id)?;
    let (base, candidate): (String, String) = db.query_row(
        "SELECT v.snapshot, r.candidate FROM integration_proposals r
         JOIN revisions v ON v.page_id = r.page_id AND v.revision = r.base_revision
         WHERE r.id = ?1",
        [id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let current = read_page(db, &summary.page_id)?;
    let current_revision = current.info.revision;
    let fields = if current.properties.is_some() {
        let collection = for_parent(db, current.info.parent_id.as_deref())?.ok_or_else(missing)?;
        read_collection_schema(db, &collection)?.properties
    } else {
        Vec::new()
    };
    let snapshot = |mut page: Page| -> Result<ProposalSnapshot> {
        if let Some(values) = page.properties.as_mut() {
            crate::collections::relations::project_values(db, &fields, values)?;
        }
        Ok(page.into())
    };
    Ok(ProposalReview {
        base: snapshot(serde_json::from_str(&base)?)?,
        candidate: snapshot(serde_json::from_str(&candidate)?)?,
        current: (current_revision != summary.base_revision)
            .then(|| snapshot(current))
            .transpose()?,
        summary,
        current_revision,
        fields,
    })
}
