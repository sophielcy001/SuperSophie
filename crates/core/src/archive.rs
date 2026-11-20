// SPDX-License-Identifier: AGPL-3.0-or-later

mod merge;
pub use merge::ArchiveMerge;

use crate::assets::{
    blob_directory, check_content, insert_attachment, persist_blob, read_attachments, transfer,
    valid_attachment,
};
use crate::bundle::{self, BundleData};
use crate::collections::{
    CollectionSnapshot, restore_collections, snapshot_collections, validate_collections,
};
use crate::imports::{ImportReceipt, restore_receipts, snapshot_receipts, validate_receipts};
use crate::navigation::{
    NavigationSnapshot, restore_navigation, snapshot_navigation, validate_navigation,
};
use crate::store::{recovered_workspace, restore_snapshot, workspace_snapshot};
use crate::templates::{
    Template, restore_templates, snapshot_templates, template_attachments, validate_templates,
};
use crate::{Attachment, Error, MAX_BUNDLE_BYTES, Result, Store, Workspace};
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Read, Seek, SeekFrom, Write},
};
use zip::{CompressionMethod, ZipArchive, ZipWriter, read::ZipFile, write::SimpleFileOptions};

const LEGACY_ARCHIVE_VERSION: u32 = 5;
const COVER_ARCHIVE_VERSION: u32 = 6;
const ARCHIVE_VERSION: u32 = 7;
const TOMBSTONE_ARCHIVE_VERSION: u32 = 8;
const HISTORICAL_PROPERTIES_ARCHIVE_VERSION: u32 = 9;
const MANIFEST: &str = "manifest.json";
const MAX_ATTACHMENTS: usize = 20_000;
const MAX_BLOBS_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: u64 = MAX_BLOBS_BYTES + MAX_BUNDLE_BYTES as u64 + 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ArchiveData {
    content: BundleData,
    attachments: Vec<Attachment>,
    databases: Option<CollectionSnapshot>,
    #[serde(default)]
    library: Option<LibrarySnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    export: Option<ExportManifest>,
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum HistoryScope {
    All,
    Current,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExportManifest {
    scope: String,
    history: HistoryScope,
    downgrades: Vec<String>,
    omissions: Vec<String>,
}

impl ExportManifest {
    fn workspace(include_history: bool) -> Self {
        let mut omissions = [
            "integrations",
            "revisionAttribution",
            "proposals",
            "changeFeed",
            "searchIndexes",
            "computedCache",
            "migrationBatches",
            "backupPolicy",
            "blobQuarantine",
        ]
        .map(str::to_owned)
        .to_vec();
        if !include_history {
            omissions.push("priorRevisions".into());
        }
        Self {
            scope: "workspace".into(),
            history: if include_history {
                HistoryScope::All
            } else {
                HistoryScope::Current
            },
            downgrades: Vec::new(),
            omissions,
        }
    }
}

impl ArchiveData {
    fn uses_historical_properties(&self) -> bool {
        uses_historical_properties(&self.content)
    }

    fn uses_covers(&self) -> bool {
        self.content
            .pages
            .iter()
            .chain(&self.content.revisions)
            .any(|page| page.cover.is_some())
            || self
                .library
                .as_ref()
                .is_some_and(|library| library.templates.iter().any(Template::uses_covers))
    }
}

fn uses_historical_properties(content: &BundleData) -> bool {
    content
        .revisions
        .iter()
        .any(|page| !page.historical_properties.is_empty())
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LibrarySnapshot {
    navigation: NavigationSnapshot,
    import_receipts: Vec<ImportReceipt>,
    templates: Vec<Template>,
}

impl Store {
    pub fn write_workspace_archive(
        &self,
        workspace: &str,
        output: impl Write + Seek,
    ) -> Result<()> {
        self.write_content_archive(workspace, true, false, output)
    }

    pub fn write_workspace_archive_with_history(
        &self,
        workspace: &str,
        include_history: bool,
        output: impl Write + Seek,
    ) -> Result<()> {
        self.write_content_archive(workspace, include_history, true, output)
    }

    fn write_content_archive(
        &self,
        workspace: &str,
        include_history: bool,
        describe_export: bool,
        output: impl Write + Seek,
    ) -> Result<()> {
        let mut db = self.connection(workspace)?;
        let _lease = self.pin_blobs()?;
        let tx = db.transaction()?;
        let databases = snapshot_collections(&tx)?;
        let has_tombstones = !databases.tombstones.is_empty();
        let templates = snapshot_templates(&tx)?;
        validate_templates(&templates, &databases.definitions)?;
        let content = workspace_snapshot(&tx, include_history)?;
        let has_historical_properties = uses_historical_properties(&content);
        let data = ArchiveData {
            content,
            attachments: read_attachments(&tx, None)?,
            databases: Some(databases),
            library: Some(LibrarySnapshot {
                navigation: snapshot_navigation(&tx)?,
                import_receipts: snapshot_receipts(&tx)?,
                templates,
            }),
            export: (describe_export || has_tombstones || has_historical_properties)
                .then(|| ExportManifest::workspace(include_history)),
        };
        let blobs = validate_attachments(&data)?;
        let version = if has_historical_properties {
            HISTORICAL_PROPERTIES_ARCHIVE_VERSION
        } else if has_tombstones {
            TOMBSTONE_ARCHIVE_VERSION
        } else if describe_export {
            ARCHIVE_VERSION
        } else if data.uses_covers() {
            COVER_ARCHIVE_VERSION
        } else {
            LEGACY_ARCHIVE_VERSION
        };
        let manifest = bundle::encode_envelope(&data, version)?;
        let mut zip = ZipWriter::new(output);
        let options = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zip.start_file(MANIFEST, options)?;
        zip.write_all(&manifest)?;
        for (hash, attachment) in blobs {
            zip.start_file(blob_entry(hash), options)?;
            self.copy_blob(workspace, attachment, &mut zip)?;
        }
        zip.finish()?;
        tx.commit()?;
        Ok(())
    }

    pub fn read_workspace_archive(&self, input: impl Read + Seek) -> Result<Workspace> {
        let (mut archive, data) = open_archive(input)?;
        let blobs = validate_attachments(&data)?;
        let workspace = recovered_workspace(&data.content.workspace);
        self.publish_workspace(&workspace, |tx, staging| {
            if !blobs.is_empty() {
                let directory = blob_directory(staging, true)?;
                for attachment in blobs.values() {
                    let file = extract_blob(&mut archive, attachment, &directory)?;
                    persist_blob(file, &directory, attachment)?;
                }
            }
            insert_content(tx, &data)
        })?;
        Ok(workspace)
    }
}

fn open_archive<R: Read + Seek>(mut input: R) -> Result<(ZipArchive<R>, ArchiveData)> {
    if input.seek(SeekFrom::End(0))? > MAX_ARCHIVE_BYTES {
        return Err(Error::new(
            "BUNDLE_TOO_LARGE",
            "工作空间包超过原件或清单上限。",
        ));
    }
    input.rewind()?;
    let mut archive = ZipArchive::new(input)?;
    if archive.len() > MAX_ATTACHMENTS + 1 {
        return Err(Error::new("BUNDLE_TOO_LARGE", "工作空间包包含过多文件。"));
    }
    if archive.index_for_name("recovery.json").is_some() {
        return Err(Error::new(
            "ARCHIVE_FORMAT",
            "整库恢复包请从「从备份恢复」打开。",
        ));
    }
    let mut manifest = Vec::new();
    let entry = archive.by_name(MANIFEST)?;
    check_zip_file(&entry)?;
    if entry.size() > MAX_BUNDLE_BYTES as u64 {
        return Err(Error::new("BUNDLE_TOO_LARGE", "工作空间清单超过 64 MiB。"));
    }
    entry
        .take(MAX_BUNDLE_BYTES as u64 + 1)
        .read_to_end(&mut manifest)?;
    let (data, version): (ArchiveData, _) = bundle::decode_envelope(
        &manifest,
        &[
            2,
            3,
            4,
            LEGACY_ARCHIVE_VERSION,
            COVER_ARCHIVE_VERSION,
            ARCHIVE_VERSION,
            TOMBSTONE_ARCHIVE_VERSION,
            HISTORICAL_PROPERTIES_ARCHIVE_VERSION,
        ],
    )?;
    if data.databases.is_some() != (version >= 3)
        || data.library.is_some() != (version >= 4)
        || data.export.is_some() != (version >= ARCHIVE_VERSION)
        || (version < COVER_ARCHIVE_VERSION && data.uses_covers())
        || (version < HISTORICAL_PROPERTIES_ARCHIVE_VERSION && data.uses_historical_properties())
        || (version < TOMBSTONE_ARCHIVE_VERSION
            && data
                .databases
                .as_ref()
                .is_some_and(|db| !db.tombstones.is_empty()))
    {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "工作空间包的清单与格式版本不符。",
        ));
    }
    if data.export.as_ref().is_some_and(|export| {
        let current_only = export.history == HistoryScope::Current;
        export.scope != "workspace"
            || current_only != export.omissions.iter().any(|item| item == "priorRevisions")
            || (current_only && data.content.revisions.len() != data.content.pages.len())
    }) {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "工作空间包的导出范围与清单不符。",
        ));
    }
    let blobs = validate_contents(&data)?;
    let mut names: HashSet<_> = blobs.keys().map(|hash| blob_entry(hash)).collect();
    names.insert(MANIFEST.into());
    for name in archive.file_names() {
        if !names.remove(name?.as_ref()) {
            return Err(Error::new("INVALID_BUNDLE", "工作空间包包含未声明的文件。"));
        }
    }
    if !names.is_empty() {
        return Err(Error::new("ATTACHMENT_MISSING", "工作空间包缺少附件原件。"));
    }
    Ok((archive, data))
}

