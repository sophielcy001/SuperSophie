// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{data_root, run, run_independent};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{
    AppHandle, State, WebviewWindow,
    ipc::{InvokeBody, Request, Response},
};
use tauri_plugin_dialog::{DialogExt, FilePath};
use zhiye_core::{
    ArchiveMerge, Attachment, AttachmentImport, CsvDestination, Error, ImportFile, ImportOutcome,
    MAX_ATTACHMENT_BYTES, MAX_BUNDLE_BYTES, MAX_CSV_IMPORT_BYTES, MAX_MARKDOWN_IMPORT_BYTES,
    MAX_MARKDOWN_IMPORT_FILES, MAX_PREVIEW_BYTES, MigrationBatch, Page, PageDraft,
    PageExportFormat, RecordDraft, Result, Store, Workspace, decode_desktop_blocks,
    image_extension, render_page_export, sync_export_file,
};

pub(crate) fn chosen_path(choice: Option<FilePath>) -> Result<Option<PathBuf>> {
    choice
        .map(|path| {
            path.into_path()
                .map_err(|error| Error::new("FILE_PATH", error.to_string()))
        })
        .transpose()
}

fn selected_name(path: &Path) -> String {
    path.file_name()
        .map(|part| part.to_string_lossy().into_owned())
        .unwrap_or_else(|| "所选文件".into())
}

fn read_import_file(path: &Path, limit: usize) -> Result<ImportFile> {
    let metadata = path.metadata()?;
    if !metadata.is_file() {
        return Err(Error::new("IMPORT_FORMAT", "请选择普通文件。"));
    }
    if metadata.len() > limit as u64 {
        return Err(Error::new(
            "IMPORT_SIZE",
            format!("文件超过 {} MiB。", limit / (1024 * 1024)),
        ));
    }
    let mut bytes = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(ImportFile {
        name: selected_name(path),
        bytes,
    })
}

#[tauri::command]
pub(crate) async fn import_markdown_files(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    parent_id: Option<String>,
) -> Result<Option<Vec<ImportOutcome>>> {
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("选择 Markdown 原件")
        .add_filter("Markdown", &["md", "markdown"])
        .blocking_pick_files();
    let Some(paths) = selection else {
        return Ok(None);
    };
    if paths.len() > MAX_MARKDOWN_IMPORT_FILES {
        return Err(Error::new(
            "IMPORT_SIZE",
            "一次最多可导入 32 个 Markdown 文件。",
        ));
    }
    let report = run(app, move |store| {
        let mut failures = Vec::new();
        let mut sources = Vec::with_capacity(paths.len());
        for (index, choice) in paths.into_iter().enumerate() {
            let path = chosen_path(Some(choice))?.expect("selection is present");
            match read_import_file(&path, MAX_MARKDOWN_IMPORT_BYTES) {
                Ok(file) => sources.push(file),
                Err(error) => failures.push((
                    index,
                    ImportOutcome::failed(selected_name(&path), error.message),
                )),
            }
        }
        let mut outcomes = store.import_markdown(&workspace, parent_id.as_deref(), sources)?;
        for (index, failure) in failures {
            outcomes.insert(index, failure);
        }
        Ok(outcomes)
    })
    .await?;
    Ok(Some(report))
}

#[tauri::command]
pub(crate) async fn import_csv_file(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    destination: CsvDestination,
) -> Result<Option<Vec<ImportOutcome>>> {
    let choice = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("选择 CSV 原件")
        .add_filter("CSV", &["csv"])
        .blocking_pick_file();
    let Some(path) = chosen_path(choice)? else {
        return Ok(None);
    };
    let report = run(app, move |store| {
        let outcome = match read_import_file(&path, MAX_CSV_IMPORT_BYTES) {
            Ok(file) => store.import_csv(&workspace, destination, file)?,
            Err(error) => ImportOutcome::failed(selected_name(&path), error.message),
        };
        Ok(vec![outcome])
    })
    .await?;
    Ok(Some(report))
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum MigrationFormat {
    NotionZip,
    ObsidianVault,
}

type RunningMigration = Option<(String, Arc<AtomicBool>)>;

#[derive(Clone, Default)]
pub(crate) struct MigrationState(Arc<Mutex<RunningMigration>>);

struct MigrationGuard {
    state: MigrationState,
    cancel: Arc<AtomicBool>,
}

impl MigrationState {
    fn begin(&self, workspace: &str) -> Result<MigrationGuard> {
        let mut running = self
            .0
            .lock()
            .map_err(|_| Error::new("STORE_STATE", "迁移服务需要重新启动。"))?;
        if running.is_some() {
            return Err(Error::new("IMPORT_BUSY", "已有迁移正在处理。"));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        *running = Some((workspace.to_owned(), Arc::clone(&cancel)));
        Ok(MigrationGuard {
            state: self.clone(),
            cancel,
        })
    }
}

impl Drop for MigrationGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.state.0.lock() {
            *running = None;
        }
    }
}

