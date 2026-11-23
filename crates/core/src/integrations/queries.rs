// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{
    Access, IntegrationQueryPage, IntegrationScope,
    content::{can_read_property, project_record_properties, searchable_block},
    cursors::Pagination,
    missing,
};
use crate::collections::{
    Arguments, MAX_PROPERTIES, RECORD_COLUMNS, field_kind, filter_clause, read_collection,
    record_order, record_row, validate_settings,
};
use crate::search::{matching_entries, pdf, query_text, snippet};
use crate::{
    Block, Collection, Error, FilterOperator, PdfProgress, PdfTarget, PdfTextMatch,
    PropertyDefinition, Result, SearchSnippet, SortDirection, Store, ViewFilter, ViewSettings,
    ViewSort,
};
use rusqlite::{Connection, Row, params_from_iter};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationFilter {
    pub field_id: String,
    pub operator: FilterOperator,
    pub value: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSort {
    pub field_id: String,
    pub direction: SortDirection,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSearchRequest {
    #[serde(default)]
    pub query: String,
    pub kind: Option<IntegrationSearchKind>,
    pub collection_id: Option<String>,
    #[serde(default)]
    pub filters: Vec<IntegrationFilter>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationSearchKind {
    Page,
    Collection,
    Record,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationSearchSource {
    Title,
    Block,
    Metadata,
    Pdf,
}

#[derive(Debug, Serialize)]
pub struct IntegrationSearchHit {
    pub page_id: String,
    pub collection_id: Option<String>,
    pub kind: IntegrationSearchKind,
    pub title: String,
    pub page_revision: u32,
    pub block_id: Option<String>,
    pub source: IntegrationSearchSource,
    pub snippet: Option<SearchSnippet>,
    #[serde(skip)]
    pub pdf: Option<PdfTextMatch>,
}

#[derive(Debug, Serialize)]
pub struct IntegrationCollection {
    pub id: String,
    pub container_page_id: String,
    pub schema_revision: u32,
    pub properties: Vec<PropertyDefinition>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationCollectionQuery {
    #[serde(default)]
    pub filters: Vec<IntegrationFilter>,
    pub sort: Option<IntegrationSort>,
    pub limit: Option<usize>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct IntegrationCollectionRecords {
    pub collection_id: String,
    pub schema_revision: u32,
    #[serde(flatten)]
    pub page: IntegrationQueryPage<IntegrationRecord>,
}

#[derive(Debug, Serialize)]
pub struct IntegrationRecord {
    pub page_id: String,
    pub title: String,
    pub page_revision: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub properties: BTreeMap<String, Value>,
    pub properties_restricted: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub restricted_property_ids: Vec<String>,
}

impl Store {
    pub fn integration_search(
        &self,
        workspace: &str,
        token: &str,
        request: &IntegrationSearchRequest,
    ) -> Result<IntegrationQueryPage<IntegrationSearchHit>> {
        self.integration_read(workspace, token, |db, access| {
            let collection = request
                .collection_id
                .as_deref()
                .map(|id| authorized_collection(db, access, id))
                .transpose()?;
            if collection.is_none() && !request.filters.is_empty() {
                return Err(Error::new("SEARCH_SCOPE", "按属性筛选时需要指定数据库。"));
            }
            let term = query_text(&request.query)?;
            if term.is_empty() && collection.is_none() && request.kind.is_none() {
                return Err(Error::new(
                    "SEARCH_QUERY",
                    "请输入关键词，或指定资源类型或数据库。",
                ));
            }
            let query = (
                "search",
                &term,
                request.kind,
                &request.collection_id,
                &request.filters,
            );
            let include_pdf =
                !term.is_empty() && access.scopes.contains(&IntegrationScope::ReadAssets);
            let pdf_index = include_pdf
                .then(|| visible_pdf_progress(db, access))
                .transpose()?;
            let paging = if let Some(index) = pdf_index {
                // PDF indexing can add results without editing a page. Only
                // published text from readable attachments joins this snapshot.
                Pagination::new(
                    db,
                    access,
                    request.limit,
                    request.cursor.as_deref(),
                    &(query, index.revision),
                )?
            } else {
                Pagination::new(db, access, request.limit, request.cursor.as_deref(), &query)?
            };
            let hits = search(
                db,
                access,
                request,
                collection.as_ref(),
                &term,
                &paging,
                include_pdf,
            )?;
            let mut result = paging.finish(hits);
            result.pdf_index = pdf_index;
            Ok(result)
        })
    }

    pub fn integration_collection(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<IntegrationCollection> {
        self.integration_read(workspace, token, |db, access| {
            let mut collection = authorized_collection(db, access, id)?;
            let mut hidden_relations = BTreeSet::new();
            for field in &mut collection.properties {
                if let Some(relation) = &field.relation {
                    match authorized_collection(db, access, &relation.collection_id) {
                        Ok(_) => {}
                        Err(error) if error.code == "RESOURCE_NOT_FOUND" => {
                            hidden_relations.insert(field.id.clone());
                            field.relation = None;
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
            for field in &mut collection.properties {
                if let Some(crate::Calculation::Rollup {
                    relation_property_id,
                    ..
                }) = &field.calculation
                    && hidden_relations.contains(relation_property_id)
                {
                    field.calculation = None;
                }
            }
            Ok(IntegrationCollection {
                id: collection.id,
                container_page_id: collection.container_page_id,
                schema_revision: collection.schema_revision,
                properties: collection.properties,
            })
        })
    }

    pub fn integration_collection_query(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
        request: &IntegrationCollectionQuery,
    ) -> Result<IntegrationCollectionRecords> {
        self.integration_read(workspace, token, |db, access| {
            let collection = authorized_collection(db, access, id)?;
            let paging = Pagination::new(
                db,
                access,
                request.limit,
                request.cursor.as_deref(),
                &("collection", id, &request.filters, &request.sort),
            )?;
            let mut arguments = Arguments::default();
            let readable = access.readable_pages_cte(&mut arguments);
            let collection_id = arguments.bind(collection.id.clone());
            let filters = build_filters(access, &collection, &request.filters, &mut arguments)?;
            let sort = request
                .sort
                .as_ref()
                .map(|sort| {
                    require_field(access, &collection, &sort.field_id)?;
                    let sort = ViewSort {
                        field_id: sort.field_id.clone(),
                        direction: sort.direction,
                    };
                    validate_settings(
                        &ViewSettings {
                            sort: Some(sort.clone()),
                            ..Default::default()
                        },
                        &collection.properties,
                    )?;
                    Ok::<_, Error>(sort)
                })
                .transpose()?;
            let order = record_order(&collection, sort.as_ref(), &mut arguments)?;
            let limit = arguments.bind((paging.limit + 1) as i64);
            let offset = arguments.bind(paging.offset as i64);
            let mut statement = db.prepare(&format!(
                "{readable}
                 SELECT {RECORD_COLUMNS}
                 FROM collection_records r JOIN readable_pages a ON a.id = r.page_id
                 JOIN pages p ON p.id = r.page_id
                 WHERE r.collection_id = {collection_id} {filters}
                 ORDER BY {order} LIMIT {limit} OFFSET {offset}",
            ))?;
            let mut items = Vec::new();
            for record in statement.query_map(params_from_iter(&arguments.0), record_row)? {
                let record = record?;
                let (properties, properties_restricted, restricted_property_ids) =
                    project_record_properties(
                        db,
                        access,
                        &collection.properties,
                        &record.properties,
                    )?;
                items.push(IntegrationRecord {
                    page_id: record.id,
                    title: record.title,
                    page_revision: record.revision,
                    created_at: record.created_at,
                    updated_at: record.updated_at,
                    properties,
                    properties_restricted,
                    restricted_property_ids,
                });
            }
            Ok(IntegrationCollectionRecords {
                collection_id: collection.id,
                schema_revision: collection.schema_revision,
                page: paging.finish(items),
            })
        })
    }
}

pub(super) fn authorized_collection(
    db: &Connection,
    access: &Access,
    id: &str,
) -> Result<Collection> {
    let collection = read_collection(db, id).map_err(|error| {
        if error.code == "COLLECTION_NOT_FOUND" {
            missing()
        } else {
            error
        }
    })?;
    access.require_page(db, &collection.container_page_id)?;
    Ok(collection)
}

fn require_field(access: &Access, collection: &Collection, id: &str) -> Result<()> {
    if can_read_property(access, field_kind(&collection.properties, id)?) {
        Ok(())
    } else {
        Err(Error::new("FORBIDDEN", "集成无权使用该属性筛选或排序。"))
    }
}

fn build_filters(
    access: &Access,
    collection: &Collection,
    filters: &[IntegrationFilter],
    arguments: &mut Arguments,
) -> Result<String> {
    if filters.len() > MAX_PROPERTIES {
        return Err(Error::new(
            "QUERY_FILTER_LIMIT",
            "每次查询最多可以使用 48 个筛选条件。",
        ));
    }
    let mut conditions = String::new();
    for filter in filters {
        require_field(access, collection, &filter.field_id)?;
        let filter = ViewFilter {
            field_id: filter.field_id.clone(),
            operator: filter.operator,
            value: filter.value.clone(),
        };
        validate_settings(
            &ViewSettings {
                filter: Some(filter.clone()),
                ..Default::default()
            },
            &collection.properties,
        )?;
        conditions.push_str(" AND ");
        conditions.push_str(&filter_clause(collection, &filter, arguments)?);
    }
    Ok(conditions)
}

fn visible_pdf_progress(db: &Connection, access: &Access) -> Result<PdfProgress> {
    let mut arguments = Arguments::default();
    let readable = access.readable_pages_cte(&mut arguments);
    let fields = pdf::PROGRESS_FIELDS;
    let live = pdf::LIVE_ATTACHMENTS;
    Ok(db.query_row(
        &format!(
            "{readable}
                  SELECT {fields}, COALESCE(max(d.revision) FILTER (WHERE d.indexed_pages > 0), 0)
                  {live} AND a.page_id IN (SELECT id FROM readable_pages)"
        ),
        params_from_iter(&arguments.0),
        pdf::read_progress,
    )?)
}

const SEARCH_PAGE: &str = "
    p.id, p.title, p.revision, c.id,
    CASE WHEN allowed_owner.id IS NOT NULL THEN r.collection_id END,
    r.collection_id IS NOT NULL
";

const SEARCH_COLLECTIONS: &str = "
    LEFT JOIN collections c ON c.container_page_id = p.id
    LEFT JOIN collection_records r ON r.page_id = p.id
    LEFT JOIN collections owner ON owner.id = r.collection_id
    LEFT JOIN readable_pages allowed_owner ON allowed_owner.id = owner.container_page_id
";

fn search_hit(
    row: &Row<'_>,
    source: IntegrationSearchSource,
    block_id: Option<String>,
    snippet: Option<SearchSnippet>,
    pdf: Option<PdfTextMatch>,
) -> rusqlite::Result<IntegrationSearchHit> {
    let own_collection: Option<String> = row.get(3)?;
    let is_record: bool = row.get(5)?;
    let kind = if own_collection.is_some() {
        IntegrationSearchKind::Collection
    } else if is_record {
        IntegrationSearchKind::Record
    } else {
        IntegrationSearchKind::Page
    };
    Ok(IntegrationSearchHit {
        page_id: row.get(0)?,
        title: row.get(1)?,
        page_revision: row.get(2)?,
        kind,
        collection_id: own_collection.or(row.get(4)?),
        block_id,
        source,
        snippet,
        pdf,
    })
}

fn search(
    db: &Connection,
    access: &Access,
    request: &IntegrationSearchRequest,
    collection: Option<&Collection>,
    term: &str,
    paging: &Pagination,
    include_pdf: bool,
) -> Result<Vec<IntegrationSearchHit>> {
    let mut arguments = Arguments::default();
    let readable = access.readable_pages_cte(&mut arguments);
    let mut filters = String::new();
    if let Some(collection) = collection {
        let id = arguments.bind(collection.id.clone());
        filters = format!("AND r.collection_id = {id}");
        filters.push_str(&build_filters(
            access,
            collection,
            &request.filters,
            &mut arguments,
        )?);
    }
    let kind = match request.kind {
        Some(IntegrationSearchKind::Page) => "AND c.id IS NULL AND r.collection_id IS NULL",
        Some(IntegrationSearchKind::Collection) => "AND c.id IS NOT NULL",
        Some(IntegrationSearchKind::Record) => "AND r.collection_id IS NOT NULL",
        None => "",
    };
    let mut pdf_arguments = include_pdf.then(|| Arguments(arguments.0.clone()));
    let (candidates, candidate_join) = if term.is_empty() {
        (String::new(), "")
    } else {
        let (entries, condition) = matching_entries(term, &mut arguments);
        // An unindexed page is checked directly. This makes an index batch or a
        // rebuild invisible to clients traversing the same content snapshot.
        let candidates = format!(
            ", candidate_pages(id) AS (
                 SELECT e.page_id FROM {entries}
                 JOIN readable_pages live ON live.id = e.page_id
                 JOIN pages current_page ON current_page.id = e.page_id AND current_page.revision = e.revision
                 WHERE {condition}
                 UNION
                 SELECT q.page_id FROM search_pending q
                 JOIN readable_pages live ON live.id = q.page_id
             )",
        );
        (candidates, "JOIN candidate_pages hit ON hit.id = p.id")
    };

    let mut query = db.prepare(&format!(
        "{readable} {candidates}
         SELECT {SEARCH_PAGE}
         FROM readable_pages a JOIN pages p ON p.id = a.id
         {candidate_join}
         {SEARCH_COLLECTIONS}
         WHERE 1=1 {kind} {filters}
         ORDER BY p.updated_at DESC, p.id ASC",
    ))?;
    let mut rows = query.query(params_from_iter(&arguments.0))?;
    let mut load_blocks = db.prepare_cached("SELECT blocks FROM pages WHERE id = ?1")?;
    let mut hits = Vec::new();
    let mut offset = paging.offset;
    while let Some(row) = rows.next()? {
        let page_id: String = row.get(0)?;
        let title: String = row.get(1)?;
        let (source, block_id, excerpt) = if term.is_empty() {
            (IntegrationSearchSource::Metadata, None, None)
        } else if let Some(excerpt) = snippet(&title, term) {
            (IntegrationSearchSource::Title, None, Some(excerpt))
        } else {
            let data: String = load_blocks.query_row([&page_id], |row| row.get(0))?;
            let blocks: Vec<Block> = serde_json::from_str(&data)?;
            let Some((id, excerpt)) = searchable_block(db, access, &page_id, &blocks, term)? else {
                continue;
            };
            (IntegrationSearchSource::Block, Some(id), Some(excerpt))
        };
        if offset > 0 {
            offset -= 1;
            continue;
        }
        hits.push(search_hit(row, source, block_id, excerpt, None)?);
        if hits.len() > paging.limit {
            return Ok(hits);
        }
    }
    if let Some(arguments) = pdf_arguments.as_mut() {
        let (entries, condition) = pdf::matching_entries(term, arguments);
        let original = pdf::MATCHING_ORIGINAL;
        let offset = arguments.bind(i64::try_from(offset).unwrap_or(i64::MAX));
        let limit = paging.limit + 1 - hits.len();
        let mut statement = db.prepare(&format!(
            "{readable}
             SELECT {SEARCH_PAGE}, a.id, a.sha256, e.page_number, a.name, e.original
             FROM {entries} {original}
             JOIN readable_pages allowed ON allowed.id = a.page_id
             JOIN pages p ON p.id = allowed.id
             {SEARCH_COLLECTIONS}
             WHERE {condition} {kind} {filters}
             ORDER BY p.updated_at DESC, p.id, a.id, e.page_number LIMIT {limit} OFFSET {offset}"
        ))?;
        let found = statement.query_map(params_from_iter(&arguments.0), |row| {
            let pdf = PdfTextMatch {
                target: PdfTarget {
                    attachment_id: row.get(6)?,
                    sha256: row.get(7)?,
                    page_number: row.get(8)?,
                },
                name: row.get(9)?,
            };
            let text: String = row.get(10)?;
            search_hit(
                row,
                IntegrationSearchSource::Pdf,
                None,
                snippet(&text, term),
                Some(pdf),
            )
        })?;
        for hit in found {
            hits.push(hit?);
        }
    }
    Ok(hits)
}
