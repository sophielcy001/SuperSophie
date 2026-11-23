// SPDX-License-Identifier: AGPL-3.0-or-later

mod backup;
pub(crate) mod computed;
mod conversion;
mod csv_export;
mod formula;
mod query;
pub(crate) mod relations;
mod schema;
pub(crate) mod views;

pub(crate) use backup::{
    CollectionSnapshot, restore_collections, snapshot_collections, validate_collections,
};
pub use computed::{
    Calculation, ComputedDraft, ComputedError, ComputedPage, ComputedValue, ComputedValues,
    RollupFunction,
};
pub(crate) use conversion::restore_historical_properties;
pub use conversion::{PropertyConversionPreview, PropertyConversionSample};
pub use csv_export::CsvExportReport;
pub(crate) use formula::remap_fields as remap_formula_fields;
pub(crate) use query::{Arguments, filter_clause};
pub(crate) use query::{RECORD_COLUMNS, record_order, record_row};
pub use relations::{
    RelatedRecord, RelationDefinition, RelationDraft, RelationTargetPage, RelationValue,
};
pub use schema::{
    CollectionView, FilterOperator, PropertyDefinition, PropertyDraft, PropertyKind,
    PropertyOption, PropertyOptionDraft, SortDirection, ViewFilter, ViewKind, ViewSettings,
    ViewSort,
};
pub(crate) use schema::{
    MAX_PROPERTIES, TITLE_FIELD, field_kind, normalize_values, parse_property_text,
    validate_settings,
};
pub use views::{GalleryCover, ScheduleMove, TimelineZoom, ViewDraft, ViewLayout, ViewRange};

