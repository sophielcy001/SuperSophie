// SPDX-License-Identifier: AGPL-3.0-or-later

use super::computed::{Engine, cache};
use super::schema::{TITLE_FIELD, definition, field_kind};
use super::{
    Collection, FilterOperator, PropertyKind, Record, RecordPage, SortDirection, ViewFilter,
    ViewKind, ViewRange, ViewSort, json_column, read_collection, view_missing,
};
use crate::{Error, Result, Store};
use rusqlite::{OptionalExtension, Row, params, params_from_iter, types::Value as SqlValue};
use serde_json::Value;

pub(crate) const RECORD_COLUMNS: &str =
    "p.id, p.title, p.revision, r.created_at, p.updated_at, r.properties";
const ACTIVE_RECORDS: &str = "FROM collection_records r JOIN pages p ON p.id = r.page_id
     WHERE r.collection_id = ?1 AND p.trash_root_id IS NULL";
const PAGE_SIZE: u32 = 50;

impl Store {
    pub fn record(&self, workspace: &str, collection: &str, id: &str) -> Result<Record> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let schema = read_collection(&tx, collection)?;
        let mut record = tx
            .query_row(
                &format!("SELECT {RECORD_COLUMNS} {ACTIVE_RECORDS} AND p.id = ?2"),
                params![collection, id],
                record_row,
            )
            .optional()?
            .ok_or_else(|| Error::new("RECORD_NOT_FOUND", "找不到这个数据库记录。"))?;
        record.relations =
            super::relations::project_values(&tx, &schema.properties, &mut record.properties)?;
        let calculation = if schema.properties.iter().any(|field| field.kind.computed()) {
            Some(Engine::new(&tx, None).cached(id)?)
        } else {
            None
        };
        tx.commit()?;
        if let Some(reading) = calculation {
            record.computed_pending = reading.pending;
            if !reading.pending {
                if let Some(sequence) = reading.write_at {
                    let _ = cache::save(&db, None, id, sequence, &reading.snapshot);
                }
                record.computed = reading.snapshot.values;
            }
        }
        Ok(record)
    }

    pub fn query_records(
        &self,
        workspace: &str,
        collection: &str,
        view: &str,
        offset: u32,
    ) -> Result<RecordPage> {
        self.query_view_records(workspace, collection, view, offset, None)
    }

    pub fn query_view_records(
        &self,
        workspace: &str,
        collection: &str,
        view: &str,
        offset: u32,
        range: Option<&ViewRange>,
    ) -> Result<RecordPage> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let collection = read_collection(&tx, collection)?;
        let view = collection
            .views
            .iter()
            .find(|item| item.id == view)
            .ok_or_else(view_missing)?;
        let (mut criteria, mut arguments) =
            record_filter(&collection, view.settings.filter.as_ref())?;
        if let Some(range) = range {
            criteria.push_str(" AND ");
            criteria.push_str(&schedule_clause(&collection, view, range, &mut arguments)?);
        }
        let total: u32 = tx.query_row(
            &format!("SELECT count(*) {criteria}"),
            params_from_iter(&arguments.0),
            |row| row.get(0),
        )?;
        let last_page = total.saturating_sub(1) / PAGE_SIZE;
        let offset = (offset / PAGE_SIZE).min(last_page) * PAGE_SIZE;
        let sorted = record_order(&collection, view.settings.sort.as_ref(), &mut arguments)?;
        let limit = arguments.bind(i64::from(PAGE_SIZE));
        let start = arguments.bind(i64::from(offset));
        let mut records: Vec<Record> = {
            let mut query = tx.prepare(&format!(
                "SELECT {RECORD_COLUMNS} {criteria} ORDER BY {sorted} LIMIT {limit} OFFSET {start}"
            ))?;
            query
                .query_map(params_from_iter(&arguments.0), record_row)?
                .collect::<rusqlite::Result<_>>()?
        };
        let computed = collection
            .properties
            .iter()
            .any(|field| field.kind.computed());
        let mut calculations = Engine::new(&tx, None);
        for record in &mut records {
            record.relations = super::relations::project_values(
                &tx,
                &collection.properties,
                &mut record.properties,
            )?;
            if computed {
                let reading = calculations.cached(&record.id)?;
                record.computed_pending = reading.pending;
                if !reading.pending {
                    record.computed = reading.snapshot.values;
                }
            }
            if view.kind == ViewKind::Gallery {
                record.cover = super::views::cover(&tx, view, record)?;
            }
        }
        tx.commit()?;
        Ok(RecordPage {
            records,
            total,
            offset,
            page_size: PAGE_SIZE,
        })
    }
}