fn library_ids<'a>(
    content: &'a BundleData,
    databases: &'a CollectionSnapshot,
) -> (HashSet<&'a str>, HashMap<&'a str, &'a str>) {
    (
        content
            .pages
            .iter()
            .map(|page| page.info.id.as_str())
            .collect(),
        databases
            .definitions
            .iter()
            .map(|item| (item.id.as_str(), item.container_page_id.as_str()))
            .collect(),
    )
}

fn validate_contents(data: &ArchiveData) -> Result<BTreeMap<&str, &Attachment>> {
    bundle::validate(&data.content)?;
    let blobs = validate_attachments(data)?;
    let empty = CollectionSnapshot::default();
    let databases = data.databases.as_ref().unwrap_or(&empty);
    validate_collections(&data.content, databases, &data.attachments)?;
    if let Some(library) = &data.library {
        let (pages, ids) = library_ids(&data.content, databases);
        validate_navigation(&library.navigation, &pages)?;
        validate_receipts(&library.import_receipts, &pages, &ids)?;
        validate_templates(&library.templates, &databases.definitions)?;
    }
    Ok(blobs)
}

fn insert_content(tx: &Transaction<'_>, data: &ArchiveData) -> Result<()> {
    restore_snapshot(tx, &data.content)?;
    for attachment in &data.attachments {
        insert_attachment(tx, attachment)?;
    }
    let empty = CollectionSnapshot::default();
    let databases = data.databases.as_ref().unwrap_or(&empty);
    restore_collections(tx, &data.content, databases)?;
    if let Some(library) = &data.library {
        let (pages, ids) = library_ids(&data.content, databases);
        restore_navigation(tx, &library.navigation, &pages)?;
        restore_receipts(tx, &library.import_receipts, &pages, &ids)?;
        restore_templates(tx, &library.templates)?;
    }
    Ok(())
}

