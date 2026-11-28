// SPDX-License-Identifier: AGPL-3.0-or-later

use super::ArchiveData;
use crate::collections::{
    CollectionSnapshot, TITLE_FIELD, relations::members, remap_formula_fields, views::view_target,
};
use crate::imports::check_cancel;
use crate::integrations::{reference_inline, visible_blocks};
use crate::model::new_id;
use crate::references::{block_target, walk};
use crate::rich_blocks::attachment_reference;
use crate::templates::Template;
use crate::{
    Attachment, Block, Calculation, Collection, CollectionView, Error, Page, PageCover,
    PropertyDefinition, PropertyKind, Result, ViewLayout,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    sync::atomic::AtomicBool,
};

type Ids = HashMap<String, String>;
type Values = BTreeMap<String, Value>;

#[derive(Default)]
struct PageIds {
    pages: Ids,
    blocks: HashMap<String, Ids>,
    attachments: Ids,
}

impl PageIds {
    fn index_page(&mut self, page: &str, blocks: &[Block]) {
        self.pages.entry(page.to_owned()).or_insert_with(new_id);
        let ids = self.blocks.entry(page.to_owned()).or_default();
        for block in walk(blocks) {
            ids.entry(block.id.clone()).or_insert_with(new_id);
        }
    }

    fn index_attachments(&mut self, attachments: &[Attachment]) {
        self.attachments
            .extend(attachments.iter().map(|item| (item.id.clone(), new_id())));
    }

    fn block(&self, page: &str, block: &str) -> Option<&String> {
        self.blocks.get(page)?.get(block)
    }

    fn remap_attachment(&self, attachment: &mut Attachment) -> Result<()> {
        attachment.page_id = mapped(&self.pages, &attachment.page_id)?;
        attachment.id = mapped(&self.attachments, &attachment.id)?;
        Ok(())
    }

    fn remap_cover(&self, cover: &mut Option<PageCover>) -> Result<()> {
        if let Some(cover) = cover {
            cover.attachment_id = mapped(&self.attachments, &cover.attachment_id)?;
        }
        Ok(())
    }
}

struct CollectionIds {
    id: String,
    fields: HashMap<String, FieldIds>,
    views: Ids,
}

struct FieldIds {
    id: String,
    kind: PropertyKind,
    options: Ids,
    relation: Option<String>,
}

impl CollectionIds {
    fn index(collection: &Collection) -> Self {
        Self {
            id: new_id(),
            fields: collection
                .properties
                .iter()
                .map(|field| {
                    (
                        field.id.clone(),
                        FieldIds {
                            id: new_id(),
                            kind: field.kind,
                            options: field
                                .options
                                .iter()
                                .map(|option| (option.id.clone(), new_id()))
                                .collect(),
                            relation: field
                                .relation
                                .as_ref()
                                .map(|relation| relation.collection_id.clone()),
                        },
                    )
                })
                .collect(),
            views: collection
                .views
                .iter()
                .map(|view| (view.id.clone(), new_id()))
                .collect(),
        }
    }

    fn field(&self, id: &str) -> Result<&FieldIds> {
        self.fields.get(id).ok_or_else(invalid_ids)
    }

    fn field_id(&self, id: &str) -> Result<String> {
        if id == TITLE_FIELD {
            Ok(id.to_owned())
        } else {
            Ok(self.field(id)?.id.clone())
        }
    }

    fn remap_view(&self, view: &mut CollectionView) -> Result<()> {
        view.id = mapped(&self.views, &view.id)?;
        if let Some(filter) = &mut view.settings.filter {
            if let Some(field) = self.fields.get(&filter.field_id)
                && field.kind.has_options()
                && let Some(value) = &mut filter.value
            {
                remap_value_id(value, &field.options)?;
            }
            filter.field_id = self.field_id(&filter.field_id)?;
        }
        if let Some(sort) = &mut view.settings.sort {
            sort.field_id = self.field_id(&sort.field_id)?;
        }
        if let Some(field) = &mut view.settings.group_by {
            *field = self.field_id(field)?;
        }
        match &mut view.layout {
            Some(ViewLayout::Calendar {
                date_field,
                end_field,
            }) => {
                *date_field = self.field_id(date_field)?;
                if let Some(end) = end_field {
                    *end = self.field_id(end)?;
                }
            }
            Some(ViewLayout::Gallery {
                cover_field: Some(field),
            }) => *field = self.field_id(field)?,
            Some(ViewLayout::Timeline {
                start_field,
                end_field,
                ..
            }) => {
                *start_field = self.field_id(start_field)?;
                *end_field = self.field_id(end_field)?;
            }
            _ => (),
        }
        Ok(())
    }
}