pub(super) fn record_filter(
    collection: &Collection,
    filter: Option<&ViewFilter>,
) -> Result<(String, Arguments)> {
    let mut arguments = Arguments(vec![collection.id.clone().into()]);
    let mut criteria = ACTIVE_RECORDS.to_owned();
    if let Some(filter) = filter {
        criteria.push_str(" AND ");
        criteria.push_str(&filter_clause(collection, filter, &mut arguments)?);
    }
    Ok((criteria, arguments))
}

pub(crate) fn record_order(
    collection: &Collection,
    sort: Option<&ViewSort>,
    arguments: &mut Arguments,
) -> Result<String> {
    match sort {
        Some(sort) => order_clause(collection, sort, arguments),
        None => Ok("r.created_at DESC, p.id ASC".into()),
    }
}

pub(crate) fn record_row(row: &Row<'_>) -> rusqlite::Result<Record> {
    Ok(Record {
        id: row.get(0)?,
        title: row.get(1)?,
        revision: row.get(2)?,
        created_at: row.get(3)?,
        updated_at: row.get(4)?,
        properties: json_column(row, 5)?,
        relations: Default::default(),
        computed: Default::default(),
        computed_pending: false,
        cover: None,
    })
}

#[derive(Default)]
pub(crate) struct Arguments(pub(crate) Vec<SqlValue>);

impl Arguments {
    pub(crate) fn bind(&mut self, value: impl Into<SqlValue>) -> String {
        self.0.push(value.into());
        format!("?{}", self.0.len())
    }
}

fn field_expression(field: &str, kind: PropertyKind, arguments: &mut Arguments) -> String {
    match kind {
        _ if field == TITLE_FIELD => "p.title".into(),
        PropertyKind::CreatedAt => "r.created_at".into(),
        PropertyKind::UpdatedAt => "p.updated_at".into(),
        _ => {
            let path = arguments.bind(format!("$.\"{field}\""));
            format!("json_extract(r.properties, {path})")
        }
    }
}

fn schedule_clause(
    collection: &Collection,
    view: &super::CollectionView,
    range: &ViewRange,
    arguments: &mut Arguments,
) -> Result<String> {
    if !matches!(view.kind, ViewKind::Calendar | ViewKind::Timeline) {
        return Err(Error::new("VIEW_DATE", "日期范围用于日历和时间线视图。"));
    }
    let Some((start, end)) = super::views::schedule_fields(view, &collection.properties) else {
        return Ok(if matches!(range, ViewRange::Unscheduled) {
            "1"
        } else {
            "0"
        }
        .into());
    };
    let first = field_expression(&start.id, start.kind, arguments);
    let last = end.map(|end| field_expression(&end.id, end.kind, arguments));
    let dated = |field: &str| {
        if start.kind == PropertyKind::DateTime {
            format!("date({field})")
        } else {
            field.into()
        }
    };
    let ordered = match &last {
        Some(last) if start.kind == PropertyKind::DateTime => {
            format!("unixepoch({last}) >= unixepoch({first})")
        }
        Some(last) => format!("{last} >= {first}"),
        None => format!("{first} IS NOT NULL"),
    };
    match range {
        ViewRange::Unscheduled => Ok(format!("NOT COALESCE(({ordered}),0)")),
        ViewRange::Between { from, through } => {
            let length = (super::views::day(through)? - super::views::day(from)?).num_days() + 1;
            if !(1..=92).contains(&length) {
                return Err(Error::new(
                    "VIEW_DATE",
                    "视图的日期范围必须在 1 至 92 天之间。",
                ));
            }
            let from = arguments.bind(from.clone());
            let through = arguments.bind(through.clone());
            let begins = dated(&first);
            let ends = last.as_deref().map(dated).unwrap_or_else(|| begins.clone());
            Ok(format!(
                "({ordered}) AND {begins} <= {through} AND {ends} >= {from}"
            ))
        }
    }
}

