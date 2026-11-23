// SPDX-License-Identifier: AGPL-3.0-or-later

use super::query::{RECORD_COLUMNS, record_filter, record_order, record_row};
use super::{PropertyDefinition, PropertyKind, Record, ViewKind, read_collection, view_missing};
use crate::assets::read_attachment;
use crate::{Attachment, Error, Result, Store};
use csv::{Terminator, WriterBuilder};
use rusqlite::{Connection, params_from_iter};
use serde_json::Value;
use std::{collections::HashMap, io::Write, iter};

#[derive(Debug)]
pub struct CsvExportReport {
    pub rows: usize,
    pub view: Option<ViewKind>,
    pub view_name: Option<String>,
    pub limitations: Vec<String>,
    pub attachments: Vec<(String, String)>,
}

impl Store {
    /// Streams the selected records from a single read snapshot. The caller
    /// stages the CSV and copies referenced attachments before publishing it.
    pub fn write_collection_csv(
        &self,
        workspace: &str,
        collection: &str,
        view_id: Option<&str>,
        mut output: impl Write,
        mut copy_attachment: impl FnMut(&Attachment) -> Result<String>,
    ) -> Result<CsvExportReport> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let collection = read_collection(&tx, collection)?;
        let view = view_id
            .map(|id| {
                collection
                    .views
                    .iter()
                    .find(|item| item.id == id)
                    .ok_or_else(view_missing)
            })
            .transpose()?;
        let (criteria, mut arguments) = record_filter(
            &collection,
            view.and_then(|view| view.settings.filter.as_ref()),
        )?;
        let order = record_order(
            &collection,
            view.and_then(|view| view.settings.sort.as_ref()),
            &mut arguments,
        )?;
        let mut limitations = vec![
            "CSV 保存标题和属性值。页面与块 ID、正文、子页、历史、视图设置、属性定义及未列入附件属性的文件随原生工作空间包保留。".into(),
            "导入新数据库时，标题以外的列以文本保存。追加到现有数据库时，支持按列名恢复可编辑属性。".into(),
        ];
        if view
            .and_then(|view| view.settings.group_by.as_ref())
            .is_some()
        {
            limitations.push("视图分组在 CSV 中展开为数据行，保留筛选与排序。".into());
        }
        if collection.properties.iter().any(|field| {
            matches!(
                field.kind,
                PropertyKind::CreatedAt | PropertyKind::UpdatedAt
            )
        }) {
            limitations
                .push("创建和修改时间以 Unix 毫秒保存，导入的记录使用自身的系统时间。".into());
        }
        if collection
            .properties
            .iter()
            .any(|field| field.kind == PropertyKind::Attachment)
        {
            limitations.push(
                "附件属性以相对路径数组表示。可从配套目录取得原件，完整附件关联可用原生包恢复。"
                    .into(),
            );
        }
        if collection
            .properties
            .iter()
            .any(|field| field.kind == PropertyKind::Relation)
        {
            limitations.push(
                "关联列导出当前可用记录的标题。恢复关联和反向属性请使用原生工作空间包。".into(),
            );
        }
        let mut report = CsvExportReport {
            rows: 0,
            view: view.map(|view| view.kind),
            view_name: view.map(|view| {
                view.name
                    .as_deref()
                    .unwrap_or_else(|| view.kind.label())
                    .to_owned()
            }),
            limitations,
            attachments: Vec::new(),
        };
        let computed = collection
            .properties
            .iter()
            .any(|field| field.kind.computed());
        if computed {
            report.limitations.push(
                "公式与汇总列保存导出时的数值、文本或错误代码，规则随原生工作空间包保留。".into(),
            );
        }
        output.write_all(b"\xef\xbb\xbf")?;
        let mut writer = WriterBuilder::new()
            .terminator(Terminator::CRLF)
            .from_writer(output);
        writer
            .write_record(
                iter::once("标题").chain(
                    collection
                        .properties
                        .iter()
                        .map(|field| field.name.as_str()),
                ),
            )
            .map_err(csv_error)?;
        {
            let mut query = tx.prepare(&format!(
                "SELECT {RECORD_COLUMNS} {criteria} ORDER BY {order}"
            ))?;
            let records = query.query_map(params_from_iter(&arguments.0), record_row)?;
            let mut exported_paths = HashMap::new();
            for record in records {
                let mut record = record?;
                record.relations = super::relations::project_values(
                    &tx,
                    &collection.properties,
                    &mut record.properties,
                )?;
                if computed {
                    record.computed = super::computed::Engine::new(&tx, None)
                        .fields(&record.id)?
                        .0;
                }
                let mut fields = Vec::with_capacity(collection.properties.len() + 1);
                fields.push(record.title.clone());
                for field in &collection.properties {
                    fields.push(csv_value(&record, field, |id| {
                        attachment_path(
                            &tx,
                            &record.id,
                            id,
                            &mut exported_paths,
                            &mut report.attachments,
                            &mut copy_attachment,
                        )
                    })?);
                }
                writer.write_record(fields).map_err(csv_error)?;
                report.rows += 1;
            }
        }
        writer.flush()?;
        tx.commit()?;
        Ok(report)
    }
}

