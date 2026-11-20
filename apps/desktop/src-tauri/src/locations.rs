// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{run, wire_page};
use std::sync::Mutex;
use tauri::{App, AppHandle, Emitter, Manager};
use tauri_plugin_deep_link::DeepLinkExt;
use zhiye_core::{Error, PageLocation, PdfTarget, ResolvedLocation, Result};

const READY_EVENT: &str = "location-ready";

#[derive(Default)]
struct LocationInbox(Mutex<Option<String>>);

fn enqueue(app: &AppHandle, urls: Vec<url::Url>) {
    let Some(url) = urls.into_iter().rev().find(|url| url.scheme() == "zhiye") else {
        return;
    };
    let inbox = app.state::<LocationInbox>();
    if let Ok(mut location) = inbox.0.lock() {
        *location = Some(url.to_string());
    }
    let _ = app.emit(READY_EVENT, ());
}

pub(crate) fn install(app: &App) -> tauri_plugin_deep_link::Result<()> {
    app.manage(LocationInbox::default());
    let handle = app.handle().clone();
    app.deep_link()
        .on_open_url(move |event| enqueue(&handle, event.urls()));
    if let Some(urls) = app.deep_link().get_current()? {
        enqueue(app.handle(), urls);
    }
    Ok(())
}

#[tauri::command]
pub(crate) fn take_location(app: AppHandle) -> Result<Option<String>> {
    let inbox = app.state::<LocationInbox>();
    let mut location = inbox
        .0
        .lock()
        .map_err(|_| Error::new("LOCATION_STATE", "当前无法取得知页定位链接。"))?;
    Ok(location.take())
}

#[tauri::command]
pub(crate) async fn resolve_location(app: AppHandle, url: String) -> Result<ResolvedLocation> {
    run(app, move |store| {
        let mut location = store.resolve_location(&url)?;
        location.page = wire_page(store, &location.workspace.id, location.page)?;
        Ok(location)
    })
    .await
}

#[tauri::command]
pub(crate) fn make_location(
    workspace: String,
    page: String,
    block: Option<String>,
    pdf: Option<PdfTarget>,
) -> Result<String> {
    if block.is_some() && pdf.is_some() {
        return Err(Error::new("INVALID_LOCATION", "一条链接只可定位一个来源。"));
    }
    let target = PageLocation::page(&workspace, &page);
    let url = if let Some(pdf) = pdf.as_ref() {
        target.pdf(&pdf.attachment_id, &pdf.sha256, pdf.page_number)
    } else {
        block.as_deref().map_or(target, |id| target.block(id))
    }
    .to_string();
    PageLocation::parse(&url)?;
    Ok(url)
}