pub(crate) fn filter_clause(
    collection: &Collection,
    filter: &ViewFilter,
    arguments: &mut Arguments,
) -> Result<String> {
    let kind = field_kind(&collection.properties, &filter.field_id)?;
    let field = field_expression(&filter.field_id, kind, arguments);
    let clause = match filter.operator {
        FilterOperator::Empty => format!("{field} IS NULL"),
        FilterOperator::NotEmpty => format!("{field} IS NOT NULL"),
        FilterOperator::Equal | FilterOperator::Contains => {
            let value = filter
                .value
                .as_ref()
                .ok_or_else(|| Error::new("VIEW_FILTER", "筛选条件缺少比较值。"))?;
            let value = arguments.bind(sql_value(value)?);
            match (filter.operator, kind) {
                (FilterOperator::Contains, PropertyKind::MultiSelect) => {
                    format!(
                        "EXISTS (SELECT 1 FROM json_each({field}) AS item WHERE item.value = {value})"
                    )
                }
                (FilterOperator::Contains, _) => {
                    format!("instr(lower({field}), lower({value})) > 0")
                }
                (_, PropertyKind::CreatedAt | PropertyKind::UpdatedAt) => {
                    format!("strftime('%Y-%m-%d', {field} / 1000.0, 'unixepoch') = {value}")
                }
                (_, PropertyKind::DateTime) => format!("unixepoch({field}) = unixepoch({value})"),
                _ => format!("{field} = {value}"),
            }
        }
    };
    Ok(clause)
}

fn sql_value(value: &Value) -> Result<SqlValue> {
    match value {
        Value::String(text) => Ok(text.clone().into()),
        Value::Bool(value) => Ok(i64::from(*value).into()),
        Value::Number(number) => {
            if let Some(value) = number.as_i64() {
                return Ok(value.into());
            }
            if let Some(value) = number.as_f64() {
                return Ok(value.into());
            }
            Err(Error::new("VIEW_FILTER", "筛选数字超出支持范围。"))
        }
        _ => Err(Error::new("VIEW_FILTER", "筛选需要单一值。")),
    }
}

fn order_clause(
    collection: &Collection,
    sort: &ViewSort,
    arguments: &mut Arguments,
) -> Result<String> {
    let kind = field_kind(&collection.properties, &sort.field_id)?;
    let mut field = field_expression(&sort.field_id, kind, arguments);
    if kind == PropertyKind::DateTime {
        field = format!("unixepoch({field})");
    }
    if matches!(kind, PropertyKind::Status | PropertyKind::Select) {
        let property = definition(&collection.properties, &sort.field_id)
            .ok_or_else(|| Error::new("PROPERTY_NOT_FOUND", "找不到排序属性。"))?;
        let options = property
            .options
            .iter()
            .enumerate()
            .map(|(position, option)| {
                format!(
                    " WHEN {} THEN {position}",
                    arguments.bind(option.id.clone())
                )
            })
            .collect::<String>();
        field = format!("CASE {field}{options} END");
    }
    let direction = match sort.direction {
        SortDirection::Asc => "ASC",
        SortDirection::Desc => "DESC",
    };
    Ok(format!(
        "({field}) IS NULL ASC, ({field}) {direction}, p.id ASC"
    ))
}
