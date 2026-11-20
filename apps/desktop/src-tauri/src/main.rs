// SPDX-License-Identifier: AGPL-3.0-or-later

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backup;
mod collections;
mod derived_jobs;
mod files;
mod integrations;
mod library;
mod local_api;
mod locations;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(feature = "performance-probe")]
mod performance;
mod starter;

use files::{PageExportResult, chosen_path, file_stem, write_export, write_page_document};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Manager, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use zhiye_core::{
    BlobCleanup, BlobCleanupPreview, DesktopPageList, Error, Page, PageDraft, PageExportFormat,
    PageInfo, PageMoves, PageVersion, PdfDocumentStatus, PdfIndexPoll, PdfIndexResult, PdfProgress,
    Result, RevisionAttribution, SearchIndexStatus, SearchRequest, SearchResponse, Store,
    TrashPreview, TrashRemoval, Workspace, WorkspaceOverview, decode_desktop_blocks,
    encode_desktop_blocks, render_workspace_markdown, web_url,
};

struct Backend {
    root: PathBuf,
    state: Mutex<OpenStore>,
}

#[derive(Default)]
struct OpenStore {
    store: Option<Arc<Store>>,
    resolved_root: Option<PathBuf>,
}

fn lock_backend(backend: &Backend) -> Result<std::sync::MutexGuard<'_, OpenStore>> {
    backend
        .state
        .lock()
        .map_err(|_| Error::new("STORE_STATE", "本地服务需要重新启动。"))
}

fn data_root(app: &AppHandle) -> Result<PathBuf> {
    let backend = app.state::<Backend>();
    let state = lock_backend(&backend)?;
    Ok(state
        .resolved_root
        .as_ref()
        .unwrap_or(&backend.root)
        .clone())
}

async fn run<T: Send + 'static>(
    app: AppHandle,
    job: impl FnOnce(&Arc<Store>) -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(move || {
        let backend = app.state::<Backend>();
        let mut state = lock_backend(&backend)?;
        if state.store.is_none() {
            let opened = Store::open(&backend.root)?;
            let overview = opened.workspace_overview()?;
            if overview.workspaces.is_empty() && overview.issues.is_empty() {
                starter::create(&opened)?;
            }
            state.resolved_root = Some(opened.data_root().to_path_buf());
            let opened = Arc::new(opened);
            derived_jobs::start(&opened);
            backup::start(&opened, app.clone());
            state.store = Some(opened);
        }
        job(state.store.as_ref().expect("store was initialized"))
    })
    .await
    .map_err(|error| Error::new("STORE_TASK", error.to_string()))?
}

async fn run_independent<T: Send + 'static>(
    app: AppHandle,
    job: impl FnOnce(&Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = run(app, |store| Ok(Arc::clone(store))).await?;
    tauri::async_runtime::spawn_blocking(move || job(&store))
        .await
        .map_err(|error| Error::new("STORE_TASK", error.to_string()))?
}

#[tauri::command]
async fn list_workspaces(app: AppHandle) -> Result<WorkspaceOverview> {
    run(app, |store| store.workspace_overview()).await
}

#[tauri::command]
async fn create_workspace(app: AppHandle, name: String) -> Result<Workspace> {
    run(app, move |store| store.create_workspace(&name)).await
}

#[tauri::command]
async fn list_pages(
    app: AppHandle,
    workspace: String,
    compact: Option<bool>,
) -> Result<DesktopPageList> {
    run(app, move |store| {
        let pages = store.pages(&workspace)?;
        Ok(DesktopPageList::new(pages, compact.unwrap_or_default()))
    })
    .await
}

#[tauri::command]
async fn read_page(app: AppHandle, workspace: String, id: String) -> Result<Page> {
    run(app, move |store| {
        wire_page(store, &workspace, store.page(&workspace, &id)?)
    })
    .await
}

#[tauri::command]
async fn create_page(app: AppHandle, workspace: String, parent_id: Option<String>) -> Result<Page> {
    run(app, move |store| {
        store.create_page(&workspace, parent_id.as_deref(), "无标题", "file")
    })
    .await
}

#[tauri::command]
async fn save_page(
    app: AppHandle,
    workspace: String,
    id: String,
    revision: u32,
    mut draft: PageDraft,
) -> Result<PageInfo> {
    run(app, move |store| {
        draft.blocks = decode_desktop_blocks(draft.blocks)?;
        store.save_page(&workspace, &id, revision, draft)
    })
    .await
}