fn extract_blob<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    attachment: &Attachment,
    directory: &std::path::Path,
) -> Result<tempfile::NamedTempFile> {
    let entry = archive.by_name(&blob_entry(&attachment.sha256))?;
    check_zip_file(&entry)?;
    if entry.size() != attachment.bytes {
        return Err(Error::new(
            "ATTACHMENT_CHECKSUM",
            "包中附件大小与清单不符。",
        ));
    }
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    let content = transfer(entry, &mut file, attachment.bytes)?;
    check_content(&content, attachment)?;
    Ok(file)
}

fn validate_attachments(data: &ArchiveData) -> Result<BTreeMap<&str, &Attachment>> {
    let templates = data
        .library
        .as_ref()
        .map_or(&[][..], |library| library.templates.as_slice());
    if data.attachments.len() + template_attachments(templates).count() > MAX_ATTACHMENTS {
        return Err(Error::new(
            "BUNDLE_TOO_LARGE",
            "工作空间包最多包含 20,000 条附件记录。",
        ));
    }
    let pages: HashSet<_> = data
        .content
        .pages
        .iter()
        .map(|page| page.info.id.as_str())
        .collect();
    let mut identities = HashSet::new();
    let mut attachments = HashSet::new();
    for attachment in &data.attachments {
        if !valid_attachment(attachment)
            || !pages.contains(attachment.page_id.as_str())
            || !identities.insert(attachment.id.as_str())
            || !attachments.insert((&attachment.page_id, &attachment.name, &attachment.sha256))
        {
            return Err(Error::new(
                "INVALID_BUNDLE",
                "工作空间包包含无效或重复的附件记录。",
            ));
        }
    }
    bundle::validate_attachments(&data.content, &data.attachments)?;
    let mut blobs: BTreeMap<&str, &Attachment> = BTreeMap::new();
    let mut bytes = 0;
    for attachment in data
        .attachments
        .iter()
        .chain(template_attachments(templates))
    {
        if let Some(existing) = blobs.get(attachment.sha256.as_str()) {
            if existing.bytes != attachment.bytes || existing.media_type != attachment.media_type {
                return Err(Error::new("INVALID_BUNDLE", "同一个原件的描述不一致。"));
            }
        } else {
            bytes += attachment.bytes;
            if bytes > MAX_BLOBS_BYTES {
                return Err(Error::new(
                    "BUNDLE_TOO_LARGE",
                    "工作空间包的原件合计超过 2 GiB。",
                ));
            }
            blobs.insert(&attachment.sha256, attachment);
        }
    }
    Ok(blobs)
}

pub(crate) fn blob_entry(hash: &str) -> String {
    format!("blobs/{hash}")
}

pub(crate) fn check_zip_file<R: Read>(entry: &ZipFile<'_, R>) -> Result<()> {
    if !entry.is_file()
        || entry.is_symlink()
        || entry.encrypted()
        || entry.compression() != CompressionMethod::Stored
    {
        return Err(Error::new(
            "INVALID_BUNDLE",
            "工作空间包仅支持未加密、未压缩的文件条目。",
        ));
    }
    Ok(())
}
