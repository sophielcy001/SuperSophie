// SPDX-License-Identifier: AGPL-3.0-or-later

use super::schema::{Values, build_property, definition, validate_schema, validate_views};
use super::{
    Collection, CollectionView, PropertyDefinition, PropertyDraft, PropertyKind, ViewSettings,
    advance_revision, check_schema_revision, computed, normalize_values, parse_property_text,
    read_collection, read_collection_schema, update_schema, validate_settings, views,
};
use crate::integrations::derived::{ComputedChange, enqueue};
use crate::store::{read_page, write_revision};
use crate::templates::{Template, collection_templates, write_template};
use crate::{Error, Result, Store};
use rusqlite::{Connection, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const SAMPLE_LIMIT: usize = 12;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyConversionSample {
    pub title: String,
    pub previous: Value,
    pub converted: Option<Value>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PropertyConversionPreview {
    pub fingerprint: String,
    pub previous: PropertyDefinition,
    pub target: PropertyDefinition,
    pub converted_records: usize,
    pub cleared_records: usize,
    pub converted_templates: Vec<String>,
    pub cleared_templates: Vec<String>,
    pub view_changes: Vec<String>,
    pub samples: Vec<PropertyConversionSample>,
}

struct Conversion {
    collection: Collection,
    preview: PropertyConversionPreview,
    records: Vec<(String, Option<Value>)>,
    templates: Vec<Template>,
}

impl Store {
    pub fn preview_property_conversion(
        &self,
        workspace: &str,
        collection: &str,
        schema_revision: u32,
        draft: PropertyDraft,
    ) -> Result<PropertyConversionPreview> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        Ok(prepare(&tx, collection, schema_revision, draft)?.preview)
    }

    pub fn apply_property_conversion(
        &self,
        workspace: &str,
        collection: &str,
        schema_revision: u32,
        draft: PropertyDraft,
        fingerprint: &str,
    ) -> Result<Collection> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let Conversion {
            mut collection,
            preview,
            records,
            templates,
        } = prepare(&tx, collection, schema_revision, draft)?;
        if preview.fingerprint != fingerprint {
            return Err(Error::new(
                "PROPERTY_CONVERSION_CHANGED",
                "记录、模板或视图已有变化，请重新预览。",
            ));
        }

        let field = &preview.previous.id;
        let computed = ComputedChange::conversion(
            &tx,
            &collection,
            field,
            records.iter().map(|(id, _)| id.as_str()),
        )?;
        retain_historical_definition(&tx, &collection.id, &preview.previous)?;
        update_schema(&tx, &mut collection)?;
        if !preview.view_changes.is_empty() {
            views::write_views(&tx, &collection.id, &collection.views)?;
        }
        for (id, value) in records {
            let mut page = read_page(&tx, &id)?;
            let properties = page.properties.as_mut().ok_or_else(invalid_value)?;
            replace_value(properties, field, value);
            let blocks = serde_json::to_string(&page.blocks)?;
            write_revision(&tx, &mut page, &blocks)?;
        }
        for template in &templates {
            write_template(&tx, template, true)?;
        }
        computed::index(&tx, &collection)?;
        enqueue(&tx, None, computed)?;
        tx.commit()?;
        Ok(collection)
    }
}

