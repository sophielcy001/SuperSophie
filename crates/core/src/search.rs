// SPDX-License-Identifier: AGPL-3.0-or-later

mod index;
pub(crate) mod pdf;

pub(crate) use index::install;

use crate::collections::{Arguments, filter_clause, read_collection, validate_settings};
use crate::store::read_info;
use crate::{Error, PageInfo, Result, Store, ViewFilter, ViewSettings};
use pdf::{PdfProgress, PdfTarget, PdfTextMatch};
use rusqlite::{Connection, params_from_iter};
use serde::{Deserialize, Serialize};

pub(crate) const PAGE_COLUMNS: &str =
    "p.id, p.parent_id, p.title, p.revision, p.updated_at, p.icon, p.trash_root_id";
const FIRST_PAGE: usize = 20;
const QUERY_LIMIT: usize = 200;

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SearchRequest {
    pub query: String,
    pub collection_id: Option<String>,
    pub filter: Option<ViewFilter>,
    #[serde(default)]
    pub offset: u32,
    #[serde(default)]
    pub page_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchIndexStatus {
    pub pending: u32,
    pub pdf: PdfProgress,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub hits: Vec<SearchHit>,
    pub index: SearchIndexStatus,
    pub next_offset: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub page: PageInfo,
    pub block_id: Option<String>,
    pub snippet: Option<SearchSnippet>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf: Option<PdfTextMatch>,
}

#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct SearchSnippet {
    pub before: String,
    pub matched: String,
    pub after: String,
}

impl Store {
    pub fn search_text(&self, workspace: &str, request: &SearchRequest) -> Result<SearchResponse> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        let index = index::status(&tx)?;
        let mut hits = find(&tx, request, &query_text(&request.query)?, FIRST_PAGE + 1)?;
        let next_offset =
            (hits.len() > FIRST_PAGE).then(|| request.offset.saturating_add(FIRST_PAGE as u32));
        hits.truncate(FIRST_PAGE);
        tx.commit()?;
        Ok(SearchResponse {
            hits,
            index,
            next_offset,
        })
    }

    /// Compatibility entry point for callers that expect a complete page list.
    /// The desktop uses search_text and advances the index in background batches.
    pub fn search(&self, workspace: &str, query: &str) -> Result<Vec<PageInfo>> {
        let text = query_text(query)?;
        if text.is_empty() {
            self.connection(workspace)?;
            return Ok(Vec::new());
        }
        while self.advance_search_index(workspace)?.pending != 0 {}
        let results = find(
            &self.connection(workspace)?,
            &SearchRequest {
                page_only: true,
                ..Default::default()
            },
            &text,
            80,
        )?;
        Ok(results.into_iter().map(|hit| hit.page).collect())
    }
}

pub(crate) fn query_text(text: &str) -> Result<String> {
    let text = text.trim();
    if text.contains('\0') {
        return Err(Error::new("SEARCH_QUERY", "搜索词不能包含空字符。"));
    }
    if text.chars().count() > QUERY_LIMIT {
        return Err(Error::new("SEARCH_QUERY", "搜索词最多可输入 200 个字符。"));
    }
    Ok(normalized(text))
}

