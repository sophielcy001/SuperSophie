// SPDX-License-Identifier: AGPL-3.0-or-later

use super::super::{
    ImportFile, ImportKind, ImportOriginal, ImportOutcome, ImportStatus, MAX_CSV_IMPORT_BYTES,
    MAX_MARKDOWN_IMPORT_BYTES, Source, csv, markdown,
};
use super::links::{Index, Rewrite, Target};
use super::source::{Inventory, extension, file_name, is_markdown, parent, stem};
use super::{check_cancel, migration_error};
use crate::collections::create_collection_on_page;
use crate::model::{new_id, validate_blocks};
use crate::store::{insert_new_page_with_attachments, new_page};
use crate::{Attachment, Error, Page, PropertyDefinition, Result};
use rusqlite::Transaction;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::AtomicBool;

const MAX_PAGES: usize = 10_000;
const MAX_TABULAR_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LINKED_ATTACHMENTS: usize = 8_192;

struct Node {
    page: Page,
    parent: usize,
    markdown: Option<String>,
    body: Option<(u64, usize)>,
    definitions: Option<Vec<PropertyDefinition>>,
    attachments: HashMap<String, Attachment>,
    names: HashSet<String>,
}

impl Node {
    fn new(page: Page, parent: usize) -> Self {
        Self {
            page,
            parent,
            markdown: None,
            body: None,
            definitions: None,
            attachments: HashMap::new(),
            names: HashSet::new(),
        }
    }

    fn attach(&mut self, path: &str, staged: &Attachment) -> &Attachment {
        self.attachments.entry(path.to_owned()).or_insert_with(|| {
            let mut asset = staged.clone();
            asset.id = new_id();
            asset.page_id = self.page.info.id.clone();
            if !self.names.insert(asset.name.to_lowercase()) {
                let basename = stem(&asset.name);
                let suffix = &asset.name[basename.len()..];
                let base: String = basename.chars().take(120).collect();
                let suffix: String = suffix.chars().take(100).collect();
                for index in 2.. {
                    let name = format!("{base} ({index}){suffix}");
                    if self.names.insert(name.to_lowercase()) {
                        asset.name = name;
                        break;
                    }
                }
            }
            asset
        })
    }
}

pub(super) struct Plan {
    nodes: Vec<Node>,
    documents: BTreeMap<String, usize>,
    folders: HashMap<String, usize>,
    owners: HashMap<String, Option<usize>>,
    outcomes: BTreeMap<String, ImportOutcome>,
    obsidian: bool,
    bodies: File,
    pub container_original: Option<ImportOriginal>,
}