#[tauri::command]
async fn set_page_appearance(
    app: AppHandle,
    workspace: String,
    id: String,
    revision: u32,
    icon: String,
    cover_attachment_id: Option<String>,
) -> Result<Page> {
    run(app, move |store| {
        let page = store.set_page_appearance(
            &workspace,
            &id,
            revision,
            &icon,
            cover_attachment_id.as_deref(),
        )?;
        wire_page(store, &workspace, page)
    })
    .await
}

#[tauri::command]
async fn move_page(
    app: AppHandle,
    workspace: String,
    id: String,
    revision: u32,
    parent_id: Option<String>,
) -> Result<PageInfo> {
    run(app, move |store| {
        store.move_page(&workspace, &id, revision, parent_id.as_deref())
    })
    .await
}

#[tauri::command]
async fn archive_page(app: AppHandle, workspace: String, id: String, revision: u32) -> Result<()> {
    run(app, move |store| {
        store.archive_page(&workspace, &id, revision)
    })
    .await
}

#[tauri::command]
async fn restore_page(app: AppHandle, workspace: String, id: String, revision: u32) -> Result<()> {
    run(app, move |store| {
        store.restore_page(&workspace, &id, revision)
    })
    .await
}

#[tauri::command]
async fn move_pages(
    app: AppHandle,
    workspace: String,
    pages: Vec<PageVersion>,
    parent_id: Option<String>,
) -> Result<Vec<PageInfo>> {
    run(app, move |store| {
        store.move_pages(&workspace, &pages, parent_id.as_deref())
    })
    .await
}

#[tauri::command]
async fn move_pages_with_impact(
    app: AppHandle,
    workspace: String,
    pages: Vec<PageVersion>,
    parent_id: Option<String>,
) -> Result<PageMoves> {
    run(app, move |store| {
        store.move_pages_with_impact(&workspace, &pages, parent_id.as_deref())
    })
    .await
}

#[tauri::command]
async fn archive_pages(
    app: AppHandle,
    workspace: String,
    pages: Vec<PageVersion>,
) -> Result<usize> {
    run(app, move |store| store.archive_pages(&workspace, &pages)).await
}

#[tauri::command]
async fn restore_pages(
    app: AppHandle,
    workspace: String,
    pages: Vec<PageVersion>,
) -> Result<usize> {
    run(app, move |store| store.restore_pages(&workspace, &pages)).await
}

#[tauri::command]
async fn preview_empty_trash(app: AppHandle, workspace: String) -> Result<TrashPreview> {
    run(app, move |store| store.preview_empty_trash(&workspace)).await
}

#[tauri::command]
async fn empty_trash(
    app: AppHandle,
    workspace: String,
    confirmation: String,
) -> Result<TrashRemoval> {
    run(app, move |store| {
        store.empty_trash(&workspace, &confirmation)
    })
    .await
}

#[tauri::command]
async fn preview_blob_cleanup(app: AppHandle, workspace: String) -> Result<BlobCleanupPreview> {
    run(app, move |store| store.preview_blob_cleanup(&workspace)).await
}

#[tauri::command]
async fn clean_blobs(
    app: AppHandle,
    workspace: String,
    confirmation: String,
) -> Result<BlobCleanup> {
    run(app, move |store| {
        store.clean_blobs(&workspace, &confirmation)
    })
    .await
}

#[derive(serde::Serialize)]
struct HistoryEntry {
    #[serde(flatten)]
    page: Page,
    #[serde(skip_serializing_if = "Option::is_none")]
    attribution: Option<RevisionAttribution>,
}

#[tauri::command]
async fn page_history(app: AppHandle, workspace: String, id: String) -> Result<Vec<HistoryEntry>> {
    run(app, move |store| {
        let history = store.history(&workspace, &id)?;
        let mut sources = store.revision_attributions(&workspace, &id)?;
        history
            .into_iter()
            .map(|page| {
                let attribution = sources.remove(&page.info.revision);
                Ok(HistoryEntry {
                    page: wire_page(store, &workspace, page)?,
                    attribution,
                })
            })
            .collect()
    })
    .await
}

#[tauri::command]
async fn restore_version(
    app: AppHandle,
    workspace: String,
    id: String,
    revision: u32,
    target: u32,
) -> Result<Page> {
    run(app, move |store| {
        wire_page(
            store,
            &workspace,
            store.restore_version(&workspace, &id, revision, target)?,
        )
    })
    .await
}

