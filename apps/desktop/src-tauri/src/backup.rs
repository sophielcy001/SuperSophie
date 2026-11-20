// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{files::chosen_path, run, run_independent};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::{AppHandle, Emitter, WebviewWindow};
use tauri_plugin_dialog::DialogExt;
use zhiye_core::{BackupAttempt, BackupPolicy, BackupStatus, Result, Store};

pub(crate) fn start(store: &Arc<Store>, app: AppHandle) {
    let store = Arc::downgrade(store);
    tauri::async_runtime::spawn(async move {
        let mut delay = Duration::from_secs(5);
        loop {
            tokio::time::sleep(delay).await;
            let Some(opened) = store.upgrade() else {
                return;
            };
            match tauri::async_runtime::spawn_blocking(move || opened.run_due_backups()).await {
                Ok(Ok(checks)) => {
                    for check in checks {
                        if let Err(error) = check.result {
                            eprintln!("无法记录工作空间的备份状态。{error}");
                        }
                        let _ = app.emit("backup-checked", &check.workspace_id);
                    }
                }
                Ok(Err(error)) => eprintln!("无法读取自动备份设置。{error}"),
                Err(error) => eprintln!("自动备份任务已中断。{error}"),
            }
            delay = Duration::from_secs(60);
        }
    });
}

#[tauri::command]
pub(crate) async fn backup_status(
    app: AppHandle,
    workspace: Option<String>,
) -> Result<BackupStatus> {
    run_independent(app, move |store| store.backup_status(workspace.as_deref())).await
}

#[tauri::command]
pub(crate) async fn set_backup_policy(
    app: AppHandle,
    policy: BackupPolicy,
) -> Result<BackupPolicy> {
    run(app, move |store| store.set_backup_policy(policy)).await
}

#[tauri::command]
pub(crate) async fn choose_backup_directory(
    app: AppHandle,
    window: WebviewWindow,
) -> Result<Option<PathBuf>> {
    chosen_path(
        app.dialog()
            .file()
            .set_parent(&window)
            .set_title("选择自动备份目录")
            .blocking_pick_folder(),
    )
}

#[tauri::command]
pub(crate) async fn run_workspace_backup(
    app: AppHandle,
    workspace: String,
) -> Result<Option<BackupAttempt>> {
    let checked = workspace.clone();
    let attempt = run_independent(app.clone(), move |store| {
        store.run_workspace_backup(&workspace)
    })
    .await;
    let _ = app.emit("backup-checked", checked);
    attempt
}

#[tauri::command]
pub(crate) async fn export_recovery(
    app: AppHandle,
    window: WebviewWindow,
    workspace: String,
) -> Result<bool> {
    let selection = app
        .dialog()
        .file()
        .set_parent(&window)
        .set_title("创建整库恢复包")
        .add_filter("知页整库恢复包", &["zip"])
        .set_file_name("知页-整库恢复.zhiye-recovery.zip")
        .blocking_save_file();
    let Some(path) = chosen_path(selection)? else {
        return Ok(false);
    };
    run_independent(app, move |store| {
        store.save_recovery_package(&workspace, &path)?;
        Ok(true)
    })
    .await
}
