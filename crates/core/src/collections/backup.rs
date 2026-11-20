// SPDX-License-Identifier: AGPL-3.0-or-later

use super::schema::{normalize_values, validate_schema, validate_views};
use super::{Collection, RecordMembership, collection_row, insert_collection, insert_membership};
use crate::bundle::BundleData;
use crate::model::valid_id;
use crate::{Attachment, Error, Result};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionSnapshot {
    pub(crate) definitions: Vec<Collection>,
    pub(crate) records: Vec<RecordMembership>,
    #[serde(default, skip_serializing_if = "CollectionTombstones::is_empty")]
    pub(crate) tombstones: CollectionTombstones,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CollectionTombstones {
    pub(crate) collections: Vec<String>,
    pub(crate) records: Vec<DeletedRecord>,
}

impl CollectionTombstones {
    pub(crate) fn is_empty(&self) -> bool {
        self.collections.is_empty() && self.records.is_empty()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DeletedRecord {
    pub(crate) page_id: String,
    pub(crate) collection_id: String,
}

pub(crate) fn snapshot_collections(tx: &Transaction<'_>) -> Result<CollectionSnapshot> {
    let mut definitions = tx.prepare(
        "SELECT id, container_page_id, schema_revision, properties, views FROM collections ORDER BY rowid",
    )?;
    let mut records = tx.prepare(
        "SELECT page_id, collection_id, created_at FROM collection_records ORDER BY page_id",
    )?;
    let mut deleted_collections = tx.prepare("SELECT id FROM deleted_collections ORDER BY id")?;
    let mut deleted_records =
        tx.prepare("SELECT page_id, collection_id FROM deleted_records ORDER BY page_id")?;
    Ok(CollectionSnapshot {
        definitions: definitions
            .query_map([], collection_row)?
            .collect::<rusqlite::Result<_>>()?,
        records: records
            .query_map([], |row| {
                Ok(RecordMembership {
                    page_id: row.get(0)?,
                    collection_id: row.get(1)?,
                    created_at: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?,
        tombstones: CollectionTombstones {
            collections: deleted_collections
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?,
            records: deleted_records
                .query_map([], |row| {
                    Ok(DeletedRecord {
                        page_id: row.get(0)?,
                        collection_id: row.get(1)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?,
        },
    })
}

pub(crate) fn validate_collections(
    content: &BundleData,
    snapshot: &CollectionSnapshot,
    attachments: &[Attachment],
) -> Result<()> {
    let pages: HashMap<_, _> = content
        .pages
        .iter()
        .map(|page| (page.info.id.as_str(), page))
        .collect();
    let mut collections = HashMap::new();
    let mut containers = HashMap::new();
    for collection in &snapshot.definitions {
        if !valid_id(&collection.id)
            || collection.schema_revision == 0
            || !pages.contains_key(collection.container_page_id.as_str())
            || collections
                .insert(collection.id.as_str(), collection)
                .is_some()
            || containers
                .insert(collection.container_page_id.as_str(), collection)
                .is_some()
        {
            return Err(invalid_database());
        }
        validate_schema(&collection.properties)?;
        validate_views(&collection.views, &collection.properties)?;
    }
    let mut deleted_collections = HashSet::new();
    for id in &snapshot.tombstones.collections {
        if !valid_id(id)
            || collections.contains_key(id.as_str())
            || !deleted_collections.insert(id.as_str())
        {
            return Err(invalid_database());
        }
    }
    let mut deleted_records = HashMap::new();
    for record in &snapshot.tombstones.records {
        if !valid_id(&record.page_id)
            || pages.contains_key(record.page_id.as_str())
            || (!collections.contains_key(record.collection_id.as_str())
                && !deleted_collections.contains(record.collection_id.as_str()))
            || deleted_records
                .insert(record.page_id.as_str(), record.collection_id.as_str())
                .is_some()
        {
            return Err(invalid_database());
        }
    }
    for source in &snapshot.definitions {
        super::computed::validate_rollups(source, |id| {
            if deleted_collections.contains(id) {
                Ok(None)
            } else {
                let target = collections.get(id).ok_or_else(invalid_database)?;
                Ok(Some((*target).clone()))
            }
        })?;
        for field in &source.properties {
            if let Some(relation) = &field.relation {
                if deleted_collections.contains(relation.collection_id.as_str()) {
                    continue;
                }
                let target = collections
                    .get(relation.collection_id.as_str())
                    .ok_or_else(invalid_database)?;
                if !super::relations::valid_inverse(source, field, target) {
                    return Err(invalid_database());
                }
            }
        }
    }

    let mut members = HashMap::new();
    for member in &snapshot.records {
        let collection = collections
            .get(member.collection_id.as_str())
            .ok_or_else(invalid_database)?;
        let page = pages
            .get(member.page_id.as_str())
            .ok_or_else(invalid_database)?;
        if member.created_at < 0
            || page.info.parent_id.as_deref() != Some(collection.container_page_id.as_str())
            || containers.contains_key(member.page_id.as_str())
            || members
                .insert(member.page_id.as_str(), *collection)
                .is_some()
        {
            return Err(invalid_database());
        }
    }

    for page in &content.pages {
        let parent_collection = page
            .info
            .parent_id
            .as_deref()
            .and_then(|parent| containers.get(parent));
        if parent_collection.map(|item| item.id.as_str())
            != members
                .get(page.info.id.as_str())
                .map(|item| item.id.as_str())
        {
            return Err(invalid_database());
        }
    }
    let owners: HashMap<_, _> = attachments
        .iter()
        .map(|attachment| (attachment.id.as_str(), attachment.page_id.as_str()))
        .collect();
    let pages_with_history = content
        .pages
        .iter()
        .map(|page| (page, false))
        .chain(content.revisions.iter().map(|page| (page, true)));
    for (page, historical) in pages_with_history {
        match (page.properties.as_ref(), members.get(page.info.id.as_str())) {
            (None, None) if page.historical_properties.is_empty() => (),
            (Some(values), Some(collection)) => {
                let mut definitions = std::borrow::Cow::Borrowed(collection.properties.as_slice());
                let mut seen = HashSet::new();
                for field in &page.historical_properties {
                    if !historical
                        || !field.kind.convertible()
                        || !values.contains_key(&field.id)
                        || !seen.insert(&field.id)
                    {
                        return Err(invalid_database());
                    }
                    validate_schema(std::slice::from_ref(field))?;
                    let current = definitions
                        .to_mut()
                        .iter_mut()
                        .find(|current| current.id == field.id && current.kind.convertible())
                        .ok_or_else(invalid_database)?;
                    *current = field.clone();
                }
                let normalized = normalize_values(&definitions, values.clone(), |attachment| {
                    Ok(owners.get(attachment) == Some(&page.info.id.as_str()))
                })?;
                if &normalized != values {
                    return Err(invalid_database());
                }
                for field in &collection.properties {
                    let Some(relation) = &field.relation else {
                        continue;
                    };
                    for id in super::relations::members(values.get(&field.id))? {
                        let target_collection =
                            members.get(id.as_str()).map(|target| target.id.as_str());
                        let deleted_target = historical
                            .then(|| deleted_records.get(id.as_str()).copied())
                            .flatten();
                        if target_collection.or(deleted_target)
                            != Some(relation.collection_id.as_str())
                        {
                            return Err(invalid_database());
                        }
                    }
                }
            }
            _ => return Err(invalid_database()),
        }
    }
    for page in &content.pages {
        let Some(collection) = members.get(page.info.id.as_str()) else {
            continue;
        };
        let values = page.properties.as_ref().ok_or_else(invalid_database)?;
        for field in &collection.properties {
            let Some(inverse) = field
                .relation
                .as_ref()
                .and_then(|relation| relation.inverse_property_id.as_deref())
            else {
                continue;
            };
            for id in super::relations::members(values.get(&field.id))? {
                let reverse = pages[id.as_str()]
                    .properties
                    .as_ref()
                    .and_then(|values| values.get(inverse));
                if !super::relations::members(reverse)?.contains(&page.info.id) {
                    return Err(invalid_database());
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn restore_collections(
    tx: &Transaction<'_>,
    content: &BundleData,
    snapshot: &CollectionSnapshot,
) -> Result<()> {
    let pages: HashMap<_, _> = content
        .pages
        .iter()
        .map(|page| (page.info.id.as_str(), page))
        .collect();
    for collection in &snapshot.definitions {
        let mut definition = collection.clone();
        super::views::expand_legacy(&mut definition.views);
        insert_collection(tx, &definition)?;
    }
    for member in &snapshot.records {
        insert_membership(
            tx,
            member,
            pages
                .get(member.page_id.as_str())
                .ok_or_else(invalid_database)?,
        )?;
    }
    let mut collection = tx.prepare("INSERT INTO deleted_collections (id) VALUES (?1)")?;
    for id in &snapshot.tombstones.collections {
        collection.execute([id])?;
    }
    let mut record =
        tx.prepare("INSERT INTO deleted_records (page_id, collection_id) VALUES (?1, ?2)")?;
    for deleted in &snapshot.tombstones.records {
        record.execute((&deleted.page_id, &deleted.collection_id))?;
    }
    Ok(())
}

fn invalid_database() -> Error {
    Error::new(
        "INVALID_BUNDLE",
        "工作空间包中的数据库、记录页或属性不一致。",
    )
}