fn prepare(db: &Connection, id: &str, revision: u32, draft: PropertyDraft) -> Result<Conversion> {
    let mut collection = read_collection(db, id)?;
    check_schema_revision(&collection, revision)?;
    let index = collection
        .properties
        .iter()
        .position(|field| Some(&field.id) == draft.id.as_ref())
        .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "找不到要转换的属性。"))?;
    let previous = collection.properties[index].clone();
    if !previous.kind.convertible() || !draft.kind.convertible() || previous.kind == draft.kind {
        return Err(Error::new(
            "PROPERTY_TYPE",
            "请选择另一种可填写的属性类型。",
        ));
    }
    let mut fingerprint = Sha256::new();
    // Bind confirmation to the user input and source state, before assigning new option IDs.
    fingerprint.update(serde_json::to_vec(&(&collection, &draft))?);
    let target = build_property(draft, Some(&previous), true)?;
    collection.properties[index] = target.clone();
    validate_schema(&collection.properties)?;
    validate_dependent_rollups(db, &collection, &previous)?;

    let mut view_changes = Vec::new();
    for view in &mut collection.views {
        if let Some(change) = adjust_view(view, &collection.properties, &previous.id)? {
            view_changes.push(change);
        }
    }
    validate_views(&collection.views, &collection.properties)?;

    let mut preview = PropertyConversionPreview {
        fingerprint: String::new(),
        previous,
        target,
        converted_records: 0,
        cleared_records: 0,
        converted_templates: Vec::new(),
        cleared_templates: Vec::new(),
        view_changes,
        samples: Vec::new(),
    };
    let mut records = Vec::new();
    let mut cleared = Vec::new();
    let mut converted = Vec::new();
    let mut query = db.prepare(
        "SELECT p.id, p.title, p.revision, r.properties FROM collection_records r
         JOIN pages p ON p.id = r.page_id WHERE r.collection_id = ?1 AND json_type(r.properties, ?2) IS NOT NULL
         ORDER BY p.id",
    )?;
    let mut rows = query.query(params![id, property_path(&preview.previous.id, "$")])?;
    while let Some(row) = rows.next()? {
        let record: String = row.get(0)?;
        let revision: u32 = row.get(2)?;
        let values: Values = serde_json::from_str(&row.get::<_, String>(3)?)?;
        let prior = values.get(&preview.previous.id).ok_or_else(invalid_value)?;
        fingerprint.update(serde_json::to_vec(&(&record, revision, prior))?);
        let value = convert_value(&preview.previous, &preview.target, prior);
        let samples = if value.is_some() {
            preview.converted_records += 1;
            &mut converted
        } else {
            preview.cleared_records += 1;
            &mut cleared
        };
        if samples.len() < SAMPLE_LIMIT {
            samples.push(PropertyConversionSample {
                title: row.get(1)?,
                previous: prior.clone(),
                converted: value.clone(),
            });
        }
        records.push((record, value));
    }
    preview
        .samples
        .extend(cleared.into_iter().chain(converted).take(SAMPLE_LIMIT));

    let mut templates = Vec::new();
    for mut template in collection_templates(db, id)? {
        let Some(values) = template
            .pages
            .first()
            .and_then(|page| page.properties.as_ref())
        else {
            return Err(invalid_value());
        };
        if let Some(prior) = values.get(&preview.previous.id) {
            fingerprint.update(serde_json::to_vec(&template)?);
            let value = convert_value(&preview.previous, &preview.target, prior);
            let names = if value.is_some() {
                &mut preview.converted_templates
            } else {
                &mut preview.cleared_templates
            };
            names.push(template.name.clone());
            let values = template.pages[0]
                .properties
                .as_mut()
                .ok_or_else(invalid_value)?;
            replace_value(values, &preview.previous.id, value);
            templates.push(template);
        }
    }
    preview.fingerprint = crate::assets::hex_sha256(&fingerprint.finalize());
    Ok(Conversion {
        collection,
        preview,
        records,
        templates,
    })
}