struct WorkspaceIds {
    pages: PageIds,
    collections: HashMap<String, CollectionIds>,
    deleted_collections: Ids,
    memberships: Ids,
}

impl WorkspaceIds {
    fn collection(&self, id: &str) -> Result<&CollectionIds> {
        self.collections.get(id).ok_or_else(invalid_ids)
    }

    fn collection_id(&self, id: &str) -> Result<String> {
        if let Some(collection) = self.collections.get(id) {
            Ok(collection.id.clone())
        } else {
            mapped(&self.deleted_collections, id)
        }
    }

    fn remap_databases(
        &self,
        snapshot: &mut CollectionSnapshot,
        cancel: &AtomicBool,
    ) -> Result<()> {
        for collection in &mut snapshot.definitions {
            check_cancel(cancel)?;
            let ids = self.collection(&collection.id)?;
            collection.id = ids.id.clone();
            collection.container_page_id =
                mapped(&self.pages.pages, &collection.container_page_id)?;
            for property in &mut collection.properties {
                let field = ids.field(&property.id)?;
                property.id = field.id.clone();
                for option in &mut property.options {
                    option.id = mapped(&field.options, &option.id)?;
                }
                if let Some(relation) = &mut property.relation {
                    if let Some(target) = self.collections.get(&relation.collection_id)
                        && let Some(inverse) = &mut relation.inverse_property_id
                    {
                        *inverse = target.field_id(inverse)?;
                    }
                    relation.collection_id = self.collection_id(&relation.collection_id)?;
                }
                match &mut property.calculation {
                    Some(Calculation::Formula { expression }) => {
                        *expression =
                            remap_formula_fields(expression, |field| ids.field_id(field).ok())?;
                    }
                    Some(Calculation::Rollup {
                        relation_property_id,
                        target_property_id,
                        ..
                    }) => {
                        if let Some(property) = target_property_id {
                            let target = ids
                                .field(relation_property_id)?
                                .relation
                                .as_deref()
                                .ok_or_else(invalid_ids)?;
                            if let Some(target) = self.collections.get(target) {
                                *property = target.field_id(property)?;
                            }
                        }
                        *relation_property_id = ids.field_id(relation_property_id)?;
                    }
                    None => (),
                }
            }
            for view in &mut collection.views {
                ids.remap_view(view)?;
            }
        }
        for record in &mut snapshot.records {
            record.page_id = mapped(&self.pages.pages, &record.page_id)?;
            record.collection_id = self.collection(&record.collection_id)?.id.clone();
        }
        for id in &mut snapshot.tombstones.collections {
            *id = mapped(&self.deleted_collections, id)?;
        }
        for record in &mut snapshot.tombstones.records {
            record.page_id = mapped(&self.pages.pages, &record.page_id)?;
            record.collection_id = self.collection_id(&record.collection_id)?;
        }
        Ok(())
    }

    fn remap_values(
        &self,
        values: &mut Values,
        collection_id: &str,
        local: &PageIds,
        template_record: Option<&str>,
        historical: &[PropertyDefinition],
    ) -> Result<()> {
        let collection = self.collection(collection_id)?;
        let mut remapped = Values::new();
        for (field_id, mut value) in std::mem::take(values) {
            let field = collection.field(&field_id)?;
            let kind = historical
                .iter()
                .find(|item| item.id == field_id)
                .map_or(field.kind, |item| item.kind);
            match kind {
                PropertyKind::Select | PropertyKind::Status => {
                    remap_value_id(&mut value, &field.options)?
                }
                PropertyKind::MultiSelect => remap_array(&mut value, &field.options)?,
                PropertyKind::Attachment => remap_array(&mut value, &local.attachments)?,
                PropertyKind::Relation => {
                    let target_collection = field.relation.as_deref().ok_or_else(invalid_ids)?;
                    let mut targets = BTreeSet::new();
                    for page in members(Some(&value))? {
                        let page = page.as_str();
                        let id = if Some(page) == template_record
                            && collection_id == target_collection
                        {
                            mapped(&local.pages, page)?
                        } else {
                            if self.memberships.get(page).map(String::as_str)
                                != Some(target_collection)
                            {
                                return Err(Error::new(
                                    "INVALID_BUNDLE",
                                    "模板的关系属性引用了包中缺失或不属于目标数据库的记录。",
                                ));
                            }
                            mapped(&self.pages.pages, page)?
                        };
                        targets.insert(id);
                    }
                    value = serde_json::to_value(targets)?;
                }
                _ => (),
            }
            remapped.insert(field.id.clone(), value);
        }
        *values = remapped;
        Ok(())
    }
}

#[derive(Hash, PartialEq, Eq)]
enum MissingTarget {
    Page(String),
    Block(String, String),
    Collection(String),
    View(String, String),
}

