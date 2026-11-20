// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{ImportFile, ImportKind, ImportOutcome, MAX_CSV_IMPORT_BYTES, Source};
use crate::collections::{MAX_PROPERTIES, create_collection_in, normalize_values, read_collection};
use crate::model::{name, new_id};
use crate::store::{insert_new_page, new_page};
use crate::{Error, PropertyDefinition, PropertyKind, Result, Store};
use ::csv::{Position, ReaderBuilder, StringRecord};
use rusqlite::TransactionBehavior;
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

const MAX_ROWS: usize = 10_000;
const MAX_COLUMNS: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CsvDestination {
    New { parent_id: Option<String> },
    Existing { collection_id: String },
}

impl Store {
    pub fn import_csv(
        &self,
        workspace: &str,
        destination: CsvDestination,
        file: ImportFile,
    ) -> Result<ImportOutcome> {
        let mut outcome = ImportOutcome::new(file.name.clone());
        let source = match Source::new(file, &["csv"], MAX_CSV_IMPORT_BYTES) {
            Ok(source) => source,
            Err(error) => return Ok(ImportOutcome::failed(outcome.name, error.message)),
        };
        let (kind, target) = match &destination {
            CsvDestination::New { parent_id } => {
                (ImportKind::CsvNew, parent_id.as_deref().unwrap_or_default())
            }
            CsvDestination::Existing { collection_id } => {
                (ImportKind::CsvExisting, collection_id.as_str())
            }
        };
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((id, trashed)) = source.earlier(&tx, kind, target)? {
            outcome.duplicate(id, trashed);
            return Ok(outcome);
        }
        let existing = match &destination {
            CsvDestination::New { .. } => None,
            CsvDestination::Existing { collection_id } => {
                Some(read_collection(&tx, collection_id)?)
            }
        };
        let result = prepare(
            &source,
            existing.as_ref().map(|db| db.properties.as_slice()),
            &mut outcome,
        );
        let (definitions, rows) = match result {
            Ok(prepared) => prepared,
            Err(error) => {
                outcome.note(error.message);
                return Ok(outcome);
            }
        };
        let container = match (&destination, existing) {
            (CsvDestination::New { parent_id }, _) => {
                let title = name(source.stem(), 200, Some("导入的数据库"))?;
                let (_, page) =
                    create_collection_in(&tx, parent_id.as_deref(), &title, Some(definitions))?;
                page.info.id
            }
            (CsvDestination::Existing { .. }, Some(collection)) => collection.container_page_id,
            _ => return Err(Error::new("COLLECTION_NOT_FOUND", "找不到目标数据库。")),
        };
        let count = rows.len();
        for (title, values) in rows {
            let mut page = new_page(Some(&container), &title, "file")?;
            page.properties = Some(values);
            insert_new_page(&tx, page)?;
        }
        let original = source.save(self, &tx, workspace, kind, target, &container)?;
        tx.commit()?;
        outcome.imported(container, count);
        outcome.original = Some(original);
        Ok(outcome)
    }
}

pub(super) type CsvRow = (String, BTreeMap<String, Value>);

fn prepare(
    source: &Source,
    existing: Option<&[PropertyDefinition]>,
    outcome: &mut ImportOutcome,
) -> Result<(Vec<PropertyDefinition>, Vec<CsvRow>)> {
    prepare_rows(source, existing, &["标题", "title"], outcome)
}

pub(super) fn prepare_notion(
    source: &Source,
    outcome: &mut ImportOutcome,
) -> Result<(Vec<PropertyDefinition>, Vec<CsvRow>)> {
    prepare_rows(source, None, &["标题", "title", "名称", "name"], outcome)
}

fn prepare_rows(
    source: &Source,
    existing: Option<&[PropertyDefinition]>,
    title_names: &[&str],
    outcome: &mut ImportOutcome,
) -> Result<(Vec<PropertyDefinition>, Vec<CsvRow>)> {
    let text = source.text()?;
    validate_quotes(text.as_bytes())?;
    let mut source_line = csv_source_lines(text.as_bytes());
    let mut reader = ReaderBuilder::new().from_reader(text.as_bytes());
    let headers = reader
        .headers()
        .map_err(|error| {
            let line = error.position().map_or(1, &mut source_line);
            csv_error(error, line)
        })?
        .clone();
    let column_names = columns(&headers)?;
    let title_index = title_names
        .iter()
        .find_map(|title| {
            column_names
                .iter()
                .position(|column| column.to_lowercase() == *title)
        })
        .unwrap_or(0);
    let definitions = match existing {
        Some(_) => Vec::new(),
        None => new_properties(&column_names, title_index)?,
    };
    let schema = existing.unwrap_or(&definitions);
    let mapping = column_names
        .iter()
        .enumerate()
        .filter_map(|(index, column)| {
            if index == title_index {
                return None;
            }
            let field = schema
                .iter()
                .find(|p| p.name.to_lowercase() == column.to_lowercase());
            match field {
                Some(field) if supports_csv(field.kind) => Some((index, field)),
                _ => {
                    outcome.note(format!(
                        "列「{column}」没有可写入的目标属性，列值保留在 CSV 原件中。"
                    ));
                    None
                }
            }
        })
        .collect::<Vec<_>>();
    let mut rows = Vec::new();
    let mut seen_rows = 0;
    for (index, row) in reader.records().enumerate() {
        if index >= MAX_ROWS {
            return Err(Error::new(
                "IMPORT_SIZE",
                format!("每个 CSV 最多可导入 {MAX_ROWS} 行记录。"),
            ));
        }
        seen_rows += 1;
        let row = match row {
            Ok(row) => row,
            Err(error) => {
                let line = error.position().map_or(index as u64 + 2, &mut source_line);
                outcome.note(csv_error(error, line).message);
                continue;
            }
        };
        let line = row.position().map_or(index as u64 + 2, &mut source_line);
        let raw_title = &row[title_index];
        let title = match name(raw_title, 200, None) {
            Ok(title) => title,
            Err(_) => {
                outcome.note(format!("第 {line} 行的标题无效，此行已跳过。"));
                continue;
            }
        };
        if title != raw_title {
            outcome.note(format!("第 {line} 行标题的首尾空白已去除。"));
        }
        let mut values = BTreeMap::new();
        for &(column, property) in &mapping {
            let raw = &row[column];
            if raw.trim().is_empty() {
                continue;
            }
            let normalized = crate::collections::parse_property_text(raw, property).map(|value| {
                normalize_values(
                    std::slice::from_ref(property),
                    BTreeMap::from([(property.id.clone(), value)]),
                    |_| Ok(false),
                )
            });
            match normalized {
                Some(Ok(value)) => values.extend(value),
                _ => outcome.note(format!(
                    "第 {line} 行「{}」无法转换，原值保存在 CSV 原件中。",
                    property.name
                )),
            }
        }
        rows.push((title, values));
    }
    if seen_rows > 0 && rows.is_empty() {
        return Err(Error::new(
            "IMPORT_EMPTY",
            "所有数据行均无有效标题或列数不匹配，原件仍在所选位置。",
        ));
    }
    Ok((definitions, rows))
}

