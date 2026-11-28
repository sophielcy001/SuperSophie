// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{
    ImportFile, ImportKind, ImportOutcome, MAX_MARKDOWN_IMPORT_BYTES, MAX_MARKDOWN_IMPORT_FILES,
    Source,
};
use crate::model::{new_id, validate_blocks};
use crate::store::{ensure_active, insert_new_page, new_page, read_page};
use crate::{Block, Error, Page, Result, Store};
use percent_encoding::percent_decode_str;
use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, LinkType, Options, Parser, Tag,
};
use rusqlite::TransactionBehavior;
use serde_json::{Value, json};
use std::{collections::HashMap, ops::Range};

pub(super) const PENDING_LINK: &str = "pendingImportLink";
pub(super) const PENDING_IMAGE: &str = "pendingImportImage";
const KIND: ImportKind = ImportKind::Markdown;

impl Store {
    pub fn import_markdown(
        &self,
        workspace: &str,
        parent: Option<&str>,
        files: Vec<ImportFile>,
    ) -> Result<Vec<ImportOutcome>> {
        if files.len() > MAX_MARKDOWN_IMPORT_FILES {
            return Err(Error::new(
                "IMPORT_SIZE",
                "一次最多可导入 32 个 Markdown 文件。",
            ));
        }
        let destination = parent.unwrap_or_default();
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(parent) = parent {
            ensure_active(&read_page(&tx, parent)?)?;
        }
        let mut outcomes = Vec::with_capacity(files.len());
        let mut pending = Vec::new();
        let mut seen = HashMap::new();
        let mut filenames = UniqueTargets::default();
        let mut stems = UniqueTargets::default();
        for file in files {
            let mut result = ImportOutcome::new(file.name.clone());
            let candidate = (|| -> Result<()> {
                let source = Source::new(file, &["md", "markdown"], MAX_MARKDOWN_IMPORT_BYTES)?;
                let key = (source.name.clone(), source.sha256.clone());
                let earlier = match seen.get(&key) {
                    Some(id) => Some((String::clone(id), false)),
                    None => source.earlier(&tx, KIND, destination)?,
                };
                let id = if let Some((id, trashed)) = earlier {
                    result.duplicate(id.clone(), trashed);
                    if trashed {
                        return Ok(());
                    }
                    id
                } else {
                    let mut page = new_page(parent, source.stem(), "file")?;
                    page.blocks = parse(source.text()?, &mut result, false)?;
                    validate_blocks(&page.blocks)?;
                    let id = page.info.id.clone();
                    seen.insert(key, id.clone());
                    pending.push((outcomes.len(), page, source));
                    // The source name and stem are also available in the outcome.
                    id
                };
                let stem = result
                    .name
                    .rsplit_once('.')
                    .map_or(result.name.as_str(), |(stem, _)| stem);
                filenames.insert(&result.name, &id);
                stems.insert(stem, &id);
                Ok(())
            })();
            if let Err(error) = candidate {
                if error.code == "DATABASE" {
                    return Err(error);
                }
                result.note(error.message);
            }
            outcomes.push(result);
        }

        for (index, page, _) in &mut pending {
            resolve_links(&mut page.blocks, &filenames, &stems, &mut outcomes[*index]);
        }
        // Validate every result before writing pages or originals.
        for (index, page, _) in &pending {
            validate_blocks(&page.blocks).map_err(|error| {
                Error::new(
                    "IMPORT_FORMAT",
                    format!(
                        "「{}」的正文无法保存。{}",
                        outcomes[*index].name, error.message
                    ),
                )
            })?;
        }
        for (index, page, source) in pending {
            let Page { info, .. } = insert_new_page(&tx, page)?;
            source.save(self, &tx, workspace, KIND, destination, &info.id)?;
            outcomes[index].imported(info.id, 0);
        }
        tx.commit()?;
        Ok(outcomes)
    }
}

#[derive(Default)]
struct UniqueTargets(HashMap<String, Option<String>>);

impl UniqueTargets {
    fn insert(&mut self, key: &str, id: &str) {
        self.0
            .entry(key.to_lowercase())
            .and_modify(|target| {
                if target.as_deref() != Some(id) {
                    *target = None;
                }
            })
            .or_insert_with(|| Some(id.to_owned()));
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.0.get(&key.to_lowercase())?.as_deref()
    }
}