#[tauri::command]
async fn search_pages(app: AppHandle, workspace: String, query: String) -> Result<Vec<PageInfo>> {
    run(app, move |store| store.search(&workspace, &query)).await
}

#[tauri::command]
async fn search_text(
    app: AppHandle,
    workspace: String,
    request: SearchRequest,
) -> Result<SearchResponse> {
    run(app, move |store| store.search_text(&workspace, &request)).await
}

#[tauri::command]
async fn advance_search_index(app: AppHandle, workspace: String) -> Result<SearchIndexStatus> {
    run(app, move |store| store.advance_search_index(&workspace)).await
}

#[tauri::command]
async fn rebuild_search_index(app: AppHandle, workspace: String) -> Result<SearchIndexStatus> {
    run(app, move |store| store.rebuild_search_index(&workspace)).await
}

#[tauri::command]
async fn poll_pdf_index(app: AppHandle, workspace: String) -> Result<PdfIndexPoll> {
    run(app, move |store| store.poll_pdf_index(&workspace)).await
}

#[tauri::command]
async fn finish_pdf_index(
    app: AppHandle,
    workspace: String,
    result: PdfIndexResult,
) -> Result<PdfProgress> {
    run(app, move |store| store.finish_pdf_index(&workspace, result)).await
}

#[tauri::command]
async fn pdf_document_status(
    app: AppHandle,
    workspace: String,
    page: String,
) -> Result<Vec<PdfDocumentStatus>> {
    run(app, move |store| {
        store.pdf_document_status(&workspace, &page)
    })
    .await
}

fn wire_page(store: &Store, workspace: &str, page: Page) -> Result<Page> {
    let mut page = store.project_desktop_page(workspace, page)?;
    page.blocks = encode_desktop_blocks(page.blocks)?;
    Ok(page)
}

#[tauri::command]
async fn export_markdown(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    id: String,
    draft: Option<PageDraft>,
) -> Result<Option<PageExportResult>> {
    let Some(draft) = draft else {
        return export_saved_page(window, app, workspace, id, PageExportFormat::Markdown).await;
    };
    let blocks = decode_desktop_blocks(draft.blocks)?;
    let Some(path) =
        page_export_destination(&app, &window, &draft.title, PageExportFormat::Markdown)?
    else {
        return Ok(None);
    };
    let markdown =
        render_workspace_markdown(&draft.title, &blocks, &Default::default(), Some(&workspace));
    tauri::async_runtime::spawn_blocking(move || {
        let root = data_root(&app)?;
        write_export(&root, &path, |file| {
            std::io::Write::write_all(file, markdown.as_bytes())?;
            Ok(())
        })
    })
    .await
    .map_err(|error| Error::new("STORE_TASK", error.to_string()))??;
    Ok(Some(PageExportResult {
        companion_folder: None,
    }))
}

#[tauri::command]
async fn export_html(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    id: String,
) -> Result<Option<PageExportResult>> {
    export_saved_page(window, app, workspace, id, PageExportFormat::Html).await
}

async fn export_saved_page(
    window: WebviewWindow,
    app: AppHandle,
    workspace: String,
    id: String,
    format: PageExportFormat,
) -> Result<Option<PageExportResult>> {
    let page = run(app.clone(), {
        let workspace = workspace.clone();
        move |store| store.page(&workspace, &id)
    })
    .await?;
    let Some(path) = page_export_destination(&app, &window, &page.info.title, format)? else {
        return Ok(None);
    };
    let folder = run(app, move |store| {
        write_page_document(store, &workspace, &page, &path, format)
    })
    .await?;
    Ok(Some(PageExportResult {
        companion_folder: Some(folder),
    }))
}

fn page_export_destination(
    app: &AppHandle,
    window: &WebviewWindow,
    title: &str,
    format: PageExportFormat,
) -> Result<Option<PathBuf>> {
    chosen_path(
        app.dialog()
            .file()
            .set_parent(window)
            .set_title(format!("导出 {}", format.label()))
            .add_filter(format.label(), &[format.extension()])
            .set_file_name(format!("{}.{}", file_stem(title), format.extension()))
            .blocking_save_file(),
    )
}

#[tauri::command]
async fn read_block_reference(
    app: AppHandle,
    workspace: String,
    page: String,
    block: String,
    known_revision: Option<u32>,
) -> Result<zhiye_core::DesktopReference> {
    run(app, move |store| {
        store.desktop_reference(&workspace, &page, &block, known_revision)
    })
    .await
}

