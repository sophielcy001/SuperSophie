// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{ComputedPage, ComputedValues, Dependency, Engine, RULE_VERSION, digest};
use crate::Result;
use crate::assets::hex_sha256;
use crate::integrations::Access;
use crate::store::now;
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;

pub(crate) struct Snapshot {
    pub values: ComputedValues,
    at: i64,
    projection: String,
    dependencies: Vec<Dependency>,
    input_hash: String,
}

pub(crate) struct Reading {
    pub snapshot: Snapshot,
    pub pending: bool,
    pub write_at: Option<i64>,
}

impl Reading {
    pub fn into_page(self, id: &str, revision: u32) -> Result<ComputedPage> {
        let snapshot = self.snapshot;
        let representation_version = if self.pending {
            hex_sha256(&Sha256::digest(serde_json::to_vec(&(
                id,
                revision,
                &snapshot.projection,
                "pending",
            ))?))
        } else {
            hex_sha256(&Sha256::digest(serde_json::to_vec(&(
                id,
                revision,
                &snapshot.projection,
            ))?))
        };
        Ok(ComputedPage {
            page_id: id.to_owned(),
            page_revision: revision,
            representation_version,
            computed_at: snapshot.at,
            stale: self.pending,
            properties: if self.pending {
                ComputedValues::new()
            } else {
                snapshot.values
            },
        })
    }
}

pub(crate) fn source_sequence(db: &Connection) -> Result<i64> {
    Ok(db
        .prepare_cached("SELECT sequence FROM workspace_state WHERE slot=1")?
        .query_row([], |row| row.get(0))?)
}

impl Engine<'_> {
    pub(crate) fn materialize(&mut self, page: &str) -> Result<Snapshot> {
        let mut dependencies = BTreeSet::new();
        let (values, projection) = self.evaluate(page, Some(&mut dependencies))?;
        let dependencies: Vec<_> = dependencies.into_iter().collect();
        let input_hash = self.input_hash(page, &dependencies)?;
        Ok(Snapshot {
            values,
            at: now(),
            projection,
            dependencies,
            input_hash,
        })
    }

    pub(crate) fn cached(&mut self, page: &str) -> Result<Reading> {
        let sequence = source_sequence(self.db)?;
        let (reader, grants, range) = self.access.map_or(("", 0, 0), Access::projection_scope);
        let cached = self.db.prepare_cached(
            "SELECT source_sequence,computed_at,representation,values_json,inputs_json,input_hash
             FROM computed_cache WHERE page_id=?1 AND reader=?2 AND grant_revision=?3 AND range_revision=?4",
        )?.query_row(params![page, reader, grants, range], |row| Ok((
            row.get::<_, i64>(0)?,
            Snapshot {
                at: row.get(1)?, projection: row.get(2)?,
                values: super::super::json_column(row, 3)?,
                dependencies: super::super::json_column(row, 4)?,
                input_hash: row.get(5)?,
            },
        ))).optional()?;
        if let Some((previous, snapshot)) = cached
            && snapshot.input_hash.starts_with(&format!("{RULE_VERSION}:"))
        {
            if previous == sequence
                || snapshot.input_hash == self.input_hash(page, &snapshot.dependencies)?
            {
                return Ok(Reading {
                    snapshot,
                    pending: false,
                    write_at: (previous != sequence).then_some(sequence),
                });
            }
            if self
                .db
                .prepare_cached(
                    "SELECT EXISTS (SELECT 1 FROM computed_jobs WHERE source_sequence>?1)",
                )?
                .query_row([previous], |row| row.get(0))?
            {
                return Ok(Reading {
                    snapshot,
                    pending: true,
                    write_at: None,
                });
            }
        }
        Ok(Reading {
            snapshot: self.materialize(page)?,
            pending: false,
            write_at: Some(sequence),
        })
    }

    fn input_hash(&mut self, page: &str, watched: &[Dependency]) -> Result<String> {
        let mut hash = Sha256::new();
        // An added or deleted output is observable even when the previous
        // snapshot never evaluated a dependency from that rule.
        if let Some(record) = self.record(page)? {
            let schema = self.schema(&record.collection)?;
            for field in &schema.properties {
                if let Some(rule) = &field.calculation {
                    digest(&mut hash, &(&field.id, rule))?;
                }
            }
        }
        for dependency in watched {
            match dependency {
                Dependency::Field(page, field) => {
                    let input = self
                        .record(page)?
                        .map(|record| self.field_input(&record, field))
                        .transpose()?;
                    digest(&mut hash, &(dependency, input))?;
                }
                Dependency::Relation(page, field) => {
                    let members = self
                        .record(page)?
                        .map(|record| self.relation_members(&record, field))
                        .transpose()?;
                    // Only the ordered readable member set is included. A
                    // hidden source edit cannot change readiness or timestamps.
                    digest(
                        &mut hash,
                        &(
                            dependency,
                            members.as_ref().map(|items| {
                                items
                                    .iter()
                                    .map(|item| item.id.as_str())
                                    .collect::<Vec<_>>()
                            }),
                        ),
                    )?;
                }
            }
        }
        Ok(format!("{RULE_VERSION}:{}", hex_sha256(&hash.finalize())))
    }
}

pub(crate) fn save(
    db: &Connection,
    access: Option<&Access>,
    page: &str,
    sequence: i64,
    snapshot: &Snapshot,
) -> Result<()> {
    let (reader, grants, range) = access.map_or(("", 0, 0), Access::projection_scope);
    db.prepare_cached(
        "INSERT INTO computed_cache(
            page_id,reader,grant_revision,range_revision,source_sequence,computed_at,
            representation,values_json,inputs_json,input_hash
         )
         SELECT ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10
         WHERE EXISTS (SELECT 1 FROM pages WHERE id=?1)
           AND (?2='' OR EXISTS (
               SELECT 1 FROM integrations WHERE id=?2 AND grant_revision=?3 AND range_revision=?4 AND revoked_at IS NULL
               AND (expires_at IS NULL OR expires_at > ?11)
           ))
         ON CONFLICT(page_id,reader) DO UPDATE SET
            grant_revision=excluded.grant_revision, range_revision=excluded.range_revision,
            source_sequence=excluded.source_sequence, representation=excluded.representation,
            values_json=excluded.values_json, inputs_json=excluded.inputs_json, input_hash=excluded.input_hash,
            computed_at=CASE WHEN computed_cache.representation=excluded.representation
                  AND computed_cache.grant_revision=excluded.grant_revision
                  AND computed_cache.range_revision=excluded.range_revision
                THEN computed_cache.computed_at ELSE excluded.computed_at END
         WHERE computed_cache.source_sequence<=excluded.source_sequence",
    )?.execute(params![
        page, reader, grants, range, sequence, snapshot.at, snapshot.projection,
        serde_json::to_string(&snapshot.values)?, serde_json::to_string(&snapshot.dependencies)?,
        snapshot.input_hash, now(),
    ])?;
    Ok(())
}