fn adjust_view(
    view: &mut CollectionView,
    properties: &[PropertyDefinition],
    changed_field: &str,
) -> Result<Option<String>> {
    let mut adjustments = Vec::new();
    if view
        .settings
        .filter
        .as_ref()
        .is_some_and(|filter| filter.field_id == changed_field && filter.value.is_some())
        || validate_settings(
            &ViewSettings {
                filter: view.settings.filter.clone(),
                ..ViewSettings::default()
            },
            properties,
        )
        .is_err()
    {
        view.settings.filter = None;
        adjustments.push("筛选");
    }
    if validate_settings(
        &ViewSettings {
            sort: view.settings.sort.clone(),
            ..ViewSettings::default()
        },
        properties,
    )
    .is_err()
    {
        view.settings.sort = None;
        adjustments.push("排序");
    }
    if validate_settings(
        &ViewSettings {
            group_by: view.settings.group_by.clone(),
            ..ViewSettings::default()
        },
        properties,
    )
    .is_err()
    {
        view.settings.group_by = None;
        adjustments.push("分组");
    }
    if views::validate_layout(view, properties).is_err() {
        view.layout = None;
        adjustments.push("日期或封面设置");
    }
    if adjustments.is_empty() {
        return Ok(None);
    }
    view.revision = advance_revision(view.revision)?;
    let label = view.name.as_deref().unwrap_or_else(|| view.kind.label());
    Ok(Some(format!(
        "「{label}」将清除{}。",
        adjustments.join("、")
    )))
}

fn validate_dependent_rollups(
    db: &Connection,
    next: &Collection,
    previous: &PropertyDefinition,
) -> Result<()> {
    if previous.kind != PropertyKind::Number {
        return Ok(());
    }
    let mut query = db.prepare(
        "SELECT DISTINCT collection_id FROM computed_dependencies WHERE input_id = ?1 AND relation_id <> ''",
    )?;
    let sources = query.query_map([&previous.id], |row| row.get::<_, String>(0))?;
    for source in sources {
        let source = read_collection_schema(db, &source?)?;
        let result = computed::validate_rollups(&source, |target| {
            if target == next.id {
                Ok(Some(next.clone()))
            } else if db.query_row(
                "SELECT EXISTS(SELECT 1 FROM deleted_collections WHERE id = ?1)",
                [target],
                |row| row.get::<_, bool>(0),
            )? {
                Ok(None)
            } else {
                Ok(Some(read_collection_schema(db, target)?))
            }
        });
        if let Err(error) = result {
            if error.code == "ROLLUP_SCHEMA" {
                return Err(Error::new(
                    "PROPERTY_CONVERSION_ROLLUP",
                    "仍有汇总属性使用这个数值字段。请先在该数据库中调整汇总目标。",
                ));
            }
            return Err(error);
        }
    }
    Ok(())
}

fn retain_historical_definition(
    tx: &Transaction<'_>,
    collection: &str,
    field: &PropertyDefinition,
) -> Result<()> {
    // Add the former definition once while keeping the original revision and value.
    tx.execute(
        "UPDATE revisions AS h SET snapshot = json_insert(
            json_insert(h.snapshot, '$.historicalProperties', json('[]')),
            '$.historicalProperties[#]', json(?3))
         WHERE h.page_id IN (SELECT page_id FROM collection_records WHERE collection_id = ?1)
           AND json_type(h.snapshot, ?2) IS NOT NULL
           AND NOT EXISTS (SELECT 1 FROM json_each(h.snapshot, '$.historicalProperties') old
                           WHERE json_extract(old.value, '$.id') = ?4)",
        params![
            collection,
            property_path(&field.id, "$.properties"),
            serde_json::to_string(field)?,
            field.id
        ],
    )?;
    Ok(())
}

fn property_path(id: &str, base: &str) -> String {
    format!("{base}.\"{id}\"")
}

fn replace_value(values: &mut Values, field: &str, value: Option<Value>) {
    if let Some(value) = value {
        values.insert(field.to_owned(), value);
    } else {
        values.remove(field);
    }
}

pub(crate) fn restore_historical_properties(
    definitions: &[PropertyDefinition],
    values: &mut Values,
    historical: &[PropertyDefinition],
) -> Result<()> {
    for source in historical {
        let target = definition(definitions, &source.id).ok_or_else(invalid_value)?;
        let prior = values.get(&source.id).ok_or_else(invalid_value)?;
        let converted = convert_value(source, target, prior);
        replace_value(values, &source.id, converted);
    }
    Ok(())
}

