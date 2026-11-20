// SPDX-License-Identifier: AGPL-3.0-or-later

use super::Calculation;
use super::relations::RelationDefinition;
use crate::model::{name, new_id, valid_id};
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};
use std::collections::{BTreeMap, HashSet};

pub(crate) const MAX_PROPERTIES: usize = 48;
const MAX_OPTIONS: usize = 64;
pub(crate) const TITLE_FIELD: &str = "title";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PropertyKind {
    Text,
    Number,
    Checkbox,
    Date,
    DateTime,
    Status,
    Select,
    MultiSelect,
    Url,
    Attachment,
    Relation,
    Rollup,
    Formula,
    CreatedAt,
    UpdatedAt,
}

impl PropertyKind {
    pub(super) fn convertible(self) -> bool {
        !matches!(
            self,
            Self::CreatedAt | Self::UpdatedAt | Self::Relation | Self::Rollup | Self::Formula
        )
    }

    pub(crate) fn computed(self) -> bool {
        matches!(self, Self::Rollup | Self::Formula)
    }

    pub(crate) fn has_options(self) -> bool {
        matches!(self, Self::Status | Self::Select | Self::MultiSelect)
    }

    fn groupable(self) -> bool {
        matches!(self, Self::Status | Self::Select | Self::Checkbox)
    }