#[tauri::command]
async fn list_backlinks(
    app: AppHandle,
    workspace: String,
    page: String,
    after: Option<String>,
) -> Result<zhiye_core::BacklinkPage> {
    run(app, move |store| {
        store.backlinks(&workspace, &page, after.as_deref())
    })
    .await
}

#[tauri::command]
fn open_web_link(app: AppHandle, href: String) -> Result<()> {
    let url = web_url(&href)
        .ok_or_else(|| Error::new("INVALID_URL", "链接需要完整的 HTTP 或 HTTPS 地址。"))?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|error| Error::new("OPEN_LINK", format!("无法打开网页链接。{error}")))
}

fn main() {
    let builder = tauri::Builder::default();
    #[cfg(feature = "performance-probe")]
    let builder = builder.on_page_load(performance::page_loaded);
    #[cfg(windows)]
    let builder = if std::env::var_os("ZHIYE_DATA_DIR").is_none() {
        builder.plugin(tauri_plugin_single_instance::init(|app, _, _| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        }))
    } else {
        // Explicit data roots keep their independent, file-based writer checks.
        builder
    };
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(macos::menu)
        .on_menu_event(macos::on_menu_event);

    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .setup(|app| {
            let root = match std::env::var_os("ZHIYE_DATA_DIR") {
                Some(directory) => std::path::absolute(PathBuf::from(directory))?,
                None => app.path().app_local_data_dir()?.join("workspaces"),
            };
            app.manage(Backend {
                root,
                state: Mutex::new(OpenStore::default()),
            });
            app.manage(integrations::ApiState::default());
            app.manage(files::MigrationState::default());
            locations::install(app)?;
            #[cfg(target_os = "macos")]
            macos::termination::install(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            #[cfg(target_os = "macos")]
            macos::termination::finish_macos_quit,
            #[cfg(feature = "performance-probe")]
            performance::record_performance_ready,
            list_workspaces,
            create_workspace,
            list_pages,
            read_page,
            read_block_reference,
            list_backlinks,
            create_page,
            save_page,
            set_page_appearance,
            move_page,
            archive_page,
            restore_page,
            move_pages,
            move_pages_with_impact,
            archive_pages,
            restore_pages,
            preview_empty_trash,
            empty_trash,
            preview_blob_cleanup,
            clean_blobs,
            page_history,
            restore_version,
            search_pages,
            search_text,
            advance_search_index,
            rebuild_search_index,
            poll_pdf_index,
            finish_pdf_index,
            pdf_document_status,
            collections::list_collections,
            collections::create_collection,
            collections::read_collection,
            collections::list_records,
            collections::read_record,
            collections::save_property,
            collections::preview_property_conversion,
            collections::apply_property_conversion,
            collections::save_computed_property,
            collections::create_relation,
            collections::list_relation_targets,
            collections::save_collection_view,
            collections::upsert_collection_view,
            collections::delete_collection_view,
            collections::move_scheduled_record,
            collections::save_record,
            backup::backup_status,
            backup::set_backup_policy,
            backup::choose_backup_directory,
            backup::run_workspace_backup,
            backup::export_recovery,
            files::export_workspace,
            files::export_collection_csv,
            files::import_workspace,
            files::merge_workspace,
            files::list_attachments,
            files::read_attachment,
            files::add_attachments,
            files::pick_images,
            files::attach_image_bytes,
            files::rename_attachment,
            files::preview_attachment,
            files::save_attachment_as,
            files::import_markdown_files,
            files::import_csv_file,
            files::import_migration,
            files::cancel_migration,
            files::latest_migration,
            files::save_import_report,
            files::export_page_draft,
            files::export_record_draft,
            library::read_navigation,
            library::set_favorite,
            library::visit_page,
            library::set_inbox,
            library::enter_inbox,
            library::list_templates,
            library::save_template,
            library::delete_template,
            library::apply_template,
            integrations::list_integrations,
            integrations::create_integration,
            integrations::revoke_integration,
            integrations::list_proposals,
            integrations::read_proposal,
            integrations::review_proposal,
            integrations::api_attachment_limit,
            integrations::set_api_attachment_limit,
            integrations::local_api_status,
            integrations::set_local_api,
            locations::take_location,
            locations::resolve_location,
            locations::make_location,
            export_markdown,
            export_html,
            open_web_link,
        ])
        .run(tauri::generate_context!())
        .expect("Could not start Zhiye");
}
