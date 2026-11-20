// SPDX-License-Identifier: AGPL-3.0-or-later

use super::super::markdown::{PENDING_IMAGE, PENDING_LINK, block, paragraph};
use super::source::{file_name, parent, safe_path, stem};
use crate::{Attachment, Block, ImportOutcome, MAX_PREVIEW_BYTES, image_extension, web_url};
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use std::collections::HashMap;

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Target {
    Page(String),
    Asset(String),
}

#[derive(Default)]
pub(super) struct Index {
    paths: HashMap<String, Option<Target>>,
    folded: HashMap<String, Option<Target>>,
    short: HashMap<String, Option<Target>>,
}

impl Index {
    pub fn path(&mut self, path: &str, target: &Target) {
        insert(&mut self.paths, path.to_owned(), target);
        insert(&mut self.folded, path.to_lowercase(), target);
    }

    pub fn short_name(&mut self, path: &str, markdown: bool, target: &Target) {
        let filename = file_name(path);
        insert(&mut self.short, filename.to_lowercase(), target);
        if markdown {
            insert(&mut self.short, stem(filename).to_lowercase(), target);
        }
    }

    fn lookup(&self, path: &str) -> Option<&Option<Target>> {
        self.paths
            .get(path)
            .or_else(|| self.folded.get(&path.to_lowercase()))
    }

    fn resolve(&self, source: &str, path: &str, wiki: bool, obsidian: bool) -> Option<&Target> {
        if path.starts_with('/') || path.contains(['\\', ':']) || path.chars().any(char::is_control)
        {
            return None;
        }
        if obsidian && wiki && !path.contains('/') && !matches!(path, "." | "..") {
            return self.short.get(&path.to_lowercase())?.as_ref();
        }
        let relative = normalize(parent(source), path)?;
        let root = (obsidian && !path.starts_with('.')).then(|| path.trim_end_matches('/'));
        let choices = if wiki {
            [root, Some(relative.as_str())]
        } else {
            [Some(relative.as_str()), root]
        };
        choices
            .into_iter()
            .flatten()
            .find_map(|key| self.lookup(key))?
            .as_ref()
    }
}

fn insert(index: &mut HashMap<String, Option<Target>>, key: String, target: &Target) {
    index
        .entry(key)
        .and_modify(|previous| {
            if previous.as_ref() != Some(target) {
                *previous = None;
            }
        })
        .or_insert_with(|| Some(target.clone()));
}

fn normalize(directory: &str, path: &str) -> Option<String> {
    let mut parts: Vec<_> = directory
        .split('/')
        .filter(|part| !part.is_empty())
        .collect();
    for part in path.trim_end_matches('/').split('/') {
        match part {
            "." => {}
            ".." => {
                parts.pop()?;
            }
            "" => return None,
            _ => parts.push(part),
        }
    }
    let result = parts.join("/");
    (result.is_empty() || safe_path(&result)).then_some(result)
}

pub(super) struct Rewrite<'a, F> {
    pub index: &'a Index,
    pub source: &'a str,
    pub page: &'a str,
    pub obsidian: bool,
    pub outcome: &'a mut ImportOutcome,
    pub attachment: F,
}

