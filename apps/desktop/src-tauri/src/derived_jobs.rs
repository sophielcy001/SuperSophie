// SPDX-License-Identifier: AGPL-3.0-or-later

use std::{sync::Arc, time::Duration};
use zhiye_core::{Result, Store};

pub(crate) fn start(store: &Arc<Store>) {
    let store = Arc::downgrade(store);
    tauri::async_runtime::spawn(async move {
        let mut delay = Duration::from_millis(900);
        loop {
            tokio::time::sleep(delay).await;
            let Some(open) = store.upgrade() else {
                return;
            };
            let progressed =
                match tauri::async_runtime::spawn_blocking(move || advance(&open)).await {
                    Ok(Ok(progressed)) => progressed,
                    Ok(Err(error)) => {
                        eprintln!("无法检查派生变更。{error}");
                        false
                    }
                    Err(error) => {
                        eprintln!("派生变更任务已中断。{error}");
                        false
                    }
                };
            delay = Duration::from_millis(if progressed { 60 } else { 900 });
        }
    });
}

fn advance(store: &Store) -> Result<bool> {
    let mut progressed = false;
    for workspace in store.ready_workspace_ids()? {
        match store.advance_derived_changes(&workspace) {
            Ok(handled) => progressed |= handled,
            Err(error) => eprintln!("派生变更将重试。{error}"),
        }
    }
    Ok(progressed)
}
