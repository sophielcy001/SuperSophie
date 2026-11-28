// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::bundle::{self, BundleData};
use crate::collections::{self, insert_membership, membership, update_properties};
use crate::migrations;
use crate::model::{name, new_id, valid_icon, valid_id, validate_blocks};
use crate::{Error, Page, PageDraft, PageInfo, Result, Workspace};
use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod recovery;

const PAGE_COLUMNS: &str = "id, parent_id, title, revision, updated_at, icon, trash_root_id";
const PAGE_PROPERTIES: &str =
    "(SELECT properties FROM collection_records WHERE page_id = pages.id)";

const SCHEMA: &str = "
    CREATE TABLE workspace (id TEXT PRIMARY KEY, name TEXT NOT NULL);
    CREATE TABLE pages (
        id TEXT PRIMARY KEY,
        parent_id TEXT REFERENCES pages(id) DEFERRABLE INITIALLY DEFERRED,
        title TEXT NOT NULL,
        revision INTEGER NOT NULL CHECK (revision > 0),
        updated_at INTEGER NOT NULL,
        icon TEXT NOT NULL,
        trash_root_id TEXT REFERENCES pages(id) DEFERRABLE INITIALLY DEFERRED,
        blocks TEXT NOT NULL,
        body_text TEXT NOT NULL
    );
    CREATE INDEX pages_parent ON pages(parent_id);
    CREATE INDEX pages_trash ON pages(trash_root_id);
    CREATE TABLE revisions (
        page_id TEXT NOT NULL REFERENCES pages(id),
        revision INTEGER NOT NULL,
        snapshot TEXT NOT NULL,
        PRIMARY KEY (page_id, revision)
    );
    CREATE TABLE block_ids (
        id TEXT PRIMARY KEY,
        page_id TEXT NOT NULL REFERENCES pages(id)
    );
    CREATE INDEX block_ids_page ON block_ids(page_id);
    PRAGMA user_version = 1;
";

pub struct Store {
    root: PathBuf,
    pub(crate) session_id: String,
    upgrade_gate: Mutex<()>,
    pub(crate) backup_gate: Mutex<()>,
    pub(crate) backup_check_errors: Mutex<BTreeMap<String, String>>,
    pub(crate) blob_gate: std::sync::RwLock<()>,
    lock: File,
}

impl Drop for Store {
    fn drop(&mut self) {
        // A forked child can retain the file descriptor until it execs.
        let _ = self.lock.unlock();
    }
}

#[derive(Debug, serde::Serialize)]
pub struct WorkspaceIssue {
    pub id: String,
    pub error: Error,
}

#[derive(Debug, serde::Serialize)]
pub struct WorkspaceOverview {
    pub workspaces: Vec<Workspace>,
    pub issues: Vec<WorkspaceIssue>,
}

impl Store {
    pub fn data_root(&self) -> &Path {
        &self.root
    }

    pub fn workspace(&self, id: &str) -> Result<Workspace> {
        workspace_info(&self.connection(id)?)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        fs::create_dir_all(&root)?;
        let root = fs::canonicalize(root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join(".write.lock"))?;
        match lock.try_lock() {
            Ok(()) => Ok(Self {
                root,
                session_id: new_id(),
                upgrade_gate: Mutex::new(()),
                backup_gate: Mutex::new(()),
                backup_check_errors: Mutex::default(),
                blob_gate: std::sync::RwLock::new(()),
                lock,
            }),
            Err(TryLockError::WouldBlock) => Err(Error::new(
                "WORKSPACE_BUSY",
                "此数据目录正在被另一个知页窗口使用。",
            )),
            Err(TryLockError::Error(error)) => Err(error.into()),
        }
    }

    pub fn workspaces(&self) -> Result<Vec<Workspace>> {
        Ok(self.workspace_overview()?.workspaces)
    }

    pub fn workspace_overview(&self) -> Result<WorkspaceOverview> {
        let mut result = Vec::new();
        let mut issues = Vec::new();
        for id in self.workspace_ids()? {
            match self.workspace(&id) {
                Ok(workspace) => result.push(workspace),
                Err(error) => issues.push(WorkspaceIssue { id, error }),
            }
        }
        result.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
        Ok(WorkspaceOverview {
            workspaces: result,
            issues,
        })
    }

