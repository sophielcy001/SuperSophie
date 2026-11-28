// SPDX-License-Identifier: AGPL-3.0-or-later

use std::fs::File;
use zhiye_core::Store;
use zhiye_core::sync_protocol::{PushResponse, PushResult};

/// Marks every prepared operation as accepted at version 1.
fn accept_all(store: &Store, workspace: &str) -> usize {
    let request = store.sync_prepare_push(workspace).unwrap().request.unwrap();
    let response = PushResponse {
        results: request
            .operations
            .iter()
            .enumerate()
            .map(|(index, operation)| PushResult::Accepted {
                op_id: operation.op_id.clone(),
                object_id: operation.object_id.clone(),
                version: 1,
                seq: index as u64 + 1,
            })
            .collect(),
    };
    store
        .sync_apply_push(workspace, &response)
        .unwrap()
        .accepted
}

#[test]
fn settings_follow_the_service_and_survive_disable() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("同步设置").unwrap().id;
    store.create_page(&workspace, None, "页面", "file").unwrap();

    let initial = store.sync_status(&workspace).unwrap();
    assert!(!initial.enabled);
    assert_eq!(initial.server_url, None);
    assert_eq!(initial.unsent, 1);

    let enabled = store
        .enable_sync(&workspace, "https://sync.example.test")
        .unwrap();
    assert!(enabled.enabled);
    assert_eq!(
        enabled.server_url.as_deref(),
        Some("https://sync.example.test")
    );
    assert_eq!(accept_all(&store, &workspace), 1);

    store
        .record_sync_attempt(&workspace, Some("无法连接同步服务"))
        .unwrap();
    let failed = store.sync_status(&workspace).unwrap();
    assert_eq!(failed.last_error.as_deref(), Some("无法连接同步服务"));
    assert!(failed.last_attempt_at.is_some());
    assert_eq!(failed.last_success_at, None);
    store.record_sync_attempt(&workspace, None).unwrap();
    let succeeded = store.sync_status(&workspace).unwrap();
    assert_eq!(succeeded.last_error, None);
    assert_eq!(succeeded.last_success_at, succeeded.last_attempt_at);

    store.disable_sync(&workspace).unwrap();
    let disabled = store.sync_status(&workspace).unwrap();
    assert!(!disabled.enabled);
    assert_eq!(disabled.tracked, 1, "disabling keeps the stream");
    let again = store
        .enable_sync(&workspace, "https://sync.example.test")
        .unwrap();
    assert_eq!((again.tracked, again.unsent), (1, 0));

    let moved = store
        .enable_sync(&workspace, "https://other.example.test")
        .unwrap();
    assert_eq!(
        (moved.tracked, moved.unsent),
        (0, 1),
        "a new service starts over"
    );
    assert_eq!(moved.last_success_at, None);
    assert!(store.enable_sync(&workspace, "").is_err());
}

#[test]
fn recovered_copy_does_not_continue_the_stream() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let workspace = store.create_workspace("原空间").unwrap().id;
    store.create_page(&workspace, None, "页面", "file").unwrap();
    store
        .enable_sync(&workspace, "https://sync.example.test")
        .unwrap();
    accept_all(&store, &workspace);

    let outside = tempfile::tempdir().unwrap();
    let package = outside.path().join("copy.zhiye-recovery.zip");
    store.save_recovery_package(&workspace, &package).unwrap();
    let restored = store
        .read_workspace_package(File::open(&package).unwrap())
        .unwrap();
    assert_ne!(restored.id, workspace);
    let status = store.sync_status(&restored.id).unwrap();
    assert!(!status.enabled);
    assert_eq!(status.server_url, None);
    assert_eq!((status.tracked, status.cursor, status.unsent), (0, 0, 1));
    // The original keeps its own state.
    assert!(store.sync_status(&workspace).unwrap().enabled);
}
