// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::assets::{read_attachments, valid_attachment};
use crate::collections::{for_parent, normalize_values, read_collection};
use crate::model::{name, new_id, valid_icon, valid_id, validate_blocks};
use crate::store::{ensure_active, insert_new_page_with_attachments, new_page, now, read_page};
use crate::{Attachment, Block, Collection, Error, Page, PageCover, PropertyKind, Result, Store};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};

const LIMIT: usize = 128;
const MAX_PAGES: usize = 64;
const MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateSummary {
    pub id: String,
    pub name: String,
    pub collection_id: Option<String>,
    pub page_count: u32,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Template {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) collection_id: Option<String>,
    pub(crate) pages: Vec<TemplatePage>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct TemplatePage {
    pub(crate) id: String,
    pub(crate) parent_id: Option<String>,
    title: String,
    icon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cover: Option<PageCover>,
    pub(crate) blocks: Vec<Block>,
    pub(crate) properties: Option<BTreeMap<String, Value>>,
    pub(crate) attachments: Vec<Attachment>,
}

impl Store {
    pub fn templates(&self, workspace: &str) -> Result<Vec<TemplateSummary>> {
        summaries(&self.connection(workspace)?)
    }

    pub fn save_template(
        &self,
        workspace: &str,
        source: &str,
        label: &str,
        existing_id: Option<&str>,
    ) -> Result<TemplateSummary> {
        let label = name(label, 80, None)?;
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let root = read_page(&tx, source)?;
        ensure_active(&root)?;
        let collection_id = for_parent(&tx, root.info.parent_id.as_deref())?;
        let collection = collection_id
            .as_deref()
            .map(|id| read_collection(&tx, id))
            .transpose()?;
        let items = summaries(&tx)?;
        if let Some(id) = existing_id {
            let old = items
                .iter()
                .find(|item| item.id == id)
                .ok_or_else(template_missing)?;
            if old.collection_id != collection_id {
                return Err(Error::new(
                    "TEMPLATE_SCOPE",
                    "请选择同一数据库或同类页面的模板。",
                ));
            }
        } else if items.len() >= LIMIT {
            return Err(Error::new(
                "TEMPLATE_LIMIT",
                "每个空间最多可保存 128 个模板。",
            ));
        }
        if items.iter().any(|item| {
            item.collection_id == collection_id
                && item.name.to_lowercase() == label.to_lowercase()
                && Some(item.id.as_str()) != existing_id
        }) {
            return Err(Error::new("TEMPLATE_NAME", "此范围内已有同名模板。"));
        }
        let mut pages = Vec::new();
        for id in descendants(&tx, source)? {
            let page = if id == source {
                root.clone()
            } else {
                read_page(&tx, &id)?
            };
            pages.push(TemplatePage {
                id: id.clone(),
                parent_id: if id == source {
                    None
                } else {
                    page.info.parent_id
                },
                title: page.info.title,
                icon: page.info.icon,
                cover: page.cover,
                blocks: page.blocks,
                properties: page.properties,
                attachments: read_attachments(&tx, Some(&id))?,
            });
        }
        let template = Template {
            id: existing_id.map_or_else(new_id, str::to_owned),
            name: label,
            collection_id,
            pages,
        };
        validate_template(&template, collection.as_ref())?;
        self.verify_template_blobs(workspace, &template)?;
        write_template(&tx, &template, true)?;
        tx.commit()?;
        Ok(template.summary())
    }

    pub fn delete_template(&self, workspace: &str, id: &str) -> Result<()> {
        let db = self.connection(workspace)?;
        if db.execute("DELETE FROM templates WHERE id = ?1", [id])? == 0 {
            return Err(template_missing());
        }
        Ok(())
    }

    pub fn apply_template(
        &self,
        workspace: &str,
        id: &str,
        parent: Option<&str>,
        title: Option<&str>,
    ) -> Result<Page> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent) = parent {
            ensure_active(&read_page(&tx, parent)?)?;
        }
        let mut template = read_template(&tx, id)?;
        let target_collection = for_parent(&tx, parent)?;
        if template.collection_id != target_collection {
            return Err(Error::new(
                "TEMPLATE_SCOPE",
                "请在该模板所属的页面或数据库范围中使用。",
            ));
        }
        let collection = target_collection
            .as_deref()
            .map(|id| read_collection(&tx, id))
            .transpose()?;
        validate_template(&template, collection.as_ref())?;
        self.verify_template_blobs(workspace, &template)?;

        let mut id_map = HashMap::new();
        let mut pages = Vec::new();
        for saved in &template.pages {
            let parent = match &saved.parent_id {
                Some(parent_id) => id_map.get(parent_id).map(String::as_str),
                None => parent,
            };
            let title = if saved.parent_id.is_none() {
                title.unwrap_or(&saved.title)
            } else {
                &saved.title
            };
            let page = new_page(parent, title, &saved.icon)?;
            id_map.insert(saved.id.clone(), page.info.id.clone());
            pages.push(page);
        }

        let block_ids: HashMap<_, HashMap<_, _>> = template
            .pages
            .iter()
            .map(|page| {
                (
                    page.id.clone(),
                    crate::references::walk(&page.blocks)
                        .map(|block| (block.id.clone(), new_id()))
                        .collect(),
                )
            })
            .collect();
        let mut root = None;
        for (index, (mut page, mut saved)) in
            pages.into_iter().zip(template.pages.drain(..)).enumerate()
        {
            let mut attachments = HashMap::new();
            for item in &mut saved.attachments {
                let new_id = new_id();
                attachments.insert(item.id.clone(), new_id.clone());
                item.id = new_id;
                item.page_id = page.info.id.clone();
                item.created_at = now();
            }
            reassign_blocks(
                &mut saved.blocks,
                &saved.id,
                &id_map,
                &block_ids,
                &attachments,
            );
            page.blocks = saved.blocks;
            page.cover = saved.cover.map(|mut cover| {
                cover.attachment_id = attachments[&cover.attachment_id].clone();
                cover
            });
            page.properties = match (saved.properties, &collection) {
                (Some(mut values), Some(collection)) => {
                    remap_attachments(collection, &mut values, &attachments)?;
                    for field in collection
                        .properties
                        .iter()
                        .filter(|field| field.kind == PropertyKind::Relation)
                    {
                        if let Some(items) = values.get_mut(&field.id).and_then(Value::as_array_mut)
                        {
                            for item in items {
                                if let Some(mapped) = item.as_str().and_then(|id| id_map.get(id)) {
                                    *item = Value::String(mapped.clone());
                                }
                            }
                        }
                    }
                    crate::collections::relations::remove_deleted_targets(
                        &tx,
                        &collection.properties,
                        &mut values,
                    )?;
                    Some(normalize_values(&collection.properties, values, |id| {
                        Ok(saved
                            .attachments
                            .iter()
                            .any(|attachment| attachment.id == id))
                    })?)
                }
                (None, _) => None,
                _ => return Err(invalid_template()),
            };
            let (inserted, _) = insert_new_page_with_attachments(&tx, page, &saved.attachments)?;
            if index == 0 {
                root = Some(inserted);
            }
        }
        let root = root.ok_or_else(invalid_template)?;
        tx.commit()?;
        Ok(root)
    }

    fn verify_template_blobs(&self, workspace: &str, template: &Template) -> Result<()> {
        let mut seen = HashSet::new();
        for attachment in template_attachments(std::slice::from_ref(template)) {
            if seen.insert(&attachment.sha256) {
                self.copy_blob(workspace, attachment, std::io::sink())?;
            }
        }
        Ok(())
    }
}