#[tauri::command]
pub(crate) async fn import_migration(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, MigrationState>,
    workspace: String,
    parent_id: Option<String>,
    format: MigrationFormat,
) -> Result<Option<Vec<ImportOutcome>>> {
    let migration = state.begin(&workspace)?;
    let picker = app.dialog().file().set_parent(&window);
    let choice = match format {
        MigrationFormat::NotionZip => picker
            .set_title("选择 Notion 导出 ZIP")
            .add_filter("Notion ZIP", &["zip"])
            .blocking_pick_file(),
        MigrationFormat::ObsidianVault => picker
            .set_title("选择 Obsidian 资料库目录")
            .blocking_pick_folder(),
    };
    let Some(path) = chosen_path(choice)? else {
        return Ok(None);
    };
    let report = run(app, move |store| {
        let migration = migration;
        match format {
            MigrationFormat::NotionZip => {
                let file = File::open(&path)?;
                if !file.metadata()?.is_file() {
                    return Err(Error::new("IMPORT_FORMAT", "请选择 ZIP 普通文件。"));
                }
                store.import_notion_zip(
                    &workspace,
                    parent_id.as_deref(),
                    &selected_name(&path),
                    file,
                    &migration.cancel,
                )
            }
            MigrationFormat::ObsidianVault => store.import_obsidian_vault(
                &workspace,
                parent_id.as_deref(),
                &path,
                &migration.cancel,
            ),
        }
    })
    .await?;
    Ok(Some(report))
}

#[tauri::command]
pub(crate) fn cancel_migration(
    state: State<'_, MigrationState>,
    workspace: String,
) -> Result<bool> {
    let running = state
        .0
        .lock()
        .map_err(|_| Error::new("STORE_STATE", "迁移服务需要重新启动。"))?;
    if let Some((_, cancel)) = running
        .as_ref()
        .filter(|(current, _)| current == &workspace)
    {
        cancel.store(true, Ordering::Relaxed);
        return Ok(true);
    }
    Ok(false)
}

#[tauri::command]
pub(crate) async fn latest_migration(
    app: AppHandle,
    workspace: String,
    native_content: Option<bool>,
) -> Result<Option<MigrationBatch>> {
    run(app, move |store| {
        store.latest_migration_for(&workspace, native_content.unwrap_or(false))
    })
    .await
}

#[tauri::command]
pub(crate) async fn save_import_report(
    window: WebviewWindow,
    app: AppHandle,
    report: String,
) -> Result<bool> {
    use std::io::Write;
    if report.len() > 32 * 1024 * 1024 {
        return Err(Error::new("IMPORT_SIZE", "报告超过 32 MiB。"));
    }
    let choice = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("保存导入报告")
        .add_filter("文本文件", &["txt"])
        .set_file_name("知页-导入报告.txt")
        .blocking_save_file();
    let Some(path) = chosen_path(choice)? else {
        return Ok(false);
    };
    run(app, move |store| {
        write_export(store.data_root(), &path, |file| {
            Ok(file.write_all(report.as_bytes())?)
        })
    })
    .await?;
    Ok(true)
}

pub(crate) fn file_stem(name: &str) -> String {
    let result: String = name
        .chars()
        .take(72)
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '-'
            } else {
                c
            }
        })
        .collect();
    format!("知页-{}", result.trim_matches(['.', ' ']))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DraftFile {
    format: &'static str,
    format_version: u8,
    workspace_id: String,
    page_id: String,
    base_revision: u32,
    draft: PageDraft,
}

fn write_draft_file(root: &Path, path: &Path, draft: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(draft)?;
    write_export(root, path, |file| {
        file.write_all(&bytes)?;
        file.write_all(b"\n")?;
        Ok(())
    })
}

#[tauri::command]
pub(crate) async fn export_page_draft(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    id: String,
    revision: u32,
    mut draft: PageDraft,
) -> Result<bool> {
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("导出页面草稿")
        .add_filter("知页草稿", &["json"])
        .set_file_name(format!("{}-草稿.zhiye-draft.json", file_stem(&draft.title)))
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || {
        draft.blocks = decode_desktop_blocks(draft.blocks)?;
        write_draft_file(
            &data_root(&app)?,
            &path,
            &DraftFile {
                format: "zhiye.page-draft",
                format_version: 1,
                workspace_id: workspace,
                page_id: id,
                base_revision: revision,
                draft,
            },
        )
    })
    .await
    .map_err(|error| Error::new("STORE_TASK", error.to_string()))??;
    Ok(true)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RecordDraftFile {
    format: &'static str,
    format_version: u8,
    workspace_id: String,
    collection_id: String,
    page_id: String,
    base_revision: u32,
    draft: RecordDraft,
}

