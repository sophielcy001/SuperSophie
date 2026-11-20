// SPDX-License-Identifier: AGPL-3.0-or-later

pub(crate) use super::computed_changes::ComputedChange;
use super::{
    Access, IntegrationScope,
    computed_changes::{ComputedJob, Observer},
    content::{project_reference_block, source_anchors},
};
use crate::store::{now, read_revision};
use crate::{Block, Error, Page, Result, Store};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

const PAGES_PER_BATCH: i64 = 16;

#[derive(Default, Serialize, Deserialize)]
struct Inputs {
    revisions: BTreeMap<String, u32>,
    recipients: BTreeMap<String, Recipient>,
}

#[derive(Serialize, Deserialize)]
struct Recipient {
    grant_revision: i64,
    range_revision: i64,
    readable: BTreeSet<String>,
}

impl Recipient {
    fn current(&self, access: &Access) -> bool {
        self.grant_revision == access.grant_revision
            && self.range_revision == access.range_revision
            && access.scopes.contains(&IntegrationScope::Read)
            && access.scopes.contains(&IntegrationScope::Changes)
    }
}

struct Reference {
    page: String,
    block: String,
    source: String,
    anchor: String,
}

impl Reference {
    fn read(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            page: row.get(0)?,
            block: row.get(1)?,
            source: row.get(2)?,
            anchor: row.get(3)?,
        })
    }
}

pub(crate) struct ReferenceChange {
    before: Inputs,
    references: Vec<Reference>,
}

impl ReferenceChange {
    pub(crate) fn edit(db: &Connection, page: &Page, blocks: &str) -> Result<Option<Self>> {
        let info = &page.info;
        // Trash transitions are recorded once, around the whole batch. Edits
        // to properties, icons and position do not change the source preview.
        let changed: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM pages
             WHERE id = ?1 AND trash_root_id IS ?2
               AND EXISTS (SELECT 1 FROM page_links
                   WHERE target_page_id = ?1 AND target_block_id <> '' AND page_id <> ?1)
               AND (title <> ?3 OR blocks <> ?4))",
            params![info.id, info.trash_root_id, info.title, blocks],
            |row| row.get(0),
        )?;
        if !changed {
            return Ok(None);
        }
        Self::capture(db, &[&info.id], false)
    }

    pub(crate) fn visibility(db: &Connection, pages: &[String]) -> Result<Option<Self>> {
        Self::capture(db, pages, true)
    }

    fn capture<T: Serialize>(
        db: &Connection,
        pages: &[T],
        visibility_change: bool,
    ) -> Result<Option<Self>> {
        let changed = serde_json::to_string(pages)?;
        let mut query = db.prepare_cached(
            "WITH changed(id) AS (SELECT value FROM json_each(?1)),
                  sources(id) AS (
                      SELECT id FROM changed
                      UNION
                      SELECT l.page_id FROM changed c JOIN page_links l ON l.target_page_id = c.id
                      WHERE ?2 AND l.target_block_id = ''
                  )
             SELECT l.page_id, l.block_id, l.target_page_id, l.target_block_id
             FROM sources s JOIN page_links l ON l.target_page_id = s.id
             JOIN pages p ON p.id = l.page_id AND p.trash_root_id IS NULL
             WHERE l.target_block_id <> '' AND l.page_id NOT IN (SELECT id FROM changed)
             UNION ALL
             SELECT e.source_page_id, e.property_id, e.target_page_id, ''
             FROM changed c JOIN relation_edges e ON e.target_page_id = c.id
             JOIN pages p ON p.id = e.source_page_id AND p.trash_root_id IS NULL
             WHERE ?2 AND e.source_page_id NOT IN (SELECT id FROM changed)",
        )?;
        // A page changed by this operation already has its own upsert or
        // tombstone. Other pages may depend on the changed source's projection.
        let references = query
            .query_map(params![changed, visibility_change], Reference::read)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if references.is_empty() {
            return Ok(None);
        }
        let before = Inputs::capture(db, &references)?;
        Ok(Some(Self { before, references }))
    }
}

