// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::model::valid_id;
use crate::{Attachment, Block, Error};
use serde_json::Value;

pub(crate) struct AttachedImage<'a> {
    pub attachment_id: &'a str,
    pub media_type: &'a str,
    pub alt: &'a str,
}

pub fn image_extension(media_type: &str) -> Option<&'static str> {
    match media_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/gif" => Some("gif"),
        "image/webp" => Some("webp"),
        "image/bmp" => Some("bmp"),
        _ => None,
    }
}

pub fn web_url(input: &str) -> Option<String> {
    if input.len() > 4096 || input.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let parsed = url::Url::parse(input).ok()?;
    (matches!(parsed.scheme(), "http" | "https") && parsed.has_host()).then(|| parsed.into())
}

pub(crate) fn image(block: &Block) -> Option<AttachedImage<'_>> {
    if block.kind != "attachmentImage"
        || block.schema_version != 1
        || !block.children.is_empty()
        || !block.extra.is_empty()
        || block.payload.as_object()?.len() != 3
    {
        return None;
    }
    let attachment_id = block.payload["attachmentId"]
        .as_str()
        .filter(|id| valid_id(id))?;
    let media_type = block.payload["mediaType"]
        .as_str()
        .filter(|mime| image_extension(mime).is_some())?;
    let alt = block.payload["alt"]
        .as_str()
        .filter(|alt| alt.chars().count() <= 500 && !alt.chars().any(char::is_control))?;
    Some(AttachedImage {
        attachment_id,
        media_type,
        alt,
    })
}

pub(crate) struct AttachmentReference<'a> {
    pub id: &'a str,
    pub media_type: Option<&'a str>,
}

impl AttachmentReference<'_> {
    pub fn matches(&self, asset: &Attachment, page: &str) -> bool {
        asset.id == self.id
            && asset.page_id == page
            && self.media_type.is_none_or(|kind| asset.media_type == kind)
    }

    pub fn missing(&self) -> Error {
        if self.media_type.is_some() {
            Error::new(
                "IMAGE_ATTACHMENT",
                "图片引用的原件不存在、格式不符或不属于此页面。",
            )
        } else {
            Error::new("BLOCK_ATTACHMENT", "此附件块的原件不存在或不属于此页面。")
        }
    }
}

pub(crate) fn attachment_file(block: &Block) -> Option<&str> {
    if block.kind != "attachmentFile"
        || !v1(block)
        || !block.children.is_empty()
        || block.payload.as_object()?.len() != 1
    {
        return None;
    }
    block.payload["attachmentId"]
        .as_str()
        .filter(|id| valid_id(id))
}

pub(crate) fn attachment_reference(block: &Block) -> Option<AttachmentReference<'_>> {
    if block.kind == "bookmark" {
        return bookmark(block).and_then(|_| bookmark_image(block));
    }
    if let Some(image) = image(block) {
        return Some(AttachmentReference {
            id: image.attachment_id,
            media_type: Some(image.media_type),
        });
    }
    Some(AttachmentReference {
        id: attachment_file(block)?,
        media_type: None,
    })
}

pub(crate) fn attachment_references(
    blocks: &[Block],
) -> impl Iterator<Item = AttachmentReference<'_>> {
    crate::references::walk(blocks).filter_map(attachment_reference)
}

fn v1(block: &Block) -> bool {
    block.schema_version == 1 && block.extra.is_empty()
}

pub(crate) fn toggle_title(block: &Block) -> Option<&str> {
    if block.kind != "toggleBlock"
        || !v1(block)
        || block.children.is_empty()
        || block.payload.as_object()?.len() != 2
        || !block.payload["collapsed"].is_boolean()
    {
        return None;
    }
    block.payload["title"]
        .as_str()
        .filter(|text| text.len() <= 512 && !text.chars().any(char::is_control))
}

pub(crate) fn callout_tone(block: &Block) -> Option<&str> {
    if block.kind != "callout"
        || !v1(block)
        || block.children.is_empty()
        || block.payload.as_object()?.len() != 1
    {
        return None;
    }
    block.payload["tone"]
        .as_str()
        .filter(|tone| matches!(*tone, "note" | "tip" | "warning"))
}

/// Notion's nine inline colors, used by the `textColor` and `backgroundColor` marks.
pub(crate) fn inline_color(color: &str) -> bool {
    matches!(
        color,
        "gray" | "brown" | "orange" | "yellow" | "green" | "blue" | "purple" | "pink" | "red"
    )
}

/// A personal comment on a text range, kept as an inline mark: 1 to 2,000 characters.
pub(crate) fn comment_text(value: &serde_json::Value) -> Option<&str> {
    value
        .as_str()
        .filter(|text| !text.trim().is_empty() && text.chars().count() <= 2000)
}

/// A bookmark card: a web address with an optional title typed by the user (nothing is fetched).
pub(crate) fn bookmark(block: &Block) -> Option<(String, Option<&str>)> {
    if block.kind != "bookmark" || !v1(block) || !block.children.is_empty() {
        return None;
    }
    let fields = block.payload.as_object()?;
    let url = web_url(block.payload["url"].as_str()?)?;
    // Optional texts: the title (500 characters) and the page's description (1,000).
    let text = |key: &str, limit: usize| match block.payload.get(key) {
        None => Some(None),
        Some(value) => value
            .as_str()
            .filter(|text| text.chars().count() <= limit && !text.chars().any(char::is_control))
            .map(Some),
    };
    let title = text("title", 500)?;
    let description = text("description", 1000)?;
    // A preview image saved as this page's attachment travels with `attachmentId` and `mediaType`.
    let image = match (
        block.payload.get("attachmentId"),
        block.payload.get("mediaType"),
    ) {
        (None, None) => 0,
        (Some(_), Some(_)) => {
            bookmark_image(block)?;
            2
        }
        _ => return None,
    };
    (fields.len() == 1 + usize::from(title.is_some()) + usize::from(description.is_some()) + image)
        .then_some((url, title))
}

/// The preview image a bookmark card keeps as an attachment of its page.
pub(crate) fn bookmark_image(block: &Block) -> Option<AttachmentReference<'_>> {
    if block.kind != "bookmark" || !v1(block) {
        return None;
    }
    let id = block.payload["attachmentId"]
        .as_str()
        .filter(|id| valid_id(id))?;
    let media_type = block.payload["mediaType"]
        .as_str()
        .filter(|mime| image_extension(mime).is_some())?;
    Some(AttachmentReference {
        id,
        media_type: Some(media_type),
    })
}

/// A whole paragraph or heading color: `red` for text or `red_background`.
pub(crate) fn block_color(color: &str) -> bool {
    let (name, background) = color
        .strip_suffix("_background")
        .map_or((color, false), |name| (name, true));
    inline_color(name) && (background || !color.contains('_'))
}

pub(crate) fn latex(block: &Block) -> Option<&str> {
    if block.kind != "mathBlock"
        || !v1(block)
        || !block.children.is_empty()
        || block.payload.as_object()?.len() != 1
    {
        return None;
    }
    block.payload["latex"]
        .as_str()
        .filter(|text| text.len() <= 4096 && !text.contains('\0'))
}

pub(crate) fn is_outline(block: &Block) -> bool {
    block.kind == "tableOfContents"
        && v1(block)
        && block.children.is_empty()
        && block
            .payload
            .as_object()
            .is_some_and(|payload| payload.is_empty())
}

pub(crate) fn table_alignment(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "left" => Some("left"),
        "center" => Some("center"),
        "right" => Some("right"),
        _ => None,
    }
}
