// SPDX-License-Identifier: AGPL-3.0-or-later

//! Desktop side of personal sync: device registration, per-workspace switches,
//! manual and periodic runs. Core decides what to send and how to merge; this
//! module only moves requests over HTTPS.

mod credentials;
pub(crate) mod remote;

use crate::run;
use remote::{Remote, display, server_url};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};
use tokio::sync::Mutex;
use zhiye_core::{Error, Result, Store, SyncReport, SyncStatus, Workspace};

const DEVICE_FILE: &str = "sync-device.json";
const FIRST_RUN: Duration = Duration::from_secs(20);
const INTERVAL: Duration = Duration::from_secs(5 * 60);
/// Guards against a service that keeps answering `hasMore`.
const MAX_PAGES: usize = 10_000;

/// Serializes sync runs so a manual run and the timer never overlap.
#[derive(Default)]
pub struct SyncState {
    runs: Mutex<()>,
}

/// This computer's registration. The token itself is in the system credential store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncDevice {
    pub server: String,
    pub device_id: String,
    pub device_name: String,
    pub registered_at: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncOverview {
    pub device: Option<SyncDevice>,
    pub status: SyncStatus,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteWorkspace {
    pub id: String,
    pub name: String,
    pub local: bool,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

async fn root(app: &AppHandle) -> Result<PathBuf> {
    run(app.clone(), |store| Ok(store.data_root().to_path_buf())).await
}

fn read_device(root: &Path) -> Result<Option<SyncDevice>> {
    match std::fs::read(root.join(DEVICE_FILE)) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn write_device(root: &Path, device: &SyncDevice) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(root)?;
    file.write_all(&serde_json::to_vec_pretty(device)?)?;
    file.as_file().sync_all()?;
    file.persist(root.join(DEVICE_FILE))
        .map_err(|error| Error::from(error.error))?;
    Ok(())
}

fn device_name(input: &str) -> Result<String> {
    let name = input.trim();
    if !zhiye_sync_protocol::valid_name(name, zhiye_sync_protocol::MAX_NAME_CHARS) {
        return Err(Error::new(
            "SYNC_DEVICE_NAME",
            "设备名称需要 1 至 64 个字符，且不能包含换行。",
        ));
    }
    Ok(name.to_owned())
}

async fn blocking<T: Send + 'static>(
    store: &Arc<Store>,
    job: impl FnOnce(&Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = Arc::clone(store);
    tokio::task::spawn_blocking(move || job(&store))
        .await
        .map_err(|error| Error::new("STORE_TASK", error.to_string()))?
}

/// One full exchange: pull, push until nothing is left, pull again.
pub(crate) async fn exchange(
    store: &Arc<Store>,
    remote: &Remote,
    workspace: &str,
) -> Result<SyncReport> {
    let mut total = SyncReport::default();
    fn merge(total: &mut SyncReport, report: SyncReport) {
        total.accepted += report.accepted;
        total.applied += report.applied;
        total.conflicts.extend(report.conflicts);
        total.pending = report.pending;
        total.cursor = report.cursor;
    }
    async fn pull(
        store: &Arc<Store>,
        remote: &Remote,
        workspace: &str,
        total: &mut SyncReport,
    ) -> Result<()> {
        for _ in 0..MAX_PAGES {
            let id = workspace.to_owned();
            let cursor = blocking(store, move |store| store.sync_cursor(&id)).await?;
            let page = remote.pull(workspace, cursor).await?;
            download(store, remote, workspace, page.blob_hashes()).await?;
            let more = page.has_more && page.next_seq > cursor;
            let id = workspace.to_owned();
            let report = blocking(store, move |store| store.sync_apply_pull(&id, &page)).await?;
            merge(total, report);
            if !more {
                return Ok(());
            }
        }
        Err(Error::new("SYNC_RESPONSE", "同步服务返回的变更页数过多。"))
    }

    pull(store, remote, workspace, &mut total).await?;
    for _ in 0..MAX_PAGES {
        let id = workspace.to_owned();
        let prepared = blocking(store, move |store| store.sync_prepare_push(&id)).await?;
        let Some(request) = prepared.request else {
            break;
        };
        upload(store, remote, workspace, &request.blob_hashes()).await?;
        let response = remote.push(workspace, &request).await?;
        let id = workspace.to_owned();
        let report = blocking(store, move |store| store.sync_apply_push(&id, &response)).await?;
        let progressed = report.accepted > 0;
        merge(&mut total, report);
        // Rejected changes are resolved by the next pull, not by resending.
        if prepared.remaining == 0 || !progressed {
            break;
        }
    }
    pull(store, remote, workspace, &mut total).await?;
    Ok(total)
}

/// Uploads the originals of a push request that the service lacks.
async fn upload(
    store: &Arc<Store>,
    remote: &Remote,
    workspace: &str,
    hashes: &[String],
) -> Result<()> {
    for hash in remote.missing_blobs(workspace, hashes).await? {
        let (id, name) = (workspace.to_owned(), hash.clone());
        let bytes = blocking(store, move |store| {
            let mut bytes = Vec::new();
            store.sync_open_blob(&id, &name)?.read_to_end(&mut bytes)?;
            Ok(bytes)
        })
        .await?;
        remote.upload_blob(workspace, &hash, bytes).await?;
    }
    Ok(())
}

/// Downloads originals that incoming pages need. A page whose original is
/// unavailable waits in the pending list instead of failing the whole run.
async fn download(
    store: &Arc<Store>,
    remote: &Remote,
    workspace: &str,
    hashes: Vec<String>,
) -> Result<()> {
    let id = workspace.to_owned();
    let missing = blocking(store, move |store| store.sync_missing_blobs(&id, &hashes)).await?;
    for hash in missing {
        let bytes = match remote.download_blob(workspace, &hash).await {
            Ok(bytes) => bytes,
            Err(error) if error.code == "SYNC_BLOB_NOT_FOUND" => continue,
            Err(error) => return Err(error),
        };
        let id = workspace.to_owned();
        blocking(store, move |store| {
            store.sync_store_blob(&id, &hash, bytes.as_slice())
        })
        .await?;
    }
    Ok(())
}

async fn device_remote(app: &AppHandle) -> Result<(SyncDevice, Remote)> {
    let device = read_device(&root(app).await?)?
        .ok_or_else(|| Error::new("SYNC_DEVICE", "请先在「同步」中登记本机设备。"))?;
    let token = credentials::load(&device.device_id)?.ok_or_else(|| {
        Error::new(
            "SYNC_DEVICE",
            "系统凭据中找不到本机的同步令牌。请重新登记本机设备。",
        )
    })?;
    let remote = Remote::new(server_url(&device.server)?, Some(token))?;
    Ok((device, remote))
}

/// Runs one exchange and records the outcome on the workspace.
async fn sync_once(app: &AppHandle, workspace: &str) -> Result<SyncReport> {
    let state = app.state::<SyncState>();
    let _running = state.runs.lock().await;
    let result = async {
        let (device, remote) = device_remote(app).await?;
        let id = workspace.to_owned();
        let status = run(app.clone(), move |store| store.sync_status(&id)).await?;
        if !status.enabled {
            return Err(Error::new("SYNC_DISABLED", "此工作空间尚未启用同步。"));
        }
        if status.server_url.as_deref() != Some(device.server.as_str()) {
            return Err(Error::new(
                "SYNC_SERVER_CHANGED",
                "本机已登记到另一个同步服务。请重新启用此工作空间的同步。",
            ));
        }
        let store = run(app.clone(), |store| Ok(Arc::clone(store))).await?;
        exchange(&store, &remote, workspace).await
    }
    .await;
    let message = result.as_ref().err().map(|error| error.message.clone());
    let id = workspace.to_owned();
    run(app.clone(), move |store| {
        store.record_sync_attempt(&id, message.as_deref())
    })
    .await?;
    if result.as_ref().is_ok_and(|report| report.applied > 0) {
        let _ = app.emit("sync-applied", workspace);
    }
    let _ = app.emit("sync-finished", workspace);
    result
}

/// Periodic sync for every enabled workspace while the app runs.
pub(crate) fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut delay = FIRST_RUN;
        loop {
            tokio::time::sleep(delay).await;
            delay = INTERVAL;
            let enabled = run(app.clone(), |store| {
                Ok(store
                    .workspaces()?
                    .into_iter()
                    .filter(|workspace| {
                        store
                            .sync_status(&workspace.id)
                            .is_ok_and(|status| status.enabled)
                    })
                    .map(|workspace| workspace.id)
                    .collect::<Vec<_>>())
            })
            .await;
            for workspace in enabled.unwrap_or_default() {
                if let Err(error) = sync_once(&app, &workspace).await {
                    eprintln!("工作空间同步失败。{error}");
                }
            }
        }
    });
}