#[derive(Default)]
struct LinkReport {
    missing: HashMap<MissingTarget, String>,
    opaque: HashSet<String>,
}

impl LinkReport {
    fn page(&mut self, local: &PageIds, global: &PageIds, old: &str) -> String {
        local
            .pages
            .get(old)
            .or_else(|| global.pages.get(old))
            .cloned()
            .unwrap_or_else(|| {
                self.missing
                    .entry(MissingTarget::Page(old.to_owned()))
                    .or_insert_with(new_id)
                    .clone()
            })
    }

    fn block(&mut self, owner: &PageIds, page: &str, block: &str) -> String {
        owner.block(page, block).cloned().unwrap_or_else(|| {
            self.missing
                .entry(MissingTarget::Block(page.to_owned(), block.to_owned()))
                .or_insert_with(new_id)
                .clone()
        })
    }

    fn remap_blocks(
        &mut self,
        blocks: &mut [Block],
        owner: &str,
        local: &PageIds,
        global: &WorkspaceIds,
    ) -> Result<()> {
        for block in blocks {
            block.id = local
                .block(owner, &block.id)
                .cloned()
                .ok_or_else(invalid_ids)?;
            if let Some(target) = block_target(block) {
                let page = self.page(local, &global.pages, target.page_id);
                let namespace = if local.pages.contains_key(target.page_id) {
                    local
                } else {
                    &global.pages
                };
                let anchor = self.block(namespace, target.page_id, target.block_id);
                block.payload["pageId"] = Value::String(page);
                block.payload["blockId"] = Value::String(anchor);
            } else if let Some(reference) = attachment_reference(block) {
                block.payload["attachmentId"] =
                    Value::String(mapped(&local.attachments, reference.id)?);
            } else if let Some((collection, view)) = view_target(block) {
                let ids = global.collections.get(collection);
                let collection_id = global.collection_id(collection).unwrap_or_else(|_| {
                    self.missing
                        .entry(MissingTarget::Collection(collection.to_owned()))
                        .or_insert_with(new_id)
                        .clone()
                });
                let view_id = ids
                    .and_then(|ids| ids.views.get(view))
                    .cloned()
                    .unwrap_or_else(|| {
                        self.missing
                            .entry(MissingTarget::View(collection.to_owned(), view.to_owned()))
                            .or_insert_with(new_id)
                            .clone()
                    });
                block.payload["collectionId"] = Value::String(collection_id);
                block.payload["viewId"] = Value::String(view_id);
            } else if reference_inline(block).is_some() {
                if let Some(inline) = block.payload["inline"].as_array_mut() {
                    for item in inline {
                        if item["type"] == "pageReference"
                            && let Some(id) = item["pageId"].as_str()
                        {
                            item["pageId"] = Value::String(self.page(local, &global.pages, id));
                        }
                    }
                }
            } else if visible_blocks(std::slice::from_ref(block)).next().is_none() {
                self.opaque.insert(block.id.clone());
            }
            self.remap_blocks(&mut block.children, owner, local, global)?;
        }
        Ok(())
    }

    fn remap_template(&mut self, template: &mut Template, global: &WorkspaceIds) -> Result<()> {
        let mut local = PageIds::default();
        for page in &template.pages {
            local.index_page(&page.id, &page.blocks);
            local.index_attachments(&page.attachments);
        }
        for page in &mut template.pages {
            self.remap_blocks(&mut page.blocks, &page.id, &local, global)?;
            if let Some(values) = &mut page.properties {
                let collection = template.collection_id.as_deref().ok_or_else(invalid_ids)?;
                global.remap_values(values, collection, &local, Some(&page.id), &[])?;
            }
            for attachment in &mut page.attachments {
                local.remap_attachment(attachment)?;
            }
            local.remap_cover(&mut page.cover)?;
            if let Some(parent) = &mut page.parent_id {
                *parent = mapped(&local.pages, parent)?;
            }
            page.id = mapped(&local.pages, &page.id)?;
        }
        if let Some(collection) = &mut template.collection_id {
            *collection = global.collection(collection)?.id.clone();
        }
        template.id = new_id();
        Ok(())
    }
}

