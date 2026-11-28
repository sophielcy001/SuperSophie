// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{Access, IntegrationScope, missing};
use crate::assets::{check_content, hex_sha256, read_attachment, transfer};
use crate::collections::{
    PropertyKind,
    computed::{Engine, cache},
    for_parent, read_collection,
};
use crate::model::valid_id;
use crate::references::{BlockTarget, backlink_page, block_target};
use crate::rich_blocks::image;
use crate::store::{now, read_page, read_page_info};
use crate::{
    Attachment, BacklinkPage, Block, ComputedPage, Page, PropertyDefinition, Result, SearchSnippet,
    Store,
};
use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom},
};

const ASSET_CHUNK_SIZE: u64 = 64 * 1024;

/// Visible property values, whether any were withheld, the withheld property IDs and the schema revision.
type RecordProperties = (
    Option<BTreeMap<String, Value>>,
    bool,
    Vec<String>,
    Option<u32>,
);

#[derive(Debug, Serialize)]
pub struct IntegrationPage {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub icon: String,
    pub page_revision: u32,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema_revision: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, Value>>,
    pub properties_restricted: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub restricted_property_ids: Vec<String>,
    pub blocks: Vec<IntegrationBlock>,
    #[serde(skip)]
    pub etag: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntegrationBlock {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub schema_version: u32,
    pub payload: Value,
    pub children: Vec<IntegrationBlock>,
    #[serde(skip_serializing_if = "is_false")]
    pub restricted: bool,
}

#[derive(Debug, Serialize)]
pub struct LocatedIntegrationBlock {
    pub page_id: String,
    pub page_revision: u32,
    pub block: IntegrationBlock,
}

#[derive(Debug, Serialize)]
pub struct IntegrationReference {
    pub page_id: String,
    pub title: String,
    pub block: IntegrationBlock,
    pub representation_version: String,
    pub computed_at: i64,
    pub stale: bool,
}

pub struct IntegrationAssetDownload {
    attachment: Attachment,
    file: File,
    remaining: u64,
}

impl IntegrationAssetDownload {
    pub fn attachment(&self) -> &Attachment {
        &self.attachment
    }
}

impl IntegrationBlock {
    pub(super) fn find(&self, id: &str) -> Option<&Self> {
        if self.id == id {
            Some(self)
        } else {
            self.children.iter().find_map(|child| child.find(id))
        }
    }