fn find(
    db: &Connection,
    request: &SearchRequest,
    text: &str,
    limit: usize,
) -> Result<Vec<SearchHit>> {
    let mut arguments = Arguments::default();
    let (join, scope) = scoped_filter(db, request, &mut arguments)?;
    if text.is_empty() {
        if request.collection_id.is_none() {
            return Ok(Vec::new());
        }
        let offset = arguments.bind(i64::from(request.offset));
        let mut statement = db.prepare(&format!(
            "SELECT {PAGE_COLUMNS} FROM pages p {join}
             WHERE p.trash_root_id IS NULL {scope} ORDER BY p.updated_at DESC, p.id LIMIT {limit} OFFSET {offset}"
        ))?;
        return Ok(statement
            .query_map(params_from_iter(&arguments.0), |row| {
                Ok(SearchHit {
                    page: read_info(row)?,
                    block_id: None,
                    snippet: None,
                    pdf: None,
                })
            })?
            .collect::<rusqlite::Result<_>>()?);
    }
    let (entries, condition) = matching_entries(text, &mut arguments);
    let mut pdf_hits = String::new();
    if !request.page_only {
        let (pdf_entries, pdf_condition) = pdf::matching_entries(text, &mut arguments);
        let original = pdf::MATCHING_ORIGINAL;
        pdf_hits = format!(
            "UNION ALL
             SELECT p.id, 2, NULL, e.original, a.id, a.sha256, e.page_number, a.name
             FROM {pdf_entries} {original} CROSS JOIN pages p ON p.id = a.page_id
             {join}
             WHERE {pdf_condition} AND p.trash_root_id IS NULL {scope}"
        );
    }
    let offset = arguments.bind(i64::from(request.offset));
    // Position zero represents the title. Select one result per page, with the
    // first matching body block as the target when the title doesn't match.
    let matched = if request.collection_id.is_none() {
        // Aggregate by page revision before looking up page metadata. Fetch
        // snippet payloads only for the page hits that can survive pagination.
        format!(
            "candidates AS (
                 SELECT e.page_id, e.revision, min(e.position) AS position
                 FROM {entries} WHERE {condition}
                 GROUP BY e.page_id, e.revision
             ),
             matched AS (
                 SELECT m.page_id, m.position
                 FROM candidates m CROSS JOIN pages p ON p.id = m.page_id AND p.revision = m.revision
                 WHERE p.trash_root_id IS NULL
                 ORDER BY (m.position != 0), p.updated_at DESC, p.id
                 LIMIT {limit} + {offset}
             )"
        )
    } else {
        format!(
            "matched AS (
                 SELECT e.page_id, min(e.position) AS position
                 FROM {entries} JOIN pages p ON p.id = e.page_id AND p.revision = e.revision
                 {join}
                 WHERE {condition} AND p.trash_root_id IS NULL {scope}
                 GROUP BY e.page_id
             )"
        )
    };
    let mut statement = db.prepare(&format!(
        "WITH {matched},
         hits(page_id, rank, block_id, original, attachment_id, sha256, page_number, name) AS (
             SELECT m.page_id, (m.position != 0), e.block_id, e.original, NULL, NULL, NULL, NULL
             FROM matched m
             JOIN search_entries e ON e.page_id = m.page_id AND e.position = m.position
             {pdf_hits}
         )
         SELECT {PAGE_COLUMNS}, h.block_id, h.original, h.attachment_id, h.sha256, h.page_number, h.name
         FROM hits h JOIN pages p ON p.id = h.page_id
         ORDER BY h.rank, p.updated_at DESC, p.id, h.attachment_id, h.page_number LIMIT {limit} OFFSET {offset}"
    ))?;
    Ok(statement
        .query_map(params_from_iter(&arguments.0), |row| {
            let original: String = row.get(8)?;
            let pdf = if let Some(attachment_id) = row.get(9)? {
                Some(PdfTextMatch {
                    target: PdfTarget {
                        attachment_id,
                        sha256: row.get(10)?,
                        page_number: row.get(11)?,
                    },
                    name: row.get(12)?,
                })
            } else {
                None
            };
            Ok(SearchHit {
                page: read_info(row)?,
                block_id: row.get(7)?,
                snippet: snippet(&original, text),
                pdf,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn matching_entries(text: &str, arguments: &mut Arguments) -> (&'static str, String) {
    entry_filter(
        text,
        arguments,
        "search_entries e",
        "search_fts JOIN search_entries e ON e.rowid = search_fts.rowid",
        "search_fts",
    )
}

pub(super) fn entry_filter(
    text: &str,
    arguments: &mut Arguments,
    short_entries: &'static str,
    full_entries: &'static str,
    index: &str,
) -> (&'static str, String) {
    if text.chars().count() >= 3 {
        let phrase = arguments.bind(format!("\"{}\"", text.replace('"', "\"\"")));
        (full_entries, format!("{index} MATCH {phrase}"))
    } else {
        let term = arguments.bind(text.to_owned());
        (short_entries, format!("instr(e.normalized, {term}) > 0"))
    }
}

fn scoped_filter(
    db: &Connection,
    request: &SearchRequest,
    arguments: &mut Arguments,
) -> Result<(&'static str, String)> {
    let Some(id) = &request.collection_id else {
        if request.filter.is_some() {
            return Err(Error::new("SEARCH_SCOPE", "请先选择要筛选的数据库。"));
        }
        return Ok(("", String::new()));
    };
    let collection = read_collection(db, id)?;
    let collection_id = arguments.bind(collection.id.clone());
    let mut scope = format!("AND r.collection_id = {collection_id}");
    if let Some(filter) = &request.filter {
        validate_settings(
            &ViewSettings {
                filter: Some(filter.clone()),
                ..Default::default()
            },
            &collection.properties,
        )?;
        let clause = filter_clause(&collection, filter, arguments)?;
        scope.push_str(" AND ");
        scope.push_str(&clause);
    }
    Ok(("JOIN collection_records r ON r.page_id = p.id", scope))
}

fn folded(ch: char) -> std::char::ToLowercase {
    (if ch == '\0' { ' ' } else { ch }).to_lowercase()
}

fn normalized(text: &str) -> String {
    text.chars().flat_map(folded).collect()
}

pub(crate) fn snippet(original: &str, query: &str) -> Option<SearchSnippet> {
    let start = normalized(original).find(query)?;
    let end = start + query.len();
    let mut offset = 0;
    let mut original_start = None;
    for (byte, ch) in original.char_indices() {
        offset += folded(ch).map(char::len_utf8).sum::<usize>();
        if offset > start && original_start.is_none() {
            original_start = Some(byte);
        }
        if offset >= end {
            let first = original_start?;
            let last = byte + ch.len_utf8();
            let leading = &original[..first];
            let trailing = &original[last..];
            let before = match leading.char_indices().rev().nth(34) {
                Some((index, ch)) => format!("…{}", &leading[index + ch.len_utf8()..]),
                None => leading.to_owned(),
            };
            let after = match trailing.char_indices().nth(54) {
                Some((index, _)) => format!("{}…", &trailing[..index]),
                None => trailing.to_owned(),
            };
            return Some(SearchSnippet {
                before,
                matched: original[first..last].to_owned(),
                after,
            });
        }
    }
    None
}