impl Plan {
    pub fn new(
        root: Page,
        inventory: &Inventory<'_>,
        kind: ImportKind,
        cancel: &AtomicBool,
    ) -> Result<Self> {
        let mut plan = Self {
            nodes: vec![Node::new(root, 0)],
            documents: BTreeMap::new(),
            folders: HashMap::from([(String::new(), 0)]),
            owners: HashMap::new(),
            outcomes: BTreeMap::new(),
            obsidian: kind == ImportKind::ObsidianVault,
            bodies: inventory.body_spool()?,
            container_original: None,
        };
        if let Some(container) = &inventory.container {
            // An empty key is reserved for the outer ZIP. Source paths are nonempty.
            let asset = plan.nodes[0].attach("", container);
            plan.container_original = Some(original(asset));
        }
        let unfolded: HashMap<_, _> = inventory
            .files
            .keys()
            .filter_map(|path| notion_full_csv(path).map(|plain| (plain, path.as_str())))
            .collect();
        for path in inventory.files.keys() {
            check_cancel(cancel)?;
            let is_csv = !plan.obsidian && extension(path).eq_ignore_ascii_case("csv");
            if !is_markdown(path) && !is_csv {
                continue;
            }
            if is_csv && notion_full_csv(path).is_none() && unfolded.contains_key(stem(path)) {
                continue;
            }
            let basename = file_name(
                notion_full_csv(path)
                    .filter(|_| is_csv)
                    .unwrap_or_else(|| stem(path)),
            );
            let title = if plan.obsidian {
                basename
            } else {
                without_notion_id(basename)
            };
            let id = plan.add(new_page(None, &page_title(title), "file")?, 0)?;
            if is_markdown(path) {
                plan.nodes[id].markdown = Some(path.clone());
            }
            if !plan.obsidian {
                plan.owner(stem(path), id);
                if let Some(full) = notion_full_csv(path).filter(|_| is_csv) {
                    plan.owner(full, id);
                }
            }
            plan.documents.insert(path.clone(), id);
        }
        let mut index = Index::default();
        let mut tabular_bytes = 0;
        // Keep CSV rows until all source pages have been assigned to their parents.
        let mut tables = Vec::new();
        for path in inventory.files.keys() {
            check_cancel(cancel)?;
            let destination = plan.folder(parent(path))?;
            let staged = &inventory.files[path];
            let mut outcome = ImportOutcome::new(path.clone());
            let id = if let Some(&id) = plan.documents.get(path) {
                plan.nodes[id].parent = destination;
                outcome.imported(plan.nodes[id].page.info.id.clone(), 0);
                if !is_markdown(path) {
                    if staged.bytes <= MAX_CSV_IMPORT_BYTES as u64 {
                        tabular_bytes += staged.bytes;
                        if tabular_bytes > MAX_TABULAR_BYTES {
                            return Err(Error::new(
                                "IMPORT_SIZE",
                                "可转换的 CSV 总量超过 64 MiB。",
                            ));
                        }
                    }
                    let input = read_file(inventory, staged, MAX_CSV_IMPORT_BYTES)?;
                    match Source::new(input, &["csv"], MAX_CSV_IMPORT_BYTES)
                        .and_then(|source| csv::prepare_notion(&source, &mut outcome))
                    {
                        Ok((definitions, rows)) => {
                            plan.nodes[id].page.info.icon = "database".into();
                            plan.nodes[id].definitions = Some(definitions);
                            outcome.records = rows.len();
                            tables.push((id, rows, path));
                        }
                        Err(error) => preserve(&mut outcome, &error.message),
                    }
                }
                let target = Target::Page(plan.nodes[id].page.info.id.clone());
                index.path(path, &target);
                if is_markdown(path) {
                    index.path(stem(path), &target);
                }
                index.short_name(path, is_markdown(path), &target);
                id
            } else if !plan.obsidian
                && extension(path).eq_ignore_ascii_case("csv")
                && let Some(full) = unfolded.get(stem(path))
            {
                let id = plan.documents[*full];
                let target = Target::Page(plan.nodes[id].page.info.id.clone());
                index.path(path, &target);
                preserve(
                    &mut outcome,
                    "已使用同一数据库的完整 CSV，筛选视图只留存原件。",
                );
                id
            } else {
                let target = Target::Asset(path.clone());
                index.path(path, &target);
                index.short_name(path, false, &target);
                preserve(&mut outcome, "原件已保存到所属页面的附件列表。");
                destination
            };
            let asset = plan.nodes[id].attach(path, staged);
            outcome.original = Some(original(asset));
            if outcome.page_id.is_none() {
                outcome.page_id = Some(asset.page_id.clone());
            }
            plan.outcomes.insert(path.clone(), outcome);
        }
        for (path, &id) in &plan.folders {
            index.path(path, &Target::Page(plan.nodes[id].page.info.id.clone()));
        }
        for (id, rows, path) in tables {
            check_cancel(cancel)?;
            plan.merge_rows(id, rows, path, cancel)?;
        }
        plan.move_nested_databases()?;
        plan.resolve_markdown(inventory, &index, cancel)?;
        Ok(plan)
    }

    fn owner(&mut self, path: &str, id: usize) {
        self.owners
            .entry(path.to_owned())
            .and_modify(|owner| {
                if *owner != Some(id) {
                    *owner = None;
                }
            })
            .or_insert(Some(id));
    }

    fn add(&mut self, page: Page, parent: usize) -> Result<usize> {
        if self.nodes.len() >= MAX_PAGES {
            return Err(Error::new(
                "IMPORT_SIZE",
                "一次迁移最多可生成 10,000 个页面和记录。",
            ));
        }
        let index = self.nodes.len();
        self.nodes.push(Node::new(page, parent));
        Ok(index)
    }