#[tauri::command]
pub(crate) async fn export_record_draft(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    collection: String,
    id: String,
    revision: u32,
    draft: RecordDraft,
) -> Result<bool> {
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("导出记录草稿")
        .add_filter("知页记录草稿", &["json"])
        .set_file_name(format!(
            "{}-属性草稿.zhiye-record-draft.json",
            file_stem(&draft.title)
        ))
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(false);
    };
    tauri::async_runtime::spawn_blocking(move || {
        write_draft_file(
            &data_root(&app)?,
            &path,
            &RecordDraftFile {
                format: "zhiye.record-draft",
                format_version: 1,
                workspace_id: workspace,
                collection_id: collection,
                page_id: id,
                base_revision: revision,
                draft,
            },
        )
    })
    .await
    .map_err(|error| Error::new("STORE_TASK", error.to_string()))??;
    Ok(true)
}

pub(crate) fn write_export<T>(
    root: &Path,
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<T>,
) -> Result<T> {
    let resolved_parent = export_parent(root, path)?;
    let mut temporary = tempfile::NamedTempFile::new_in(&resolved_parent)?;
    let result = write(temporary.as_file_mut())?;
    sync_export_file(temporary.as_file())?;
    temporary
        .persist(path)
        .map_err(|error| Error::from(error.error))?;
    Ok(result)
}

fn export_parent(root: &Path, path: &Path) -> Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::new("FILE_PATH", "请选择有效的文件位置。"))?;
    let resolved_parent = parent.canonicalize()?;
    let resolved_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    if resolved_parent.starts_with(&resolved_root)
        || path
            .canonicalize()
            .is_ok_and(|p| p.starts_with(&resolved_root))
    {
        return Err(Error::new(
            "FILE_PATH",
            "请把导出文件保存到工作空间数据目录以外。",
        ));
    }
    Ok(resolved_parent)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PageExportResult {
    pub companion_folder: Option<String>,
}

pub(crate) fn write_page_document(
    store: &Store,
    workspace: &str,
    page: &Page,
    path: &Path,
    format: PageExportFormat,
) -> Result<String> {
    let root = store.data_root();
    let parent = export_parent(root, path)?;
    let assets = store.markdown_attachments(workspace, page)?;
    let prefix = if assets.is_empty() {
        match format {
            PageExportFormat::Markdown => "zhiye-md-",
            PageExportFormat::Html => "zhiye-html-",
        }
    } else if assets
        .iter()
        .all(|asset| image_extension(&asset.media_type).is_some())
    {
        "zhiye-images-"
    } else {
        "zhiye-files-"
    };
    let companion = tempfile::Builder::new().prefix(prefix).tempdir_in(parent)?;
    let folder = selected_name(companion.path());
    let mut exported_assets = Vec::with_capacity(assets.len());
    for asset in assets {
        let name = format!("{}.{}", asset.id, export_extension(&asset));
        let mut output = File::create(companion.path().join(&name))?;
        store.copy_attachment(workspace, &page.info.id, &asset.id, &mut output)?;
        sync_export_file(&output)?;
        exported_assets.push((asset, format!("{folder}/{name}")));
    }
    let document = render_page_export(page, &exported_assets, workspace, format);
    let mut report = File::create(companion.path().join("导出报告.txt"))?;
    report.write_all(document.report.as_bytes())?;
    sync_export_file(&report)?;
    drop(report);
    write_export(root, path, |file| {
        Ok(file.write_all(document.document.as_bytes())?)
    })?;
    let _ = companion.keep();
    Ok(folder)
}