fn descendants(db: &Connection, id: &str) -> Result<Vec<String>> {
    let mut query = db.prepare(
        "WITH RECURSIVE branch(id, depth) AS (
             SELECT ?1, 0
             UNION ALL
             SELECT p.id, branch.depth + 1 FROM pages p JOIN branch ON p.parent_id = branch.id
             WHERE p.trash_root_id IS NULL LIMIT ?2
         )
         SELECT p.id, EXISTS(SELECT 1 FROM collections c WHERE c.container_page_id = p.id)
         FROM branch JOIN pages p ON p.id = branch.id ORDER BY branch.depth, p.rowid",
    )?;
    let mut pages = Vec::new();
    for item in query.query_map(params![id, MAX_PAGES as u32 + 1], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
    })? {
        let (id, database) = item?;
        if database {
            return Err(Error::new(
                "TEMPLATE_DATABASE",
                "页面模板可包含子页面。请从不含子数据库的页面或单条记录创建模板。",
            ));
        }
        if pages.len() == MAX_PAGES {
            return Err(Error::new("TEMPLATE_LIMIT", "一个模板最多包含 64 页。"));
        }
        pages.push(id);
    }
    Ok(pages)
}

impl Template {
    pub(crate) fn uses_covers(&self) -> bool {
        self.pages.iter().any(|page| page.cover.is_some())
    }

    fn summary(&self) -> TemplateSummary {
        TemplateSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            collection_id: self.collection_id.clone(),
            page_count: self.pages.len() as u32,
        }
    }
}