pub(crate) fn enqueue(
    tx: &Transaction<'_>,
    references: Option<ReferenceChange>,
    computed: Option<ComputedChange>,
) -> Result<()> {
    let (before, after, references) = match references {
        Some(change) => (
            change.before,
            Inputs::capture(tx, &change.references)?,
            change.references,
        ),
        None => (Inputs::default(), Inputs::default(), Vec::new()),
    };
    if references.is_empty() && computed.is_none() {
        return Ok(());
    }
    let job: i64 = tx.query_row(
        "INSERT INTO derived_jobs (before_state, after_state) VALUES (?1, ?2) RETURNING id",
        params![
            serde_json::to_string(&before)?,
            serde_json::to_string(&after)?
        ],
        |row| row.get(0),
    )?;
    if !references.is_empty() {
        let mut insert =
            tx.prepare("INSERT INTO derived_job_references VALUES (?1, ?2, ?3, ?4, ?5)")?;
        let mut relation =
            tx.prepare("INSERT INTO derived_job_relations VALUES (?1, ?2, ?3, ?4)")?;
        for item in references {
            if item.anchor.is_empty() {
                relation.execute(params![job, item.page, item.block, item.source])?;
            } else {
                insert.execute(params![
                    job,
                    item.page,
                    item.block,
                    item.source,
                    item.anchor
                ])?;
            }
        }
    }
    if let Some(computed) = computed {
        computed.enqueue(tx, job)?;
    }
    Ok(())
}