    pub(crate) fn workspace_ids(&self) -> Result<Vec<String>> {
        let mut ids = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            if valid_id(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// List spaces that a background job can use without initiating an upgrade.
    pub fn ready_workspace_ids(&self) -> Result<Vec<String>> {
        Ok(self
            .workspace_ids()?
            .into_iter()
            .filter(|id| {
                self.open_workspace_db(id, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .and_then(|db| migrations::version(&db))
                    .is_ok_and(|version| version == migrations::VERSION)
            })
            .collect())
    }

    pub fn create_workspace(&self, input: &str) -> Result<Workspace> {
        let workspace = Workspace {
            id: new_id(),
            name: name(input, 64, None)?,
        };
        self.publish_workspace(&workspace, |_, _| Ok(()))?;
        Ok(workspace)
    }

    pub(crate) fn publish_workspace(
        &self,
        workspace: &Workspace,
        fill: impl FnOnce(&Transaction<'_>, &Path) -> Result<()>,
    ) -> Result<()> {
        let staging = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&self.root)?;
        {
            let mut db = Connection::open(staging.path().join("workspace.sqlite"))?;
            configure(&db)?;
            let tx = db.transaction()?;
            initialize_schema(&tx, migrations::VERSION)?;
            tx.execute(
                "INSERT INTO workspace VALUES (?1, ?2)",
                params![workspace.id, workspace.name],
            )?;
            fill(&tx, staging.path())?;
            tx.commit()?;
        }
        self.publish_staging(&workspace.id, staging.path())
    }

    pub(crate) fn publish_staging(&self, id: &str, staging: &Path) -> Result<()> {
        let target = self.root.join(id);
        match fs::symlink_metadata(&target) {
            Ok(_) => return Err(Error::new("WORKSPACE_EXISTS", "恢复或创建目标已存在。")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        fs::rename(staging, target)?;
        Ok(())
    }

    pub(crate) fn open_workspace_db(
        &self,
        workspace: &str,
        flags: rusqlite::OpenFlags,
    ) -> Result<Connection> {
        if !valid_id(workspace) {
            return Err(Error::new("WORKSPACE_NOT_FOUND", "工作空间不存在。"));
        }
        let directory = self.root.join(workspace);
        let path = directory.join("workspace.sqlite");
        if !path.is_file() {
            return Err(Error::new("WORKSPACE_NOT_FOUND", "工作空间不存在。"));
        }
        if fs::canonicalize(&directory)?.parent() != Some(self.root.as_path())
            || fs::canonicalize(&path)?.parent() != Some(directory.as_path())
        {
            return Err(Error::new("WORKSPACE_PATH", "数据目录不能指向其他位置。"));
        }
        let db = Connection::open_with_flags(path, flags)?;
        configure(&db)?;
        if workspace_info(&db)?.id != workspace {
            return Err(Error::new(
                "WORKSPACE_MISMATCH",
                "工作空间的目录与数据标识不一致。",
            ));
        }
        Ok(db)
    }

    pub(crate) fn connection(&self, workspace: &str) -> Result<Connection> {
        let mut db =
            self.open_workspace_db(workspace, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        if migrations::version(&db)? != migrations::VERSION {
            let _upgrade = self
                .upgrade_gate
                .lock()
                .map_err(|_| Error::new("STORE_STATE", "工作空间升级服务需要重新启动。"))?;
            migrations::upgrade(&mut db, self, workspace)?;
        }
        Ok(db)
    }

    pub fn pages(&self, workspace: &str) -> Result<Vec<PageInfo>> {
        let db = self.connection(workspace)?;
        let mut query = db.prepare(&format!("SELECT {PAGE_COLUMNS} FROM pages ORDER BY rowid"))?;
        Ok(query
            .query_map([], read_info)?
            .collect::<rusqlite::Result<_>>()?)
    }

    pub fn page(&self, workspace: &str, id: &str) -> Result<Page> {
        read_page(&self.connection(workspace)?, id)
    }

    pub fn create_page(
        &self,
        workspace: &str,
        parent: Option<&str>,
        title: &str,
        icon: &str,
    ) -> Result<Page> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let page = create_page_in(&tx, parent, title, icon)?;
        tx.commit()?;
        Ok(page)
    }

    pub fn save_page(
        &self,
        workspace: &str,
        id: &str,
        revision: u32,
        draft: PageDraft,
    ) -> Result<PageInfo> {
        self.edit(workspace, id, revision, document_edit(draft)?)
            .map(|page| page.info)
    }

    pub fn set_page_appearance(
        &self,
        workspace: &str,
        id: &str,
        revision: u32,
        icon: &str,
        cover_attachment_id: Option<&str>,
    ) -> Result<Page> {
        if !valid_icon(icon) {
            return Err(Error::new("INVALID_ICON", "页面图标无效。"));
        }
        self.edit(workspace, id, revision, |db, page| {
            page.cover = cover_attachment_id
                .map(|attachment| crate::assets::read_page_cover(db, id, attachment))
                .transpose()?;
            page.info.icon = icon.into();
            Ok(())
        })
    }

    pub(crate) fn edit(
        &self,
        workspace: &str,
        id: &str,
        revision: u32,
        apply: impl FnOnce(&Transaction<'_>, &mut Page) -> Result<()>,
    ) -> Result<Page> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let after = stage_edit(&tx, id, revision, apply)?;
        tx.commit()?;
        Ok(after)
    }

    pub fn history(&self, workspace: &str, id: &str) -> Result<Vec<Page>> {
        let db = self.connection(workspace)?;
        read_page(&db, id)?;
        snapshots(
            &db,
            "SELECT snapshot FROM revisions WHERE page_id = ?1 ORDER BY revision DESC",
            [id],
        )
    }

    pub fn restore_version(
        &self,
        workspace: &str,
        id: &str,
        revision: u32,
        target: u32,
    ) -> Result<Page> {
        self.edit(workspace, id, revision, |db, page| {
            let snapshot = read_revision(db, id, target)?;
            page.info.title = snapshot.info.title;
            page.info.icon = snapshot.info.icon;
            page.blocks = snapshot.blocks;
            page.properties = snapshot.properties;
            if let Some(values) = &mut page.properties {
                let collection = collections::for_parent(db, page.info.parent_id.as_deref())?
                    .ok_or_else(|| Error::new("RELATION_SCHEMA", "找不到记录所属的数据库。"))?;
                let schema = collections::read_collection_schema(db, &collection)?;
                collections::restore_historical_properties(
                    &schema.properties,
                    values,
                    &snapshot.historical_properties,
                )?;
                collections::relations::remove_deleted_targets(db, &schema.properties, values)?;
            }
            page.cover = snapshot.cover;
            Ok(())
        })
    }

    pub fn export_workspace(&self, workspace: &str) -> Result<Vec<u8>> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction()?;
        if tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM attachments) OR EXISTS(SELECT 1 FROM collections)
              OR EXISTS(SELECT 1 FROM favorites) OR EXISTS(SELECT 1 FROM recent_pages)
              OR EXISTS(SELECT 1 FROM workspace_inbox) OR EXISTS(SELECT 1 FROM templates)
              OR EXISTS(SELECT 1 FROM deleted_collections) OR EXISTS(SELECT 1 FROM deleted_records)
              OR EXISTS(SELECT 1 FROM import_receipts)",
            [],
            |row| row.get::<_, bool>(0),
        )? {
            return Err(Error::new(
                "ARCHIVE_REQUIRED",
                "含数据库、附件或工作空间设置的数据请使用 .zhiye.zip 导出。",
            ));
        }
        let bundle = workspace_snapshot(&tx, true)?;
        tx.commit()?;
        bundle::encode(&bundle)
    }

    pub fn import_workspace(&self, data: &[u8]) -> Result<Workspace> {
        let bundle = bundle::decode(data)?;
        let workspace = recovered_workspace(&bundle.workspace);
        self.publish_workspace(&workspace, |tx, _| restore_snapshot(tx, &bundle))?;
        Ok(workspace)
    }
}

pub(crate) fn workspace_snapshot(
    tx: &Transaction<'_>,
    include_history: bool,
) -> Result<BundleData> {
    let pages = {
        let mut query = tx.prepare(&format!(
            "SELECT {PAGE_COLUMNS}, blocks, {PAGE_PROPERTIES}, cover FROM pages ORDER BY rowid"
        ))?;
        query
            .query_map([], read_page_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    let revisions = if include_history {
        snapshots(
            tx,
            "SELECT snapshot FROM revisions ORDER BY page_id, revision",
            [],
        )?
    } else {
        pages.clone()
    };
    Ok(BundleData {
        workspace: workspace_info(tx)?,
        pages,
        revisions,
    })
}

pub(crate) fn recovered_workspace(source: &Workspace) -> Workspace {
    let label: String = source.name.trim().chars().take(58).collect();
    Workspace {
        id: new_id(),
        name: format!("{label}（恢复）"),
    }
}

pub(crate) fn restore_snapshot(tx: &Transaction<'_>, bundle: &BundleData) -> Result<()> {
    for page in &bundle.pages {
        insert_page(tx, page)?;
    }
    for snapshot in &bundle.revisions {
        insert_revision(tx, snapshot)?;
    }
    Ok(())
}

pub(crate) fn configure(db: &Connection) -> Result<()> {
    db.busy_timeout(Duration::from_secs(3))?;
    db.execute_batch("PRAGMA foreign_keys = ON; PRAGMA synchronous = FULL;")?;
    Ok(())
}

pub(crate) fn initialize_schema(tx: &Transaction<'_>, version: u32) -> Result<()> {
    tx.execute_batch(SCHEMA)?;
    migrations::apply(tx, 1, version)
}

fn document_edit(
    draft: PageDraft,
) -> Result<impl FnOnce(&Transaction<'_>, &mut Page) -> Result<()>> {
    let title = name(&draft.title, 200, Some("无标题"))?;
    Ok(move |_: &Transaction<'_>, page: &mut Page| {
        page.info.title = title;
        page.blocks = draft.blocks;
        Ok(())
    })
}

pub(crate) fn stage_edit(
    tx: &Transaction<'_>,
    id: &str,
    revision: u32,
    apply: impl FnOnce(&Transaction<'_>, &mut Page) -> Result<()>,
) -> Result<Page> {
    Ok(stage_edit_with_related(tx, id, revision, apply)?.0)
}

pub(crate) fn stage_edit_with_related(
    tx: &Transaction<'_>,
    id: &str,
    revision: u32,
    apply: impl FnOnce(&Transaction<'_>, &mut Page) -> Result<()>,
) -> Result<(Page, Vec<PageInfo>)> {
    let before = read_page(tx, id)?;
    ensure_revision(&before.info, revision)?;
    ensure_active(&before)?;
    let mut after = before.clone();
    apply(tx, &mut after)?;
    let related = if before != after {
        commit_revision(tx, &mut after)?
    } else {
        vec![]
    };
    Ok((after, related))
}

pub(crate) fn workspace_info(db: &Connection) -> Result<Workspace> {
    Ok(db.query_row("SELECT id, name FROM workspace", [], |row| {
        Ok(Workspace {
            id: row.get(0)?,
            name: row.get(1)?,
        })
    })?)
}

pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

pub(crate) fn read_info(row: &Row<'_>) -> rusqlite::Result<PageInfo> {
    Ok(PageInfo {
        id: row.get(0)?,
        parent_id: row.get(1)?,
        title: row.get(2)?,
        revision: row.get(3)?,
        updated_at: row.get(4)?,
        icon: row.get(5)?,
        trash_root_id: row.get(6)?,
    })
}

fn read_page_row(row: &Row<'_>) -> rusqlite::Result<Page> {
    let data: String = row.get(7)?;
    let blocks = serde_json::from_str(&data).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(7, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(Page {
        info: read_info(row)?,
        blocks,
        properties: optional_json(row, 8)?,
        historical_properties: Vec::new(),
        cover: optional_json(row, 9)?,
    })
}

fn optional_json<T: serde::de::DeserializeOwned>(
    row: &Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    row.get::<_, Option<String>>(index)?
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

pub(crate) fn read_page(db: &Connection, id: &str) -> Result<Page> {
    db.query_row(
        &format!(
            "SELECT {PAGE_COLUMNS}, blocks, {PAGE_PROPERTIES}, cover FROM pages WHERE id = ?1"
        ),
        [id],
        read_page_row,
    )
    .optional()?
    .ok_or_else(|| Error::new("PAGE_NOT_FOUND", "找不到这个页面。"))
}

pub(crate) fn read_page_info(db: &Connection, id: &str) -> Result<PageInfo> {
    db.query_row(
        &format!("SELECT {PAGE_COLUMNS} FROM pages WHERE id = ?1"),
        [id],
        read_info,
    )
    .optional()?
    .ok_or_else(|| Error::new("PAGE_NOT_FOUND", "找不到这个页面。"))
}

pub(crate) fn create_page_in(
    tx: &Transaction<'_>,
    parent: Option<&str>,
    title: &str,
    icon: &str,
) -> Result<Page> {
    insert_new_page(tx, new_page(parent, title, icon)?)
}

pub(crate) fn new_page(parent: Option<&str>, title: &str, icon: &str) -> Result<Page> {
    if !valid_icon(icon) {
        return Err(Error::new("INVALID_ICON", "页面图标无效。"));
    }
    Ok(Page {
        info: PageInfo {
            id: new_id(),
            parent_id: parent.map(str::to_owned),
            title: name(title, 200, Some("无标题"))?,
            revision: 1,
            updated_at: now(),
            icon: icon.into(),
            trash_root_id: None,
        },
        blocks: vec![],
        properties: None,
        historical_properties: Vec::new(),
        cover: None,
    })
}

pub(crate) fn insert_new_page(tx: &Transaction<'_>, page: Page) -> Result<Page> {
    Ok(insert_new_page_with_attachments(tx, page, &[])?.0)
}

pub(crate) fn insert_new_page_with_attachments(
    tx: &Transaction<'_>,
    mut page: Page,
    attachments: &[crate::Attachment],
) -> Result<(Page, Vec<PageInfo>)> {
    let parent = page.info.parent_id.as_deref();
    if let Some(parent) = parent {
        ensure_active(&read_page(tx, parent)?)?;
    }
    let collection = collections::for_parent(tx, parent)?;
    if collection.is_some() {
        page.properties.get_or_insert_with(Default::default);
    } else if page.properties.is_some() {
        return Err(Error::new("RECORD_NOT_FOUND", "页面属性需要所属数据库。"));
    }
    let mirrors = collections::relations::prepare_mirrors(tx, &mut page, true)?;
    let computed = crate::integrations::derived::ComputedChange::edit(
        tx,
        std::iter::once(&page).chain(&mirrors),
    )?;
    insert_page(tx, &page)?;
    for attachment in attachments {
        crate::assets::insert_attachment(tx, attachment)?;
    }
    crate::assets::validate_page_attachments(tx, &page)?;
    if let Some(collection) = collection {
        insert_membership(tx, &membership(&page, collection), &page)?;
    }
    insert_revision(tx, &page)?;
    let related = write_related_revisions(tx, mirrors)?;
    crate::integrations::derived::enqueue(tx, None, computed)?;
    Ok((page, related))
}

pub(crate) fn ensure_active(page: &Page) -> Result<()> {
    ensure_active_info(&page.info)
}

pub(crate) fn ensure_active_info(page: &PageInfo) -> Result<()> {
    if page.trash_root_id.is_some() {
        return Err(Error::new("PAGE_TRASHED", "该页面在回收站中，请先恢复。"));
    }
    Ok(())
}

pub(crate) fn ensure_revision(page: &PageInfo, revision: u32) -> Result<()> {
    if page.revision != revision {
        return Err(Error::new(
            "REVISION_CONFLICT",
            "页面已有更新。当前编辑已保留，可以导出草稿。",
        ));
    }
    Ok(())
}

fn snapshots(db: &Connection, sql: &str, parameters: impl rusqlite::Params) -> Result<Vec<Page>> {
    let mut query = db.prepare(sql)?;
    let data = query.query_map(parameters, |row| row.get::<_, String>(0))?;
    data.map(|entry| Ok(serde_json::from_str(&entry?)?))
        .collect()
}

pub(crate) fn insert_page(db: &Transaction<'_>, page: &Page) -> Result<()> {
    let info = &page.info;
    db.execute("INSERT INTO pages (id,parent_id,title,revision,updated_at,icon,trash_root_id,blocks,body_text,cover)
        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,'',?9)", params![info.id, info.parent_id, info.title, info.revision,
        info.updated_at, info.icon, info.trash_root_id, serde_json::to_string(&page.blocks)?,
        page.cover.as_ref().map(serde_json::to_string).transpose()?])?;
    sync_block_ids(db, page)?;
    crate::references::index_page(db, &page.info.id, &page.blocks)
}

fn sync_block_ids(db: &Transaction<'_>, page: &Page) -> Result<()> {
    let ids = validate_blocks(&page.blocks)?;
    let anchors: std::collections::HashMap<_, _> =
        crate::integrations::source_anchors(&page.blocks)
            .map(|block| (block.id.as_str(), crate::references::anchor_kind(block)))
            .collect();
    db.execute("DELETE FROM block_ids WHERE page_id = ?1", [&page.info.id])?;
    let mut insert = db
        .prepare("INSERT INTO block_ids (id,page_id,reference_anchor) VALUES (?1,?2,?3) ON CONFLICT (id) DO NOTHING")?;
    for id in ids {
        if insert.execute(params![
            id,
            page.info.id,
            anchors.get(id).copied().unwrap_or(0)
        ])? == 0
        {
            return Err(Error::new(
                "INVALID_BLOCK",
                "块标识已被同一工作空间中的其他页面使用。",
            ));
        }
    }
    Ok(())
}

pub(crate) fn insert_revision(db: &Transaction<'_>, page: &Page) -> Result<()> {
    db.execute(
        "INSERT INTO revisions (page_id,revision,snapshot) VALUES (?1,?2,?3)",
        params![
            page.info.id,
            page.info.revision,
            serde_json::to_string(page)?
        ],
    )?;
    Ok(())
}

pub(crate) fn read_revision(db: &Connection, id: &str, revision: u32) -> Result<Page> {
    let data: Option<String> = db
        .query_row(
            "SELECT snapshot FROM revisions WHERE page_id = ?1 AND revision = ?2",
            (id, revision),
            |row| row.get(0),
        )
        .optional()?;
    let data = data.ok_or_else(|| Error::new("REVISION_NOT_FOUND", "找不到这条历史修订。"))?;
    Ok(serde_json::from_str(&data)?)
}

pub(crate) fn commit_revision(db: &Transaction<'_>, page: &mut Page) -> Result<Vec<PageInfo>> {
    let mirrors = collections::relations::prepare_mirrors(db, page, false)?;
    commit_page_and_mirrors(db, page, mirrors)
}

pub(crate) fn commit_removed_relations(db: &Transaction<'_>, page: &mut Page) -> Result<()> {
    // The reverse edges belong to records that the same transaction deletes.
    commit_page_and_mirrors(db, page, Vec::new()).map(drop)
}

fn commit_page_and_mirrors(
    db: &Transaction<'_>,
    page: &mut Page,
    mirrors: Vec<Page>,
) -> Result<Vec<PageInfo>> {
    let blocks = serde_json::to_string(&page.blocks)?;
    let derived = crate::integrations::derived::ReferenceChange::edit(db, page, &blocks)?;
    let computed = crate::integrations::derived::ComputedChange::edit(
        db,
        std::iter::once(&*page).chain(&mirrors),
    )?;
    write_revision(db, page, &blocks)?;
    let revisions = write_related_revisions(db, mirrors)?;
    crate::integrations::derived::enqueue(db, derived, computed)?;
    Ok(revisions)
}

fn write_related_revisions(db: &Transaction<'_>, mirrors: Vec<Page>) -> Result<Vec<PageInfo>> {
    mirrors
        .into_iter()
        .map(|mut page| {
            let blocks = serde_json::to_string(&page.blocks)?;
            write_revision(db, &mut page, &blocks)?;
            Ok(page.info)
        })
        .collect()
}

pub(crate) fn write_revision(db: &Transaction<'_>, page: &mut Page, blocks: &str) -> Result<()> {
    page.info.revision = page
        .info
        .revision
        .checked_add(1)
        .ok_or_else(|| Error::new("REVISION_LIMIT", "页面修订号已达上限。"))?;
    page.info.updated_at = now();
    sync_block_ids(db, page)?;
    crate::assets::validate_page_attachments(db, page)?;
    let info = &page.info;
    db.execute(
        "UPDATE pages SET parent_id=?2, title=?3, revision=?4, updated_at=?5, icon=?6,
        trash_root_id=?7, blocks=?8, body_text='', cover=?9 WHERE id=?1",
        params![
            info.id,
            info.parent_id,
            info.title,
            info.revision,
            info.updated_at,
            info.icon,
            info.trash_root_id,
            blocks,
            page.cover.as_ref().map(serde_json::to_string).transpose()?,
        ],
    )?;
    if let Some(properties) = &page.properties {
        update_properties(db, &info.id, properties)?;
    }
    crate::references::index_page(db, &info.id, &page.blocks)?;
    insert_revision(db, page)?;
    Ok(())
}