fn columns(headers: &StringRecord) -> Result<Vec<String>> {
    if headers.is_empty() || headers.len() > MAX_COLUMNS {
        return Err(Error::new(
            "CSV_HEADER",
            "需要一行 CSV 列名，最多支持 128 列。",
        ));
    }
    let mut names = HashSet::new();
    headers
        .iter()
        .map(|raw| {
            let column = name(raw, 64, None).map_err(|_| {
                Error::new("CSV_HEADER", "列名需要 1 至 64 个字符，且不能包含换行。")
            })?;
            if !names.insert(column.to_lowercase()) {
                return Err(Error::new("CSV_HEADER", format!("列名「{column}」重复。")));
            }
            Ok(column)
        })
        .collect()
}

fn new_properties(columns: &[String], title: usize) -> Result<Vec<PropertyDefinition>> {
    if columns.len() - 1 > MAX_PROPERTIES {
        return Err(Error::new(
            "PROPERTY_LIMIT",
            "新数据库最多支持 48 个自定义属性。",
        ));
    }
    Ok(columns
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != title)
        .map(|(_, column)| PropertyDefinition {
            id: new_id(),
            name: column.clone(),
            kind: PropertyKind::Text,
            relation: None,
            calculation: None,
            options: Vec::new(),
        })
        .collect())
}

fn supports_csv(kind: PropertyKind) -> bool {
    !matches!(
        kind,
        PropertyKind::Attachment
            | PropertyKind::Relation
            | PropertyKind::Formula
            | PropertyKind::Rollup
            | PropertyKind::CreatedAt
            | PropertyKind::UpdatedAt
    )
}

fn csv_source_lines(input: &[u8]) -> impl FnMut(&Position) -> u64 + '_ {
    let mut line_breaks = input
        .iter()
        .enumerate()
        .filter_map(|(offset, &byte)| {
            let ends_line =
                byte == b'\n' || (byte == b'\r' && input.get(offset + 1) != Some(&b'\n'));
            ends_line.then_some(offset)
        })
        .peekable();
    let mut line = 1;
    move |position| {
        let offset = position.byte() as usize;
        // The reader can report the pending LF of CRLF or a skipped empty line.
        let skipped = input[offset..]
            .iter()
            .take_while(|&&byte| matches!(byte, b'\r' | b'\n'))
            .count();
        let start = offset + skipped;
        while line_breaks.next_if(|&end| end < start).is_some() {
            line += 1;
        }
        line
    }
}

fn csv_error(error: ::csv::Error, line: u64) -> Error {
    let message = if let ::csv::ErrorKind::UnequalLengths {
        expected_len, len, ..
    } = error.kind()
    {
        format!("第 {line} 行有 {len} 列，预期 {expected_len} 列，此行已跳过。")
    } else {
        "CSV 记录无法读取。".into()
    };
    Error::new("CSV_FORMAT", message)
}

// The CSV reader is deliberately permissive. Detect invalid quoting before it
// could accept a damaged record and merge subsequent rows into one field.
fn validate_quotes(input: &[u8]) -> Result<()> {
    #[derive(Clone, Copy)]
    enum Field {
        Start,
        Plain,
        Quoted,
        AfterQuote,
    }
    use Field::{AfterQuote, Plain, Quoted, Start};
    let mut state = Start;
    let mut line = 1;
    let mut column = 1;
    let mut prior_cr = false;
    for &byte in input {
        state = match (state, byte) {
            (Start | AfterQuote, b'"') => Quoted,
            (Quoted, b'"') => AfterQuote,
            (Quoted, _) => Quoted,
            (_, b',' | b'\n' | b'\r') => Start,
            (Start | Plain, byte) if byte != b'"' => Plain,
            _ => {
                return Err(Error::new(
                    "CSV_FORMAT",
                    format!("第 {line} 行第 {column} 列的引号格式无效。"),
                ));
            }
        };
        if byte == b'\r' || (byte == b'\n' && !prior_cr) {
            line += 1;
            column = 1;
        } else if byte == b',' && matches!(state, Start) {
            column += 1;
        }
        prior_cr = byte == b'\r';
    }
    if matches!(state, Quoted) {
        return Err(Error::new("CSV_FORMAT", "CSV 文件结束时仍有未闭合的引号。"));
    }
    Ok(())
}