fn csv_value(
    record: &Record,
    field: &PropertyDefinition,
    mut attachment_path: impl FnMut(&str) -> Result<String>,
) -> Result<String> {
    let value = record.properties.get(&field.id);
    let option_name = |id: &str| {
        field
            .options
            .iter()
            .find(|item| item.id == id)
            .map(|item| item.name.as_str())
            .ok_or_else(|| {
                Error::new(
                    "CSV_VALUE",
                    format!("「{}」含有无法找到的选项。", field.name),
                )
            })
    };
    let invalid = || {
        Error::new(
            "CSV_VALUE",
            format!("「{}」的值与当前属性定义不符。", field.name),
        )
    };
    Ok(match (field.kind, value) {
        (PropertyKind::Formula | PropertyKind::Rollup, _) => match record.computed.get(&field.id) {
            Some(super::ComputedValue::Value {
                value: Value::String(text),
            }) => text.clone(),
            Some(super::ComputedValue::Value { value }) if !value.is_null() => value.to_string(),
            Some(super::ComputedValue::Error { code }) => serde_json::to_value(code)?
                .as_str()
                .unwrap_or_default()
                .into(),
            _ => String::new(),
        },
        (PropertyKind::CreatedAt, _) => record.created_at.to_string(),
        (PropertyKind::UpdatedAt, _) => record.updated_at.to_string(),
        (PropertyKind::Relation, _) => {
            let titles = record
                .relations
                .get(&field.id)
                .map(|field| {
                    field
                        .items
                        .iter()
                        .map(|item| item.title.as_str())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if titles.is_empty() {
                String::new()
            } else {
                serde_json::to_string(&titles)?
            }
        }
        (_, None) => String::new(),
        (PropertyKind::Status | PropertyKind::Select, Some(Value::String(id))) => {
            option_name(id)?.to_owned()
        }
        (PropertyKind::MultiSelect, Some(Value::Array(ids))) => serde_json::to_string(
            &ids.iter()
                .map(|id| option_name(id.as_str().ok_or_else(invalid)?))
                .collect::<Result<Vec<_>>>()?,
        )?,
        (PropertyKind::Attachment, Some(Value::Array(ids))) => serde_json::to_string(
            &ids.iter()
                .map(|id| attachment_path(id.as_str().ok_or_else(invalid)?))
                .collect::<Result<Vec<_>>>()?,
        )?,
        (
            PropertyKind::Text | PropertyKind::Date | PropertyKind::DateTime | PropertyKind::Url,
            Some(Value::String(text)),
        ) => text.clone(),
        (PropertyKind::Number, Some(Value::Number(value))) => value.to_string(),
        (PropertyKind::Checkbox, Some(Value::Bool(value))) => value.to_string(),
        _ => return Err(invalid()),
    })
}

fn attachment_path(
    db: &Connection,
    record: &str,
    id: &str,
    paths: &mut HashMap<String, (String, String)>,
    attachments: &mut Vec<(String, String)>,
    copy_attachment: &mut impl FnMut(&Attachment) -> Result<String>,
) -> Result<String> {
    if let Some((owner, path)) = paths.get(id) {
        if owner == record {
            return Ok(path.clone());
        }
        return Err(Error::new("ATTACHMENT_NOT_FOUND", "找不到该记录的附件。"));
    }
    let attachment = read_attachment(db, record, id)?;
    let path = copy_attachment(&attachment)?;
    attachments.push((attachment.name, path.clone()));
    paths.insert(id.into(), (attachment.page_id, path.clone()));
    Ok(path)
}

fn csv_error(error: csv::Error) -> Error {
    Error::new("CSV_EXPORT", format!("无法写入 CSV 文件。{error}"))
}