    fn sortable(self) -> bool {
        !self.computed() && !matches!(self, Self::MultiSelect | Self::Attachment | Self::Relation)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertyDefinition {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: PropertyKind,
    pub options: Vec<PropertyOption>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relation: Option<RelationDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calculation: Option<Calculation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyOption {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PropertyDraft {
    pub id: Option<String>,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: PropertyKind,
    #[serde(default)]
    pub options: Vec<PropertyOptionDraft>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyOptionDraft {
    pub id: Option<String>,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewKind {
    Table,
    Board,
    List,
    Calendar,
    Gallery,
    Timeline,
}

impl ViewKind {
    pub(super) const ALL: [Self; 6] = [
        Self::Table,
        Self::Board,
        Self::List,
        Self::Calendar,
        Self::Gallery,
        Self::Timeline,
    ];

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Table => "表格",
            Self::Board => "看板",
            Self::List => "列表",
            Self::Calendar => "日历",
            Self::Gallery => "画廊",
            Self::Timeline => "时间线",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollectionView {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ViewKind,
    pub revision: u32,
    pub settings: ViewSettings,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<super::views::ViewLayout>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSettings {
    pub filter: Option<ViewFilter>,
    pub sort: Option<ViewSort>,
    pub group_by: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewFilter {
    pub field_id: String,
    pub operator: FilterOperator,
    pub value: Option<Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FilterOperator {
    Equal,
    Contains,
    Empty,
    NotEmpty,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewSort {
    pub field_id: String,
    pub direction: SortDirection,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortDirection {
    Asc,
    Desc,
}

pub(super) type Values = BTreeMap<String, Value>;

pub(super) fn build_property(
    draft: PropertyDraft,
    previous: Option<&PropertyDefinition>,
    converting: bool,
) -> Result<PropertyDefinition> {
    if !converting && previous.is_some_and(|property| property.kind != draft.kind) {
        return Err(Error::new("PROPERTY_TYPE", "已创建属性的类型暂不能更改。"));
    }
    if draft.options.len() > MAX_OPTIONS {
        return Err(Error::new(
            "PROPERTY_OPTIONS",
            "每个属性最多支持 64 个选项。",
        ));
    }
    let mut options = Vec::new();
    for option in draft.options {
        let id = match option.id {
            Some(id) if previous.is_some_and(|property| has_option(property, &id)) => id,
            None => new_id(),
            Some(_) => return Err(Error::new("PROPERTY_OPTIONS", "找不到要更新的选项。")),
        };
        options.push(PropertyOption {
            id,
            name: name(&option.name, 64, None)?,
        });
    }
    let property = PropertyDefinition {
        id: previous.map_or_else(new_id, |old| old.id.clone()),
        name: name(&draft.name, 64, None)?,
        kind: draft.kind,
        options,
        relation: previous.and_then(|old| old.relation.clone()),
        calculation: previous.and_then(|old| old.calculation.clone()),
    };
    if !converting
        && previous.is_some_and(|old| {
            old.options
                .iter()
                .any(|option| !has_option(&property, &option.id))
        })
    {
        return Err(Error::new("PROPERTY_OPTIONS", "已有选项可以改名。"));
    }
    Ok(property)
}

pub(super) fn validate_schema(properties: &[PropertyDefinition]) -> Result<()> {
    if properties.len() > MAX_PROPERTIES {
        return Err(Error::new(
            "PROPERTY_LIMIT",
            "每个数据库最多支持 48 个自定义属性。",
        ));
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::from([String::from("标题")]);
    for property in properties {
        if !valid_id(&property.id)
            || !ids.insert(&property.id)
            || name(&property.name, 64, None)? != property.name
            || !names.insert(property.name.to_lowercase())
        {
            return Err(Error::new("PROPERTY_SCHEMA", "属性名称或标识无效或重复。"));
        }
        if property.kind.has_options() == property.options.is_empty()
            || property.options.len() > MAX_OPTIONS
        {
            return Err(Error::new(
                "PROPERTY_OPTIONS",
                "选择类属性需要 1 至 64 个选项。",
            ));
        }
        if (property.kind == PropertyKind::Relation) != property.relation.is_some()
            || property.relation.as_ref().is_some_and(|relation| {
                !valid_id(&relation.collection_id)
                    || relation
                        .inverse_property_id
                        .as_deref()
                        .is_some_and(|id| !valid_id(id) || id == property.id)
            })
        {
            return Err(Error::new(
                "RELATION_SCHEMA",
                "关联属性需要有效的目标数据库和反向属性。",
            ));
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        for option in &property.options {
            if !valid_id(&option.id)
                || !ids.insert(&option.id)
                || name(&option.name, 64, None)? != option.name
                || !names.insert(option.name.to_lowercase())
            {
                return Err(Error::new("PROPERTY_OPTIONS", "选项名称或标识无效或重复。"));
            }
        }
    }
    super::computed::validate_calculations(properties)
}

pub(super) fn validate_views(
    views: &[CollectionView],
    properties: &[PropertyDefinition],
) -> Result<()> {
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    if !(1..=24).contains(&views.len()) {
        return Err(Error::new("VIEW_CONFIG", "数据库需要保留 1 至 24 个视图。"));
    }
    for view in views {
        if !valid_id(&view.id) || !ids.insert(&view.id) || view.revision == 0 {
            return Err(Error::new("VIEW_CONFIG", "数据库视图标识或版本无效。"));
        }
        let label = view.name.as_deref().unwrap_or_else(|| view.kind.label());
        if name(label, 48, None)? != label || !names.insert(label.to_lowercase()) {
            return Err(Error::new("VIEW_CONFIG", "视图名称无效或已被使用。"));
        }
        validate_settings(&view.settings, properties)?;
        super::views::validate_layout(view, properties)?;
    }
    Ok(())
}

pub(crate) fn validate_settings(
    settings: &ViewSettings,
    properties: &[PropertyDefinition],
) -> Result<()> {
    if let Some(filter) = &settings.filter {
        let kind = field_kind(properties, &filter.field_id)?;
        if kind == PropertyKind::Relation || kind.computed() {
            return Err(Error::new("VIEW_FILTER", "当前视图筛选使用普通属性。"));
        }
        let property = definition(properties, &filter.field_id);
        let valid = match (filter.operator, filter.value.as_ref()) {
            (FilterOperator::Empty | FilterOperator::NotEmpty, None) => true,
            (FilterOperator::Contains, Some(value)) if kind == PropertyKind::MultiSelect => value
                .as_str()
                .is_some_and(|id| property.is_some_and(|item| has_option(item, id))),
            (FilterOperator::Contains, Some(value))
                if matches!(kind, PropertyKind::Text | PropertyKind::Url) =>
            {
                value.as_str().is_some_and(|text| {
                    !text.trim().is_empty() && text.chars().count() <= 256 && !text.contains('\0')
                })
            }
            (FilterOperator::Equal, Some(value))
                if matches!(kind, PropertyKind::CreatedAt | PropertyKind::UpdatedAt) =>
            {
                value.as_str().is_some_and(valid_date)
            }
            (FilterOperator::Equal, Some(value)) if field_kind_without_definition(kind) => {
                valid_scalar(kind, value)
            }
            (FilterOperator::Equal, Some(value))
                if matches!(kind, PropertyKind::Status | PropertyKind::Select) =>
            {
                value
                    .as_str()
                    .is_some_and(|id| property.is_some_and(|item| has_option(item, id)))
            }
            _ => false,
        };
        if !valid {
            return Err(Error::new("VIEW_FILTER", "筛选条件与属性类型不匹配。"));
        }
    }
    if let Some(sort) = &settings.sort
        && !field_kind(properties, &sort.field_id)?.sortable()
    {
        return Err(Error::new("VIEW_SORT", "该属性类型暂不能排序。"));
    }
    if let Some(field) = &settings.group_by
        && !field_kind(properties, field)?.groupable()
    {
        return Err(Error::new("VIEW_GROUP", "分组需要状态、单选或复选框属性。"));
    }
    Ok(())
}

fn field_kind_without_definition(kind: PropertyKind) -> bool {
    matches!(
        kind,
        PropertyKind::Text
            | PropertyKind::Number
            | PropertyKind::Checkbox
            | PropertyKind::Date
            | PropertyKind::DateTime
            | PropertyKind::Url
    )
}

pub(super) fn definition<'a>(
    properties: &'a [PropertyDefinition],
    id: &str,
) -> Option<&'a PropertyDefinition> {
    properties.iter().find(|property| property.id == id)
}

pub(crate) fn field_kind(properties: &[PropertyDefinition], id: &str) -> Result<PropertyKind> {
    if id == TITLE_FIELD {
        return Ok(PropertyKind::Text);
    }
    definition(properties, id)
        .map(|item| item.kind)
        .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "找不到这个属性。"))
}

fn has_option(property: &PropertyDefinition, id: &str) -> bool {
    property.options.iter().any(|option| option.id == id)
}

pub(crate) fn parse_property_text(raw: &str, property: &PropertyDefinition) -> Option<Value> {
    let trimmed = raw.trim();
    let option = |input: &str| {
        property
            .options
            .iter()
            .find(|option| option.name.to_lowercase() == input.to_lowercase())
            .map(|option| Value::String(option.id.clone()))
    };
    match property.kind {
        PropertyKind::Text => Some(Value::String(raw.into())),
        PropertyKind::Date | PropertyKind::DateTime | PropertyKind::Url => {
            Some(Value::String(trimmed.into()))
        }
        PropertyKind::Number => trimmed
            .parse::<i64>()
            .ok()
            .map(Number::from)
            .or_else(|| trimmed.parse::<f64>().ok().and_then(Number::from_f64))
            .map(Value::Number),
        PropertyKind::Checkbox => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "是" => Some(Value::Bool(true)),
            "false" | "0" | "no" | "否" => Some(Value::Bool(false)),
            _ => None,
        },
        PropertyKind::Status | PropertyKind::Select => option(trimmed),
        PropertyKind::MultiSelect => {
            if let Ok(labels) = serde_json::from_str::<Vec<String>>(trimmed) {
                return labels
                    .iter()
                    .map(|label| option(label))
                    .collect::<Option<Vec<_>>>()
                    .map(Value::Array);
            }
            if let Some(value) = option(trimmed) {
                return Some(Value::Array(vec![value]));
            }
            trimmed
                .split(';')
                .map(|label| option(label.trim()))
                .collect::<Option<Vec<_>>>()
                .map(Value::Array)
        }
        PropertyKind::Attachment
        | PropertyKind::Relation
        | PropertyKind::Formula
        | PropertyKind::Rollup
        | PropertyKind::CreatedAt
        | PropertyKind::UpdatedAt => None,
    }
}

pub(crate) fn normalize_values(
    definitions: &[PropertyDefinition],
    values: Values,
    mut owns_attachment: impl FnMut(&str) -> Result<bool>,
) -> Result<Values> {
    let mut result = Values::new();
    for (id, value) in values {
        let property = definition(definitions, &id)
            .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "记录包含未定义的属性。"))?;
        let valid = match property.kind {
            PropertyKind::CreatedAt
            | PropertyKind::UpdatedAt
            | PropertyKind::Rollup
            | PropertyKind::Formula => false,
            _ if value.is_null() => continue,
            PropertyKind::Relation => {
                let ids = super::relations::members(Some(&value))?;
                if !ids.is_empty() {
                    result.insert(id, serde_json::to_value(ids)?);
                }
                continue;
            }
            PropertyKind::Status | PropertyKind::Select => {
                value.as_str().is_some_and(|id| has_option(property, id))
            }
            PropertyKind::MultiSelect | PropertyKind::Attachment => {
                let Some(items) = value.as_array() else {
                    return Err(property_error(property));
                };
                if items.is_empty() {
                    continue;
                }
                if items.len() > MAX_OPTIONS {
                    return Err(property_error(property));
                }
                let mut seen = HashSet::new();
                for item in items {
                    let Some(id) = item.as_str().filter(|id| valid_id(id) && seen.insert(*id))
                    else {
                        return Err(property_error(property));
                    };
                    let belongs = if property.kind == PropertyKind::Attachment {
                        owns_attachment(id)?
                    } else {
                        has_option(property, id)
                    };
                    if !belongs {
                        return Err(property_error(property));
                    }
                }
                true
            }
            PropertyKind::Text
            | PropertyKind::Date
            | PropertyKind::DateTime
            | PropertyKind::Url
                if value.as_str().is_some_and(|text| text.trim().is_empty()) =>
            {
                continue;
            }
            kind => valid_scalar(kind, &value),
        };
        if !valid {
            return Err(property_error(property));
        }
        result.insert(id, value);
    }
    Ok(result)
}

fn property_error(property: &PropertyDefinition) -> Error {
    Error::new("PROPERTY_VALUE", format!("「{}」的值无效。", property.name))
}

fn valid_scalar(kind: PropertyKind, value: &Value) -> bool {
    match kind {
        PropertyKind::Text => value.as_str().is_some_and(|text| {
            !text.trim().is_empty() && text.chars().count() <= 2_000 && !text.contains('\0')
        }),
        PropertyKind::Number => value
            .as_f64()
            .is_some_and(|number| number.abs() <= 9_007_199_254_740_991.0),
        PropertyKind::Checkbox => value.is_boolean(),
        PropertyKind::Date => value.as_str().is_some_and(valid_date),
        PropertyKind::DateTime => value
            .as_str()
            .is_some_and(|text| super::views::date_time(text).is_some()),
        PropertyKind::Url => value.as_str().is_some_and(|text| {
            text.len() <= 2_048
                && text == text.trim()
                && !text.chars().any(char::is_whitespace)
                && url::Url::parse(text).is_ok_and(|url| {
                    matches!(url.scheme(), "http" | "https") && url.host().is_some()
                })
        }),
        _ => false,
    }
}

pub(super) fn valid_date(text: &str) -> bool {
    if text.len() != 10 {
        return false;
    }
    let parts: Vec<_> = text.split('-').collect();
    let [year, month, day] = parts.as_slice() else {
        return false;
    };
    if [year.len(), month.len(), day.len()] != [4, 2, 2]
        || !parts
            .iter()
            .all(|part| part.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return false;
    }
    let (year, month, day) = (
        year.parse::<u32>().unwrap(),
        month.parse::<u32>().unwrap(),
        day.parse::<u32>().unwrap(),
    );
    let days = match month {
        4 | 6 | 9 | 11 => 30,
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => {
            29
        }
        2 => 28,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    year > 0 && (1..=days).contains(&day)
}