fn export_extension(asset: &Attachment) -> &str {
    image_extension(&asset.media_type)
        .or_else(|| (asset.media_type == "application/pdf").then_some("pdf"))
        .or_else(|| {
            Path::new(&asset.name).extension()?.to_str().filter(|ext| {
                !ext.is_empty()
                    && ext.len() <= 12
                    && ext.bytes().all(|byte| byte.is_ascii_alphanumeric())
            })
        })
        .unwrap_or("bin")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CsvExport {
    rows: usize,
    attachments: usize,
    companion_folder: String,
    limitations: Vec<String>,
}

fn write_collection_csv(
    store: &Store,
    workspace: &str,
    collection: &str,
    view_id: Option<&str>,
    path: &Path,
) -> Result<CsvExport> {
    let root = store.data_root();
    let parent = export_parent(root, path)?;
    let companion = tempfile::Builder::new()
        .prefix("zhiye-csv-")
        .tempdir_in(parent)?;
    let folder = selected_name(companion.path());
    let mut files: HashMap<String, String> = HashMap::new();
    let report = write_export(root, path, |output| {
        let report =
            store.write_collection_csv(workspace, collection, view_id, output, |asset| {
                if let Some(path) = files.get(&asset.sha256) {
                    return Ok(path.clone());
                }
                let ext = export_extension(asset);
                let name = format!("{}.{}", asset.id, ext);
                let directory = companion.path().join("attachments");
                std::fs::create_dir_all(&directory)?;
                let mut file = File::create(directory.join(&name))?;
                store.copy_attachment(workspace, &asset.page_id, &asset.id, &mut file)?;
                sync_export_file(&file)?;
                let path = format!("{folder}/attachments/{name}");
                files.insert(asset.sha256.clone(), path.clone());
                Ok(path)
            })?;
        let scope = report
            .view_name
            .as_deref()
            .map_or_else(|| "整个数据库".to_owned(), |view| format!("{view}视图"));
        let mut notes = File::create(companion.path().join("导出报告.txt"))?;
        writeln!(
            notes,
            "知页 CSV 导出报告\n\n范围  {scope}\n记录  {}\n附件引用  {}\n",
            report.rows,
            report.attachments.len()
        )?;
        for limitation in &report.limitations {
            writeln!(notes, "- {limitation}")?;
        }
        for (name, path) in &report.attachments {
            writeln!(notes, "{name}\t{path}")?;
        }
        sync_export_file(&notes)?;
        Ok(report)
    })?;
    let _ = companion.keep();
    Ok(CsvExport {
        rows: report.rows,
        attachments: files.len(),
        companion_folder: folder,
        limitations: report.limitations,
    })
}

#[tauri::command]
pub(crate) async fn export_collection_csv(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    collection: String,
    view_id: Option<String>,
) -> Result<Option<CsvExport>> {
    let title = run(app.clone(), {
        let workspace = workspace.clone();
        let collection = collection.clone();
        move |store| {
            let schema = store.collection(&workspace, &collection)?;
            Ok(store
                .page(&workspace, &schema.container_page_id)?
                .info
                .title)
        }
    })
    .await?;
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("导出 CSV")
        .add_filter("CSV", &["csv"])
        .set_file_name(format!("{}.csv", file_stem(&title)))
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(None);
    };
    run(app, move |store| {
        write_collection_csv(store, &workspace, &collection, view_id.as_deref(), &path).map(Some)
    })
    .await
}

#[tauri::command]
pub(crate) async fn export_workspace(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    include_history: Option<bool>,
) -> Result<bool> {
    let label = run(app.clone(), {
        let workspace = workspace.clone();
        move |store| Ok(store.workspace(&workspace)?.name)
    })
    .await?;
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("导出工作空间")
        .add_filter("知页工作空间包", &["zip"])
        .set_file_name(format!("{}.zhiye.zip", file_stem(&label)))
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(false);
    };
    run(app, move |store| {
        write_export(store.data_root(), &path, |file| {
            store.write_workspace_archive_with_history(
                &workspace,
                include_history.unwrap_or(true),
                file,
            )
        })
    })
    .await?;
    Ok(true)
}

#[tauri::command]
pub(crate) async fn import_workspace(
    window: WebviewWindow,
    app: AppHandle,
) -> Result<Option<Workspace>> {
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("恢复工作空间")
        .add_filter("知页工作空间包和整库恢复包", &["zip", "json"])
        .blocking_pick_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(None);
    };
    run_independent(app, move |store| {
        let mut file = File::open(path)?;
        let mut prefix = [0; 4];
        file.read_exact(&mut prefix)?;
        file.rewind()?;
        if prefix.starts_with(b"PK") {
            store.read_workspace_package(file).map(Some)
        } else {
            let mut data = Vec::new();
            file.take(MAX_BUNDLE_BYTES as u64 + 1)
                .read_to_end(&mut data)?;
            store.import_workspace(&data).map(Some)
        }
    })
    .await
}

#[tauri::command]
pub(crate) async fn merge_workspace(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, MigrationState>,
    workspace: String,
) -> Result<Option<ArchiveMerge>> {
    let migration = state.begin(&workspace)?;
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("选择要合并的知页内容包")
        .add_filter("知页内容包", &["zip", "json"])
        .blocking_pick_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(None);
    };
    run_independent(app, move |store| {
        let migration = migration;
        if !path.metadata()?.is_file() {
            return Err(Error::new("IMPORT_FORMAT", "请选择普通文件。"));
        }
        let file = File::open(&path)?;
        store
            .merge_workspace_package(&workspace, &selected_name(&path), file, &migration.cancel)
            .map(Some)
    })
    .await
}

#[tauri::command]
pub(crate) async fn list_attachments(
    app: AppHandle,
    workspace: String,
    page: String,
) -> Result<Vec<Attachment>> {
    run(app, move |store| store.attachments(&workspace, &page)).await
}

#[tauri::command]
pub(crate) async fn read_attachment(
    app: AppHandle,
    workspace: String,
    page: String,
    id: String,
) -> Result<Attachment> {
    run(app, move |store| store.attachment(&workspace, &page, &id)).await
}

#[tauri::command]
pub(crate) async fn rename_attachment(
    app: AppHandle,
    workspace: String,
    page: String,
    id: String,
    name: String,
) -> Result<Attachment> {
    run(app, move |store| {
        store.rename_attachment(&workspace, &page, &id, &name)
    })
    .await
}

#[derive(Default, Serialize)]
pub(crate) struct AttachmentBatch {
    added: Vec<AttachmentImport>,
    failed: Vec<AttachmentFailure>,
}