fn resolve_links(
    blocks: &mut [Block],
    filenames: &UniqueTargets,
    stems: &UniqueTargets,
    outcome: &mut ImportOutcome,
) {
    for block in blocks {
        if let Some(inline) = block
            .payload
            .get_mut("inline")
            .and_then(Value::as_array_mut)
        {
            for item in inline
                .iter_mut()
                .filter(|item| item["type"] == PENDING_LINK)
            {
                let dest = item["destination"].as_str().unwrap_or_default();
                let label = item["label"].as_str().unwrap_or_default();
                let line = item["line"].as_u64().unwrap_or(1);
                let wiki = item["wiki"].as_bool().unwrap_or(false);
                let (path, fragment) = dest.split_once('#').unwrap_or((dest, ""));
                let path = percent_decode_str(path.trim_start_matches("./")).decode_utf8();
                let target = path
                    .as_deref()
                    .ok()
                    .filter(|path| !path.is_empty() && !path.contains(['/', '\\', ':', '?']))
                    .and_then(|path| {
                        let file = filenames.get(path);
                        if wiki && !filenames.0.contains_key(&path.to_lowercase()) {
                            stems.get(path)
                        } else {
                            file
                        }
                    });
                if let Some(id) = target {
                    if !fragment.is_empty() {
                        outcome.note(format!(
                            "第 {line} 行的链接已指向页面。标题锚点保留在原件里。"
                        ));
                    }
                    *item = json!({"type": "pageReference", "pageId": id, "label": label});
                } else if let Some(url) = crate::web_url(dest).filter(|_| !wiki) {
                    *item = json!({"type": "text", "text": label, "marks": [{"type": "link", "href": url}]});
                } else {
                    outcome.note(format!("第 {line} 行的链接保留为文本。目标格式暂不支持，或无法在所选文件中唯一匹配。"));
                    let rendered = if label == dest || dest.is_empty() {
                        label.to_owned()
                    } else {
                        format!("{label}（{dest}）")
                    };
                    *item = json!({"type": "text", "text": rendered});
                }
            }
        }
        resolve_links(&mut block.children, filenames, stems, outcome);
    }
}

struct Node<'a> {
    event: Event<'a>,
    source: Range<usize>,
    children: Vec<Node<'a>>,
}

pub(super) fn parse(
    source: &str,
    outcome: &mut ImportOutcome,
    migration: bool,
) -> Result<Vec<Block>> {
    let mut options = Options::ENABLE_TABLES
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_WIKILINKS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    if migration {
        options |= Options::ENABLE_GFM;
    }
    let mut open = Vec::<Node>::new();
    let mut root = Vec::new();
    for (event, source) in Parser::new_ext(source, options).into_offset_iter() {
        if matches!(event, Event::Start(_)) {
            // Keep tree operations bounded, including malformed or adversarial nesting.
            if open.len() >= 64 {
                return Err(Error::new(
                    "IMPORT_NESTING",
                    "Markdown 的嵌套层级超出支持范围。",
                ));
            }
            open.push(Node {
                event,
                source,
                children: Vec::new(),
            });
            continue;
        }
        let node = if matches!(event, Event::End(_)) {
            let mut node = open
                .pop()
                .ok_or_else(|| Error::new("MARKDOWN_FORMAT", "无法读取 Markdown 的块结构。"))?;
            node.source.end = node.source.end.max(source.end);
            node
        } else {
            Node {
                event,
                source,
                children: Vec::new(),
            }
        };
        if let Some(parent) = open.last_mut() {
            parent.children.push(node);
        } else {
            root.push(node);
        }
    }
    let newlines = source.match_indices('\n').map(|(i, _)| i).collect();
    Ok(Builder {
        source,
        newlines,
        outcome,
        migration,
    }
    .blocks(&root))
}

struct Builder<'a, 'out> {
    source: &'a str,
    newlines: Vec<usize>,
    outcome: &'out mut ImportOutcome,
    migration: bool,
}

impl Builder<'_, '_> {
    fn line(&self, node: &Node<'_>) -> usize {
        self.newlines
            .partition_point(|index| *index < node.source.start)
            + 1
    }

    fn note(&mut self, node: &Node<'_>, note: &str) {
        self.outcome
            .note(format!("第 {} 行{note}", self.line(node)));
    }

