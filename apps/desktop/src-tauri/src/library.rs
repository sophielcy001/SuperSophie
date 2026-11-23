// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{run, wire_page};
use tauri::AppHandle;
use zhiye_core::{Navigation, Page, Result, TemplateSummary};

#[tauri::command]
pub(crate) async fn read_navigation(app: AppHandle, workspace: String) -> Result<Navigation> {
    run(app, move |store| store.navigation(&workspace)).await
}

#[tauri::command]
pub(crate) async fn set_favorite(
    app: AppHandle,
    workspace: String,
    page: String,
    favorite: bool,
) -> Result<Navigation> {
    run(app, move |store| {
        store.set_favorite(&workspace, &page, favorite)
    })
    .await
}

#[tauri::command]
pub(crate) async fn visit_page(
    app: AppHandle,
    workspace: String,
    page: String,
) -> Result<Navigation> {
    run(app, move |store| store.visit_page(&workspace, &page)).await
}

#[tauri::command]
pub(crate) async fn set_inbox(
    app: AppHandle,
    workspace: String,
    page: String,
) -> Result<Navigation> {
    run(app, move |store| store.set_inbox(&workspace, &page)).await
}

#[tauri::command]
pub(crate) async fn enter_inbox(app: AppHandle, workspace: String, capture: bool) -> Result<Page> {
    run(app, move |store| {
        let page = if capture {
            store.capture_page(&workspace, "无标题")?
        } else {
            store.open_inbox(&workspace)?
        };
        wire_page(store, &workspace, page)
    })
    .await
}

#[tauri::command]
pub(crate) async fn list_templates(
    app: AppHandle,
    workspace: String,
) -> Result<Vec<TemplateSummary>> {
    run(app, move |store| store.templates(&workspace)).await
}

#[tauri::command]
pub(crate) async fn save_template(
    app: AppHandle,
    workspace: String,
    source: String,
    name: String,
    template_id: Option<String>,
) -> Result<TemplateSummary> {
    run(app, move |store| {
        store.save_template(&workspace, &source, &name, template_id.as_deref())
    })
    .await
}

#[tauri::command]
pub(crate) async fn delete_template(app: AppHandle, workspace: String, id: String) -> Result<()> {
    run(app, move |store| store.delete_template(&workspace, &id)).await
}

#[tauri::command]
pub(crate) async fn apply_template(
    app: AppHandle,
    workspace: String,
    id: String,
    parent_id: Option<String>,
    title: Option<String>,
) -> Result<Page> {
    run(app, move |store| {
        wire_page(
            store,
            &workspace,
            store.apply_template(&workspace, &id, parent_id.as_deref(), title.as_deref())?,
        )
    })
    .await
}
