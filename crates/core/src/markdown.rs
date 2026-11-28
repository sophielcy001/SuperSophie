// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{Attachment, Block, Page, PageLocation};
use serde_json::Value;
use std::cell::{Cell, RefCell};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write;

struct Links<'a> {
    image_paths: &'a HashMap<String, String>,
    api_base: Option<&'a str>,
    workspace: Option<&'a str>,
    outline: Option<&'a str>,
    cover_path: Option<&'a str>,
    approximated: Cell<bool>,
    export: Option<ExportContext<'a>>,
}

#[derive(Clone, Copy)]
struct ExportContext<'a> {
    format: PageExportFormat,
    details: &'a RefCell<MarkdownDetails>,
}

#[derive(Default)]
struct MarkdownDetails {
    losses: BTreeSet<(String, String, &'static str)>,
    locations: BTreeSet<(String, String, String)>,
}

impl Links<'_> {
    fn for_html(&self) -> bool {
        self.export
            .is_some_and(|export| export.format == PageExportFormat::Html)
    }

    fn note(&self, block: &Block, detail: &'static str) {
        if let Some(export) = self.export {
            export.details.borrow_mut().losses.insert((
                block.id.clone(),
                block.kind.clone(),
                detail,
            ));
        }
    }

    fn approximate(&self, block: &Block, detail: &'static str) {
        self.approximated.set(true);
        self.note(block, detail);
    }

    fn map_location(&self, block: &Block, label: &str, location: PageLocation<'_>) {
        if let Some(export) = self.export {
            export.details.borrow_mut().locations.insert((
                block.id.clone(),
                label.to_owned(),
                location.to_string(),
            ));
        }
    }
}

pub struct MarkdownExport {
    pub markdown: String,
    pub report: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PageExportFormat {
    Markdown,
    Html,
}

impl PageExportFormat {
    pub fn label(self) -> &'static str {
        match self {
            Self::Markdown => "Markdown",
            Self::Html => "HTML",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Markdown => "md",
            Self::Html => "html",
        }
    }
}

pub struct PageExport {
    pub document: String,
    pub report: String,
}

pub fn render_markdown_export(
    page: &Page,
    assets: &[(Attachment, String)],
    workspace: &str,
) -> MarkdownExport {
    let PageExport { document, report } =
        render_page_export(page, assets, workspace, PageExportFormat::Markdown);
    MarkdownExport {
        markdown: document,
        report,
    }
}

pub fn render_page_export(
    page: &Page,
    assets: &[(Attachment, String)],
    workspace: &str,
    format: PageExportFormat,
) -> PageExport {
    let paths = assets
        .iter()
        .map(|(asset, path)| (asset.id.clone(), path.clone()))
        .collect();
    let details = RefCell::new(MarkdownDetails::default());
    let markdown = page_markdown(
        page,
        &paths,
        workspace,
        Some(ExportContext {
            format,
            details: &details,
        }),
    );
    let document = match format {
        PageExportFormat::Markdown => markdown,
        PageExportFormat::Html => {
            crate::html_export::render_document(&page.info.title, &markdown, assets)
        }
    };
    let details = details.into_inner();
    let mut report = format!(
        "知页 {} 导出报告\n\n页面  {:?}\n页面 ID  {}\n修订  {}\n\n导出范围\n\
         - 当前页标题、正文及正文或封面引用的附件。\n\
         - 使用原生工作空间包恢复页面树、页面与块身份、图标、其他附件、数据库、属性及修订历史。\n",
        format.label(),
        page.info.title,
        page.info.id,
        page.info.revision,
    );
    if page.cover.is_some() {
        report.push_str("- 页面封面按正文图片导出。原生包保留封面设置。\n");
    }
    report.push_str("\n正文转换\n");
    if details.losses.is_empty() {
        report.push_str("- 无需要单列的块或行内转换。\n");
    }
    for (id, kind, detail) in details.losses {
        let _ = writeln!(report, "- {id}（{kind:?}） {detail}");
    }
    report.push_str("\n链接映射（正文块 ID、显示名称、知页源目标）\n");
    for (id, label, location) in details.locations {
        let _ = writeln!(report, "- {id}\t{label:?}\t{location}");
    }
    let _ = writeln!(
        report,
        "\n附件清单（{} 份，名称、相对路径、SHA-256）",
        assets.len()
    );
    for (asset, path) in assets {
        let _ = writeln!(report, "- {:?}\t{path}\t{}", asset.name, asset.sha256);
    }
    PageExport { document, report }
}