fn convert_value(
    source: &PropertyDefinition,
    target: &PropertyDefinition,
    value: &Value,
) -> Option<Value> {
    let converted = if source.kind.has_options() && target.kind.has_options() {
        convert_options(source, target, value)?
    } else if source.kind == target.kind {
        value.clone()
    } else if target.kind == PropertyKind::Attachment {
        return None;
    } else {
        let input = match source.kind {
            PropertyKind::Attachment => return None,
            PropertyKind::Status | PropertyKind::Select => option_label(source, value)?.to_owned(),
            PropertyKind::MultiSelect => {
                let labels = value
                    .as_array()?
                    .iter()
                    .map(|id| option_label(source, id))
                    .collect::<Option<Vec<_>>>()?;
                match labels.as_slice() {
                    [label] => (*label).into(),
                    _ if target.kind == PropertyKind::Text => {
                        serde_json::to_string(&labels).ok()?
                    }
                    _ => return None,
                }
            }
            _ => value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string()),
        };
        let parsed = parse_property_text(&input, target)?;
        if target.kind == PropertyKind::Number && decimal(&input) != decimal(&parsed.to_string()) {
            return None;
        }
        parsed
    };
    let supplied = Values::from([(target.id.clone(), converted)]);
    normalize_values(std::slice::from_ref(target), supplied, |_| {
        Ok(source.kind == PropertyKind::Attachment)
    })
    .ok()?
    .remove(&target.id)
}

fn decimal(input: &str) -> Option<(bool, String, i64)> {
    let input = input.trim();
    let (base, exponent) = match input.split_once(['e', 'E']) {
        Some((base, exponent)) => (base, i64::from(exponent.parse::<i32>().ok()?)),
        None => (input, 0),
    };
    let negative = base.starts_with('-');
    let base = base.trim_start_matches(['-', '+']);
    let (whole, fraction) = base.split_once('.').unwrap_or((base, ""));
    if (whole.is_empty() && fraction.is_empty())
        || !whole
            .bytes()
            .chain(fraction.bytes())
            .all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let combined = format!("{whole}{fraction}");
    let digits = combined.trim_matches('0');
    if digits.is_empty() {
        return Some((false, String::new(), 0));
    }
    let trailing = combined.len() - combined.trim_end_matches('0').len();
    Some((
        negative,
        digits.into(),
        exponent - fraction.len() as i64 + trailing as i64,
    ))
}

fn option_label<'a>(field: &'a PropertyDefinition, value: &Value) -> Option<&'a str> {
    let id = value.as_str()?;
    field
        .options
        .iter()
        .find(|option| option.id == id)
        .map(|option| option.name.as_str())
}

fn convert_options(
    source: &PropertyDefinition,
    target: &PropertyDefinition,
    value: &Value,
) -> Option<Value> {
    let input = if source.kind == PropertyKind::MultiSelect {
        value.as_array()?.as_slice()
    } else {
        std::slice::from_ref(value)
    };
    let options = input
        .iter()
        .map(|item| {
            let id = item.as_str()?;
            let label = option_label(source, item)?;
            target
                .options
                .iter()
                .find(|option| option.id == id)
                .or_else(|| {
                    target
                        .options
                        .iter()
                        .find(|option| option.name.to_lowercase() == label.to_lowercase())
                })
                .map(|option| Value::String(option.id.clone()))
        })
        .collect::<Option<Vec<_>>>()?;
    if target.kind == PropertyKind::MultiSelect {
        Some(Value::Array(options))
    } else if options.len() == 1 {
        options.into_iter().next()
    } else {
        None
    }
}

fn invalid_value() -> Error {
    Error::new("PROPERTY_VALUE", "保存的记录属性不完整，转换未提交。")
}