    fn as_block(&self) -> Block {
        Block {
            id: self.id.clone(),
            kind: self.kind.clone(),
            schema_version: self.schema_version,
            payload: self.payload.clone(),
            children: self.children.iter().map(Self::as_block).collect(),
            extra: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Serialize)]
pub struct IntegrationMarkdown {
    pub markdown: String,
    pub losses: Vec<&'static str>,
}

pub fn integration_markdown(
    workspace: &str,
    title: Option<&str>,
    blocks: &[IntegrationBlock],
) -> IntegrationMarkdown {
    let converted: Vec<_> = blocks.iter().map(IntegrationBlock::as_block).collect();
    let (markdown, approximated) =
        crate::markdown::render_projected_markdown(title, &converted, workspace);
    let mut losses = Vec::new();
    if blocks.iter().any(|block| block.restricted) {
        losses.push("restricted_content");
    }
    if approximated {
        losses.push("markdown_approximation");
    }
    IntegrationMarkdown { markdown, losses }
}

fn is_false(value: &bool) -> bool {
    !value
}

impl Store {
    pub fn integration_computed(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<ComputedPage> {
        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction()?;
        let access = Access::authenticate(&tx, token)?;
        access.require_page(&tx, id)?;
        let page = read_page_info(&tx, id)?;
        let reading = Engine::new(&tx, Some(&access)).cached(id)?;
        tx.commit()?;
        if let Some(sequence) = reading.write_at {
            let _ = cache::save(&db, Some(&access), id, sequence, &reading.snapshot);
        }
        access.mark_read_use(&db);
        reading.into_page(id, page.revision)
    }

    pub fn integration_page(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<IntegrationPage> {
        self.integration_read(workspace, token, |db, access| {
            access.require_page(db, id)?;
            project_page(db, access, &read_page(db, id)?)
        })
    }

    pub fn integration_block(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<LocatedIntegrationBlock> {
        self.integration_read(workspace, token, |db, access| {
            let page_id = owner(db, "SELECT page_id FROM block_ids WHERE id = ?1", id)?;
            access.require_page(db, &page_id)?;
            let page = project_page(db, access, &read_page(db, &page_id)?)?;
            let block = page.block(id).ok_or_else(missing)?.clone();
            Ok(LocatedIntegrationBlock {
                page_id: page.id,
                page_revision: page.page_revision,
                block,
            })
        })
    }

    pub fn integration_reference(
        &self,
        workspace: &str,
        token: &str,
        page_id: &str,
        reference_id: &str,
    ) -> Result<IntegrationReference> {
        self.integration_read(workspace, token, |db, access| {
            access.require_page(db, page_id)?;
            let referring = read_page(db, page_id)?;
            let mut projection = Projection::without_references(db, access, false);
            let target = visible_blocks(&referring.blocks)
                .find(|block| block.id == reference_id)
                .and_then(block_target)
                .ok_or_else(missing)?;
            if !projection.can_read(target.page_id)? || !projection.can_reference(target)? {
                return Err(missing());
            }
            let BlockTarget {
                page_id: source_id,
                block_id,
            } = target;
            let source = if source_id == page_id {
                &referring
            } else {
                &read_page(db, source_id)?
            };
            let raw = source_anchors(&source.blocks)
                .find(|block| block.id == block_id)
                .ok_or_else(missing)?;
            // A preview uses the normal field/scope checks. Embedded references
            // remain opaque, so reading a preview can never follow another edge.
            let block = project_reference_block(db, access, source, Some(raw), None)?
                .ok_or_else(missing)?;
            let mut hash = Sha256::new();
            hash.update(serde_json::to_vec(&(
                "block-reference/v1",
                &access.id,
                page_id,
                reference_id,
                referring.info.revision,
                source_id,
                &source.info.title,
                &block,
            ))?);
            Ok(IntegrationReference {
                page_id: source.info.id.clone(),
                title: source.info.title.clone(),
                block,
                representation_version: hex_sha256(&hash.finalize()),
                computed_at: now(),
                stale: false,
            })
        })
    }

    pub fn integration_backlinks(
        &self,
        workspace: &str,
        token: &str,
        page_id: &str,
        after: Option<&str>,
        limit: Option<usize>,
    ) -> Result<BacklinkPage> {
        self.integration_read(workspace, token, |db, access| {
            access.require_page(db, page_id)?;
            backlink_page(
                db,
                page_id,
                after,
                super::cursors::checked_limit(limit)?,
                Some(access),
            )
        })
    }

    pub fn integration_asset(&self, workspace: &str, token: &str, id: &str) -> Result<Attachment> {
        self.integration_read(workspace, token, |db, access| asset(db, access, id))
    }

    pub fn open_integration_asset(
        &self,
        workspace: &str,
        token: &str,
        id: &str,
    ) -> Result<IntegrationAssetDownload> {
        let attachment = self.integration_asset(workspace, token, id)?;
        let mut file = self.open_attachment_blob(workspace, &attachment)?;
        let content = transfer(&mut file, io::sink(), attachment.bytes)?;
        check_content(&content, &attachment)?;
        if self.integration_asset(workspace, token, id)? != attachment {
            return Err(missing());
        }
        Ok(IntegrationAssetDownload {
            remaining: attachment.bytes,
            attachment,
            file,
        })
    }

    pub fn read_integration_asset_chunk(
        &self,
        workspace: &str,
        token: &str,
        download: &mut IntegrationAssetDownload,
    ) -> Result<Option<Vec<u8>>> {
        if download.remaining == 0 {
            return Ok(None);
        }
        let mut db = self.integration_connection(workspace, token)?;
        let tx = db.transaction()?;
        let access = Access::authenticate(&tx, token)?;
        if asset(&tx, &access, &download.attachment.id)? != download.attachment {
            return Err(missing());
        }
        // Keep the authorization view and chunk read in the same transaction.
        let length = download.remaining.min(ASSET_CHUNK_SIZE);
        let mut buffer = vec![0; length as usize];
        download.file.seek(SeekFrom::Start(
            download.attachment.bytes - download.remaining,
        ))?;
        download.file.read_exact(&mut buffer)?;
        tx.commit()?;
        download.remaining -= length;
        Ok(Some(buffer))
    }
}

impl IntegrationPage {
    pub fn block(&self, id: &str) -> Option<&IntegrationBlock> {
        self.blocks.iter().find_map(|block| block.find(id))
    }
}

pub(super) fn asset(db: &Connection, access: &Access, id: &str) -> Result<Attachment> {
    access.require(IntegrationScope::ReadAssets)?;
    let page_id = owner(db, "SELECT page_id FROM attachments WHERE id = ?1", id)?;
    access.require_page(db, &page_id)?;
    read_attachment(db, &page_id, id)
}

fn owner(db: &Connection, sql: &str, id: &str) -> Result<String> {
    db.query_row(sql, [id], |row| row.get(0))
        .optional()?
        .ok_or_else(missing)
}

pub(super) fn project_page(
    db: &Connection,
    access: &Access,
    page: &Page,
) -> Result<IntegrationPage> {
    let mut projection = Projection::new(db, access);
    let info = &page.info;
    let parent_id = match info.parent_id.as_deref() {
        Some(id) if projection.can_read(id)? => Some(id.into()),
        _ => None,
    };
    let (properties, properties_restricted, restricted_property_ids, schema_revision) =
        projection.properties(page, parent_id.as_deref())?;
    let mut output = IntegrationPage {
        id: info.id.clone(),
        parent_id,
        title: info.title.clone(),
        icon: info.icon.clone(),
        page_revision: info.revision,
        updated_at: info.updated_at,
        schema_revision,
        properties,
        properties_restricted,
        restricted_property_ids,
        blocks: projection.blocks(&page.blocks, &info.id)?,
        etag: String::new(),
    };
    let mut hash = Sha256::new();
    hash.update(access.id.as_bytes());
    hash.update(serde_json::to_vec(&output)?);
    output.etag = format!("\"{}\"", hex_sha256(&hash.finalize()));
    Ok(output)
}

struct Projection<'a> {
    db: &'a Connection,
    access: &'a Access,
    pages: HashMap<String, bool>,
    captured_pages: Option<&'a BTreeSet<String>>,
    show_references: bool,
    keep_search_boundaries: bool,
}

pub(super) fn project_reference_block(
    db: &Connection,
    access: &Access,
    source: &Page,
    block: Option<&Block>,
    captured_pages: Option<&BTreeSet<String>>,
) -> Result<Option<IntegrationBlock>> {
    let Some(block) = block else {
        return Ok(None);
    };
    let mut projection = Projection::without_references(db, access, false);
    projection.captured_pages = captured_pages;
    if !projection.can_read(&source.info.id)? {
        return Ok(None);
    }
    let projected = projection.block(block, &source.info.id)?;
    Ok((projected.kind != "restricted").then_some(projected))
}

pub(super) fn project_block(
    db: &Connection,
    access: &Access,
    page_id: &str,
    block: &Block,
) -> Result<IntegrationBlock> {
    Projection::new(db, access).block(block, page_id)
}

pub(super) fn searchable_block(
    db: &Connection,
    access: &Access,
    page_id: &str,
    blocks: &[Block],
    term: &str,
) -> Result<Option<(String, SearchSnippet)>> {
    let mut projection = Projection::without_references(db, access, true);
    for block in blocks {
        if let Some(found) = projected_match(&projection.block(block, page_id)?, term) {
            return Ok(Some(found));
        }
    }
    Ok(None)
}

fn projected_match(block: &IntegrationBlock, term: &str) -> Option<(String, SearchSnippet)> {
    use crate::search::snippet;
    let matched = match block.kind.as_str() {
        "codeBlock" => block.payload["text"]
            .as_str()
            .and_then(|text| snippet(text, term)),
        "attachmentImage" => block.payload["alt"]
            .as_str()
            .and_then(|alt| snippet(alt, term)),
        "toggleBlock" => block.payload["title"]
            .as_str()
            .and_then(|text| snippet(text, term)),
        "mathBlock" => block.payload["latex"]
            .as_str()
            .and_then(|text| snippet(text, term)),
        "paragraph" | "heading" => {
            let inline = block.payload["inline"].as_array()?;
            let mut parts = inline.split(|item| item["type"] == "restricted");
            parts.find_map(|part| {
                let text = part
                    .iter()
                    .filter_map(|item| match item["type"].as_str() {
                        Some("text") => item["text"].as_str(),
                        Some("pageReference") => item["label"].as_str(),
                        Some("hardBreak") => Some("\n"),
                        _ => None,
                    })
                    .collect::<String>();
                snippet(&text, term)
            })
        }
        _ => None,
    };
    matched
        .map(|excerpt| (block.id.clone(), excerpt))
        .or_else(|| {
            block
                .children
                .iter()
                .find_map(|child| projected_match(child, term))
        })
}

pub(super) fn can_read_property(access: &Access, kind: PropertyKind) -> bool {
    match kind {
        PropertyKind::Attachment => access.scopes.contains(&IntegrationScope::ReadAssets),
        PropertyKind::Text
        | PropertyKind::Number
        | PropertyKind::Checkbox
        | PropertyKind::Date
        | PropertyKind::DateTime
        | PropertyKind::Status
        | PropertyKind::Select
        | PropertyKind::MultiSelect
        | PropertyKind::Url
        | PropertyKind::Relation
        | PropertyKind::Formula
        | PropertyKind::Rollup
        | PropertyKind::CreatedAt
        | PropertyKind::UpdatedAt => true,
    }
}

pub(super) fn project_record_properties(
    db: &Connection,
    access: &Access,
    schema: &[PropertyDefinition],
    values: &BTreeMap<String, Value>,
) -> Result<(BTreeMap<String, Value>, bool, Vec<String>)> {
    let mut visible: BTreeMap<_, _> = schema
        .iter()
        .filter(|field| {
            !matches!(
                field.kind,
                PropertyKind::CreatedAt | PropertyKind::UpdatedAt
            ) && !field.kind.computed()
                && can_read_property(access, field.kind)
        })
        .filter_map(|field| {
            values
                .get(&field.id)
                .map(|value| (field.id.clone(), value.clone()))
        })
        .collect();
    let mut restricted_property_ids = Vec::new();
    let mut readability = HashMap::new();
    for field in schema
        .iter()
        .filter(|field| field.kind == PropertyKind::Relation)
    {
        let Some(value) = visible.get_mut(&field.id) else {
            continue;
        };
        let mut members = Vec::new();
        let raw = crate::collections::relations::members(Some(value))?;
        for id in &raw {
            let allowed = match readability.get(id) {
                Some(&allowed) => allowed,
                None => {
                    let allowed = access.can_read(db, id)?;
                    readability.insert(id.clone(), allowed);
                    allowed
                }
            };
            if allowed {
                members.push(Value::String(id.clone()));
            }
        }
        if raw.len() != members.len() {
            restricted_property_ids.push(field.id.clone());
        }
        *value = Value::Array(members);
    }
    let restricted = visible.len() != values.len() || !restricted_property_ids.is_empty();
    Ok((visible, restricted, restricted_property_ids))
}

impl<'a> Projection<'a> {
    fn without_references(
        db: &'a Connection,
        access: &'a Access,
        keep_search_boundaries: bool,
    ) -> Self {
        Self {
            db,
            access,
            pages: HashMap::new(),
            captured_pages: None,
            show_references: false,
            keep_search_boundaries,
        }
    }

    fn new(db: &'a Connection, access: &'a Access) -> Self {
        Self {
            show_references: true,
            ..Self::without_references(db, access, false)
        }
    }

    fn can_reference(&self, target: BlockTarget<'_>) -> Result<bool> {
        let mut query = self.db.prepare_cached(
            "SELECT EXISTS(SELECT 1 FROM block_ids
              WHERE id = ?1 AND page_id = ?2 AND reference_anchor BETWEEN 1 AND ?3)",
        )?;
        Ok(query.query_row(
            (
                target.block_id,
                target.page_id,
                1 + i32::from(self.access.reads_assets()),
            ),
            |row| row.get(0),
        )?)
    }

    fn can_read(&mut self, id: &str) -> Result<bool> {
        if let Some(pages) = self.captured_pages {
            return Ok(pages.contains(id));
        }
        if let Some(&allowed) = self.pages.get(id) {
            return Ok(allowed);
        }
        let allowed = self.access.can_read(self.db, id)?;
        self.pages.insert(id.into(), allowed);
        Ok(allowed)
    }

    fn properties(&self, page: &Page, visible_parent: Option<&str>) -> Result<RecordProperties> {
        let Some(values) = &page.properties else {
            return Ok((None, false, vec![], None));
        };
        let Some(collection_id) = for_parent(self.db, visible_parent)? else {
            return Ok((None, true, vec![], None));
        };
        let schema = read_collection(self.db, &collection_id)?;
        let (visible, restricted, fields) =
            project_record_properties(self.db, self.access, &schema.properties, values)?;
        Ok((
            Some(visible),
            restricted,
            fields,
            Some(schema.schema_revision),
        ))
    }

    fn blocks(&mut self, blocks: &[Block], page_id: &str) -> Result<Vec<IntegrationBlock>> {
        blocks
            .iter()
            .map(|block| self.block(block, page_id))
            .collect()
    }

    fn block(&mut self, block: &Block, page_id: &str) -> Result<IntegrationBlock> {
        let restricted = || IntegrationBlock {
            id: block.id.clone(),
            kind: "restricted".into(),
            schema_version: 1,
            payload: json!({}),
            children: vec![],
            restricted: true,
        };
        if block.schema_version != 1 || !block.extra.is_empty() {
            return Ok(restricted());
        }
        let (payload, partial) = match block.kind.as_str() {
            "paragraph" | "heading" => {
                let Some(visible) = self.inline_block(block)? else {
                    return Ok(restricted());
                };
                visible
            }
            "blockReference" => {
                let Some(target) = block_target(block) else {
                    return Ok(restricted());
                };
                if !self.show_references
                    || !self.can_read(target.page_id)?
                    || !self.can_reference(target)?
                {
                    return Ok(restricted());
                }
                (block.payload.clone(), false)
            }
            "collectionView" => {
                if !self.show_references {
                    return Ok(restricted());
                }
                let Some((collection, view)) = crate::collections::views::view_target(block) else {
                    return Ok(restricted());
                };
                let target: Option<String> = self.db.query_row(
                    "SELECT container_page_id FROM collections WHERE id=?1
                     AND EXISTS (SELECT 1 FROM json_each(views) WHERE json_extract(value,'$.id')=?2)",
                    (collection, view), |row| row.get(0),
                ).optional()?;
                if !match target {
                    Some(page) => self.can_read(&page)?,
                    None => false,
                } {
                    return Ok(restricted());
                }
                (block.payload.clone(), false)
            }
            // A bookmark's preview image is shown only with asset access, like other images.
            "bookmark" if crate::rich_blocks::bookmark(block).is_some() => {
                let mut payload = block.payload.clone();
                if let Some(reference) = crate::rich_blocks::bookmark_image(block) {
                    let readable = self.access.scopes.contains(&IntegrationScope::ReadAssets)
                        && matches!(read_attachment(self.db, page_id, reference.id),
                            Ok(attachment) if reference.matches(&attachment, page_id));
                    if !readable && let Some(fields) = payload.as_object_mut() {
                        fields.remove("attachmentId");
                        fields.remove("mediaType");
                    }
                }
                (payload, false)
            }
            "attachmentImage" | "attachmentFile" => {
                let Some(reference) = crate::rich_blocks::attachment_reference(block) else {
                    return Ok(restricted());
                };
                if !self.access.scopes.contains(&IntegrationScope::ReadAssets) {
                    return Ok(restricted());
                }
                match read_attachment(self.db, page_id, reference.id) {
                    Ok(attachment) if reference.matches(&attachment, page_id) => {}
                    Ok(_) => return Ok(restricted()),
                    Err(error) if error.code == "ATTACHMENT_NOT_FOUND" => return Ok(restricted()),
                    Err(error) => return Err(error),
                }
                (block.payload.clone(), false)
            }
            _ if known_payload(block) => (block.payload.clone(), false),
            _ => return Ok(restricted()),
        };
        let children = self.blocks(&block.children, page_id)?;
        Ok(IntegrationBlock {
            id: block.id.clone(),
            kind: block.kind.clone(),
            schema_version: block.schema_version,
            payload,
            restricted: partial || children.iter().any(|child| child.restricted),
            children,
        })
    }

    fn inline_block(&mut self, block: &Block) -> Result<Option<(Value, bool)>> {
        let Some(items) = reference_inline(block) else {
            return Ok(None);
        };

        let mut hidden = false;
        let mut visible = Vec::with_capacity(items.len());
        for item in items {
            if item["type"] == "pageReference"
                && !self.can_read(item["pageId"].as_str().unwrap_or_default())?
            {
                if !hidden || self.keep_search_boundaries {
                    visible.push(json!({"type": "restricted", "label": "不可访问"}));
                }
                hidden = true;
            } else {
                visible.push(item.clone());
            }
        }
        let mut payload = block.payload.clone();
        payload["inline"] = Value::Array(visible);
        Ok(Some((payload, hidden)))
    }
}

pub(crate) fn reference_inline(block: &Block) -> Option<&[Value]> {
    // Paragraphs and headings may hold child blocks (indented under them, as in Notion);
    // those are read on their own.
    if block.schema_version != 1 || !block.extra.is_empty() {
        return None;
    }
    let fields = block.payload.as_object()?;
    // A whole-block color (Notion's "red" or "red_background") is the one optional field.
    let color = match block.payload.get("color") {
        None => 0,
        Some(value) if value.as_str().is_some_and(crate::rich_blocks::block_color) => 1,
        Some(_) => return None,
    };
    match block.kind.as_str() {
        "heading"
            if fields.len() == 2 + color
                && block.payload["level"]
                    .as_u64()
                    .is_some_and(|level| (1..=3).contains(&level)) => {}
        "paragraph" if fields.len() == 1 + color => {}
        _ => return None,
    };
    let items = block.payload["inline"].as_array()?;
    items
        .iter()
        .all(|item| {
            let Some(fields) = item.as_object() else {
                return false;
            };
            match item["type"].as_str() {
                Some("pageReference") => {
                    fields.len() == 3
                        && item["label"].is_string()
                        && item["pageId"].as_str().is_some_and(valid_id)
                }
                Some("text") => valid_text(item),
                Some("hardBreak") => fields.len() == 1,
                _ => false,
            }
        })
        .then_some(items)
}

pub(crate) fn source_anchors(blocks: &[Block]) -> impl Iterator<Item = &Block> {
    visible_blocks(blocks)
        .filter(|block| !matches!(block.kind.as_str(), "blockReference" | "tableOfContents"))
}

pub(crate) fn visible_blocks(blocks: &[Block]) -> impl Iterator<Item = &Block> {
    let mut stack = vec![blocks.iter()];
    std::iter::from_fn(move || {
        while let Some(level) = stack.last_mut() {
            let Some(block) = level.next() else {
                stack.pop();
                continue;
            };
            if block.schema_version != 1 || !block.extra.is_empty() {
                continue;
            }
            let visible = match block.kind.as_str() {
                "heading" | "paragraph" => reference_inline(block).is_some(),
                "attachmentImage" => image(block).is_some(),
                "attachmentFile" => crate::rich_blocks::attachment_file(block).is_some(),
                "blockReference" => block_target(block).is_some(),
                _ => known_payload(block),
            };
            if visible {
                if !block.children.is_empty() {
                    stack.push(block.children.iter());
                }
                return Some(block);
            }
        }
        None
    })
}

fn valid_text(item: &Value) -> bool {
    let Some(fields) = item.as_object() else {
        return false;
    };
    item["text"].is_string()
        && fields.iter().all(|(key, value)| match key.as_str() {
            "type" | "text" => true,
            "marks" => value
                .as_array()
                .is_some_and(|marks| marks.iter().all(valid_mark)),
            _ => false,
        })
}

fn valid_mark(mark: &Value) -> bool {
    let Some(fields) = mark.as_object() else {
        return false;
    };
    match mark["type"].as_str() {
        Some("bold" | "italic" | "strike" | "underline" | "code") => fields.len() == 1,
        // A comment with an optional thread of up to 50 replies, as in Notion.
        Some("comment") => {
            let replies = match mark.get("replies") {
                None => Some(0),
                Some(Value::Array(items))
                    if !items.is_empty()
                        && items.len() <= 50
                        && items
                            .iter()
                            .all(|item| crate::rich_blocks::comment_text(item).is_some()) =>
                {
                    Some(1)
                }
                Some(_) => None,
            };
            replies.is_some_and(|extra| fields.len() == 2 + extra)
                && crate::rich_blocks::comment_text(&mark["text"]).is_some()
        }
        Some("textColor" | "backgroundColor") => {
            fields.len() == 2
                && mark["color"]
                    .as_str()
                    .is_some_and(crate::rich_blocks::inline_color)
        }
        Some("link") => {
            fields.len() == 2 && mark["href"].as_str().and_then(crate::web_url).is_some()
        }
        _ => false,
    }
}

fn known_payload(block: &Block) -> bool {
    let Some(fields) = block.payload.as_object() else {
        return false;
    };
    match block.kind.as_str() {
        "toggleBlock" => crate::rich_blocks::toggle_title(block).is_some(),
        "callout" => crate::rich_blocks::callout_tone(block).is_some(),
        "mathBlock" => crate::rich_blocks::latex(block).is_some(),
        "tableOfContents" => crate::rich_blocks::is_outline(block),
        "horizontalRule" => fields.is_empty() && block.children.is_empty(),
        "blockquote" | "bulletList" | "listItem" | "taskList" | "table" | "tableRow" => {
            fields.is_empty()
        }
        // Notion-style indentation (Tab) and columns hold other blocks and have no data of their own.
        "indentBlock" | "columnList" => fields.is_empty() && !block.children.is_empty(),
        // A column may carry its relative width from dragging the gap between columns.
        "column" => {
            !block.children.is_empty()
                && fields.iter().all(|(key, value)| {
                    // Hundredths of an even share (100); whole numbers stay exact in JavaScript.
                    key == "width"
                        && value
                            .as_u64()
                            .is_some_and(|width| (10..=1000).contains(&width))
                })
        }
        "bookmark" => crate::rich_blocks::bookmark(block).is_some(),
        "codeBlock" => {
            block.children.is_empty()
                && block.payload["text"].is_string()
                && fields.iter().all(|(key, value)| {
                    matches!(key.as_str(), "text" | "language") && value.is_string()
                })
        }
        "orderedList" => fields.iter().all(|(key, value)| match key.as_str() {
            "start" => value.is_i64(),
            "type" => matches!(value.as_str(), Some("1" | "a" | "A" | "i" | "I")),
            _ => false,
        }),
        "taskItem" => fields
            .iter()
            .all(|(key, value)| key == "checked" && value.is_boolean()),
        "tableCell" | "tableHeader" => fields.iter().all(|(key, value)| match key.as_str() {
            "rowspan" | "colspan" => value.is_u64(),
            "colwidth" => value
                .as_array()
                .is_some_and(|widths| widths.iter().all(Value::is_u64)),
            "align" => crate::rich_blocks::table_alignment(value).is_some(),
            _ => false,
        }),
        _ => false,
    }
}