pub fn render_markdown(page: &Page) -> String {
    render_markdown_document(&page.info.title, &page.blocks)
}

pub fn render_markdown_document(title: &str, blocks: &[Block]) -> String {
    render_markdown_with_images(title, blocks, &HashMap::new())
}

pub fn render_markdown_with_images(
    title: &str,
    blocks: &[Block],
    image_paths: &HashMap<String, String>,
) -> String {
    render_workspace_markdown(title, blocks, image_paths, None)
}

pub fn render_workspace_markdown(
    title: &str,
    blocks: &[Block],
    image_paths: &HashMap<String, String>,
    workspace: Option<&str>,
) -> String {
    render_local_markdown(title, blocks, image_paths, workspace, None, None)
}

pub fn render_page_markdown(
    page: &Page,
    image_paths: &HashMap<String, String>,
    workspace: &str,
) -> String {
    page_markdown(page, image_paths, workspace, None)
}

fn page_markdown(
    page: &Page,
    image_paths: &HashMap<String, String>,
    workspace: &str,
    export: Option<ExportContext<'_>>,
) -> String {
    let cover = page
        .cover
        .as_ref()
        .and_then(|cover| image_paths.get(&cover.attachment_id));
    render_local_markdown(
        &page.info.title,
        &page.blocks,
        image_paths,
        Some(workspace),
        cover.map(String::as_str),
        export,
    )
}

fn render_local_markdown(
    title: &str,
    blocks: &[Block],
    image_paths: &HashMap<String, String>,
    workspace: Option<&str>,
    cover_path: Option<&str>,
    export: Option<ExportContext<'_>>,
) -> String {
    let outline = crate::integrations::visible_blocks(blocks)
        .any(crate::rich_blocks::is_outline)
        .then(|| outline_list(blocks));
    render(
        Some(title),
        blocks,
        &Links {
            image_paths,
            api_base: None,
            workspace,
            outline: outline.as_deref(),
            cover_path,
            approximated: Cell::new(false),
            export,
        },
    )
}

pub(crate) fn render_projected_markdown(
    title: Option<&str>,
    blocks: &[Block],
    workspace: &str,
) -> (String, bool) {
    let base = format!("/api/v1/workspaces/{workspace}");
    let links = Links {
        image_paths: &HashMap::new(),
        api_base: Some(&base),
        workspace: None,
        outline: None,
        cover_path: None,
        approximated: Cell::new(false),
        export: None,
    };
    (render(title, blocks, &links), links.approximated.get())
}

fn outline_list(blocks: &[Block]) -> String {
    let mut items = Vec::new();
    let mut ancestors = Vec::new();
    for block in crate::integrations::visible_blocks(blocks).filter(|block| block.kind == "heading")
    {
        if let Some(inline) = crate::integrations::reference_inline(block) {
            let title = inline
                .iter()
                .filter_map(|part| match part["type"].as_str() {
                    Some("text") => part["text"].as_str(),
                    Some("pageReference") => part["label"].as_str(),
                    Some("hardBreak") => Some(" "),
                    _ => None,
                })
                .collect::<String>();
            if !title.trim().is_empty() {
                let level = block.payload["level"].as_u64().unwrap_or(1);
                while ancestors.last().is_some_and(|previous| *previous >= level) {
                    ancestors.pop();
                }
                let label = title.split_whitespace().collect::<Vec<_>>().join(" ");
                items.push(format!(
                    "{}- {}",
                    "  ".repeat(ancestors.len()),
                    escape(&label)
                ));
                ancestors.push(level);
            }
        }
    }
    items.join("\n")
}