use crate::model::{name, new_id};
use crate::store::new_page;
use crate::{Error, Page, Result, Store};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use schema::{Values, build_property, validate_schema};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectionSummary {
    pub id: String,
    pub container_page_id: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Collection {
    pub id: String,
    pub container_page_id: String,
    pub schema_revision: u32,
    pub properties: Vec<PropertyDefinition>,
    pub views: Vec<CollectionView>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Record {
    pub id: String,
    pub title: String,
    pub revision: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub properties: Values,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub relations: std::collections::BTreeMap<String, RelationValue>,
    #[serde(skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub computed: ComputedValues,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub computed_pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<GalleryCover>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RecordDraft {
    pub title: String,
    pub schema_revision: u32,
    pub properties: Values,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordPage {
    pub records: Vec<Record>,
    pub total: u32,
    pub offset: u32,
    pub page_size: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RecordMembership {
    pub(crate) page_id: String,
    pub(crate) collection_id: String,
    created_at: i64,
}

impl Store {
    pub fn collections(&self, workspace: &str) -> Result<Vec<CollectionSummary>> {
        let db = self.connection(workspace)?;
        let mut query =
            db.prepare("SELECT id, container_page_id FROM collections ORDER BY rowid")?;
        Ok(query
            .query_map([], |row| {
                Ok(CollectionSummary {
                    id: row.get(0)?,
                    container_page_id: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn collection(&self, workspace: &str, id: &str) -> Result<Collection> {
        read_collection(&self.connection(workspace)?, id)
    }

    pub fn create_collection(
        &self,
        workspace: &str,
        parent: Option<&str>,
        title: &str,
    ) -> Result<(CollectionSummary, Page)> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (collection, page) = create_collection_in(&tx, parent, title, None)?;
        tx.commit()?;
        Ok((
            CollectionSummary {
                id: collection.id,
                container_page_id: collection.container_page_id,
            },
            page,
        ))
    }

    pub fn save_property(
        &self,
        workspace: &str,
        collection: &str,
        schema_revision: u32,
        draft: PropertyDraft,
    ) -> Result<Collection> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current = read_collection(&tx, collection)?;
        check_schema_revision(&current, schema_revision)?;
        let index = draft
            .id
            .as_ref()
            .map(|id| {
                current
                    .properties
                    .iter()
                    .position(|item| &item.id == id)
                    .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "找不到要修改的属性。"))
            })
            .transpose()?;
        let property = build_property(draft, index.map(|index| &current.properties[index]), false)?;
        let property_id = property.id.clone();
        let labels_changed =
            index.is_some() && matches!(property.kind, PropertyKind::Status | PropertyKind::Select);
        if let Some(index) = index {
            if current.properties[index] == property {
                return Ok(current);
            }
            current.properties[index] = property;
        } else {
            current.properties.push(property);
        }
        validate_schema(&current.properties)?;
        if labels_changed {
            save_schema(&tx, &mut current, &[&property_id])?;
        } else {
            update_schema(&tx, &mut current)?;
        }
        tx.commit()?;
        Ok(current)
    }

    pub fn save_view(
        &self,
        workspace: &str,
        collection: &str,
        view: &str,
        revision: u32,
        settings: ViewSettings,
    ) -> Result<CollectionView> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current = read_collection(&tx, collection)?;
        validate_settings(&settings, &current.properties)?;
        let target = current
            .views
            .iter_mut()
            .find(|item| item.id == view)
            .ok_or_else(view_missing)?;
        views::check_revision(target, revision)?;
        if target.settings == settings {
            return Ok(target.clone());
        }
        target.revision = advance_revision(target.revision)?;
        target.settings = settings;
        let updated = target.clone();
        views::write_views(&tx, &current.id, &current.views)?;
        tx.commit()?;
        Ok(updated)
    }

    pub fn save_record(
        &self,
        workspace: &str,
        collection: &str,
        id: &str,
        revision: u32,
        draft: RecordDraft,
    ) -> Result<Page> {
        let title = name(&draft.title, 200, Some("无标题"))?;
        self.edit(workspace, id, revision, |tx, page| {
            let current = read_collection(tx, collection)?;
            if page.properties.is_none()
                || page.info.parent_id.as_deref() != Some(&current.container_page_id)
            {
                return Err(Error::new("RECORD_NOT_FOUND", "这条记录不属于当前数据库。"));
            }
            check_schema_revision(&current, draft.schema_revision)?;
            let mut owns_attachment = tx.prepare(
                "SELECT EXISTS(SELECT 1 FROM attachments WHERE id = ?1 AND page_id = ?2)",
            )?;
            let mut proposals = draft.properties;
            let mut relation_values = Values::new();
            let current_values = page.properties.as_ref().expect("record was checked");
            for field in current
                .properties
                .iter()
                .filter(|field| field.kind == PropertyKind::Relation)
            {
                if let Some(value) = current_values.get(&field.id) {
                    relation_values.insert(field.id.clone(), value.clone());
                }
                if let Some(delta) = proposals.remove(&field.id) {
                    relations::apply_delta(tx, field, &mut relation_values, delta, |_| Ok(true))?;
                }
            }
            let mut normal = normalize_values(&current.properties, proposals, |attachment| {
                Ok(owns_attachment.query_row(params![attachment, id], |row| row.get(0))?)
            })?;
            normal.extend(relation_values);
            page.properties = Some(normal);
            page.info.title = title;
            Ok(())
        })
    }
}

pub(crate) fn create_collection_in(
    tx: &Transaction<'_>,
    parent: Option<&str>,
    title: &str,
    properties: Option<Vec<PropertyDefinition>>,
) -> Result<(Collection, Page)> {
    let title = name(title, 200, Some("无标题数据库"))?;
    let page = new_page(parent, &title, "database")?;
    create_collection_on_page(tx, page, properties, &[])
}

pub(crate) fn create_collection_on_page(
    tx: &Transaction<'_>,
    page: Page,
    properties: Option<Vec<PropertyDefinition>>,
    attachments: &[crate::Attachment],
) -> Result<(Collection, Page)> {
    if for_parent(tx, page.info.parent_id.as_deref())?.is_some() {
        return Err(Error::new(
            "COLLECTION_PARENT",
            "请先打开一条记录，再新建子数据库。",
        ));
    }
    let properties = properties.unwrap_or_else(|| {
        vec![PropertyDefinition {
            id: new_id(),
            name: "状态".into(),
            kind: PropertyKind::Status,
            relation: None,
            calculation: None,
            options: ["待处理", "进行中", "已完成"]
                .into_iter()
                .map(|label| PropertyOption {
                    id: new_id(),
                    name: label.into(),
                })
                .collect(),
        }]
    });
    validate_schema(&properties)?;
    let groupable = properties
        .iter()
        .find(|property| property.kind == PropertyKind::Status);
    let views = ViewKind::ALL
        .into_iter()
        .map(|kind| {
            let mut view = views::default_view(kind);
            if kind == ViewKind::Board {
                view.settings.group_by = groupable.map(|p| p.id.clone());
            }
            view
        })
        .collect();
    let (page, _) = crate::store::insert_new_page_with_attachments(tx, page, attachments)?;
    let collection = Collection {
        id: new_id(),
        container_page_id: page.info.id.clone(),
        schema_revision: 1,
        properties,
        views,
    };
    insert_collection(tx, &collection)?;
    Ok((collection, page))
}

pub(crate) fn for_parent(db: &Connection, parent: Option<&str>) -> Result<Option<String>> {
    let Some(parent) = parent else {
        return Ok(None);
    };
    Ok(db
        .query_row(
            "SELECT id FROM collections WHERE container_page_id = ?1",
            [parent],
            |row| row.get(0),
        )
        .optional()?)
}

pub(crate) fn membership(page: &Page, collection_id: String) -> RecordMembership {
    RecordMembership {
        page_id: page.info.id.clone(),
        collection_id,
        created_at: page.info.updated_at,
    }
}

pub(crate) fn insert_membership(
    tx: &Transaction<'_>,
    record: &RecordMembership,
    page: &Page,
) -> Result<()> {
    let properties = page
        .properties
        .as_ref()
        .ok_or_else(|| Error::new("INVALID_BUNDLE", "数据库记录缺少属性数据。"))?;
    tx.execute(
        "INSERT INTO collection_records (page_id, collection_id, created_at, properties) VALUES (?1, ?2, ?3, ?4)",
        params![record.page_id, record.collection_id, record.created_at, serde_json::to_string(properties)?],
    )?;
    Ok(())
}

pub(crate) fn update_properties(tx: &Transaction<'_>, id: &str, properties: &Values) -> Result<()> {
    let data = serde_json::to_string(properties)?;
    tx.execute(
        "UPDATE collection_records SET properties = ?2 WHERE page_id = ?1 AND properties <> ?2",
        params![id, data],
    )?;
    Ok(())
}

pub(crate) fn read_collection(db: &Connection, id: &str) -> Result<Collection> {
    db.query_row(
        "SELECT c.id, c.container_page_id, c.schema_revision, c.properties, c.views
         FROM collections c JOIN pages p ON p.id = c.container_page_id
         WHERE c.id = ?1 AND p.trash_root_id IS NULL",
        [id],
        collection_row,
    )
    .optional()?
    .ok_or_else(|| Error::new("COLLECTION_NOT_FOUND", "找不到可用的数据库。"))
}

fn collection_row(row: &Row<'_>) -> rusqlite::Result<Collection> {
    Ok(Collection {
        id: row.get(0)?,
        container_page_id: row.get(1)?,
        schema_revision: row.get(2)?,
        properties: json_column(row, 3)?,
        views: json_column(row, 4)?,
    })
}

pub(crate) fn read_collection_schema(db: &Connection, id: &str) -> Result<Collection> {
    db.query_row(
        "SELECT id, container_page_id, schema_revision, properties, views FROM collections WHERE id = ?1",
        [id],
        collection_row,
    ).optional()?.ok_or_else(|| Error::new("COLLECTION_NOT_FOUND", "找不到数据库定义。"))
}

fn update_schema(tx: &Transaction<'_>, collection: &mut Collection) -> Result<()> {
    collection.schema_revision = advance_revision(collection.schema_revision)?;
    tx.execute(
        "UPDATE collections SET properties = ?2, schema_revision = ?3 WHERE id = ?1",
        params![
            collection.id,
            serde_json::to_string(&collection.properties)?,
            collection.schema_revision
        ],
    )?;
    Ok(())
}

fn save_schema(tx: &Transaction<'_>, collection: &mut Collection, fields: &[&str]) -> Result<()> {
    let computed = crate::integrations::derived::ComputedChange::schema(tx, collection, fields)?;
    update_schema(tx, collection)?;
    computed::index(tx, collection)?;
    crate::integrations::derived::enqueue(tx, None, computed)
}

fn json_column<T: serde::de::DeserializeOwned>(row: &Row<'_>, index: usize) -> rusqlite::Result<T> {
    serde_json::from_str(&row.get::<_, String>(index)?).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn insert_collection(tx: &Transaction<'_>, collection: &Collection) -> Result<()> {
    tx.execute(
        "INSERT INTO collections (id, container_page_id, schema_revision, properties, views) VALUES (?1,?2,?3,?4,?5)",
        params![
            collection.id, collection.container_page_id, collection.schema_revision,
            serde_json::to_string(&collection.properties)?, serde_json::to_string(&collection.views)?,
        ],
    )?;
    computed::index(tx, collection)?;
    Ok(())
}

fn view_missing() -> Error {
    Error::new("VIEW_NOT_FOUND", "找不到这个视图。")
}

fn check_schema_revision(collection: &Collection, revision: u32) -> Result<()> {
    if collection.schema_revision != revision {
        return Err(Error::new(
            "SCHEMA_CONFLICT",
            "数据库属性已有更新，请重新打开编辑面板。",
        ));
    }
    Ok(())
}

fn advance_revision(revision: u32) -> Result<u32> {
    revision
        .checked_add(1)
        .ok_or_else(|| Error::new("REVISION_LIMIT", "数据库修订号已达上限。"))
}
