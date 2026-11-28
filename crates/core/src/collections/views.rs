// SPDX-License-Identifier: AGPL-3.0-or-later

use super::schema::{definition, valid_date, validate_views};
use super::{
    Collection, CollectionView, PropertyDefinition, PropertyKind, Record, ViewKind, ViewSettings,
    advance_revision, json_column, read_collection, view_missing,
};
use crate::model::{name, new_id, valid_id};
use crate::{Error, Page, Result, Store};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimelineZoom {
    Week,
    #[default]
    Fortnight,
    Month,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ViewLayout {
    Calendar {
        date_field: String,
        /// A second date property of the same type, read as each record's last day.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        end_field: Option<String>,
    },
    Gallery {
        cover_field: Option<String>,
    },
    Timeline {
        start_field: String,
        end_field: String,
        #[serde(default)]
        zoom: TimelineZoom,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ViewDraft {
    pub id: Option<String>,
    pub revision: Option<u32>,
    #[serde(rename = "type")]
    pub kind: ViewKind,
    pub name: String,
    pub settings: ViewSettings,
    pub layout: Option<ViewLayout>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "mode", rename_all = "camelCase", deny_unknown_fields)]
pub enum ViewRange {
    Between { from: String, through: String },
    Unscheduled,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleMove {
    pub view_id: String,
    pub view_revision: u32,
    pub page_id: String,
    pub page_revision: u32,
    pub day: String,
}

/// Which end of a scheduled record a resize moves.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleEdge {
    Start,
    End,
}

/// Moves one end of a record's period to `day`, as dragging an edge of a calendar or timeline bar does.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ScheduleResize {
    pub view_id: String,
    pub view_revision: u32,
    pub page_id: String,
    pub page_revision: u32,
    pub edge: ScheduleEdge,
    pub day: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GalleryCover {
    pub attachment_id: String,
    pub name: String,
    pub media_type: String,
}

pub(crate) fn view_target(block: &crate::Block) -> Option<(&str, &str)> {
    if block.kind != "collectionView"
        || block.schema_version != 1
        || !block.children.is_empty()
        || !block.extra.is_empty()
        || block.payload.as_object()?.len() != 2
    {
        return None;
    }
    let collection = block.payload["collectionId"]
        .as_str()
        .filter(|id| valid_id(id))?;
    let view = block.payload["viewId"].as_str().filter(|id| valid_id(id))?;
    Some((collection, view))
}

impl Store {
    pub fn upsert_view(
        &self,
        workspace: &str,
        collection: &str,
        draft: ViewDraft,
    ) -> Result<Collection> {
        if draft.id.is_some() != draft.revision.is_some() {
            return Err(Error::new(
                "VIEW_CONFIG",
                "修改视图时需要提供标识和修订号。",
            ));
        }
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current = read_collection(&tx, collection)?;
        let view = CollectionView {
            id: draft.id.unwrap_or_else(new_id),
            kind: draft.kind,
            revision: 1,
            name: Some(name(&draft.name, 48, None)?),
            settings: draft.settings,
            layout: draft.layout,
        };
        if let Some(revision) = draft.revision {
            let target = current
                .views
                .iter_mut()
                .find(|item| item.id == view.id)
                .ok_or_else(view_missing)?;
            check_revision(target, revision)?;
            if view.kind != target.kind {
                return Err(Error::new("VIEW_CONFIG", "已保存视图的类型不能修改。"));
            }
            let mut updated = CollectionView { revision, ..view };
            if *target == updated {
                return Ok(current);
            }
            updated.revision = advance_revision(revision)?;
            *target = updated;
        } else {
            current.views.push(view);
        }
        validate_views(&current.views, &current.properties)?;
        write_views(&tx, collection, &current.views)?;
        tx.commit()?;
        Ok(current)
    }

    pub fn delete_view(
        &self,
        workspace: &str,
        collection: &str,
        view: &str,
        revision: u32,
    ) -> Result<Collection> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut current = read_collection(&tx, collection)?;
        let index = current
            .views
            .iter()
            .position(|item| item.id == view)
            .ok_or_else(view_missing)?;
        check_revision(&current.views[index], revision)?;
        if current.views.len() == 1 {
            return Err(Error::new("VIEW_LAST", "请至少保留一个视图。"));
        }
        current.views.remove(index);
        write_views(&tx, collection, &current.views)?;
        tx.commit()?;
        Ok(current)
    }

    pub fn move_scheduled_record(
        &self,
        workspace: &str,
        collection: &str,
        change: ScheduleMove,
    ) -> Result<Page> {
        let target = day(&change.day)?;
        self.edit(
            workspace,
            &change.page_id,
            change.page_revision,
            |tx, page| {
                let collection = read_collection(tx, collection)?;
                if page.info.parent_id.as_deref() != Some(&collection.container_page_id) {
                    return Err(Error::new("RECORD_NOT_FOUND", "这条记录不属于当前数据库。"));
                }
                let view = collection
                    .views
                    .iter()
                    .find(|item| item.id == change.view_id)
                    .ok_or_else(view_missing)?;
                check_revision(view, change.view_revision)?;
                let (start, end) =
                    schedule_fields(view, &collection.properties).ok_or_else(schedule_error)?;
                let values = page.properties.as_mut().ok_or_else(schedule_error)?;
                let original = values
                    .get(&start.id)
                    .and_then(Value::as_str)
                    .ok_or_else(schedule_error)?;
                let days = target.signed_duration_since(projected_day(start.kind, original)?);
                if days.is_zero() {
                    return Ok(());
                }
                let changed = shift(start.kind, original, days)?;
                let finish = end.and_then(|end| Some((end, values.get(&end.id)?.as_str()?)));
                if let Some((end, finish)) = finish {
                    let ordered = if start.kind == PropertyKind::Date {
                        day(original)? <= day(finish)?
                    } else {
                        date_time(original).ok_or_else(schedule_error)?
                            <= date_time(finish).ok_or_else(schedule_error)?
                    };
                    if !ordered {
                        return Err(schedule_error());
                    }
                    let shifted = shift(end.kind, finish, days)?;
                    values.insert(end.id.clone(), Value::from(shifted));
                }
                values.insert(start.id.clone(), Value::from(changed));
                Ok(())
            },
        )
    }

    pub fn resize_scheduled_record(
        &self,
        workspace: &str,
        collection: &str,
        change: ScheduleResize,
    ) -> Result<Page> {
        let target = day(&change.day)?;
        self.edit(
            workspace,
            &change.page_id,
            change.page_revision,
            |tx, page| {
                let collection = read_collection(tx, collection)?;
                if page.info.parent_id.as_deref() != Some(&collection.container_page_id) {
                    return Err(Error::new("RECORD_NOT_FOUND", "这条记录不属于当前数据库。"));
                }
                let view = collection
                    .views
                    .iter()
                    .find(|item| item.id == change.view_id)
                    .ok_or_else(view_missing)?;
                check_revision(view, change.view_revision)?;
                let (start, end) =
                    schedule_fields(view, &collection.properties).ok_or_else(schedule_error)?;
                let end =
                    end.ok_or_else(|| Error::new("VIEW_DATE", "请先为此视图选择结束日期属性。"))?;
                let values = page.properties.as_mut().ok_or_else(schedule_error)?;
                let original = values
                    .get(&start.id)
                    .and_then(Value::as_str)
                    .ok_or_else(schedule_error)?
                    .to_owned();
                let finish = values
                    .get(&end.id)
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| original.clone());
                let first = projected_day(start.kind, &original)?;
                let last = projected_day(end.kind, &finish)?.max(first);
                let (first, last) = match change.edge {
                    ScheduleEdge::Start => (target.min(last), last),
                    ScheduleEdge::End => (first, target.max(first)),
                };
                let moved_start = shift(
                    start.kind,
                    &original,
                    first.signed_duration_since(projected_day(start.kind, &original)?),
                )?;
                let moved_end = shift(
                    end.kind,
                    &finish,
                    last.signed_duration_since(projected_day(end.kind, &finish)?),
                )?;
                values.insert(start.id.clone(), Value::from(moved_start));
                if first == last && end.kind == PropertyKind::Date {
                    values.remove(&end.id);
                } else {
                    values.insert(end.id.clone(), Value::from(moved_end));
                }
                Ok(())
            },
        )
    }
}