    fn folder(&mut self, path: &str) -> Result<usize> {
        if let Some(&id) = self.folders.get(path) {
            return Ok(id);
        }
        let id = if let Some(Some(owner)) = self.owners.get(path) {
            *owner
        } else {
            let parent = self.folder(parent(path))?;
            let name = file_name(path);
            let title = if self.obsidian {
                name
            } else {
                without_notion_id(name)
            };
            self.add(new_page(None, &page_title(title), "folder")?, parent)?
        };
        self.folders.insert(path.to_owned(), id);
        Ok(id)
    }

    fn merge_rows(
        &mut self,
        table: usize,
        rows: Vec<csv::CsvRow>,
        path: &str,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut notes = HashMap::new();
        for (name, &id) in &self.documents {
            if self.nodes[id].parent == table && is_markdown(name) {
                let title = without_notion_id(stem(file_name(name))).trim();
                notes
                    .entry(title.to_owned())
                    .and_modify(|entry| *entry = None)
                    .or_insert(Some(id));
            }
        }
        let mut counts = HashMap::new();
        for (title, _) in &rows {
            *counts.entry(title.clone()).or_insert(0) += 1;
        }
        let mut matched = 0;
        let mut ambiguous = 0;
        for (title, values) in rows {
            check_cancel(cancel)?;
            let candidate = notes.get(title.as_str());
            if let Some(Some(id)) = candidate
                && counts[&title] == 1
            {
                self.nodes[*id].page.properties = Some(values);
                matched += 1;
            } else {
                ambiguous += usize::from(candidate.is_some());
                let mut page = new_page(None, &title, "file")?;
                page.properties = Some(values);
                self.add(page, table)?;
            }
        }
        let outcome = self.outcomes.get_mut(path).expect("table outcome");
        if matched > 0 {
            outcome.note(format!("已将 {matched} 行的属性与同名 Markdown 正文合并。"));
        }
        if ambiguous > 0 {
            outcome.note(format!(
                "{ambiguous} 行的记录标题有重名，CSV 记录和 Markdown 正文分别导入。"
            ));
        }
        Ok(())
    }

    fn move_nested_databases(&mut self) -> Result<()> {
        let ids: Vec<_> = self
            .documents
            .values()
            .copied()
            .filter(|&id| {
                let node = &self.nodes[id];
                node.definitions.is_some() && self.nodes[node.parent].definitions.is_some()
            })
            .collect();
        for id in ids {
            let node = &self.nodes[id];
            let title = page_title(&format!("{} · 记录", node.page.info.title));
            let bridge = self.add(new_page(None, &title, "folder")?, node.parent)?;
            self.nodes[id].parent = bridge;
            if let Some((path, _)) = self
                .documents
                .iter()
                .find(|&(_, &candidate)| candidate == id)
            {
                self.outcomes
                    .get_mut(path)
                    .expect("table outcome")
                    .note("子数据库放入上级数据库的一条独立记录中。");
            }
        }
        Ok(())
    }

    fn resolve_markdown(
        &mut self,
        inventory: &Inventory<'_>,
        index: &Index,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut linked = 0;
        let mut body_bytes = 0;
        for node in &mut self.nodes {
            check_cancel(cancel)?;
            let Some(path) = node.markdown.take() else {
                continue;
            };
            let outcome = self.outcomes.get_mut(&path).expect("Markdown outcome");
            let staged = &inventory.files[&path];
            let file = read_file(inventory, staged, MAX_MARKDOWN_IMPORT_BYTES)?;
            let parsed = Source::new(file, &["md", "markdown"], MAX_MARKDOWN_IMPORT_BYTES)
                .and_then(|source| markdown::parse(source.text()?, outcome, true));
            let blocks = match parsed {
                Ok(blocks) => blocks,
                Err(error) => {
                    preserve(outcome, &error.message);
                    continue;
                }
            };
            let page_id = node.page.info.id.clone();
            let rewritten = Rewrite {
                index,
                source: &path,
                page: &page_id,
                obsidian: self.obsidian,
                outcome,
                attachment: |reference: &str| {
                    if let Some(asset) = node.attachments.get(reference) {
                        return Some(asset.clone());
                    }
                    if linked >= MAX_LINKED_ATTACHMENTS {
                        return None;
                    }
                    let staged = inventory.files.get(reference)?;
                    linked += 1;
                    Some(node.attach(reference, staged).clone())
                },
            }
            .blocks(blocks, true);
            match validate_blocks(&rewritten) {
                Ok(_) => {
                    let body = serde_json::to_vec(&rewritten)?;
                    node.body = Some((body_bytes, body.len()));
                    body_bytes += body.len() as u64;
                    if body_bytes > 128 * 1024 * 1024 {
                        return Err(Error::new(
                            "IMPORT_SIZE",
                            "迁移生成的页面正文总量超过 128 MiB。",
                        ));
                    }
                    self.bodies.write_all(&body)?;
                }
                Err(error) => preserve(outcome, &error.message),
            }
        }
        Ok(())
    }