#[derive(Serialize)]
struct AttachmentFailure {
    name: String,
    error: Error,
}

#[tauri::command]
pub(crate) async fn add_attachments(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    page: String,
) -> Result<AttachmentBatch> {
    select_attachments(window, app, workspace, page, false, false).await
}

#[tauri::command]
pub(crate) async fn pick_images(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    page: String,
    single: Option<bool>,
) -> Result<AttachmentBatch> {
    select_attachments(window, app, workspace, page, true, single.unwrap_or(false)).await
}

async fn select_attachments(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    page: String,
    images: bool,
    single: bool,
) -> Result<AttachmentBatch> {
    let picker = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title(match (images, single) {
            (true, true) => "导入封面图片",
            (true, false) => "插入图片",
            _ => "添加附件",
        });
    let picker = if images {
        picker.add_filter("图片", &["png", "jpg", "jpeg", "gif", "webp", "bmp"])
    } else {
        picker
    };
    let selection = if single {
        picker.blocking_pick_file().map(|path| vec![path])
    } else {
        picker.blocking_pick_files()
    };
    let Some(paths) = selection else {
        return Ok(AttachmentBatch::default());
    };
    if paths.len() > 50 {
        return Err(Error::new("TOO_MANY_FILES", "每次最多添加 50 个附件。"));
    }
    run(app, move |store| {
        let mut batch = AttachmentBatch::default();
        for selection in paths {
            let path = chosen_path(Some(selection))?.expect("selection is present");
            let name = selected_name(&path);
            let result = (|| -> Result<_> {
                let metadata = path.metadata()?;
                if !metadata.is_file() {
                    return Err(Error::new("INVALID_ATTACHMENT", "请选择普通文件。"));
                }
                let limit = if images {
                    MAX_PREVIEW_BYTES
                } else {
                    MAX_ATTACHMENT_BYTES
                };
                if metadata.len() > limit {
                    return Err(Error::new(
                        "FILE_TOO_LARGE",
                        format!("单个文件不能超过 {} MiB。", limit / 1024 / 1024),
                    ));
                }
                let file = File::open(&path)?;
                if images {
                    store.add_image(&workspace, &page, &name, file)
                } else {
                    store.add_attachment(&workspace, &page, &name, file)
                }
            })();
            match result {
                Ok(import) => batch.added.push(import),
                Err(error) => batch.failed.push(AttachmentFailure { name, error }),
            }
        }
        Ok(batch)
    })
    .await
}

#[tauri::command]
pub(crate) async fn attach_image_bytes(
    app: AppHandle,
    request: Request<'_>,
) -> Result<AttachmentImport> {
    let header = |key: &str| -> Result<&str> {
        request
            .headers()
            .get(key)
            .and_then(|field| field.to_str().ok())
            .ok_or_else(|| Error::new("IMAGE_INPUT", "图片传输缺少目标或名称。"))
    };
    let workspace = header("x-zhiye-workspace")?.to_owned();
    let page = header("x-zhiye-page")?.to_owned();
    let name = percent_encoding::percent_decode_str(header("x-zhiye-name")?)
        .decode_utf8()
        .map_err(|_| Error::new("INVALID_ATTACHMENT", "附件名称编码无效。"))?
        .into_owned();
    let InvokeBody::Raw(data) = request.body() else {
        return Err(Error::new("IMAGE_INPUT", "图片需要以二进制传输。"));
    };
    if data.len() as u64 > MAX_PREVIEW_BYTES {
        return Err(Error::new("FILE_TOO_LARGE", "正文图片最大为 24 MiB。"));
    }
    let data = data.clone();
    run(app, move |store| {
        store.add_image(&workspace, &page, &name, data.as_slice())
    })
    .await
}

#[tauri::command]
pub(crate) async fn preview_attachment(
    app: AppHandle,
    workspace: String,
    page: String,
    id: String,
) -> Result<Response> {
    run(app, move |store| {
        let attachment = store.attachment(&workspace, &page, &id)?;
        if attachment.bytes > MAX_PREVIEW_BYTES
            || attachment.media_type == "application/octet-stream"
        {
            return Err(Error::new("PREVIEW_UNAVAILABLE", "此附件可另存为原件。"));
        }
        let mut bytes = Vec::with_capacity(attachment.bytes as usize);
        store.copy_attachment(&workspace, &page, &id, &mut bytes)?;
        Ok(Response::new(bytes))
    })
    .await
}