pub(super) fn check_revision(view: &CollectionView, revision: u32) -> Result<()> {
    if view.revision != revision {
        return Err(Error::new(
            "VIEW_CONFLICT",
            "视图设置已有更新，请重新打开此视图。",
        ));
    }
    Ok(())
}

pub(super) fn default_view(kind: ViewKind) -> CollectionView {
    CollectionView {
        id: new_id(),
        kind,
        name: Some(kind.label().to_owned()),
        revision: 1,
        settings: ViewSettings::default(),
        layout: None,
    }
}

pub(super) fn expand_legacy(views: &mut Vec<CollectionView>) -> bool {
    let originals = [ViewKind::Table, ViewKind::Board, ViewKind::List];
    if views.len() != originals.len()
        || views
            .iter()
            .any(|view| view.name.is_some() || view.layout.is_some())
        || !originals
            .iter()
            .all(|kind| views.iter().any(|view| &view.kind == kind))
    {
        return false;
    }
    for view in views.iter_mut() {
        view.name = Some(view.kind.label().into());
    }
    views.extend([ViewKind::Calendar, ViewKind::Gallery, ViewKind::Timeline].map(default_view));
    true
}

pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<()> {
    let mut query = tx.prepare("SELECT id, views FROM collections")?;
    let collections: Vec<(String, Vec<CollectionView>)> = query
        .query_map([], |row| Ok((row.get(0)?, json_column(row, 1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    for (id, mut views) in collections {
        if expand_legacy(&mut views) {
            write_views(tx, &id, &views)?;
        }
    }
    Ok(())
}

pub(super) fn write_views(
    tx: &Transaction<'_>,
    collection: &str,
    views: &[CollectionView],
) -> Result<()> {
    tx.execute(
        "UPDATE collections SET views=?2 WHERE id=?1",
        (collection, serde_json::to_string(views)?),
    )?;
    Ok(())
}

pub(super) fn validate_layout(view: &CollectionView, fields: &[PropertyDefinition]) -> Result<()> {
    let valid = match (view.kind, &view.layout) {
        (_, None) => true,
        (
            ViewKind::Calendar,
            Some(ViewLayout::Calendar {
                date_field,
                end_field,
            }),
        ) => {
            let start = definition(fields, date_field);
            is_date(start)
                && end_field.as_ref().is_none_or(|id| {
                    id != date_field
                        && definition(fields, id)
                            .zip(start)
                            .is_some_and(|(end, start)| end.kind == start.kind)
                })
        }
        (
            ViewKind::Timeline,
            Some(ViewLayout::Timeline {
                start_field,
                end_field,
                ..
            }),
        ) => {
            let (start, end) = (
                definition(fields, start_field),
                definition(fields, end_field),
            );
            start_field != end_field
                && is_date(start)
                && start
                    .zip(end)
                    .is_some_and(|(start, end)| start.kind == end.kind)
        }
        (ViewKind::Gallery, Some(ViewLayout::Gallery { cover_field })) => {
            cover_field.as_ref().is_none_or(|id| {
                definition(fields, id).is_some_and(|field| field.kind == PropertyKind::Attachment)
            })
        }
        _ => false,
    };
    if !valid {
        return Err(Error::new(
            "VIEW_LAYOUT",
            "请选择与视图匹配的日期或封面属性。时间线的起止属性须为相同日期类型。",
        ));
    }
    Ok(())
}

pub(super) fn schedule_fields<'a>(
    view: &CollectionView,
    fields: &'a [PropertyDefinition],
) -> Option<(&'a PropertyDefinition, Option<&'a PropertyDefinition>)> {
    match (view.kind, &view.layout) {
        (
            ViewKind::Calendar,
            Some(ViewLayout::Calendar {
                date_field,
                end_field,
            }),
        ) => Some((
            definition(fields, date_field)?,
            match end_field {
                Some(id) => Some(definition(fields, id)?),
                None => None,
            },
        )),
        (ViewKind::Calendar, None) => {
            Some((fields.iter().find(|field| is_date(Some(field)))?, None))
        }
        (
            ViewKind::Timeline,
            Some(ViewLayout::Timeline {
                start_field,
                end_field,
                ..
            }),
        ) => Some((
            definition(fields, start_field)?,
            Some(definition(fields, end_field)?),
        )),
        (ViewKind::Timeline, None) => {
            for (index, start) in fields
                .iter()
                .enumerate()
                .filter(|(_, field)| is_date(Some(field)))
            {
                if let Some(end) = fields[index + 1..]
                    .iter()
                    .find(|field| field.kind == start.kind)
                {
                    return Some((start, Some(end)));
                }
            }
            None
        }
        _ => None,
    }
}

fn is_date(field: Option<&PropertyDefinition>) -> bool {
    field.is_some_and(|field| matches!(field.kind, PropertyKind::Date | PropertyKind::DateTime))
}

pub(super) fn day(value: &str) -> Result<NaiveDate> {
    if !valid_date(value) {
        return Err(schedule_error());
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| schedule_error())
}

pub(super) fn date_time(text: &str) -> Option<DateTime<FixedOffset>> {
    let (date, time) = text.split_once('T')?;
    if !valid_date(date)
        || !text.is_ascii()
        || !(time.len() == 9 && time.ends_with('Z')
            || time.len() == 14
                && matches!(time.as_bytes()[8], b'+' | b'-')
                && time.as_bytes()[11] == b':')
        || time.as_bytes()[2] != b':'
        || time.as_bytes()[5] != b':'
    {
        return None;
    }
    let value = DateTime::parse_from_rfc3339(text).ok()?;
    (value.timestamp_subsec_nanos() == 0
        && value.offset().local_minus_utc().abs() <= 14 * 3_600
        && (1..=9999).contains(&value.naive_utc().year()))
    .then_some(value)
}

fn projected_day(kind: PropertyKind, value: &str) -> Result<NaiveDate> {
    if kind == PropertyKind::DateTime {
        Ok(date_time(value)
            .ok_or_else(schedule_error)?
            .naive_utc()
            .date())
    } else {
        day(value)
    }
}

fn shift(kind: PropertyKind, value: &str, days: chrono::TimeDelta) -> Result<String> {
    let next = if kind == PropertyKind::DateTime {
        date_time(value)
            .and_then(|time| time.date_naive().checked_add_signed(days))
            .map(|date| format!("{date}{}", &value[10..]))
    } else {
        day(value)?
            .checked_add_signed(days)
            .map(|date| date.to_string())
    }
    .ok_or_else(schedule_error)?;
    projected_day(kind, &next)?;
    Ok(next)
}

fn schedule_error() -> Error {
    Error::new("VIEW_DATE", "请选择有效日期，并核对记录的排期属性。")
}

pub(super) fn cover(
    db: &Connection,
    view: &CollectionView,
    record: &Record,
) -> Result<Option<GalleryCover>> {
    let field = match &view.layout {
        Some(ViewLayout::Gallery { cover_field }) => cover_field.as_deref(),
        _ => None,
    };
    let allowed = match field {
        Some(id) => match record.properties.get(id) {
            Some(list) => serde_json::to_string(list)?,
            None => return Ok(None),
        },
        None => String::new(),
    };
    let mut query = db.prepare_cached(
        "SELECT a.id, a.name, a.media_type
         FROM attachments a JOIN pages p ON p.id=a.page_id
         WHERE a.page_id=?1 AND a.bytes<=?2 AND a.media_type IN ('image/png','image/jpeg','image/gif','image/webp','image/bmp')
           AND (?3 IS NULL OR a.id IN (SELECT value FROM json_each(?3)))
         ORDER BY CASE WHEN ?3 IS NULL AND a.id=json_extract(p.cover, '$.attachmentId') THEN 0 ELSE 1 END,
                  a.created_at, a.id LIMIT 1",
    )?;
    Ok(query
        .query_row(
            params![
                record.id,
                crate::MAX_PREVIEW_BYTES as i64,
                field.map(|_| &allowed),
            ],
            |row| {
                Ok(GalleryCover {
                    attachment_id: row.get(0)?,
                    name: row.get(1)?,
                    media_type: row.get(2)?,
                })
            },
        )
        .optional()?)
}