    pub fn publish(
        mut self,
        tx: &Transaction<'_>,
        cancel: &AtomicBool,
    ) -> Result<Vec<ImportOutcome>> {
        let mut children = vec![Vec::new(); self.nodes.len()];
        let mut nodes: Vec<_> = self
            .nodes
            .into_iter()
            .enumerate()
            .map(|(id, node)| {
                if id != 0 {
                    children[node.parent].push(id);
                }
                Some(node)
            })
            .collect();
        let mut queue = VecDeque::from([0]);
        let mut published = 0;
        while let Some(id) = queue.pop_front() {
            check_cancel(cancel)?;
            let mut node = nodes[id].take().expect("node published once");
            if let Some((offset, size)) = node.body {
                let mut body = vec![0; size];
                self.bodies.seek(SeekFrom::Start(offset))?;
                self.bodies.read_exact(&mut body)?;
                node.page.blocks = serde_json::from_slice(&body)?;
            }
            let mut attachments: Vec<_> = node.attachments.into_values().collect();
            attachments.sort_by(|a, b| a.name.cmp(&b.name));
            let page = match node.definitions.take() {
                Some(definitions) => {
                    create_collection_on_page(tx, node.page, Some(definitions), &attachments)?.1
                }
                None => insert_new_page_with_attachments(tx, node.page, &attachments)?.0,
            };
            published += 1;
            for &child in &children[id] {
                nodes[child]
                    .as_mut()
                    .expect("unpublished child")
                    .page
                    .info
                    .parent_id = Some(page.info.id.clone());
                queue.push_back(child);
            }
        }
        if published != nodes.len() {
            return Err(migration_error("迁移包的页面层级无法连通。"));
        }
        Ok(self.outcomes.into_values().collect())
    }
}

fn preserve(outcome: &mut ImportOutcome, message: &str) {
    outcome.status = ImportStatus::Preserved;
    outcome.note(message);
}

fn original(asset: &Attachment) -> ImportOriginal {
    ImportOriginal {
        page_id: asset.page_id.clone(),
        attachment_id: asset.id.clone(),
        name: asset.name.clone(),
    }
}

fn read_file(
    inventory: &Inventory<'_>,
    staged: &Attachment,
    max_bytes: usize,
) -> Result<ImportFile> {
    let mut bytes = Vec::new();
    File::open(inventory.blob(staged))?
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(ImportFile {
        name: staged.name.clone(),
        bytes,
    })
}

fn notion_full_csv(path: &str) -> Option<&str> {
    if !extension(path).eq_ignore_ascii_case("csv") {
        return None;
    }
    let stem = stem(path);
    let plain = stem.get(..stem.len().checked_sub(4)?)?;
    if !stem[plain.len()..].eq_ignore_ascii_case("_all") {
        return None;
    }
    let filename = file_name(plain);
    (notion_id(filename).is_some()).then_some(plain)
}

fn notion_id(filename: &str) -> Option<&str> {
    let (title, id) = filename.rsplit_once(' ')?;
    let hex = if id.len() == 36 {
        let bytes = id.as_bytes();
        if [8, 13, 18, 23].iter().any(|&i| bytes[i] != b'-') {
            return None;
        }
        id.chars().filter(|&ch| ch != '-').collect::<String>()
    } else {
        id.to_owned()
    };
    (hex.len() == 32 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(title)
}

fn without_notion_id(filename: &str) -> &str {
    notion_id(filename).unwrap_or(filename)
}

pub(super) fn page_title(title: &str) -> String {
    let trimmed: String = title
        .trim()
        .chars()
        .filter(|ch| !ch.is_control())
        .take(200)
        .collect();
    if trimmed.trim().is_empty() {
        "迁移原件".into()
    } else {
        trimmed
    }
}