#[tauri::command]
pub(crate) async fn save_attachment_as(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    page: String,
    id: String,
) -> Result<bool> {
    let attachment = run(app.clone(), {
        let (workspace, page, id) = (workspace.clone(), page.clone(), id.clone());
        move |store| store.attachment(&workspace, &page, &id)
    })
    .await?;
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("另存附件")
        .set_file_name(&attachment.name)
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(false);
    };
    run(app, move |store| {
        write_export(store.data_root(), &path, |file| {
            store
                .copy_attachment(&workspace, &page, &id, file)
                .map(|_| ())
        })
    })
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use zhiye_core::{PropertyDraft, PropertyKind, RecordDraft};

    fn export_destination() -> tempfile::TempDir {
        let mut directory = tempfile::Builder::new();
        directory.prefix("zhiye-export-test-");
        if let Some(path) = std::env::var_os("ZHIYE_TEST_EXPORT_DESTINATION") {
            directory.tempdir_in(path).unwrap()
        } else {
            directory.tempdir().unwrap()
        }
    }

    #[test]
    fn export_capacity_error_preserves_previous_file_and_allows_retry() {
        let root = tempfile::tempdir().unwrap();
        let destination = export_destination();
        let target = destination.path().join("草稿.json");
        std::fs::write(&target, b"previous export").unwrap();

        for (kind, message) in [
            (std::io::ErrorKind::StorageFull, "存储空间不足"),
            (std::io::ErrorKind::QuotaExceeded, "存储配额不足"),
        ] {
            let error = write_export(root.path(), &target, |file| -> Result<()> {
                file.write_all(b"partial replacement")?;
                Err(std::io::Error::from(kind).into())
            })
            .unwrap_err();
            assert_eq!(error.code, "STORAGE_FULL");
            assert!(error.message.contains(message), "{error}");
            assert_eq!(std::fs::read(&target).unwrap(), b"previous export");
            assert_eq!(destination.path().read_dir().unwrap().count(), 1);
        }

        write_export(root.path(), &target, |file| {
            file.write_all(b"complete replacement")?;
            Ok(())
        })
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"complete replacement");
        assert_eq!(destination.path().read_dir().unwrap().count(), 1);
    }

    #[test]
    fn structured_draft_keeps_precommit_content_and_wide_unknown_payload() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("workspaces");
        std::fs::create_dir(&root).unwrap();
        let destination = export_destination();
        let file = destination.path().join("test.zhiye-draft.json");
        let unknown = serde_json::json!({
            "id": "8207e258-dacf-4f70-97dc-d86940c456ad", "type": "futureWidget", "schemaVersion": 9,
            "payload": {"wide": 9_007_199_254_740_993_u64}, "children": [], "plugin": "retain"
        });
        let document = DraftFile {
            format: "zhiye.page-draft",
            format_version: 1,
            workspace_id: "workspace".into(),
            page_id: "page".into(),
            base_revision: 12,
            draft: PageDraft {
                title: "  写入之前的中文  ".into(),
                blocks: vec![serde_json::from_value(unknown.clone()).unwrap()],
            },
        };
        write_draft_file(&root, &file, &document).unwrap();
        let saved: serde_json::Value = serde_json::from_reader(File::open(file).unwrap()).unwrap();
        assert_eq!(saved["format"], "zhiye.page-draft");
        assert_eq!(saved["formatVersion"], 1);
        assert_eq!(saved["baseRevision"], 12);
        assert_eq!(saved["draft"]["title"], "  写入之前的中文  ");
        assert_eq!(saved["draft"]["blocks"][0], unknown);
    }

    #[test]
    fn markdown_report_maps_source_links_and_describes_exported_changes() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("workspaces")).unwrap();
        let workspace = store.create_workspace("Markdown 导出").unwrap().id;
        let page = store
            .create_page(&workspace, None, "观测资料", "book")
            .unwrap();
        let source = store
            .create_page(&workspace, None, "引用源页", "file")
            .unwrap();
        let asset = store
            .add_attachment(
                &workspace,
                &page.info.id,
                "记录.txt",
                b"original".as_slice(),
            )
            .unwrap()
            .attachment;
        let unused = store
            .add_attachment(&workspace, &page.info.id, "备用.txt", b"unused".as_slice())
            .unwrap()
            .attachment;
        let reference = "ba46e583-321e-4183-bcf5-25f4ddb4cf40";
        let fold = "ed2e7faa-419b-476f-8773-1e2f081aacf8";
        let inline = "ba46e582-321e-4183-bcf5-25f4ddb4cf40";
        let future = "55aa5179-c92f-4455-9746-a776247be3ba";
        let source_block = "34b59887-7f5b-42e3-8b4a-db2cbcac3b83";
        store
            .save_page(
                &workspace,
                &source.info.id,
                source.info.revision,
                PageDraft {
                    title: source.info.title,
                    blocks: serde_json::from_value(serde_json::json!([{
                        "id": source_block, "type": "paragraph", "schemaVersion": 1,
                        "payload": {"inline": [{"type": "text", "text": "可回到源块"}]}
                    }]))
                    .unwrap(),
                },
            )
            .unwrap();
        store
            .save_page(
                &workspace,
                &page.info.id,
                page.info.revision,
                PageDraft {
                    title: page.info.title,
                    blocks: serde_json::from_value(serde_json::json!([
                        {
                            "id": fold, "type": "toggleBlock", "schemaVersion": 1,
                            "payload": {"title": "观测", "collapsed": true}, "children": [{
                                "id": inline, "type": "paragraph", "schemaVersion": 1,
                                "payload": {"inline": [
                                    {"type": "pageReference", "pageId": source.info.id, "label": "回到源页"},
                                    {"type": "text", "text": "注", "marks": [{"type": "underline"}]}
                                ]}
                            }]
                        },
                        {
                            "id": future, "type": "futureWidget", "schemaVersion": 3,
                            "payload": {"privateData": "raw only in the native package"}
                        },
                        {
                            "id": reference, "type": "blockReference", "schemaVersion": 1,
                            "payload": {"pageId": source.info.id, "blockId": source_block}
                        },
                        {
                            "id": "855fd901-801f-43a4-8c17-77e739e4a827",
                            "type": "attachmentFile", "schemaVersion": 1, "payload": {"attachmentId": asset.id}
                        }
                    ]))
                    .unwrap(),
                },
            )
            .unwrap();
        let snapshot = store.page(&workspace, &page.info.id).unwrap();
        let destination = export_destination();
        let target = destination.path().join("观测.md");
        let folder = write_page_document(
            &store,
            &workspace,
            &snapshot,
            &target,
            PageExportFormat::Markdown,
        )
        .unwrap();
        let companion = destination.path().join(&folder);
        let original = format!("{}.txt", asset.id);
        let relative = format!("{folder}/{original}");
        let original_path = companion.join(original);
        let markdown = std::fs::read_to_string(&target).unwrap();
        let report = std::fs::read_to_string(companion.join("导出报告.txt")).unwrap();
        assert!(markdown.contains("> [[回到源页]]<u>注</u>"));
        assert!(markdown.contains(&format!("[附件](<{relative}>)")));
        assert!(!markdown.contains("知页 Markdown 导出报告"));
        assert_eq!(std::fs::read(&original_path).unwrap(), b"original");
        let source_location = zhiye_core::PageLocation::page(&workspace, &source.info.id);
        for (id, detail) in [
            (fold, "展开状态"),
            (inline, "下划线"),
            (future, "futureWidget"),
            (inline, &source_location.to_string()),
            (reference, &source_location.block(source_block).to_string()),
            (&asset.sha256, &relative),
        ] {
            assert!(
                report
                    .lines()
                    .any(|line| line.contains(id) && line.contains(detail)),
                "{id} {detail}"
            );
        }
        assert!(report.contains(&format!("修订  {}", snapshot.info.revision)));
        assert!(!report.contains(&unused.id));
        assert!(!report.contains("raw only in the native package"));
        assert_eq!(store.page(&workspace, &page.info.id).unwrap(), snapshot);

        std::fs::remove_file(
            store
                .data_root()
                .join(&workspace)
                .join("blobs")
                .join(&asset.sha256),
        )
        .unwrap();
        let failed = write_page_document(
            &store,
            &workspace,
            &snapshot,
            &target,
            PageExportFormat::Markdown,
        );
        assert_eq!(failed.err().unwrap().code, "ATTACHMENT_MISSING");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), markdown);
        assert_eq!(
            std::fs::read_to_string(companion.join("导出报告.txt")).unwrap(),
            report
        );
        assert_eq!(std::fs::read(original_path).unwrap(), b"original");
        assert_eq!(destination.path().read_dir().unwrap().count(), 2);
    }

    #[test]
    fn html_file_export_saves_portable_markup_original_and_report() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("workspaces")).unwrap();
        let workspace = store.create_workspace("HTML 导出").unwrap().id;
        let page = store
            .create_page(&workspace, None, "用量 <&>", "file")
            .unwrap();
        let source = store
            .create_page(&workspace, None, "源记录", "file")
            .unwrap();
        let asset = store
            .add_attachment(
                &workspace,
                &page.info.id,
                "测量.txt",
                b"measurements".as_slice(),
            )
            .unwrap()
            .attachment;
        store
            .save_page(
                &workspace,
                &page.info.id,
                page.info.revision,
                PageDraft {
                    title: page.info.title,
                    blocks: serde_json::from_value(serde_json::json!([
                        {
                            "id": "3d665fd4-cfdb-4cfe-ace2-02a038893b25", "type": "paragraph", "schemaVersion": 1,
                            "payload": {"inline": [
                                {"type": "text", "text": "静态文字", "marks": [{"type": "underline"}]},
                                {"type": "pageReference", "pageId": source.info.id, "label": "原记录"},
                                {"type": "text", "text": "<script>文本</script>"}
                            ]}
                        },
                        {
                            "id": "3d665fd4-cfdb-4cfe-ace2-02a038893b26", "type": "attachmentFile", "schemaVersion": 1,
                            "payload": {"attachmentId": asset.id}
                        },
                        {
                            "id": "3d665fd4-cfdb-4cfe-ace2-02a038893b27", "type": "futureWidget", "schemaVersion": 2,
                            "payload": {"raw": "仅保留在原生包"}
                        }
                    ]))
                    .unwrap(),
                },
            )
            .unwrap();
        let snapshot = store.page(&workspace, &page.info.id).unwrap();
        let destination = export_destination();
        let target = destination.path().join("使用说明.html");
        let folder = write_page_document(
            &store,
            &workspace,
            &snapshot,
            &target,
            PageExportFormat::Html,
        )
        .unwrap();
        let html = std::fs::read_to_string(&target).unwrap();
        let relative = format!("{folder}/{}.txt", asset.id);
        let original_path = destination.path().join(&relative);
        let report_path = destination.path().join(&folder).join("导出报告.txt");
        let report = std::fs::read_to_string(&report_path).unwrap();
        let location = zhiye_core::PageLocation::page(&workspace, &source.info.id).to_string();
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.contains("<title>用量 &lt;&amp;&gt;</title>"));
        assert!(html.contains("<h1>用量 &lt;&amp;&gt;</h1>"));
        assert!(html.contains("<u>静态文字</u>"));
        assert!(html.contains(&format!("<a href=\"{location}\">原记录</a>")));
        assert!(html.contains(&format!("href=\"{relative}\"")));
        assert!(html.contains("&lt;script&gt;文本&lt;/script&gt;"));
        assert!(!html.contains("<script>") && !html.contains("仅保留在原生包"));
        assert!(report.starts_with("知页 HTML 导出报告"));
        assert!(report.contains("futureWidget") && report.contains(&location));
        assert!(report.contains(&relative) && report.contains(&asset.sha256));
        assert_eq!(std::fs::read(&original_path).unwrap(), b"measurements");

        std::fs::remove_file(
            store
                .data_root()
                .join(&workspace)
                .join("blobs")
                .join(asset.sha256),
        )
        .unwrap();
        assert_eq!(
            write_page_document(
                &store,
                &workspace,
                &snapshot,
                &target,
                PageExportFormat::Html
            )
            .unwrap_err()
            .code,
            "ATTACHMENT_MISSING"
        );
        assert_eq!(std::fs::read_to_string(&target).unwrap(), html);
        assert_eq!(std::fs::read_to_string(report_path).unwrap(), report);
        assert_eq!(std::fs::read(original_path).unwrap(), b"measurements");
        assert_eq!(destination.path().read_dir().unwrap().count(), 2);
    }

    #[test]
    fn csv_export_copies_original_and_preserves_previous_export_after_failure() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path().join("workspaces")).unwrap();
        let workspace = store.create_workspace("导出事务").unwrap().id;
        let (database, _) = store.create_collection(&workspace, None, "资料").unwrap();
        let schema = store
            .save_property(
                &workspace,
                &database.id,
                1,
                PropertyDraft {
                    id: None,
                    name: "原件".into(),
                    kind: PropertyKind::Attachment,
                    options: Vec::new(),
                },
            )
            .unwrap();
        let field = schema.properties.last().unwrap().id.clone();
        let record = store
            .create_page(
                &workspace,
                Some(&database.container_page_id),
                "文件",
                "file",
            )
            .unwrap();
        let asset = store
            .add_attachment(
                &workspace,
                &record.info.id,
                "原件.txt",
                b"original".as_slice(),
            )
            .unwrap()
            .attachment;
        store
            .save_record(
                &workspace,
                &database.id,
                &record.info.id,
                1,
                RecordDraft {
                    title: "文件".into(),
                    schema_revision: schema.schema_revision,
                    properties: BTreeMap::from([(field, serde_json::json!([asset.id]))]),
                },
            )
            .unwrap();
        let destination = export_destination();
        let target = destination.path().join("existing.csv");
        std::fs::write(&target, "existing").unwrap();
        let exported =
            write_collection_csv(&store, &workspace, &database.id, None, &target).unwrap();
        assert_eq!(exported.rows, 1);
        assert_eq!(exported.attachments, 1);
        let relative = format!("{}/attachments/{}.txt", exported.companion_folder, asset.id);
        let report_path = destination
            .path()
            .join(&exported.companion_folder)
            .join("导出报告.txt");
        let csv = std::fs::read_to_string(&target).unwrap();
        let report = std::fs::read_to_string(&report_path).unwrap();
        assert!(csv.contains(&relative));
        assert!(report.contains(&relative) && report.contains("原件.txt"));
        let original = destination.path().join(relative);
        assert_eq!(std::fs::read(&original).unwrap(), b"original");

        std::fs::remove_file(
            store
                .data_root()
                .join(&workspace)
                .join("blobs")
                .join(asset.sha256),
        )
        .unwrap();
        let export = write_collection_csv(&store, &workspace, &database.id, None, &target);
        assert_eq!(export.err().unwrap().code, "ATTACHMENT_MISSING");
        assert_eq!(std::fs::read_to_string(&target).unwrap(), csv);
        assert_eq!(std::fs::read_to_string(report_path).unwrap(), report);
        assert_eq!(std::fs::read(original).unwrap(), b"original");
        assert_eq!(destination.path().read_dir().unwrap().count(), 2);
    }
}