fn render(title: Option<&str>, blocks: &[Block], links: &Links<'_>) -> String {
    let mut result = title.map_or_else(String::new, |title| {
        let title = if title.trim().is_empty() {
            "无标题"
        } else {
            title.trim()
        };
        format!("# {}\n", escape(title))
    });
    if let Some(cover) = links.cover_path {
        result.push_str(&format!("\n![页面封面]({cover})\n"));
    }
    for block in blocks {
        result.push('\n');
        result.push_str(&render_block(block, links));
        result.push('\n');
    }
    result
}

fn escape(text: &str) -> String {
    let mut escaped = String::new();
    for character in text.chars() {
        if "\\`*_[]<>&#|~!-".contains(character) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

fn fence(text: &str, min: usize) -> String {
    let max_run = text
        .split(|character| character != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    "`".repeat(min.max(max_run + 1))
}

fn fallback(block: &Block, links: &Links<'_>) -> String {
    links.note(
        block,
        "正文使用占位提示，原始块数据可通过原生工作空间包恢复。",
    );
    let label = escape(&block.kind);
    format!("> [保留块：{label}。请使用原生工作空间包保留完整数据。]")
}

fn inline(block: &Block, links: &Links<'_>) -> String {
    let mut output = String::new();
    let Some(items) = block.payload["inline"].as_array() else {
        links.note(block, "无法转换此块的行内数据。");
        return output;
    };
    for item in items {
        match item.get("type").and_then(Value::as_str) {
            Some("hardBreak") => output.push_str("  \n"),
            Some("pageReference") => {
                let label = item.get("label").and_then(Value::as_str).unwrap_or("页面");
                let target = item["pageId"]
                    .as_str()
                    .filter(|id| crate::model::valid_id(id));
                if let (Some(base), Some(id)) = (links.api_base, target) {
                    output.push_str(&format!("[{}](<{base}/pages/{id}>)", escape(label)));
                } else if let (Some(workspace), Some(id)) = (links.workspace, target) {
                    let location = PageLocation::page(workspace, id);
                    links.map_location(block, label, location);
                    if links.for_html() {
                        output.push_str(&format!("[{}](<{location}>)", escape(label)));
                    } else {
                        links.note(block, "页面引用保留显示名称，知页目标见链接映射。");
                        output.push_str(&format!("[[{}]]", escape(label)));
                    }
                } else {
                    output.push_str(&format!("[[{}]]", escape(label)));
                }
            }
            Some("restricted") if links.api_base.is_some() => output.push_str("［不可访问］"),
            Some("text") => {
                let text = item.get("text").and_then(Value::as_str).unwrap_or_default();
                let mut part = escape(text);
                if let Some(marks) = item.get("marks").and_then(Value::as_array) {
                    for mark in marks {
                        part = match mark.get("type").and_then(Value::as_str) {
                            Some("bold") => format!("**{part}**"),
                            Some("italic") => format!("*{part}*"),
                            Some("strike") => format!("~~{part}~~"),
                            Some("underline") => {
                                if !links.for_html() {
                                    links.note(block, "下划线使用行内 HTML，显示与阅读器有关。");
                                }
                                format!("<u>{part}</u>")
                            }
                            Some("code") => {
                                let marker = fence(text, 1);
                                let pad =
                                    if text.starts_with(['`', ' ']) || text.ends_with(['`', ' ']) {
                                        " "
                                    } else {
                                        ""
                                    };
                                format!("{marker}{pad}{text}{pad}{marker}")
                            }
                            // Comments stay in 知页; exports keep the commented text, as in Notion.
                            Some("comment") => part,
                            // Markdown has no colors; like Notion's export, keep the text.
                            // HTML keeps them as spans styled by the exported page.
                            Some(kind @ ("textColor" | "backgroundColor")) => {
                                match mark["color"].as_str() {
                                    Some(color)
                                        if links.for_html()
                                            && crate::rich_blocks::inline_color(color) =>
                                    {
                                        let attribute = if kind == "textColor" {
                                            "data-text-color"
                                        } else {
                                            "data-background-color"
                                        };
                                        format!("<span {attribute}=\"{color}\">{part}</span>")
                                    }
                                    _ => part,
                                }
                            }
                            Some("link") => {
                                if let Some(url) = mark
                                    .get("href")
                                    .and_then(Value::as_str)
                                    .and_then(crate::web_url)
                                {
                                    let url = url
                                        .replace('<', "%3C")
                                        .replace('>', "%3E")
                                        .replace('|', "%7C");
                                    format!("[{part}](<{url}>)")
                                } else {
                                    links.note(block, "无法转换的网页链接仅保留显示文字。");
                                    part
                                }
                            }
                            _ => {
                                links.note(block, "无法转换的行内格式仅保留文字。");
                                part
                            }
                        };
                    }
                }
                output.push_str(&part);
            }
            _ => {
                links.note(block, "无法转换的行内内容使用占位提示。");
                output.push_str("[保留的行内内容]");
            }
        }
    }
    match block.payload["color"].as_str() {
        Some(color) if links.for_html() && crate::rich_blocks::block_color(color) => {
            format!("<span data-block-color=\"{color}\">{output}</span>")
        }
        _ => output,
    }
}

/// A paragraph or heading followed by its child blocks (Notion's indented blocks).
fn with_children(block: &Block, text: String, links: &Links<'_>) -> String {
    if block.children.is_empty() {
        return text;
    }
    links.approximate(block, "子块在 Markdown 中按顺序跟在原块之后。");
    let children = block
        .children
        .iter()
        .map(|child| render_block(child, links))
        .collect::<Vec<_>>()
        .join("\n\n");
    format!("{text}\n\n{children}")
}

fn render_block(block: &Block, links: &Links<'_>) -> String {
    if block.schema_version != 1 || !block.extra.is_empty() {
        return fallback(block, links);
    }
    match block.kind.as_str() {
        "restricted" if links.api_base.is_some() => "> ［不可访问］".into(),
        "toggleBlock" | "callout" => {
            let heading = if block.kind == "toggleBlock" {
                let Some(title) = crate::rich_blocks::toggle_title(block) else {
                    return fallback(block, links);
                };
                links.approximate(block, "折叠块转为引用，展开状态由原生包保留。");
                format!(
                    "**{}**",
                    escape(if title.trim().is_empty() {
                        "折叠内容"
                    } else {
                        title
                    })
                )
            } else {
                let marker = match crate::rich_blocks::callout_tone(block) {
                    Some("tip") => "TIP",
                    Some("warning") => "WARNING",
                    Some("note") => "NOTE",
                    _ => return fallback(block, links),
                };
                links.approximate(block, "提示块转为引用，提示样式由阅读器解释。");
                format!("[!{marker}]")
            };
            let body = block
                .children
                .iter()
                .map(|child| render_block(child, links))
                .collect::<Vec<_>>()
                .join("\n\n");
            format!(
                "> {heading}\n>\n{}",
                body.lines()
                    .map(|line| format!("> {line}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }
        "mathBlock" => {
            let Some(tex) = crate::rich_blocks::latex(block) else {
                return fallback(block, links);
            };
            links.approximate(block, "公式以代码块保留 LaTeX 源码。");
            let delimiter = fence(tex, 3);
            format!("{delimiter}math\n{tex}\n{delimiter}")
        }
        "tableOfContents" if crate::rich_blocks::is_outline(block) => {
            links.approximate(block, "动态目录转为静态标题列表或占位。");
            links.outline.filter(|text| !text.is_empty()).map_or_else(
                || "> ［目录按当前页面标题生成］".into(),
                |entries| format!("**目录**\n\n{entries}"),
            )
        }
        "attachmentFile" => {
            let Some(id) = crate::rich_blocks::attachment_file(block) else {
                return fallback(block, links);
            };
            links.approximate(block, "附件卡片转为普通链接。");
            if let Some(base) = links.api_base {
                return format!("[附件](<{base}/assets/{id}/content>)");
            }
            links.image_paths.get(id).map_or_else(
                || {
                    links.note(block, "未复制附件原件，正文显示占位。");
                    "> ［附件原件随工作空间包保留］".into()
                },
                |path| format!("[附件](<{path}>)"),
            )
        }
        "blockReference" => {
            let Some(target) = crate::references::block_target(block) else {
                return fallback(block, links);
            };
            links.approximate(block, "块引用转为定位链接，链接仍指向知页中的源块。");
            if let Some(base) = links.api_base {
                return format!("> [块引用](<{base}/blocks/{}>)", target.block_id);
            }
            if let Some(workspace) = links.workspace {
                let link = PageLocation::page(workspace, target.page_id).block(target.block_id);
                links.map_location(block, "块引用", link);
                return format!("> [块引用](<{link}>)");
            }
            "> ［块引用］".into()
        }
        "paragraph" => with_children(block, inline(block, links), links),
        "collectionView" => {
            let Some((collection, _)) = crate::collections::views::view_target(block) else {
                return fallback(block, links);
            };
            links.approximate(
                block,
                "嵌入数据库视图转为占位，数据和视图配置由原生包保留。",
            );
            if let Some(base) = links.api_base {
                return format!("> [数据库视图](<{base}/collections/{collection}>)");
            }
            "> ［数据库视图随原生工作空间包保留］".into()
        }
        "heading" => {
            let level = block.payload["level"].as_u64().unwrap_or(1).clamp(1, 3) as usize;
            let text = format!("{} {}", "#".repeat(level + 1), inline(block, links));
            with_children(block, text, links)
        }
        "horizontalRule" => "---".into(),
        "attachmentImage" => {
            let Some(image) = crate::rich_blocks::image(block) else {
                return fallback(block, links);
            };
            if let Some(base) = links.api_base {
                return format!(
                    "![{}](<{base}/assets/{}/content>)",
                    escape(image.alt),
                    image.attachment_id
                );
            }
            match links.image_paths.get(image.attachment_id) {
                Some(path) => format!("![{}]({})", escape(image.alt), path),
                None => {
                    links.note(block, "未复制图片原件，正文显示占位和图片说明。");
                    format!("> [图片原件随工作空间包保留] {}", escape(image.alt))
                }
            }
        }
        "table" => render_table(block, links).unwrap_or_else(|| {
            links.approximated.set(true);
            fallback(block, links)
        }),
        "codeBlock" => {
            let body = block.payload["text"].as_str().unwrap_or_default();
            let delimiter = fence(body, 3);
            let language = block.payload["language"].as_str().unwrap_or_default();
            let language = if language
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                language
            } else {
                links.note(block, "代码围栏保留正文，省略无法安全写入的语言标记。");
                ""
            };
            format!("{delimiter}{language}\n{body}\n{delimiter}")
        }
        "indentBlock" | "columnList" | "column" => {
            links.approximate(block, "缩进与分栏在 Markdown 中按顺序展开为普通内容。");
            block
                .children
                .iter()
                .map(|child| render_block(child, links))
                .collect::<Vec<_>>()
                .join("\n\n")
        }
        "bookmark" => {
            let Some((url, title)) = crate::rich_blocks::bookmark(block) else {
                return fallback(block, links);
            };
            let url = url
                .replace('<', "%3C")
                .replace('>', "%3E")
                .replace('|', "%7C");
            let label = title.filter(|text| !text.trim().is_empty()).unwrap_or(&url);
            format!("[{}](<{url}>)", escape(label))
        }
        "blockquote" => block
            .children
            .iter()
            .map(|child| render_block(child, links))
            .collect::<Vec<_>>()
            .join("\n\n")
            .split('\n')
            .map(|line| format!("> {line}"))
            .collect::<Vec<_>>()
            .join("\n"),
        "bulletList" | "orderedList" | "taskList" => {
            let start = block.payload["start"].as_i64().unwrap_or(1);
            if block.kind == "orderedList"
                && block.payload["type"]
                    .as_str()
                    .is_some_and(|kind| kind != "1")
            {
                links.approximate(block, "有序列表的字母或罗马数字编号转为阿拉伯数字。");
            }
            block
                .children
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let prefix = match block.kind.as_str() {
                        "orderedList" => format!("{}. ", start.saturating_add(index as i64)),
                        "taskList" => {
                            if item.payload["checked"].as_bool() == Some(true) {
                                "- [x] ".into()
                            } else {
                                "- [ ] ".into()
                            }
                        }
                        _ => "- ".into(),
                    };
                    let content = item
                        .children
                        .iter()
                        .map(|child| render_block(child, links))
                        .collect::<Vec<_>>()
                        .join("\n");
                    let indentation = " ".repeat(prefix.chars().count());
                    let mut lines = content.split('\n');
                    let mut formatted = format!("{prefix}{}", lines.next().unwrap_or_default());
                    for line in lines {
                        formatted.push_str(&format!("\n{indentation}{line}"));
                    }
                    formatted
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => {
            links.approximated.set(true);
            fallback(block, links)
        }
    }
}

fn render_table(table: &Block, links: &Links<'_>) -> Option<String> {
    let row = table.children.first()?;
    if table.children.iter().any(|row| {
        row.kind != "tableRow"
            || row.schema_version != 1
            || !row.extra.is_empty()
            || row.children.iter().any(|cell| {
                !matches!(cell.kind.as_str(), "tableCell" | "tableHeader")
                    || cell.schema_version != 1
                    || !cell.extra.is_empty()
            })
    }) {
        return None;
    }
    let width = table.children.iter().map(|row| row.children.len()).max()?;
    if width == 0 {
        return None;
    }
    let header = row.children.iter().all(|cell| cell.kind == "tableHeader");
    let complex = table.children.iter().enumerate().any(|(index, row)| {
        row.children.len() != width
            || row.children.iter().enumerate().any(|(column, cell)| {
                cell.payload["colspan"].as_u64().unwrap_or(1) != 1
                    || cell.payload["rowspan"].as_u64().unwrap_or(1) != 1
                    || cell.payload.get("colwidth").is_some()
                    || (index != 0 || !header) && cell.kind == "tableHeader"
                    || row_alignment(table, column)
                        != crate::rich_blocks::table_alignment(&cell.payload["align"])
                    || cell.children.len() != 1
                    || cell.children[0].kind != "paragraph"
            })
    });
    let render_row = |row: Option<&Block>| {
        let values = (0..width)
            .map(|column| {
                row.and_then(|row| row.children.get(column))
                    .map(|cell| {
                        cell.children
                            .iter()
                            .map(|child| render_block(child, links))
                            .map(|content| table_cell_text(&content))
                            .collect::<Vec<_>>()
                            .join("<br>")
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>();
        format!("| {} |", values.join(" | "))
    };
    let alignment = (0..width)
        .map(|column| match row_alignment(table, column) {
            Some("left") => ":---",
            Some("center") => ":---:",
            Some("right") => "---:",
            _ => "---",
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let mut lines = vec![
        render_row(header.then_some(row)),
        format!("| {alignment} |"),
    ];
    for row in table.children.iter().skip(usize::from(header)) {
        lines.push(render_row(Some(row)));
    }
    if complex {
        if !links.for_html() {
            lines.push("\n> 导出提示：表格内容已转为普通 Markdown 行列。原始单元格结构与格式可从工作空间包恢复。".into());
        }
        links.approximate(
            table,
            "复杂表格转为普通行列，原生包保留完整单元格结构与格式。",
        );
    }
    if !header {
        links.approximate(table, "无独立表头的表格在导出时补一个空表头。");
    }
    Some(lines.join("\n"))
}

fn row_alignment(table: &Block, column: usize) -> Option<&'static str> {
    crate::rich_blocks::table_alignment(
        &table.children.first()?.children.get(column)?.payload["align"],
    )
}

fn table_cell_text(text: &str) -> String {
    let mut content = String::new();
    let mut escaped = false;
    for c in text.chars() {
        match c {
            '|' if !escaped => content.push_str("\\|"),
            '\r' => (),
            '\n' => content.push_str("<br>"),
            _ => content.push(c),
        }
        escaped = c == '\\' && !escaped;
    }
    content
}
