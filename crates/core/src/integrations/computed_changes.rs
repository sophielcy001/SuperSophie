// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{Access, Connection, IntegrationScope};
use crate::collections::{
    Collection, PropertyKind,
    computed::{Engine, RecordInput, cache},
    for_parent, read_collection_schema,
};
use crate::store::now;
use crate::{Page, Result};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
pub(super) struct Observer {
    id: String,
    grant_revision: i64,
    range_revision: i64,
}

impl Observer {
    fn access(&self, db: &Connection) -> Result<Option<Access>> {
        Ok(Access::by_id(db, &self.id)?.filter(|access| {
            self.grant_revision == access.grant_revision
                && self.range_revision == access.range_revision
                && access.scopes.contains(&IntegrationScope::Read)
        }))
    }

    pub(super) fn current(&self, db: &Connection, page: &str) -> Result<Option<Access>> {
        if let Some(access) = self.access(db)?
            && access.scopes.contains(&IntegrationScope::Changes)
            && access.can_read(db, page)?
        {
            return Ok(Some(access));
        }
        Ok(None)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Seeds {
    records: Vec<(String, String)>,
    schema: Vec<(String, String)>,
    edited: BTreeSet<String>,
}

#[derive(Default)]
pub(crate) struct ComputedChange {
    seeds: Seeds,
    records: BTreeMap<String, Option<RecordInput>>,
    schema: Option<Collection>,
}

impl ComputedChange {
    pub(crate) fn edit<'a>(
        db: &Connection,
        pages: impl IntoIterator<Item = &'a Page>,
    ) -> Result<Option<Self>> {
        let pages: Vec<_> = pages
            .into_iter()
            .filter(|page| page.properties.is_some())
            .collect();
        if pages.is_empty() {
            return Ok(None);
        }
        let indexed = has_dependencies(db)?;
        let mut change = Self::default();
        let mut updated_fields = BTreeMap::<String, Vec<String>>::new();
        for page in pages {
            let id = &page.info.id;
            let values = page.properties.as_ref().expect("record page");
            let previous = RecordInput::read(db, id)?;
            let Some(old) = previous else {
                let collection = for_parent(db, page.info.parent_id.as_deref())?;
                if let Some(collection) = collection
                    && has_own_calculations(db, &collection)?
                {
                    change.record(id, None, [String::new()]);
                }
                continue;
            };
            // Archive and restore capture the complete command once.
            if !indexed || !old.active || page.info.trash_root_id.is_some() {
                continue;
            }
            let fields = if let Some(fields) = updated_fields.get(&old.collection) {
                fields
            } else {
                let schema = read_collection_schema(db, &old.collection)?;
                updated_fields
                    .entry(old.collection.clone())
                    .or_insert_with(|| {
                        schema
                            .properties
                            .into_iter()
                            .filter(|field| field.kind == PropertyKind::UpdatedAt)
                            .map(|field| field.id)
                            .collect()
                    })
            };
            let mut modified: BTreeSet<String> = old
                .properties
                .keys()
                .chain(values.keys())
                .filter(|field| old.properties.get(*field) != values.get(*field))
                .cloned()
                .collect();
            if page.info.title != old.title {
                modified.insert("title".into());
            }
            modified.extend(fields.iter().cloned());
            if !modified.is_empty() {
                change.record(id, Some(old), modified);
            }
        }
        Ok((!change.records.is_empty()).then_some(change))
    }

    pub(crate) fn visibility(db: &Connection, pages: &[String]) -> Result<Option<Self>> {
        if !has_dependencies(db)? {
            return Ok(None);
        }
        let mut change = Self::default();
        for id in pages {
            if let Some(before) = RecordInput::read(db, id)? {
                change.record(id, Some(before), [String::new()]);
            }
        }
        Ok((!change.records.is_empty()).then_some(change))
    }

    pub(crate) fn schema(
        db: &Connection,
        next: &Collection,
        fields: &[&str],
    ) -> Result<Option<Self>> {
        let before = read_collection_schema(db, &next.id)?;
        if !before
            .properties
            .iter()
            .chain(&next.properties)
            .any(|field| field.calculation.is_some())
            && !has_dependencies(db)?
        {
            return Ok(None);
        }
        let mut change = Self::default();
        change.seeds.schema.extend(
            fields
                .iter()
                .map(|field| (next.id.clone(), (*field).into())),
        );
        change.schema = Some(before);
        Ok(Some(change))
    }

    pub(crate) fn conversion<'a>(
        db: &Connection,
        next: &Collection,
        field: &str,
        records: impl Iterator<Item = &'a str>,
    ) -> Result<Option<Self>> {
        let Some(mut change) = Self::schema(db, next, &[field])? else {
            return Ok(None);
        };
        let modified = std::iter::once(field.to_owned())
            .chain(
                next.properties
                    .iter()
                    .filter(|property| property.kind == PropertyKind::UpdatedAt)
                    .map(|property| property.id.clone()),
            )
            .collect::<Vec<_>>();
        for id in records {
            if let Some(before) = RecordInput::read(db, id)?
                && before.active
            {
                change.record(id, Some(before), modified.iter().cloned());
            }
        }
        Ok(Some(change))
    }