    fn blocks(&mut self, nodes: &[Node<'_>]) -> Vec<Block> {
        let mut blocks = Vec::new();
        let mut loose_inline = Vec::new();
        for node in nodes {
            if let Some(block) = self.block(node) {
                if !loose_inline.is_empty() {
                    blocks.push(paragraph(std::mem::take(&mut loose_inline)));
                }
                blocks.push(block);
            } else {
                self.inline(node, &mut loose_inline, &[]);
            }
        }
        if !loose_inline.is_empty() {
            blocks.push(paragraph(loose_inline));
        }
        blocks
    }

    fn block(&mut self, node: &Node<'_>) -> Option<Block> {
        let tag = match &node.event {
            Event::Start(tag) => tag,
            Event::Rule => return Some(block("horizontalRule", json!({}), vec![])),
            _ => return None,
        };
        let children = &node.children;
        let result = match tag {
            Tag::Paragraph => paragraph(self.inlines(children)),
            Tag::Heading { level, .. } => {
                let level = *level as u8;
                if level > 3 {
                    self.note(node, "的四至六级标题已转换为三级标题。");
                }
                block(
                    "heading",
                    json!({"level": level.min(3), "inline": self.inlines(children)}),
                    vec![],
                )
            }
            Tag::BlockQuote(kind) => {
                if self.migration
                    && let Some(kind) = kind
                {
                    let tone = match kind {
                        BlockQuoteKind::Note => "note",
                        BlockQuoteKind::Tip => "tip",
                        _ => "warning",
                    };
                    return Some(block(
                        "callout",
                        json!({"tone": tone}),
                        with_paragraph(self.blocks(children), false),
                    ));
                }
                if kind.is_some() {
                    self.note(node, "的警示框已转换为引用。");
                } else if self.migration
                    && self.source[node.source.clone()]
                        .trim_start_matches(['>', ' '])
                        .starts_with("[!")
                {
                    self.note(node, "的自定义提示框以引用保留，原标记仍在正文和原件中。");
                }
                block(
                    "blockquote",
                    json!({}),
                    with_paragraph(self.blocks(children), false),
                )
            }
            Tag::CodeBlock(kind) => {
                let mut payload = json!({"text": plain(children)});
                if let CodeBlockKind::Fenced(info) = kind {
                    let mut parts = info.split_whitespace();
                    if let Some(language) = parts.next() {
                        payload["language"] = json!(language);
                    }
                    if parts.next().is_some() {
                        self.note(node, "的代码块扩展参数保留在原件里。");
                    }
                }
                block("codeBlock", payload, vec![])
            }
            Tag::List(start) => self.list(node, *start),
            Tag::Table(alignments) => self.table(node, alignments),
            Tag::HtmlBlock => self.literal(node, "的 HTML 已保留为文本。", "html"),
            Tag::FootnoteDefinition(_) => {
                self.literal(node, "的脚注定义已转为 Markdown 文本。", "markdown")
            }
            Tag::MetadataBlock(_) => {
                self.literal(node, "的文档元数据已转为 Markdown 文本。", "markdown")
            }
            _ => return None,
        };
        Some(result)
    }

    fn literal(&mut self, node: &Node<'_>, note: &str, language: &str) -> Block {
        self.note(node, note);
        block(
            "codeBlock",
            json!({"text": &self.source[node.source.clone()], "language": language}),
            vec![],
        )
    }

    fn table(&mut self, node: &Node<'_>, alignments: &[Alignment]) -> Block {
        let rows = node
            .children
            .iter()
            .map(|row| {
                let kind = if matches!(row.event, Event::Start(Tag::TableHead)) {
                    "tableHeader"
                } else {
                    "tableCell"
                };
                let cells = row
                    .children
                    .iter()
                    .zip(alignments)
                    .map(|(cell, alignment)| {
                        let payload = match alignment {
                            Alignment::Left => json!({"align": "left"}),
                            Alignment::Center => json!({"align": "center"}),
                            Alignment::Right => json!({"align": "right"}),
                            Alignment::None => json!({}),
                        };
                        block(kind, payload, vec![paragraph(self.inlines(&cell.children))])
                    })
                    .collect();
                block("tableRow", json!({}), cells)
            })
            .collect();
        block("table", json!({}), rows)
    }

    fn list(&mut self, node: &Node<'_>, start: Option<u64>) -> Block {
        let marks = node.children.iter().map(find_task).collect::<Vec<_>>();
        let tasks = start.is_none() && marks.iter().all(Option::is_some);
        if !tasks && marks.iter().any(Option::is_some) {
            self.note(node, "的混合或有序列表已将待办标记保留为文本。");
        }
        let items = node
            .children
            .iter()
            .zip(marks)
            .map(|(item, checked)| {
                let blocks = with_paragraph(self.blocks(&item.children), true);
                if tasks {
                    block(
                        "taskItem",
                        json!({"checked": checked.unwrap_or(false)}),
                        blocks,
                    )
                } else {
                    let mut blocks = blocks;
                    if let Some(checked) = checked
                        && let Some(inline) = blocks[0].payload["inline"].as_array_mut()
                    {
                        inline.insert(
                            0,
                            json!({"type": "text", "text": if checked { "[x] " } else { "[ ] " }}),
                        );
                    }
                    block("listItem", json!({}), blocks)
                }
            })
            .collect();
        match (tasks, start) {
            (true, _) => block("taskList", json!({}), items),
            (_, Some(start)) => block("orderedList", json!({"start": start}), items),
            _ => block("bulletList", json!({}), items),
        }
    }

    fn inlines(&mut self, nodes: &[Node<'_>]) -> Vec<Value> {
        let mut result = Vec::new();
        for node in nodes {
            self.inline(node, &mut result, &[]);
        }
        result
    }

    fn inline(&mut self, node: &Node<'_>, out: &mut Vec<Value>, marks: &[&str]) {
        let mark = match &node.event {
            Event::Start(Tag::Emphasis) => Some("italic"),
            Event::Start(Tag::Strong) => Some("bold"),
            Event::Start(Tag::Strikethrough) => Some("strike"),
            _ => None,
        };
        if let Some(mark) = mark {
            let mut marks = marks.to_vec();
            if !marks.contains(&mark) {
                marks.push(mark);
            }
            for child in &node.children {
                self.inline(child, out, &marks);
            }
            return;
        }
        match &node.event {
            Event::Text(text) => push_text(out, text, marks),
            Event::Code(text) => push_text(out, text, &["code"]),
            Event::SoftBreak => push_text(out, " ", marks),
            Event::HardBreak => out.push(json!({"type": "hardBreak"})),
            Event::InlineHtml(text) | Event::Html(text) => {
                self.note(node, "的 HTML 已保留为文本。");
                push_text(out, text, marks);
            }
            Event::FootnoteReference(label) => {
                self.note(node, "的脚注引用已保留为文本。");
                push_text(out, &format!("[^{label}]"), marks);
            }
            Event::Start(Tag::Link {
                link_type,
                dest_url,
                title,
                ..
            }) => {
                let label = plain(&node.children);
                if !title.is_empty() || !marks.is_empty() || has_link_formatting(&node.children) {
                    self.note(node, "的链接提示或链接内格式保留在原件里。");
                }
                out.push(json!({
                    "type": PENDING_LINK,
                    "destination": dest_url.as_ref(),
                    "label": if label.is_empty() && dest_url.is_empty() { &self.source[node.source.clone()] }
                             else if label.is_empty() { dest_url.as_ref() } else { label.as_str() },
                    "line": self.line(node),
                    "wiki": matches!(link_type, LinkType::WikiLink { .. }),
                    "original": &self.source[node.source.clone()],
                }));
            }
            Event::Start(Tag::Image {
                link_type,
                dest_url,
                ..
            }) if self.migration => {
                let wiki = matches!(link_type, LinkType::WikiLink { .. });
                let label = plain(&node.children);
                out.push(json!({
                    "type": PENDING_IMAGE, "destination": dest_url.as_ref(), "line": self.line(node),
                    "label": label, "wiki": wiki, "original": &self.source[node.source.clone()],
                }));
            }
            Event::Start(Tag::Image { .. }) => {
                self.note(node, "的图片引用已保留为 Markdown 文本。");
                push_text(out, &self.source[node.source.clone()], marks);
            }
            Event::TaskListMarker(_) => {}
            _ => {
                self.note(node, "的格式已保留为 Markdown 文本。");
                push_text(out, &self.source[node.source.clone()], marks);
            }
        }
    }
}

fn with_paragraph(mut blocks: Vec<Block>, first: bool) -> Vec<Block> {
    if blocks.is_empty() || (first && blocks[0].kind != "paragraph") {
        blocks.insert(0, paragraph(vec![]));
    }
    blocks
}

fn find_task(item: &Node<'_>) -> Option<bool> {
    let first = item.children.first()?;
    let marker = match first.event {
        Event::Start(Tag::Paragraph | Tag::Heading { .. }) => first.children.first()?,
        _ => first,
    };
    match marker.event {
        Event::TaskListMarker(checked) => Some(checked),
        _ => None,
    }
}

fn has_link_formatting(nodes: &[Node<'_>]) -> bool {
    nodes
        .iter()
        .any(|node| !matches!(node.event, Event::Text(_) | Event::SoftBreak))
}

fn plain(nodes: &[Node<'_>]) -> String {
    let mut out = String::new();
    for node in nodes {
        match &node.event {
            Event::Text(text) | Event::Code(text) => out.push_str(text),
            Event::HardBreak | Event::SoftBreak => out.push(' '),
            _ => out.push_str(&plain(&node.children)),
        }
    }
    out
}

fn push_text(out: &mut Vec<Value>, text: &str, marks: &[&str]) {
    if text.is_empty() {
        return;
    }
    let mut item = json!({"type": "text", "text": text});
    if !marks.is_empty() {
        item["marks"] = Value::Array(marks.iter().map(|mark| json!({"type": mark})).collect());
    }
    out.push(item);
}

pub(super) fn paragraph(inline: Vec<Value>) -> Block {
    block("paragraph", json!({"inline": inline}), vec![])
}

pub(super) fn block(kind: &str, payload: Value, children: Vec<Block>) -> Block {
    Block {
        id: new_id(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children,
        extra: Default::default(),
    }
}