async fn overview(app: &AppHandle, workspace: String) -> Result<SyncOverview> {
    let device = read_device(&root(app).await?)?;
    let status = run(app.clone(), move |store| store.sync_status(&workspace)).await?;
    Ok(SyncOverview { device, status })
}

#[tauri::command]
pub async fn sync_overview(app: AppHandle, workspace: String) -> Result<SyncOverview> {
    overview(&app, workspace).await
}

/// Registers this computer with the service using the administrator token.
/// The administrator token is used once and never stored.
#[tauri::command]
pub async fn register_sync_device(
    app: AppHandle,
    server: String,
    admin_token: String,
    name: String,
) -> Result<SyncDevice> {
    let name = device_name(&name)?;
    let url = server_url(&server)?;
    let remote = Remote::new(url.clone(), None)?;
    remote.health().await?;
    let issued = remote.register(admin_token.trim(), &name).await?;
    let root = root(&app).await?;
    let previous = read_device(&root)?;
    credentials::save(&issued.device.id, &issued.token)?;
    let device = SyncDevice {
        server: display(&url),
        device_id: issued.device.id,
        device_name: issued.device.name,
        registered_at: now(),
    };
    if let Err(error) = write_device(&root, &device) {
        let _ = credentials::delete(&device.device_id);
        return Err(error);
    }
    if let Some(previous) = previous {
        let _ = credentials::delete(&previous.device_id);
    }
    Ok(device)
}