pub(super) fn remap(
    data: &mut ArchiveData,
    root: Page,
    notices: &mut Vec<String>,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut ids = WorkspaceIds {
        pages: PageIds::default(),
        collections: HashMap::new(),
        deleted_collections: Ids::new(),
        memberships: Ids::new(),
    };
    if let Some(databases) = &data.databases {
        for collection in &databases.definitions {
            check_cancel(cancel)?;
            ids.collections
                .insert(collection.id.clone(), CollectionIds::index(collection));
        }
        ids.memberships.extend(
            databases
                .records
                .iter()
                .map(|record| (record.page_id.clone(), record.collection_id.clone())),
        );
        for id in &databases.tombstones.collections {
            ids.deleted_collections.insert(id.clone(), new_id());
        }
        for record in &databases.tombstones.records {
            ids.pages.pages.insert(record.page_id.clone(), new_id());
            ids.memberships
                .insert(record.page_id.clone(), record.collection_id.clone());
        }
    }
    let content = &mut data.content;
    for page in content.pages.iter().chain(&content.revisions) {
        check_cancel(cancel)?;
        ids.pages.index_page(&page.info.id, &page.blocks);
        if !page.historical_properties.is_empty() {
            let collection = ids.memberships.get(&page.info.id).ok_or_else(invalid_ids)?;
            let fields = &mut ids
                .collections
                .get_mut(collection)
                .ok_or_else(invalid_ids)?
                .fields;
            for source in &page.historical_properties {
                let field = fields.get_mut(&source.id).ok_or_else(invalid_ids)?;
                for option in &source.options {
                    field
                        .options
                        .entry(option.id.clone())
                        .or_insert_with(new_id);
                }
            }
        }
    }
    ids.pages.index_attachments(&data.attachments);
    let mut links = LinkReport::default();
    for page in content.pages.iter_mut().chain(&mut content.revisions) {
        check_cancel(cancel)?;
        let old = &page.info.id;
        links.remap_blocks(&mut page.blocks, old, &ids.pages, &ids)?;
        if let Some(values) = &mut page.properties {
            let collection = ids.memberships.get(old).ok_or_else(invalid_ids)?;
            ids.remap_values(
                values,
                collection,
                &ids.pages,
                None,
                &page.historical_properties,
            )?;
            let collection = ids.collection(collection)?;
            for source in &mut page.historical_properties {
                let field = collection.field(&source.id)?;
                source.id = field.id.clone();
                for option in &mut source.options {
                    option.id = mapped(&field.options, &option.id)?;
                }
            }
        }
        ids.pages.remap_cover(&mut page.cover)?;
        page.info.id = mapped(&ids.pages.pages, old)?;
        page.info.parent_id = Some(match &page.info.parent_id {
            Some(parent) => links.page(&ids.pages, &ids.pages, parent),
            None => root.info.id.clone(),
        });
        if let Some(source) = &mut page.info.trash_root_id {
            *source = links.page(&ids.pages, &ids.pages, source);
        }
    }
    for attachment in &mut data.attachments {
        ids.pages.remap_attachment(attachment)?;
    }
    if let Some(databases) = &mut data.databases {
        ids.remap_databases(databases, cancel)?;
    }
    if let Some(library) = &mut data.library {
        for template in &mut library.templates {
            check_cancel(cancel)?;
            links.remap_template(template, &ids)?;
        }
        for receipt in &mut library.import_receipts {
            receipt.output_page_id = mapped(&ids.pages.pages, &receipt.output_page_id)?;
            receipt.destination = match receipt.kind.as_str() {
                "csvExisting" => ids.collection(&receipt.destination)?.id.clone(),
                _ if receipt.destination.is_empty() => root.info.id.clone(),
                _ => mapped(&ids.pages.pages, &receipt.destination)?,
            };
        }
        let navigation = &mut library.navigation;
        if navigation.inbox_page_id.take().is_some() || !navigation.recent_pages.is_empty() {
            notices.push("已沿用目标空间的收集箱和最近访问。".into());
        }
        navigation.recent_pages.clear();
        for favorite in &mut navigation.favorites {
            favorite.page_id = mapped(&ids.pages.pages, &favorite.page_id)?;
        }
    }
    if !links.missing.is_empty() {
        let count = links.missing.len();
        notices.push(format!("{count} 个引用目标缺失，相关链接仍不可用。"));
    }
    if !links.opaque.is_empty() {
        let count = links.opaque.len();
        notices.push(format!("{count} 个未知块保留了原始负载。"));
    }
    content.pages.insert(0, root.clone());
    content.revisions.push(root);
    Ok(())
}

fn invalid_ids() -> Error {
    Error::new("INVALID_BUNDLE", "工作空间包中的对象标识与其引用不一致。")
}

fn mapped(ids: &Ids, old: &str) -> Result<String> {
    ids.get(old).cloned().ok_or_else(invalid_ids)
}

fn remap_value_id(value: &mut Value, ids: &Ids) -> Result<()> {
    *value = Value::String(mapped(ids, value.as_str().ok_or_else(invalid_ids)?)?);
    Ok(())
}

fn remap_array(value: &mut Value, ids: &Ids) -> Result<()> {
    for item in value.as_array_mut().ok_or_else(invalid_ids)? {
        remap_value_id(item, ids)?;
    }
    Ok(())
}