impl Inputs {
    fn capture(db: &Connection, references: &[Reference]) -> Result<Self> {
        let sources = references
            .iter()
            .map(|edge| &edge.source)
            .collect::<BTreeSet<_>>();
        let sources_json = serde_json::to_string(&sources)?;
        let mut versions = db.prepare_cached(
            "SELECT id, revision FROM pages WHERE id IN (SELECT value FROM json_each(?1))",
        )?;
        let revisions = versions
            .query_map([&sources_json], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
        let mut inline = db.prepare_cached(
            "SELECT DISTINCT target_page_id FROM page_links
             WHERE page_id IN (SELECT value FROM json_each(?1)) AND target_block_id = ''",
        )?;
        let inline = inline
            .query_map([&sources_json], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<BTreeSet<_>>>()?;
        let relevant = sources
            .into_iter()
            .chain(inline.iter())
            .collect::<BTreeSet<_>>();
        let mut recipients = BTreeMap::new();
        let mut subjects = db.prepare_cached(
            "SELECT id FROM integrations
             WHERE revoked_at IS NULL AND (expires_at IS NULL OR expires_at > ?1)
             AND 'changes:read' IN (SELECT value FROM json_each(scopes))",
        )?;
        for id in subjects.query_map([now()], |row| row.get::<_, String>(0))? {
            let id = id?;
            let Some(access) = Access::by_id(db, &id)? else {
                continue;
            };
            let mut readable = BTreeSet::new();
            for page in &relevant {
                if access.can_read(db, page)? {
                    readable.insert((*page).clone());
                }
            }
            recipients.insert(
                id,
                Recipient {
                    grant_revision: access.grant_revision,
                    range_revision: access.range_revision,
                    readable,
                },
            );
        }
        Ok(Self {
            revisions,
            recipients,
        })
    }
}

impl Store {
    /// Serializes a bounded batch with business writers. A failed batch keeps
    /// both its input revisions and its unacknowledged reference positions.
    pub fn advance_derived_changes(&self, workspace: &str) -> Result<bool> {
        let mut db = self.connection(workspace)?;
        let time = now();
        let ready = |db: &Connection| -> Result<Option<(i64, u32)>> {
            Ok(db
                .query_row(
                    "SELECT j.id, j.attempts FROM derived_jobs j
                 WHERE j.retry_at <= ?1 AND (
                    NOT EXISTS (SELECT 1 FROM computed_jobs c WHERE c.job_id=j.id)
                    OR j.id = (SELECT min(job_id) FROM computed_jobs)
                 ) ORDER BY j.id LIMIT 1",
                    [time],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?)
        };
        if ready(&db)?.is_none() {
            return Ok(false);
        }
        let mut tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Some((id, attempts)) = ready(&tx)? else {
            return Ok(false);
        };
        let mut batch = tx.savepoint()?;
        let processed = advance(&batch, id);
        if processed.is_err() {
            batch.rollback()?;
        }
        batch.commit()?;
        if processed.is_err() {
            let retry = time + (1_000_i64 << attempts.min(6));
            tx.execute(
                "UPDATE derived_jobs SET attempts = attempts + 1, retry_at = ?2 WHERE id = ?1",
                (id, retry),
            )?;
        }
        tx.commit()?;
        processed.map(|_| true)
    }
}

fn advance(db: &Connection, job: i64) -> Result<()> {
    ComputedJob::expand(db, job)?;
    let mut pages = db.prepare(
        "SELECT page_id FROM derived_job_references WHERE job_id = ?1
         UNION SELECT page_id FROM derived_job_relations WHERE job_id = ?1
         UNION SELECT page_id FROM derived_job_computed WHERE job_id = ?1
         ORDER BY page_id LIMIT ?2",
    )?;
    let pages = pages
        .query_map(params![job, PAGES_PER_BATCH], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if let Some(last) = pages.last() {
        publish_batch(db, job, last)?;
        db.execute(
            "DELETE FROM derived_job_references WHERE job_id = ?1 AND page_id <= ?2",
            (job, last),
        )?;
        db.execute(
            "DELETE FROM derived_job_relations WHERE job_id = ?1 AND page_id <= ?2",
            (job, last),
        )?;
        db.execute(
            "DELETE FROM derived_job_computed WHERE job_id = ?1 AND page_id <= ?2",
            (job, last),
        )?;
    }
    db.execute(
        "DELETE FROM derived_jobs WHERE id = ?1
         AND NOT EXISTS (SELECT 1 FROM derived_job_references WHERE job_id = ?1)
         AND NOT EXISTS (SELECT 1 FROM derived_job_relations WHERE job_id = ?1)
         AND NOT EXISTS (SELECT 1 FROM derived_job_computed WHERE job_id = ?1)",
        [job],
    )?;
    Ok(())
}

fn publish_batch(db: &Connection, job: i64, last: &str) -> Result<()> {
    let (before, after): (String, String) = db.query_row(
        "SELECT before_state, after_state FROM derived_jobs WHERE id = ?1",
        [job],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let before: Inputs = serde_json::from_str(&before)?;
    let after: Inputs = serde_json::from_str(&after)?;
    let mut query = db.prepare(
        "SELECT j.page_id, j.block_id, j.source_page_id, j.source_block_id, p.revision, NULL
         FROM derived_job_references j
         JOIN pages p ON p.id = j.page_id AND p.trash_root_id IS NULL
         JOIN page_links l ON l.page_id = j.page_id AND l.block_id = j.block_id
            AND l.target_page_id = j.source_page_id AND l.target_block_id = j.source_block_id
         WHERE j.job_id = ?1 AND j.page_id <= ?2
         UNION ALL
         SELECT j.page_id, j.property_id, j.target_page_id, '', p.revision, c.container_page_id
         FROM derived_job_relations j
         JOIN pages p ON p.id = j.page_id AND p.trash_root_id IS NULL
         JOIN relation_edges e ON e.source_page_id=j.page_id
             AND e.property_id=j.property_id AND e.target_page_id=j.target_page_id
         JOIN collection_records r ON r.page_id=j.page_id
         JOIN collections c ON c.id=r.collection_id
         WHERE j.job_id = ?1 AND j.page_id <= ?2",
    )?;
    let references = query
        .query_map((job, last), |row| {
            Ok((
                Reference::read(row)?,
                row.get::<_, u32>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_source = BTreeMap::<&str, Vec<&Reference>>::new();
    let mut pages = BTreeMap::new();
    for (item, revision, container) in &references {
        if container.is_none() {
            by_source.entry(&item.source).or_default().push(item);
        }
        pages.insert(item.page.as_str(), *revision);
    }
    let mut recipients = Vec::new();
    for (id, old) in &before.recipients {
        if let Some(new) = after.recipients.get(id)
            && let Some(access) = Access::by_id(db, id)?
            && old.current(&access)
            && new.current(&access)
        {
            let mut readable = BTreeSet::new();
            for &page in pages.keys() {
                if access.can_read(db, page)? {
                    readable.insert(page);
                }
            }
            if !readable.is_empty() {
                recipients.push((access, old, new, readable));
            }
        }
    }
    let mut audience = BTreeMap::<&str, BTreeSet<usize>>::new();
    let mut checked_containers = HashMap::new();
    for (item, _, container) in &references {
        let Some(container) = container else {
            continue;
        };
        for (index, (access, old, new, readable)) in recipients.iter().enumerate() {
            if !readable.contains(item.page.as_str())
                || old.readable.contains(&item.source) == new.readable.contains(&item.source)
            {
                continue;
            }
            let key = (index, container.as_str());
            let permitted = match checked_containers.get(&key) {
                Some(&value) => value,
                None => {
                    let value = access.can_read(db, container)?;
                    checked_containers.insert(key, value);
                    value
                }
            };
            if permitted {
                audience
                    .entry(item.page.as_str())
                    .or_default()
                    .insert(index);
            }
        }
    }
    for (source, items) in by_source {
        let observers = recipients
            .iter()
            .enumerate()
            .filter(|(_, (_, before, after, pages))| {
                (before.readable.contains(source) || after.readable.contains(source))
                    && items.iter().any(|item| pages.contains(item.page.as_str()))
            })
            .collect::<Vec<_>>();
        if observers.is_empty() {
            continue;
        }
        let old_revision = input_revision(&before, source)?;
        let new_revision = input_revision(&after, source)?;
        let old_page = read_revision(db, source, old_revision)?;
        let updated = if old_revision == new_revision {
            None
        } else {
            Some(read_revision(db, source, new_revision)?)
        };
        let new_page = updated.as_ref().unwrap_or(&old_page);
        let old_anchors = anchors_by_id(&old_page);
        let new_anchors = updated.as_ref().map(anchors_by_id);
        let new_anchors = new_anchors.as_ref().unwrap_or(&old_anchors);
        for (index, (access, old, new, readable)) in observers {
            let mut compared = HashMap::new();
            for item in &items {
                let page = item.page.as_str();
                if !readable.contains(page)
                    || audience.get(page).is_some_and(|set| set.contains(&index))
                {
                    continue;
                }
                let target = item.anchor.as_str();
                let changed = match compared.get(target) {
                    Some(&changed) => changed,
                    None => {
                        let old_value = project_reference_block(
                            db,
                            access,
                            &old_page,
                            old_anchors.get(target).copied(),
                            Some(&old.readable),
                        )?
                        .map(|block| (&old_page.info.title, block));
                        let new_value = project_reference_block(
                            db,
                            access,
                            new_page,
                            new_anchors.get(target).copied(),
                            Some(&new.readable),
                        )?
                        .map(|block| (&new_page.info.title, block));
                        let changed = old_value != new_value;
                        compared.insert(target, changed);
                        changed
                    }
                };
                if changed {
                    audience.entry(page).or_default().insert(index);
                }
            }
        }
    }
    let mut computed = db.prepare(
        "SELECT j.page_id, p.revision, j.audience FROM derived_job_computed j
         JOIN pages p ON p.id=j.page_id AND p.trash_root_id IS NULL
         WHERE j.job_id=?1 AND j.page_id<=?2",
    )?;
    let candidates = computed
        .query_map((job, last), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?
        .collect::<rusqlite::Result<Vec<(String, u32, String)>>>()?;
    let current = if let Some(computation) = ComputedJob::read(db, job)? {
        computation.calculate(db, &candidates)?
    } else {
        // Preserve already-queued audiences when opening a pre-v16 workspace.
        let mut current = BTreeMap::new();
        for (page, revision, audience) in candidates {
            let mut observers = Vec::new();
            for observer in serde_json::from_str::<Vec<Observer>>(&audience)? {
                if let Some(access) = observer.current(db, &page)? {
                    observers.push(access);
                }
            }
            current.insert(page, (revision, observers));
        }
        current
    };
    for page in audience
        .keys()
        .copied()
        .chain(current.keys().map(String::as_str))
        .collect::<BTreeSet<_>>()
    {
        let mut observers = BTreeMap::new();
        if let Some(list) = audience.get(page) {
            for &index in list {
                let access = &recipients[index].0;
                observers.insert(&access.id, access);
            }
        }
        let revision = if let Some((revision, list)) = current.get(page) {
            for access in list {
                observers.insert(&access.id, access);
            }
            *revision
        } else {
            pages[page]
        };
        if !observers.is_empty() {
            super::feed::publish_derived(
                db,
                page,
                revision,
                &observers.into_values().collect::<Vec<_>>(),
            )?;
        }
    }
    Ok(())
}

fn input_revision(inputs: &Inputs, source: &str) -> Result<u32> {
    inputs
        .revisions
        .get(source)
        .copied()
        .ok_or_else(|| Error::new("DERIVED_INPUT", "失效任务缺少源页面修订，将在后续重试。"))
}

fn anchors_by_id(page: &Page) -> HashMap<&str, &Block> {
    source_anchors(&page.blocks)
        .map(|block| (block.id.as_str(), block))
        .collect()
}