/// Removes this computer's token and settings. The service keeps its data;
/// revoke the device there to block the old token for good.
#[tauri::command]
pub async fn forget_sync_device(app: AppHandle) -> Result<()> {
    let state = app.state::<SyncState>();
    let _running = state.runs.lock().await;
    let root = root(&app).await?;
    if let Some(device) = read_device(&root)? {
        credentials::delete(&device.device_id)?;
    }
    match std::fs::remove_file(root.join(DEVICE_FILE)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[tauri::command]
pub async fn enable_workspace_sync(app: AppHandle, workspace: String) -> Result<SyncReport> {
    let (device, remote) = device_remote(&app).await?;
    let id = workspace.clone();
    let info = run(app.clone(), move |store| store.workspace(&id)).await?;
    remote.put_workspace(&info.id, &info.name).await?;
    let id = workspace.clone();
    run(app.clone(), move |store| {
        store.enable_sync(&id, &device.server).map(drop)
    })
    .await?;
    sync_once(&app, &workspace).await
}

#[tauri::command]
pub async fn disable_workspace_sync(app: AppHandle, workspace: String) -> Result<SyncOverview> {
    let state = app.state::<SyncState>();
    let running = state.runs.lock().await;
    let id = workspace.clone();
    run(app.clone(), move |store| store.disable_sync(&id)).await?;
    drop(running);
    overview(&app, workspace).await
}

#[tauri::command]
pub async fn sync_workspace_now(app: AppHandle, workspace: String) -> Result<SyncReport> {
    sync_once(&app, &workspace).await
}

#[tauri::command]
pub async fn list_remote_workspaces(app: AppHandle) -> Result<Vec<RemoteWorkspace>> {
    let (_, remote) = device_remote(&app).await?;
    let remote = remote.workspaces().await?;
    let local: std::collections::HashSet<String> = run(app, |store| {
        Ok(store
            .workspaces()?
            .into_iter()
            .map(|workspace| workspace.id)
            .collect())
    })
    .await?;
    Ok(remote
        .into_iter()
        .map(|workspace| RemoteWorkspace {
            local: local.contains(&workspace.id),
            id: workspace.id,
            name: workspace.name,
        })
        .collect())
}

/// Creates a local copy of a workspace from the service and syncs it once.
#[tauri::command]
pub async fn join_remote_workspace(app: AppHandle, id: String, name: String) -> Result<Workspace> {
    let (device, _) = device_remote(&app).await?;
    let (target, server) = (id.clone(), device.server.clone());
    let workspace = run(app.clone(), move |store| {
        let workspace = store.create_synced_workspace(&target, &name)?;
        store.enable_sync(&workspace.id, &server)?;
        Ok(workspace)
    })
    .await?;
    // The copy exists even if the first run fails; the timer retries it.
    if let Err(error) = sync_once(&app, &id).await {
        eprintln!("首次同步失败，稍后会自动重试。{error}");
    }
    Ok(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zhiye_core::{PageDraft, Store};
    use zhiye_sync_server::{Engine, Service, serve};

    const ADMIN: &str = "desktop-test-admin-token-0123456789ab";

    async fn start_server() -> (url::Url, tempfile::TempDir) {
        let directory = tempfile::tempdir().unwrap();
        let service =
            Arc::new(Service::new(Engine::open(directory.path()).unwrap(), ADMIN).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(serve(listener, service));
        (server_url(&format!("http://{address}")).unwrap(), directory)
    }

    async fn device(base: &url::Url, name: &str) -> Remote {
        let issued = Remote::new(base.clone(), None)
            .unwrap()
            .register(ADMIN, name)
            .await
            .unwrap();
        Remote::new(base.clone(), Some(issued.token)).unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn exchanges_through_the_real_service_over_http() {
        let (base, _server) = start_server().await;
        let health = Remote::new(base.clone(), None)
            .unwrap()
            .health()
            .await
            .unwrap();
        assert_eq!(health.protocol, 1);
        let bad = Remote::new(base.clone(), None)
            .unwrap()
            .register("wrong-admin-token", "x")
            .await
            .unwrap_err();
        assert_eq!(bad.code, "SYNC_UNAUTHORIZED");

        let first = device(&base, "书房电脑").await;
        let second = device(&base, "笔记本").await;
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let first_store = Arc::new(Store::open(one.path()).unwrap());
        let second_store = Arc::new(Store::open(two.path()).unwrap());

        let workspace = first_store.create_workspace("个人资料").unwrap();
        first
            .put_workspace(&workspace.id, &workspace.name)
            .await
            .unwrap();
        first_store
            .enable_sync(&workspace.id, &display(&base))
            .unwrap();
        let page = first_store
            .create_page(&workspace.id, None, "会议纪要", "📄")
            .unwrap();
        let original = first_store
            .add_attachment(
                &workspace.id,
                &page.info.id,
                "纪要原件.bin",
                [9_u8; 2048].as_slice(),
            )
            .unwrap()
            .attachment;
        for index in 0..250 {
            first_store
                .create_page(
                    &workspace.id,
                    Some(&page.info.id),
                    &format!("第 {index} 条"),
                    "📄",
                )
                .unwrap();
        }
        let sent = exchange(&first_store, &first, &workspace.id).await.unwrap();
        assert_eq!(sent.accepted, 251, "two push batches of at most 200");

        let listed = second.workspaces().await.unwrap();
        assert_eq!(listed[0].id, workspace.id);
        second_store
            .create_synced_workspace(&listed[0].id, &listed[0].name)
            .unwrap();
        second_store
            .enable_sync(&workspace.id, &display(&base))
            .unwrap();
        let received = exchange(&second_store, &second, &workspace.id)
            .await
            .unwrap();
        assert_eq!(received.applied, 251, "two pull pages of at most 200");
        assert_eq!(second_store.pages(&workspace.id).unwrap().len(), 251);
        let mut copied = Vec::new();
        second_store
            .copy_attachment(&workspace.id, &page.info.id, &original.id, &mut copied)
            .unwrap();
        assert_eq!(copied, [9_u8; 2048], "the original arrives over HTTP");

        // Offline edits on both sides produce one conflict copy.
        for (store, text) in [(&first_store, "书房"), (&second_store, "笔记本")] {
            store
                .save_page(
                    &workspace.id,
                    &page.info.id,
                    1,
                    PageDraft {
                        title: format!("会议纪要 · {text}"),
                        blocks: vec![],
                    },
                )
                .unwrap();
        }
        exchange(&first_store, &first, &workspace.id).await.unwrap();
        let merged = exchange(&second_store, &second, &workspace.id)
            .await
            .unwrap();
        assert_eq!(merged.conflicts.len(), 1);
        assert_eq!(merged.conflicts[0].kind, "EDIT_EDIT");
        exchange(&first_store, &first, &workspace.id).await.unwrap();
        assert_eq!(first_store.pages(&workspace.id).unwrap().len(), 252);
        assert!(
            first_store
                .pages(&workspace.id)
                .unwrap()
                .iter()
                .any(|page| page.title == "会议纪要 · 笔记本（冲突副本）")
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn reports_unreachable_and_non_sync_services() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = Remote::new(server_url(&format!("http://{address}")).unwrap(), None)
            .unwrap()
            .health()
            .await
            .unwrap_err();
        assert_eq!(error.code, "SYNC_NETWORK");
    }

    #[test]
    fn device_settings_round_trip_without_the_token() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(read_device(root.path()).unwrap(), None);
        let device = SyncDevice {
            server: "https://sync.example.com".into(),
            device_id: uuid::Uuid::new_v4().to_string(),
            device_name: "书房电脑".into(),
            registered_at: 1,
        };
        write_device(root.path(), &device).unwrap();
        assert_eq!(read_device(root.path()).unwrap(), Some(device));
        let text = std::fs::read_to_string(root.path().join(DEVICE_FILE)).unwrap();
        assert!(!text.contains("zsd_"));
        assert!(device_name(" ").is_err());
        assert_eq!(device_name(" 笔记本 ").unwrap(), "笔记本");
    }
}
