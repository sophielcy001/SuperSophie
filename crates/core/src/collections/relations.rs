// SPDX-License-Identifier: AGPL-3.0-or-later

use super::schema::{Values, definition, validate_schema};
use super::{
    Collection, PropertyDefinition, PropertyKind, check_schema_revision, for_parent, json_column,
    read_collection, read_collection_schema, update_schema,
};
use crate::model::{name, new_id, valid_id};
use crate::store::read_page;
use crate::{Error, Page, Result, Store};
use rusqlite::{Connection, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelationDefinition {
    pub collection_id: String,
    pub inverse_property_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelationDraft {
    pub name: String,
    pub collection_id: String,
    pub inverse_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelationDelta {
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelatedRecord {
    pub id: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationValue {
    pub items: Vec<RelatedRecord>,
    pub restricted: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationTargetPage {
    pub items: Vec<RelatedRecord>,
    pub next_offset: Option<u32>,
}

impl Store {
    pub fn create_relation(
        &self,
        workspace: &str,
        collection: &str,
        revision: u32,
        draft: RelationDraft,
    ) -> Result<Collection> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut source = read_collection(&tx, collection)?;
        check_schema_revision(&source, revision)?;
        let same_collection = collection == draft.collection_id;
        let mut target = if same_collection {
            None
        } else {
            Some(read_collection(&tx, &draft.collection_id)?)
        };
        let id = new_id();
        let inverse = draft.inverse_name.as_ref().map(|_| new_id());
        source.properties.push(PropertyDefinition {
            id: id.clone(),
            name: name(&draft.name, 64, None)?,
            kind: PropertyKind::Relation,
            calculation: None,
            options: vec![],
            relation: Some(RelationDefinition {
                collection_id: draft.collection_id,
                inverse_property_id: inverse.clone(),
            }),
        });
        if let Some((inverse_id, inverse_name)) = inverse.zip(draft.inverse_name) {
            let field = PropertyDefinition {
                id: inverse_id,
                name: name(&inverse_name, 64, None)?,
                kind: PropertyKind::Relation,
                calculation: None,
                options: vec![],
                relation: Some(RelationDefinition {
                    collection_id: source.id.clone(),
                    inverse_property_id: Some(id),
                }),
            };
            if let Some(target) = &mut target {
                target.properties.push(field);
                validate_schema(&target.properties)?;
                update_schema(&tx, target)?;
            } else {
                source.properties.push(field);
            }
        }
        validate_schema(&source.properties)?;
        update_schema(&tx, &mut source)?;
        tx.commit()?;
        Ok(source)
    }

    pub fn relation_targets(
        &self,
        workspace: &str,
        collection: &str,
        term: &str,
        offset: u32,
    ) -> Result<RelationTargetPage> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        if deleted_collection(&tx, collection)? {
            return Err(Error::new(
                "RELATION_TARGET_DELETED",
                "此关联的目标数据库已永久删除。",
            ));
        }
        read_collection(&tx, collection)?;
        let mut query = tx.prepare(
            "SELECT p.id, p.title FROM collection_records r JOIN pages p ON p.id = r.page_id
             WHERE r.collection_id = ?1 AND p.trash_root_id IS NULL
               AND instr(lower(p.title), lower(?2)) > 0
             ORDER BY p.title COLLATE NOCASE, p.id LIMIT 26 OFFSET ?3",
        )?;
        let mut items = query
            .query_map(params![collection, term.trim(), offset], |row| {
                Ok(RelatedRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let next_offset = (items.len() > 25).then(|| {
            items.pop();
            offset.saturating_add(25)
        });
        Ok(RelationTargetPage { items, next_offset })
    }

    pub fn project_desktop_page(&self, workspace: &str, mut page: Page) -> Result<Page> {
        if let Some(values) = page.properties.as_mut() {
            let mut db = self.connection(workspace)?;
            let tx = db.transaction()?;
            if let Some(id) = for_parent(&tx, page.info.parent_id.as_deref())? {
                let collection = read_collection_schema(&tx, &id)?;
                project_values(&tx, &collection.properties, values)?;
            }
        }
        Ok(page)
    }
}

pub(crate) fn members(value: Option<&Value>) -> Result<BTreeSet<String>> {
    let Some(value) = value else {
        return Ok(BTreeSet::new());
    };
    let ids = value.as_array().ok_or_else(invalid_value)?;
    let mut items = BTreeSet::new();
    for value in ids {
        let id = value
            .as_str()
            .filter(|id| valid_id(id))
            .ok_or_else(invalid_value)?;
        if !items.insert(id.to_owned()) {
            return Err(invalid_value());
        }
    }
    Ok(items)
}

fn set_members(values: &mut Values, field: &str, ids: BTreeSet<String>) {
    if ids.is_empty() {
        values.remove(field);
    } else {
        values.insert(
            field.to_owned(),
            Value::Array(ids.into_iter().map(Value::String).collect()),
        );
    }
}

pub(crate) fn remove_deleted_targets(
    db: &Connection,
    properties: &[PropertyDefinition],
    values: &mut Values,
) -> Result<()> {
    let mut exists = db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM deleted_records WHERE page_id = ?1 AND collection_id = ?2)",
    )?;
    for property in properties {
        let Some(relation) = &property.relation else {
            continue;
        };
        let Some(value) = values.get(&property.id) else {
            continue;
        };
        let mut kept = BTreeSet::new();
        for id in members(Some(value))? {
            let deleted: bool =
                exists.query_row((&id, &relation.collection_id), |row| row.get(0))?;
            if !deleted {
                kept.insert(id);
            }
        }
        set_members(values, &property.id, kept);
    }
    Ok(())
}

pub(super) fn deleted_collection(db: &Connection, id: &str) -> Result<bool> {
    Ok(db
        .prepare_cached("SELECT EXISTS(SELECT 1 FROM deleted_collections WHERE id = ?1)")?
        .query_row([id], |row| row.get(0))?)
}

fn target_exists(
    db: &Connection,
    collection: &str,
    target: &str,
    include_archived: bool,
) -> Result<bool> {
    let mut query = db.prepare_cached(
        "SELECT EXISTS(SELECT 1 FROM collection_records r JOIN pages p ON p.id = r.page_id
         WHERE r.page_id = ?1 AND r.collection_id = ?2 AND (?3 OR p.trash_root_id IS NULL))",
    )?;
    Ok(
        query.query_row(params![target, collection, include_archived], |row| {
            row.get(0)
        })?,
    )
}

pub(crate) fn apply_delta(
    db: &Connection,
    field: &PropertyDefinition,
    values: &mut Values,
    delta: Value,
    mut can_read: impl FnMut(&str) -> Result<bool>,
) -> Result<()> {
    let relation = field.relation.as_ref().ok_or_else(invalid_schema)?;
    let mut current = members(values.get(&field.id))?;
    if !delta.is_object() {
        for target in &current {
            if !can_read(target)? || !target_exists(db, &relation.collection_id, target, false)? {
                return Err(Error::new(
                    "PARTIAL_CONTENT",
                    "关联字段含不可访问的记录，请按项增删。",
                ));
            }
        }
        return Err(delta_required());
    }
    let RelationDelta { add, remove } =
        serde_json::from_value(delta).map_err(|_| delta_required())?;
    let mut seen = BTreeSet::new();
    for target in add.iter().chain(&remove) {
        if !valid_id(target) || !seen.insert(target) {
            return Err(delta_required());
        }
        if !can_read(target)? || !target_exists(db, &relation.collection_id, target, false)? {
            return Err(Error::new("RESOURCE_NOT_FOUND", "找不到可用的关联记录。"));
        }
    }
    for target in remove {
        current.remove(&target);
    }
    current.extend(add);
    set_members(values, &field.id, current);
    Ok(())
}

pub(crate) fn prepare_mirrors(
    db: &Connection,
    page: &mut Page,
    creating: bool,
) -> Result<Vec<Page>> {
    let Some(values) = page.properties.as_mut() else {
        return Ok(vec![]);
    };
    let previous: Values = if creating {
        Values::new()
    } else {
        db.query_row(
            "SELECT properties FROM collection_records WHERE page_id = ?1",
            [&page.info.id],
            |row| json_column(row, 0),
        )?
    };
    if *values == previous {
        return Ok(vec![]);
    }
    let collection_id =
        for_parent(db, page.info.parent_id.as_deref())?.ok_or_else(invalid_schema)?;
    let collection = read_collection_schema(db, &collection_id)?;
    let mut mirrors = BTreeMap::<String, Vec<(String, bool)>>::new();
    for field in &collection.properties {
        let Some(relation) = &field.relation else {
            continue;
        };
        let old = previous.get(&field.id);
        let new = values.get(&field.id);
        if old == new {
            continue;
        }
        let old = members(old)?;
        let new = members(new)?;
        for target in new.difference(&old) {
            let exists = if target == &page.info.id {
                relation.collection_id == collection_id
            } else {
                target_exists(db, &relation.collection_id, target, true)?
            };
            if !exists {
                return Err(Error::new("RELATION_TARGET", "关联记录不属于目标数据库。"));
            }
        }
        if let Some(inverse) = &relation.inverse_property_id {
            for target in old.symmetric_difference(&new) {
                mirrors
                    .entry(target.clone())
                    .or_default()
                    .push((inverse.clone(), new.contains(target)));
            }
        }
    }
    let adjust = |values: &mut Values, edits: Vec<(String, bool)>| -> Result<()> {
        for (field, add) in edits {
            let mut list = members(values.get(&field))?;
            if add {
                list.insert(page.info.id.clone());
            } else {
                list.remove(&page.info.id);
            }
            set_members(values, &field, list);
        }
        Ok(())
    };
    if let Some(own_edits) = mirrors.remove(&page.info.id) {
        adjust(values, own_edits)?;
    }
    let mut updated = Vec::new();
    for (id, edits) in mirrors {
        let mut target = read_page(db, &id)?;
        let values = target.properties.as_mut().ok_or_else(invalid_schema)?;
        let original = values.clone();
        adjust(values, edits)?;
        if *values != original {
            updated.push(target);
        }
    }
    Ok(updated)
}

pub(crate) fn project_values(
    db: &Connection,
    definitions: &[PropertyDefinition],
    values: &mut Values,
) -> Result<BTreeMap<String, RelationValue>> {
    let mut displays = BTreeMap::new();
    for field in definitions {
        let Some(relation) = &field.relation else {
            continue;
        };
        let Some(value) = values.get(&field.id) else {
            continue;
        };
        let count = value.as_array().ok_or_else(invalid_value)?.len();
        let mut query = db.prepare_cached(
            "SELECT p.id, p.title FROM json_each(?1) j
             JOIN collection_records r ON r.page_id = j.value
             JOIN pages p ON p.id = r.page_id AND p.trash_root_id IS NULL
             WHERE r.collection_id = ?2 ORDER BY p.title COLLATE NOCASE, p.id",
        )?;
        let items = query
            .query_map(params![value.to_string(), relation.collection_id], |row| {
                Ok(RelatedRecord {
                    id: row.get(0)?,
                    title: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        values.insert(
            field.id.clone(),
            Value::Array(
                items
                    .iter()
                    .map(|item| Value::String(item.id.clone()))
                    .collect(),
            ),
        );
        displays.insert(
            field.id.clone(),
            RelationValue {
                restricted: items.len() != count,
                items,
            },
        );
    }
    Ok(displays)
}

pub(super) fn valid_inverse(
    source: &Collection,
    field: &PropertyDefinition,
    target: &Collection,
) -> bool {
    let Some(config) = &field.relation else {
        return false;
    };
    config.collection_id == target.id
        && config.inverse_property_id.as_deref().is_none_or(|inverse| {
            definition(&target.properties, inverse)
                .and_then(|field| field.relation.as_ref())
                .is_some_and(|related| {
                    related.collection_id == source.id
                        && related.inverse_property_id.as_deref() == Some(&field.id)
                })
        })
}

fn invalid_value() -> Error {
    Error::new("PROPERTY_VALUE", "关联值包含无效或重复的记录标识。")
}

fn invalid_schema() -> Error {
    Error::new("RELATION_SCHEMA", "找不到关联属性的定义。")
}

fn delta_required() -> Error {
    Error::new("RELATION_DELTA", "使用 add 和 remove 指定关联记录的增删。")
}