fn summaries(db: &Connection) -> Result<Vec<TemplateSummary>> {
    let mut query =
        db.prepare("SELECT id, name, collection_id, page_count FROM templates ORDER BY rowid")?;
    Ok(query
        .query_map([], |row| {
            Ok(TemplateSummary {
                id: row.get(0)?,
                name: row.get(1)?,
                collection_id: row.get(2)?,
                page_count: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn read_template(db: &Connection, id: &str) -> Result<Template> {
    let item = db
        .query_row(
            "SELECT name, collection_id, snapshot FROM templates WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?)),
        )
        .optional()?
        .ok_or_else(template_missing)?;
    Ok(Template {
        id: id.to_owned(),
        name: item.0,
        collection_id: item.1,
        pages: serde_json::from_str(&item.2)?,
    })
}

pub(crate) fn write_template(db: &Connection, item: &Template, update: bool) -> Result<()> {
    if db.execute(
        "INSERT INTO templates (id, name, collection_id, page_count, snapshot) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, collection_id = excluded.collection_id,
           page_count = excluded.page_count, snapshot = excluded.snapshot WHERE ?6",
        params![item.id, item.name, item.collection_id, item.pages.len() as u32, serialize_pages(item)?, update],
    )? == 0 {
        return Err(invalid_template());
    }
    Ok(())
}

fn serialize_pages(item: &Template) -> Result<String> {
    let text = serde_json::to_string(&item.pages)?;
    if text.len() > MAX_BYTES {
        return Err(Error::new(
            "TEMPLATE_LIMIT",
            "模板正文和附件清单最大为 8 MiB。",
        ));
    }
    Ok(text)
}

pub(crate) fn snapshot_templates(db: &Connection) -> Result<Vec<Template>> {
    summaries(db)?
        .iter()
        .map(|item| read_template(db, &item.id))
        .collect()
}

pub(crate) fn collection_templates(db: &Connection, collection: &str) -> Result<Vec<Template>> {
    let mut query = db.prepare("SELECT id FROM templates WHERE collection_id = ?1 ORDER BY id")?;
    let ids = query
        .query_map([collection], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter().map(|id| read_template(db, id)).collect()
}

pub(crate) fn validate_templates(items: &[Template], collections: &[Collection]) -> Result<()> {
    if items.len() > LIMIT {
        return Err(invalid_template());
    }
    let mut ids = HashSet::new();
    let mut names = HashSet::new();
    for item in items {
        if !ids.insert(&item.id)
            || !names.insert((item.collection_id.as_ref(), item.name.to_lowercase()))
        {
            return Err(invalid_template());
        }
        let collection = item
            .collection_id
            .as_deref()
            .and_then(|id| collections.iter().find(|db| db.id == id));
        validate_template(item, collection)?;
    }
    Ok(())
}

pub(crate) fn restore_templates(db: &Connection, templates: &[Template]) -> Result<()> {
    templates
        .iter()
        .try_for_each(|item| write_template(db, item, false))
}

pub(crate) fn fit_merged_templates(db: &Connection, templates: &mut [Template]) -> Result<usize> {
    if templates.is_empty() {
        return Ok(0);
    }
    let existing = summaries(db)?;
    if existing.len() + templates.len() > LIMIT {
        return Err(Error::new(
            "TEMPLATE_LIMIT",
            "合并后每个空间最多可保存 128 个模板。",
        ));
    }
    let mut used: HashSet<_> = existing
        .into_iter()
        .map(|item| (item.collection_id, item.name.to_lowercase()))
        .collect();
    let mut renamed = 0;
    for item in templates {
        if used.insert((item.collection_id.clone(), item.name.to_lowercase())) {
            continue;
        }
        let original = item.name.clone();
        for number in 2..=LIMIT + 1 {
            let suffix = format!("（{number}）");
            let stem: String = original.chars().take(80 - suffix.chars().count()).collect();
            let candidate = format!("{}{suffix}", stem.trim_end());
            if used.insert((item.collection_id.clone(), candidate.to_lowercase())) {
                item.name = candidate;
                renamed += 1;
                break;
            }
        }
    }
    Ok(renamed)
}

pub(crate) fn template_attachments(templates: &[Template]) -> impl Iterator<Item = &Attachment> {
    templates
        .iter()
        .flat_map(|template| &template.pages)
        .flat_map(|page| &page.attachments)
}

fn validate_template(template: &Template, collection: Option<&Collection>) -> Result<()> {
    if !valid_id(&template.id)
        || name(&template.name, 80, None)? != template.name
        || template.collection_id.as_deref() != collection.map(|db| db.id.as_str())
        || template.pages.is_empty()
        || template.pages.len() > MAX_PAGES
    {
        return Err(invalid_template());
    }
    serialize_pages(template)?;
    let mut seen = HashSet::new();
    let mut block_ids = HashSet::new();
    let mut attachment_ids = HashSet::new();
    for (index, page) in template.pages.iter().enumerate() {
        let parent_ok = match &page.parent_id {
            Some(parent) => index != 0 && seen.contains(parent),
            None => index == 0,
        };
        if !valid_id(&page.id)
            || !parent_ok
            || !seen.insert(&page.id)
            || name(&page.title, 200, None)? != page.title
            || !valid_icon(&page.icon)
        {
            return Err(invalid_template());
        }
        if validate_blocks(&page.blocks)?
            .into_iter()
            .any(|id| !block_ids.insert(id))
        {
            return Err(invalid_template());
        }
        let mut keys = HashSet::new();
        for item in &page.attachments {
            if !valid_attachment(item)
                || item.page_id != page.id
                || !attachment_ids.insert(&item.id)
                || !keys.insert((&item.name, &item.sha256))
            {
                return Err(invalid_template());
            }
        }
        for reference in crate::rich_blocks::attachment_references(&page.blocks) {
            if !page
                .attachments
                .iter()
                .any(|item| reference.matches(item, &page.id))
            {
                return Err(invalid_template());
            }
        }
        if let Some(cover) = &page.cover
            && !page
                .attachments
                .iter()
                .any(|item| cover.matches(item, &page.id))
        {
            return Err(invalid_template());
        }
        match (index, &page.properties, collection) {
            (0, Some(values), Some(collection)) => {
                let normalized = normalize_values(&collection.properties, values.clone(), |id| {
                    Ok(page
                        .attachments
                        .iter()
                        .any(|attachment| attachment.id == id))
                })?;
                if normalized != *values {
                    return Err(invalid_template());
                }
            }
            (0, None, Some(_)) | (_, Some(_), _) => return Err(invalid_template()),
            _ => {}
        }
    }
    Ok(())
}

fn remap_attachments(
    collection: &Collection,
    values: &mut BTreeMap<String, Value>,
    attachments: &HashMap<String, String>,
) -> Result<()> {
    for field in collection
        .properties
        .iter()
        .filter(|p| p.kind == PropertyKind::Attachment)
    {
        if let Some(value) = values.get_mut(&field.id) {
            let ids = value.as_array_mut().ok_or_else(invalid_template)?;
            for id in ids {
                let mapped = id
                    .as_str()
                    .and_then(|id| attachments.get(id))
                    .ok_or_else(invalid_template)?;
                *id = Value::String(mapped.clone());
            }
        }
    }
    Ok(())
}

fn reassign_blocks(
    blocks: &mut [Block],
    page_id: &str,
    page_ids: &HashMap<String, String>,
    block_ids: &HashMap<String, HashMap<String, String>>,
    attachment_ids: &HashMap<String, String>,
) {
    for block in blocks {
        block.id = block_ids[page_id][&block.id].clone();
        if let Some(target) = crate::references::block_target(block) {
            if let Some((page, target_block)) = page_ids.get(target.page_id).zip(
                block_ids
                    .get(target.page_id)
                    .and_then(|ids| ids.get(target.block_id)),
            ) {
                block.payload = serde_json::json!({"pageId": page, "blockId": target_block});
            }
        }
        if let Some(id) = crate::rich_blocks::attachment_reference(block)
            .and_then(|reference| attachment_ids.get(reference.id))
        {
            block.payload["attachmentId"] = Value::String(id.clone());
        }
        if block.schema_version == 1 && matches!(block.kind.as_str(), "paragraph" | "heading") {
            if let Some(nodes) = block
                .payload
                .get_mut("inline")
                .and_then(Value::as_array_mut)
            {
                for item in nodes
                    .iter_mut()
                    .filter(|item| item["type"] == "pageReference")
                {
                    if let Some(id) = item["pageId"].as_str().and_then(|id| page_ids.get(id)) {
                        item["pageId"] = Value::String(id.clone());
                    }
                }
            }
        }
        reassign_blocks(
            &mut block.children,
            page_id,
            page_ids,
            block_ids,
            attachment_ids,
        );
    }
}

fn template_missing() -> Error {
    Error::new("TEMPLATE_NOT_FOUND", "找不到此模板。")
}

fn invalid_template() -> Error {
    Error::new("TEMPLATE_FORMAT", "模板包含无效的页面、属性或附件。")
}