impl<F> Rewrite<'_, F>
where
    F: FnMut(&str) -> Option<Attachment>,
{
    pub fn blocks(&mut self, incoming: Vec<Block>, allow_assets: bool) -> Vec<Block> {
        let mut output = Vec::new();
        for mut block in incoming {
            // Table cells accept text and page references in the editor. Keep
            // files accessible from the page's attachment list.
            let children_allow_assets = allow_assets && block.kind != "table";
            block.children = self.blocks(block.children, children_allow_assets);
            if matches!(block.kind.as_str(), "listItem" | "taskItem")
                && block
                    .children
                    .first()
                    .is_none_or(|first| first.kind != "paragraph")
            {
                block.children.insert(0, paragraph(vec![]));
            }
            let Some(inline) = block
                .payload
                .get_mut("inline")
                .and_then(Value::as_array_mut)
            else {
                output.push(block);
                continue;
            };
            let mut items = Vec::new();
            let can_split = allow_assets && block.kind == "paragraph";
            let mut split = false;
            for item in std::mem::take(inline) {
                match self.inline(item, can_split) {
                    Inline::Value(item) => items.push(item),
                    Inline::Asset(asset) => {
                        split = true;
                        if !items.is_empty() {
                            output.push(paragraph(std::mem::take(&mut items)));
                        }
                        output.push(asset);
                    }
                }
            }
            if split {
                if !items.is_empty() {
                    output.push(paragraph(items));
                }
            } else {
                *inline = items;
                output.push(block);
            }
        }
        output
    }

    fn inline(&mut self, item: Value, can_split: bool) -> Inline {
        let embed = item["type"] == PENDING_IMAGE;
        if !embed && item["type"] != PENDING_LINK {
            return Inline::Value(item);
        }
        let raw = item["destination"].as_str().unwrap_or_default();
        let label = item["label"]
            .as_str()
            .filter(|label| !label.is_empty())
            .unwrap_or(raw);
        let original = item["original"]
            .as_str()
            .filter(|text| !text.is_empty())
            .unwrap_or(label);
        let wiki = item["wiki"].as_bool().unwrap_or_default();
        let line = item["line"].as_u64().unwrap_or(1);
        if !wiki
            && !embed
            && let Some(url) = web_url(raw)
        {
            return Inline::Value(json!({
                "type": "text", "text": label, "marks": [{"type": "link", "href": url}],
            }));
        }
        let (path, fragment) = raw.split_once('#').unwrap_or((raw, ""));
        let decoded = percent_decode_str(path).decode_utf8();
        let here =
            (!fragment.is_empty() && path.is_empty()).then(|| Target::Page(self.page.to_owned()));
        let target = here.as_ref().or_else(|| {
            decoded
                .as_deref()
                .ok()
                .filter(|_| !path.contains('?'))
                .and_then(|path| self.index.resolve(self.source, path, wiki, self.obsidian))
        });
        match target {
            Some(Target::Page(id)) => {
                if !fragment.is_empty() {
                    self.outcome.note(format!(
                        "第 {line} 行链接已指向页面，标题或块锚点留存在原件里。"
                    ));
                }
                if embed {
                    self.outcome
                        .note(format!("第 {line} 行笔记嵌入已转换为页面引用。"));
                }
                Inline::Value(json!({"type": "pageReference", "pageId": id, "label": label}))
            }
            Some(Target::Asset(path)) => {
                let Some(asset) = (self.attachment)(path) else {
                    return self.unmatched(original, line);
                };
                if !can_split {
                    self.outcome.note(format!(
                        "第 {line} 行附件已加入页面的附件列表，正文保留原链接文本。"
                    ));
                    return Inline::text(original);
                }
                let display_size = wiki && embed && is_image_size(label);
                if !fragment.is_empty() || display_size {
                    self.outcome
                        .note(format!("第 {line} 行的附件定位或显示尺寸留存在原件里。"));
                }
                let image = embed && image_extension(&asset.media_type).is_some();
                if image && asset.bytes <= MAX_PREVIEW_BYTES {
                    let label = if display_size { &asset.name } else { label };
                    let alt: String = label
                        .chars()
                        .filter(|ch| !ch.is_control())
                        .take(500)
                        .collect();
                    Inline::Asset(block(
                        "attachmentImage",
                        json!({
                            "attachmentId": asset.id, "mediaType": asset.media_type, "alt": alt,
                        }),
                        vec![],
                    ))
                } else {
                    if image {
                        self.outcome
                            .note(format!("第 {line} 行的图片超过预览上限，已转换为附件块。"));
                    }
                    Inline::Asset(block(
                        "attachmentFile",
                        json!({"attachmentId": asset.id}),
                        vec![],
                    ))
                }
            }
            None => self.unmatched(original, line),
        }
    }

    fn unmatched(&mut self, original: &str, line: u64) -> Inline {
        self.outcome.unmatched_links += 1;
        let excerpt: String = original.chars().take(300).collect();
        self.outcome.note(format!(
            "第 {line} 行的链接无法唯一匹配或格式不受支持，已保留为文本。{excerpt}"
        ));
        Inline::text(original)
    }
}

fn is_image_size(label: &str) -> bool {
    let dimension = |value: &str| value.parse::<u32>().is_ok_and(|size| size > 0);
    let (width, height) = label.split_once('x').unwrap_or((label, "1"));
    dimension(width) && dimension(height)
}

enum Inline {
    Value(Value),
    Asset(Block),
}

impl Inline {
    fn text(text: &str) -> Self {
        Self::Value(json!({"type": "text", "text": text}))
    }
}