    fn record(
        &mut self,
        id: &str,
        previous: Option<RecordInput>,
        fields: impl IntoIterator<Item = String>,
    ) {
        self.records.insert(id.into(), previous);
        self.seeds.edited.insert(id.into());
        self.seeds
            .records
            .extend(fields.into_iter().map(|field| (id.into(), field)));
    }

    pub(super) fn enqueue(self, db: &Connection, job: i64) -> Result<()> {
        let observers: String = db.query_row(
            "SELECT json_group_array(json_object(
                 'id', id, 'grant_revision', grant_revision, 'range_revision', range_revision
             )) FROM integrations WHERE revoked_at IS NULL
             AND (expires_at IS NULL OR expires_at > ?1)
             AND 'content:read' IN (SELECT value FROM json_each(scopes))",
            [now()],
            |row| row.get(0),
        )?;
        db.execute(
            "INSERT INTO computed_jobs(job_id,seeds,observers,source_sequence)
             VALUES (?1,?2,?3,?4)",
            params![
                job,
                serde_json::to_string(&self.seeds)?,
                observers,
                cache::source_sequence(db)?
            ],
        )?;
        let mut insert = db.prepare_cached(
            "INSERT INTO computed_record_before(page_id,job_id,state) VALUES (?1,?2,?3)",
        )?;
        for (page, previous) in self.records {
            let snapshot = previous
                .map(|value| serde_json::to_string(&value))
                .transpose()?;
            insert.execute(params![page, job, snapshot])?;
        }
        if let Some(schema) = self.schema {
            // The schema has already been written. Index its previous rules
            // from the captured definitions, rather than reading the live edges.
            let dependencies = crate::collections::computed::dependency_fields(&schema.properties)?;
            db.execute(
                "INSERT INTO computed_schema_before(collection_id,job_id,properties,dependencies)
                 VALUES (?1,?2,?3,?4)",
                params![
                    schema.id,
                    job,
                    serde_json::to_string(&schema.properties)?,
                    serde_json::to_string(&dependencies)?
                ],
            )?;
        }
        Ok(())
    }
}

fn has_dependencies(db: &Connection) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS (SELECT 1 FROM computed_dependencies)
             OR EXISTS (SELECT 1 FROM computed_jobs)",
        [],
        |row| row.get(0),
    )?)
}

fn has_own_calculations(db: &Connection, collection: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT EXISTS (
            SELECT 1 FROM collections c JOIN json_each(c.properties) f
            WHERE c.id=?1 AND json_type(f.value, '$.calculation')='object'
         )",
        [collection],
        |row| row.get(0),
    )?)
}

pub(super) struct ComputedJob {
    id: i64,
    seeds: Seeds,
    readers: Vec<Access>,
}

impl ComputedJob {
    pub(super) fn expand(db: &Connection, id: i64) -> Result<()> {
        let seeds: Option<String> = db
            .query_row(
                "SELECT seeds FROM computed_jobs WHERE job_id=?1 AND expanded=0",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(seeds) = seeds {
            db.execute(include_str!("computed_targets.sql"), params![id, seeds])?;
            db.execute("UPDATE computed_jobs SET expanded=1 WHERE job_id=?1", [id])?;
        }
        Ok(())
    }

    pub(super) fn read(db: &Connection, id: i64) -> Result<Option<Self>> {
        let row: Option<(String, String)> = db
            .query_row(
                "SELECT seeds, observers FROM computed_jobs WHERE job_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((seeds, observers)) = row else {
            return Ok(None);
        };
        let mut readers = Vec::new();
        for observer in serde_json::from_str::<Vec<Observer>>(&observers)? {
            if let Some(access) = observer.access(db)? {
                readers.push(access);
            }
        }
        Ok(Some(Self {
            id,
            seeds: serde_json::from_str(&seeds)?,
            readers,
        }))
    }

    pub(super) fn calculate(
        &self,
        db: &Connection,
        pages: &[(String, u32, String)],
    ) -> Result<BTreeMap<String, (u32, Vec<Access>)>> {
        let sequence = cache::source_sequence(db)?;
        let mut local = Engine::new(db, None);
        let mut engines: Vec<_> = self
            .readers
            .iter()
            .map(|access| {
                (
                    access,
                    Engine::historical(db, access, self.id),
                    Engine::historical(db, access, self.id + 1),
                    Engine::new(db, Some(access)),
                )
            })
            .collect();
        let mut completed = BTreeMap::new();
        for (page, revision, _) in pages {
            cache::save(db, None, page, sequence, &local.materialize(page)?)?;
            let mut audience = Vec::new();
            for (access, before, after, live) in &mut engines {
                if !access.can_read(db, page)? {
                    continue;
                }
                let can_notify = access.scopes.contains(&IntegrationScope::Changes)
                    && !self.seeds.edited.contains(page);
                if can_notify && before.fields(page)?.1 != after.fields(page)?.1 {
                    audience.push((*access).clone());
                }
                cache::save(db, Some(access), page, sequence, &live.materialize(page)?)?;
            }
            completed.insert(page.clone(), (*revision, audience));
        }
        Ok(completed)
    }
}
