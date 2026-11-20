// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{
    local_api::{LocalApi, LocalApiStatus},
    run,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use zhiye_core::{
    Integration, IntegrationDraft, IssuedIntegration, ProposalAction, ProposalReview,
    ProposalSummary, Result,
};

#[derive(Default)]
pub struct ApiState(Mutex<Option<LocalApi>>);

#[tauri::command]
pub async fn list_integrations(app: AppHandle, workspace: String) -> Result<Vec<Integration>> {
    run(app, move |store| store.integrations(&workspace)).await
}

#[tauri::command]
pub async fn create_integration(
    app: AppHandle,
    workspace: String,
    draft: IntegrationDraft,
    expires_at: Option<i64>,
) -> Result<IssuedIntegration> {
    run(app, move |store| {
        store.issue_integration_with_expiry(&workspace, draft, expires_at)
    })
    .await
}

#[tauri::command]
pub async fn revoke_integration(app: AppHandle, workspace: String, id: String) -> Result<()> {
    run(app, move |store| store.revoke_integration(&workspace, &id)).await
}

#[tauri::command]
pub async fn list_proposals(
    app: AppHandle,
    workspace: String,
    page_id: Option<String>,
) -> Result<Vec<ProposalSummary>> {
    run(app, move |store| {
        store.proposals(&workspace, page_id.as_deref())
    })
    .await
}

#[tauri::command]
pub async fn read_proposal(
    app: AppHandle,
    workspace: String,
    id: String,
) -> Result<ProposalReview> {
    run(app, move |store| store.proposal_review(&workspace, &id)).await
}

#[tauri::command]
pub async fn review_proposal(
    app: AppHandle,
    workspace: String,
    id: String,
    review_revision: u32,
    action: ProposalAction,
) -> Result<ProposalReview> {
    run(app, move |store| {
        store.review_proposal(&workspace, &id, review_revision, action)
    })
    .await
}

#[tauri::command]
pub async fn api_attachment_limit(app: AppHandle, workspace: String) -> Result<u32> {
    run(app, move |store| store.api_attachment_limit_mib(&workspace)).await
}

#[tauri::command]
pub async fn set_api_attachment_limit(app: AppHandle, workspace: String, limit: u32) -> Result<()> {
    run(app, move |store| {
        store.set_api_attachment_limit_mib(&workspace, limit)
    })
    .await
}

#[tauri::command]
pub async fn local_api_status(app: AppHandle) -> LocalApiStatus {
    let state = app.state::<ApiState>();
    let api = state.0.lock().await;
    LocalApi::status(api.as_ref())
}

#[tauri::command]
pub async fn set_local_api(app: AppHandle, enabled: bool) -> Result<LocalApiStatus> {
    let state = app.state::<ApiState>();
    let mut api = state.0.lock().await;
    if enabled {
        if LocalApi::status(api.as_ref()).address.is_none() {
            let store = run(app.clone(), |store| Ok(Arc::clone(store))).await?;
            let window_app = app.clone();
            *api = Some(LocalApi::start(store, move |workspace| {
                let _ = window_app.emit("workspace-content-changed", workspace);
            })?);
        }
    } else {
        drop(api.take());
    }
    Ok(LocalApi::status(api.as_ref()))
}
