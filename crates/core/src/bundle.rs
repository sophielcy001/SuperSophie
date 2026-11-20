// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::model::{name, valid_icon, valid_id, validate_blocks};
use crate::{Error, MAX_BUNDLE_BYTES, Page, Result, Workspace};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const FORMAT: &str = "zhiye.workspace";
const VERSION: u32 = 1;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Envelope {
    format: String,
    format_version: u32,
    sha256: String,
    data: Value,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct BundleData {
    pub workspace: Workspace,
    pub pages: Vec<Page>,
    pub revisions: Vec<Page>,
}

fn checksum(data: &mut Value) -> Result<String> {
    data.sort_all_objects();
    let bytes = serde_json::to_vec(data)?;
    Ok(crate::assets::hex_sha256(&Sha256::digest(bytes)))
}

pub(crate) fn encode_envelope(bundle: &impl Serialize, version: u32) -> Result<Vec<u8>> {
    let mut data = serde_json::to_value(bundle)?;
    let bytes = serde_json::to_vec_pretty(&Envelope {
        format: FORMAT.into(),
        format_version: version,
        sha256: checksum(&mut data)?,
        data,
    })?;
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(Error::new("BUNDLE_TOO_LARGE", "工作空间包超过 64 MiB。"));
    }
    Ok(bytes)
}

pub(crate) fn decode_envelope<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    versions: &[u32],
) -> Result<(T, u32)> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(Error::new("BUNDLE_TOO_LARGE", "工作空间包超过 64 MiB。"));
    }
    let mut envelope: Envelope = serde_json::from_slice(bytes)?;
    if envelope.format != FORMAT || !versions.contains(&envelope.format_version) {
        return Err(Error::new(
            "FORMAT_VERSION",
            "此版本的知页不能打开该工作空间包。",
        ));
    }
    if checksum(&mut envelope.data)? != envelope.sha256 {
        return Err(Error::new(
            "CHECKSUM",
            "工作空间包校验失败，文件可能不完整或已改动。",
        ));
    }
    Ok((
        serde_json::from_value(envelope.data)?,
        envelope.format_version,
    ))
}

pub(crate) fn encode(bundle: &BundleData) -> Result<Vec<u8>> {
    validate_attachments(bundle, &[])?;
    encode_envelope(bundle, VERSION)
}

pub(crate) fn decode(bytes: &[u8]) -> Result<BundleData> {
    let (bundle, _): (BundleData, _) = decode_envelope(bytes, &[VERSION])?;
    validate(&bundle)?;
    validate_attachments(&bundle, &[])?;
    if bundle
        .pages
        .iter()
        .chain(&bundle.revisions)
        .any(|page| page.properties.is_some() || !page.historical_properties.is_empty())
    {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "旧版 JSON 包不支持数据库记录。",
        ));
    }
    Ok(bundle)
}

pub(crate) fn validate_attachments(
    bundle: &BundleData,
    assets: &[crate::Attachment],
) -> Result<()> {
    let by_id: HashMap<_, _> = assets
        .iter()
        .map(|asset| (asset.id.as_str(), asset))
        .collect();
    for page in bundle.pages.iter().chain(&bundle.revisions) {
        for reference in crate::rich_blocks::attachment_references(&page.blocks) {
            if !by_id
                .get(reference.id)
                .is_some_and(|asset| reference.matches(asset, &page.info.id))
            {
                return Err(reference.missing());
            }
        }
        if let Some(cover) = &page.cover
            && !by_id
                .get(cover.attachment_id.as_str())
                .is_some_and(|asset| cover.matches(asset, &page.info.id))
        {
            return Err(crate::assets::invalid_page_cover());
        }
    }
    Ok(())
}

pub(crate) fn validate(bundle: &BundleData) -> Result<()> {
    name(&bundle.workspace.name, 64, None)?;
    if !valid_id(&bundle.workspace.id) {
        return Err(Error::new("INVALID_BUNDLE", "工作空间标识无效。"));
    }

    let mut by_id = HashMap::new();
    let mut all_blocks = HashSet::new();
    for page in &bundle.pages {
        validate_page(page)?;
        if !page.historical_properties.is_empty() {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "当前页面不能使用历史属性定义。",
            ));
        }
        if by_id.insert(page.info.id.as_str(), page).is_some() {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "工作空间包中存在重复的页面标识。",
            ));
        }
        for id in validate_blocks(&page.blocks)? {
            if !all_blocks.insert(id) {
                return Err(Error::new(
                    "INVALID_BLOCK",
                    "不同页面不能使用相同的块标识。",
                ));
            }
        }
    }

    let mut checked = HashSet::new();
    for page in &bundle.pages {
        let parent = page
            .info
            .parent_id
            .as_deref()
            .and_then(|id| by_id.get(id).copied());
        if page.info.parent_id.is_some() && parent.is_none() {
            return Err(Error::new("INVALID_BUNDLE", "工作空间包中有页面缺少父页。"));
        }
        match page.info.trash_root_id.as_deref() {
            None if parent.is_some_and(|item| item.info.trash_root_id.is_some()) => {
                return Err(Error::new(
                    "INVALID_BUNDLE",
                    "正常页面不能位于已移入回收站的页面下。",
                ));
            }
            Some(root)
                if root != page.info.id
                    && parent.and_then(|p| p.info.trash_root_id.as_deref()) != Some(root) =>
            {
                return Err(Error::new("INVALID_BUNDLE", "回收站中的页面批次不完整。"));
            }
            _ => {}
        }

        let mut chain = HashSet::new();
        let mut node = Some(page);
        while let Some(item) = node {
            let id = item.info.id.as_str();
            if checked.contains(id) {
                break;
            }
            if !chain.insert(id) {
                return Err(Error::new(
                    "PAGE_CYCLE",
                    "工作空间包中的页面层级形成了循环。",
                ));
            }
            node = item
                .info
                .parent_id
                .as_deref()
                .and_then(|parent| by_id.get(parent).copied());
        }
        checked.extend(chain);
    }

    let mut revisions = HashSet::new();
    let mut current = HashSet::new();
    for snapshot in &bundle.revisions {
        validate_page(snapshot)?;
        validate_blocks(&snapshot.blocks)?;
        let id = snapshot.info.id.as_str();
        let Some(page) = by_id.get(id) else {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "修订记录不属于此包中的任何页面。",
            ));
        };
        if snapshot.info.revision > page.info.revision
            || !revisions.insert((id, snapshot.info.revision))
        {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "工作空间包中的修订号无效或重复。",
            ));
        }
        if snapshot == *page {
            current.insert(id);
        }
    }
    if current.len() != bundle.pages.len() {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "工作空间包中缺少与当前页面相符的修订。",
        ));
    }
    Ok(())
}

fn validate_page(page: &Page) -> Result<()> {
    name(&page.info.title, 200, None)?;
    if !valid_id(&page.info.id)
        || !valid_icon(&page.info.icon)
        || page.info.revision == 0
        || page.info.updated_at < 0
        || page
            .info
            .parent_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
        || page
            .info
            .trash_root_id
            .as_deref()
            .is_some_and(|id| !valid_id(id))
    {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "工作空间包中的页面元数据无效。",
        ));
    }
    Ok(())
}
