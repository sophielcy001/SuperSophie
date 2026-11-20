// SPDX-License-Identifier: AGPL-3.0-or-later

use super::cursors::checked_limit;
use super::feed_cursors::{
    CATALOG_TTL, CHANGES_TTL, CursorCodec, Position, Sweep, expired, invalid,
};
use super::{Access, IntegrationScope};
use crate::collections::Arguments;
use crate::model::new_id;
use crate::store::now;
use crate::{Result, Store};
use rusqlite::{Connection, Row, Transaction, params, params_from_iter};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CatalogItem {
    Page {
        id: String,
        revision: i64,
        title: String,
        parent_id: Option<String>,
    },
    Collection {
        id: String,
        revision: i64,
        title: String,
        container_page_id: String,
        schema_revision: i64,
    },
    Asset {
        id: String,
        revision: i64,
        name: String,
        page_id: String,
        bytes: i64,
        media_type: String,
    },
}

impl CatalogItem {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let id = row.get(1)?;
        let revision = row.get(2)?;
        Ok(match row.get::<_, String>(0)?.as_str() {
            "page" => Self::Page {
                id,
                revision,
                title: row.get(3)?,
                parent_id: row.get(4)?,
            },
            "collection" => Self::Collection {
                id,
                revision,
                title: row.get(3)?,
                container_page_id: row.get(4)?,
                schema_revision: row.get(5)?,
            },
            "asset" => Self::Asset {
                id,
                revision,
                name: row.get(3)?,
                page_id: row.get(4)?,
                bytes: row.get(5)?,
                media_type: row.get(6)?,
            },
            _ => return Err(rusqlite::Error::InvalidQuery),
        })
    }

    fn position(&self) -> Position {
        let (kind, id) = match self {
            Self::Page { id, .. } => ("page", id),
            Self::Collection { id, .. } => ("collection", id),
            Self::Asset { id, .. } => ("asset", id),
        };
        Position::Catalog {
            kind: kind.into(),
            id: id.clone(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CatalogPage {
    pub items: Vec<CatalogItem>,
    pub catalog_cursor: Option<String>,
    pub change_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ChangeEvent {
    pub event_id: String,
    pub kind: String,
    pub id: String,
    pub revision: i64,
    pub action: String,
}

#[derive(Debug, Serialize)]
pub struct ChangePage {
    pub items: Vec<ChangeEvent>,
    pub watermark: String,
    pub next_cursor: String,
    pub has_more: bool,
}

impl Store {
    pub fn integration_catalog(
        &self,
        workspace: &str,
        token: &str,
        limit: Option<usize>,
        cursor: Option<&str>,
    ) -> Result<CatalogPage> {
        self.integration_read(workspace, token, |db, access| {
            let limit = checked_limit(limit)?;
            let codec = CursorCodec::new(db, access)?;
            let (kind, id, expires_at, change_cursor) = match cursor {
                Some(token) => {
                    let (Position::Catalog { kind, id }, expires_at) = codec.open(token)? else {
                        return Err(invalid());
                    };
                    (kind, id, expires_at, None)
                }
                None => {
                    let time = now();
                    let first = codec.seal(
                        Position::Changes {
                            after: high_water(db)?,
                            sweep: None,
                        },
                        time + CHANGES_TTL,
                    )?;
                    (
                        String::new(),
                        String::new(),
                        time + CATALOG_TTL,
                        Some(first),
                    )
                }
            };
            let mut items = catalog_items(db, access, kind, id, limit + 1)?;
            let catalog_cursor = if items.len() > limit {
                items.pop();
                Some(codec.seal(items.last().unwrap().position(), expires_at)?)
            } else {
                None
            };
            Ok(CatalogPage {
                items,
                catalog_cursor,
                change_cursor,
            })
        })
    }

    pub fn integration_changes(
        &self,
        workspace: &str,
        token: &str,
        limit: Option<usize>,
        cursor: &str,
    ) -> Result<ChangePage> {
        self.integration_read(workspace, token, |db, access| {
            access.require(IntegrationScope::Changes)?;
            let limit = checked_limit(limit)?;
            let codec = CursorCodec::new(db, access)?;
            let (Position::Changes { after, sweep }, expires_at) = codec.open(cursor)? else {
                return Err(invalid());
            };
            let floor: i64 =
                db.query_row("SELECT floor FROM feed_state WHERE slot = 1", [], |row| {
                    row.get(0)
                })?;
            if after < floor {
                return Err(expired());
            }
            let sweep = match sweep {
                Some(sweep) => sweep,
                None => Sweep {
                    high_water: high_water(db)?,
                    watermark: new_id(),
                },
            };
            let mut rows = scan_changes(db, access, after, sweep.high_water, limit + 1)?;
            let has_more = rows.len() > limit;
            let watermark = sweep.watermark.clone();
            let next_cursor = if has_more {
                rows.pop();
                let after = rows.last().unwrap().0;
                codec.seal(
                    Position::Changes {
                        after,
                        sweep: Some(sweep),
                    },
                    expires_at,
                )?
            } else {
                codec.seal(
                    Position::Changes {
                        after: sweep.high_water,
                        sweep: None,
                    },
                    now() + CHANGES_TTL,
                )?
            };
            Ok(ChangePage {
                items: rows.into_iter().map(|(_, event)| event).collect(),
                watermark,
                next_cursor,
                has_more,
            })
        })
    }
}

fn catalog_items(
    db: &Connection,
    access: &Access,
    kind: String,
    id: String,
    limit: usize,
) -> Result<Vec<CatalogItem>> {
    let mut arguments = Arguments::default();
    let readable = access.readable_pages_cte(&mut arguments);
    let assets = arguments.bind(access.scopes.contains(&IntegrationScope::ReadAssets));
    let last_kind = arguments.bind(kind);
    let last_id = arguments.bind(id);
    let limit = arguments.bind(limit as i64);
    let mut query = db.prepare(&format!(
        "{readable},
         catalog(kind, id, revision, name, parent, extra, media_type) AS (
           SELECT 'page', p.id, p.revision, p.title, parent.id, NULL, NULL
           FROM readable_pages a JOIN pages p ON p.id = a.id
           LEFT JOIN readable_pages parent ON parent.id = p.parent_id
           UNION ALL
           SELECT 'collection', c.id, coalesce(v.revision, 1), p.title, a.id, c.schema_revision, NULL
           FROM readable_pages a JOIN collections c ON c.container_page_id = a.id
           JOIN pages p ON p.id = a.id
           LEFT JOIN feed_versions v ON v.kind = 'collection' AND v.object_id = c.id
           UNION ALL
           SELECT 'asset', x.id, coalesce(v.revision, 1), x.name, a.id, x.bytes, x.media_type
           FROM readable_pages a JOIN attachments x ON x.page_id = a.id
           LEFT JOIN feed_versions v ON v.kind = 'asset' AND v.object_id = x.id WHERE {assets}
         )
         SELECT * FROM catalog WHERE (kind, id) > ({last_kind}, {last_id})
         ORDER BY kind, id LIMIT {limit}",
    ))?;
    Ok(query
        .query_map(params_from_iter(arguments.0), CatalogItem::from_row)?
        .collect::<rusqlite::Result<_>>()?)
}

fn high_water(db: &Connection) -> Result<i64> {
    Ok(db.query_row(
        "SELECT coalesce((SELECT seq FROM sqlite_sequence WHERE name = 'feed_events'), 0)",
        [],
        |row| row.get(0),
    )?)
}

fn scan_changes(
    db: &Connection,
    access: &Access,
    after: i64,
    horizon: i64,
    limit: usize,
) -> Result<Vec<(i64, ChangeEvent)>> {
    let mut arguments = Arguments::default();
    let covered = access.covered_pages_cte(&mut arguments, true);
    let assets = arguments.bind(access.scopes.contains(&IntegrationScope::ReadAssets));
    let subject = arguments.bind(access.id.clone());
    let grant = arguments.bind(access.grant_revision);
    let range = arguments.bind(access.range_revision);
    let after = arguments.bind(after);
    let horizon = arguments.bind(horizon);
    let limit = arguments.bind(limit as i64);
    // Filter before LIMIT. The caller receives no empty pages that count hidden events.
    let mut query = db.prepare(&format!(
        "{covered}
         SELECT e.seq, e.event_id, e.kind, e.object_id, e.revision,
                CASE WHEN e.derived = 1 THEN 'derived.invalidated' ELSE e.action END
         FROM feed_events e
         JOIN readable_pages a ON a.id = e.owner_page_id
         JOIN pages p ON p.id = a.id
         LEFT JOIN feed_tombstones t ON t.seq = e.seq AND t.integration_id = {subject}
         LEFT JOIN feed_derived_audience d ON d.seq = e.seq AND d.integration_id = {subject}
         WHERE e.seq > {after} AND e.seq <= {horizon}
           AND ({assets} OR e.kind <> 'asset')
           AND ((e.action = 'upsert' AND p.trash_root_id IS NULL AND (
                    e.derived = 0 OR (d.grant_revision = {grant} AND d.range_revision = {range})
               ))
             OR (e.action = 'delete' AND t.grant_revision = {grant} AND t.range_revision = {range}))
         ORDER BY e.seq LIMIT {limit}",
    ))?;
    Ok(query
        .query_map(params_from_iter(arguments.0), |row| {
            Ok((
                row.get(0)?,
                ChangeEvent {
                    event_id: row.get(1)?,
                    kind: row.get(2)?,
                    id: row.get(3)?,
                    revision: row.get(4)?,
                    action: row.get(5)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(super) fn publish_derived(
    db: &Connection,
    page: &str,
    revision: u32,
    audience: &[&Access],
) -> Result<()> {
    let seq: i64 = db.query_row(
        "INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision, derived)
         VALUES ('page', ?1, ?1, 'upsert', ?2, 1) RETURNING seq",
        (page, revision),
        |row| row.get(0),
    )?;
    let mut insert =
        db.prepare_cached("INSERT INTO feed_derived_audience VALUES (?1, ?2, ?3, ?4)")?;
    for access in audience {
        insert.execute(params![
            seq,
            access.id,
            access.grant_revision,
            access.range_revision
        ])?;
    }
    Ok(())
}

fn subjects(db: &Connection) -> Result<Vec<(String, String)>> {
    let mut query = db.prepare(
        "SELECT id, name FROM integrations WHERE revoked_at IS NULL
         AND (expires_at IS NULL OR expires_at > ?1) ORDER BY name, id",
    )?;
    Ok(query
        .query_map([now()], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn record_archive(tx: &Transaction<'_>, ids: &[String]) -> Result<()> {
    let before = high_water(tx)?;
    let ids = serde_json::to_string(ids)?;
    tx.execute(
        "WITH targets(id) AS (SELECT value FROM json_each(?1))
         INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
         SELECT 'page', p.id, p.id, 'delete', p.revision + 1
         FROM targets t JOIN pages p ON p.id = t.id
         UNION ALL
         SELECT 'collection', c.id, t.id, 'delete', coalesce(v.revision, 1) + 1
         FROM targets t JOIN collections c ON c.container_page_id = t.id
         LEFT JOIN feed_versions v ON v.kind = 'collection' AND v.object_id = c.id
         UNION ALL
         SELECT 'asset', a.id, t.id, 'delete', coalesce(v.revision, 1) + 1
         FROM targets t JOIN attachments a ON a.page_id = t.id
         LEFT JOIN feed_versions v ON v.kind = 'asset' AND v.object_id = a.id",
        [&ids],
    )?;
    // Authorize before any node of the branch is put in the trash.
    for (id, _) in subjects(tx)? {
        let Some(access) = Access::by_id(tx, &id)? else {
            continue;
        };
        if access.require(IntegrationScope::Read).is_err()
            || access.require(IntegrationScope::Changes).is_err()
        {
            continue;
        }
        let mut arguments = Arguments::default();
        let readable = access.readable_pages_cte(&mut arguments);
        let id = arguments.bind(id);
        let grant = arguments.bind(access.grant_revision);
        let range = arguments.bind(access.range_revision);
        let after = arguments.bind(before);
        let assets = arguments.bind(access.scopes.contains(&IntegrationScope::ReadAssets));
        tx.execute(
            &format!(
                "{readable}
             INSERT INTO feed_tombstones
             SELECT e.seq, {id}, {grant}, {range}
             FROM feed_events e JOIN readable_pages a ON a.id = e.owner_page_id
             WHERE e.seq > {after} AND e.action = 'delete' AND ({assets} OR e.kind <> 'asset')",
            ),
            params_from_iter(arguments.0),
        )?;
    }
    Ok(())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveImpact {
    pub integration_name: String,
    pub added: usize,
    pub removed: usize,
}

pub(crate) struct RangeSnapshot(BTreeMap<String, (String, HashMap<String, bool>)>);

impl RangeSnapshot {
    pub fn capture(db: &Connection) -> Result<Self> {
        let mut ranges = BTreeMap::new();
        for (id, name) in subjects(db)? {
            if let Some(access) = Access::by_id(db, &id)? {
                ranges.insert(id, (name, coverage(db, &access)?));
            }
        }
        Ok(Self(ranges))
    }

    pub fn finish(self, tx: &Transaction<'_>) -> Result<Vec<MoveImpact>> {
        let mut impacts = Vec::new();
        for (id, (name, before)) in self.0 {
            let Some(access) = Access::by_id(tx, &id)? else {
                continue;
            };
            let after = coverage(tx, &access)?;
            let added = after
                .iter()
                .filter(|(id, _)| !before.contains_key(*id))
                .collect::<Vec<_>>();
            let removed = before
                .iter()
                .filter(|(id, _)| !after.contains_key(*id))
                .collect::<Vec<_>>();
            if !added.is_empty() || !removed.is_empty() {
                tx.execute(
                    "UPDATE integrations SET range_revision = range_revision + 1 WHERE id = ?1",
                    [&id],
                )?;
                let added = added.iter().filter(|(_, active)| **active).count();
                let removed = removed.iter().filter(|(_, active)| **active).count();
                impacts.push(MoveImpact {
                    integration_name: name,
                    added,
                    removed,
                });
            }
        }
        impacts.sort_by(|left, right| left.integration_name.cmp(&right.integration_name));
        Ok(impacts)
    }
}

fn coverage(db: &Connection, access: &Access) -> Result<HashMap<String, bool>> {
    let mut arguments = Arguments::default();
    let covered = access.covered_pages_cte(&mut arguments, true);
    let mut query = db.prepare(&format!(
        "{covered}
         SELECT a.id, p.trash_root_id IS NULL FROM readable_pages a JOIN pages p ON p.id = a.id",
    ))?;
    Ok(query
        .query_map(params_from_iter(arguments.0), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<_>>()?)
}
