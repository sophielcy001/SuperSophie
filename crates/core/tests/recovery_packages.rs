// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{Cursor, Read},
};
use zhiye_core::{
    IntegrationDraft, IntegrationGrant, IntegrationMutation, IntegrationScope, PageDraft,
    PagePatch, Store,
};

#[test]
fn a_disaster_recovery_copy_preserves_audit_and_disables_the_source_token() {
    let source = tempfile::tempdir().unwrap();
    let store = Store::open(source.path()).unwrap();
    let workspace = store.create_workspace("备份原始库").unwrap();
    let page = store
        .create_page(&workspace.id, None, "初版", "file")
        .unwrap();
    let expires_at = page.info.updated_at + 3_600_000;
    let issued = store
        .issue_integration_with_expiry(
            &workspace.id,
            IntegrationDraft {
                name: "可审计集成".into(),
                scopes: BTreeSet::from([IntegrationScope::Read, IntegrationScope::Update]),
                grants: vec![IntegrationGrant::Page(page.info.id.clone())],
            },
            Some(expires_at),
        )
        .unwrap();
    let current = store
        .integration_page(&workspace.id, &issued.token, &page.info.id)
        .unwrap();
    store
        .mutate_as_integration(
            &workspace.id,
            &issued.token,
            "synthetic-recovery-history",
            Some(&current.etag),
            IntegrationMutation::PatchPage {
                page_id: page.info.id.clone(),
                patch: PagePatch {
                    title: Some("快照时的修订".into()),
                    icon: None,
                },
            },
        )
        .unwrap();
    let original = b"detached immutable recovery fixture";
    let asset = store
        .add_attachment(
            &workspace.id,
            &page.info.id,
            "样本.txt",
            original.as_slice(),
        )
        .unwrap()
        .attachment;

    let output = tempfile::tempdir().unwrap();
    let package = output.path().join("审计备份.zhiye-recovery.zip");
    store
        .save_recovery_package(&workspace.id, &package)
        .unwrap();
    let live =
        rusqlite::Connection::open(source.path().join(&workspace.id).join("workspace.sqlite"))
            .unwrap();
    let digest: Vec<u8> = live
        .query_row("SELECT token_hash FROM integrations", [], |row| row.get(0))
        .unwrap();
    let cursor_key: Vec<u8> = live
        .query_row("SELECT query_cursor_key FROM workspace_state", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut archive = zip::ZipArchive::new(File::open(&package).unwrap()).unwrap();
    let mut snapshot = Vec::new();
    archive
        .by_name("workspace.sqlite")
        .unwrap()
        .read_to_end(&mut snapshot)
        .unwrap();
    for secret in [&digest, &cursor_key] {
        assert_eq!(secret.len(), 32);
        assert!(
            !snapshot
                .windows(secret.len())
                .any(|bytes| bytes == secret.as_slice())
        );
    }
    store
        .save_page(
            &workspace.id,
            &page.info.id,
            2,
            PageDraft {
                title: "备份后的新写入".into(),
                blocks: vec![],
            },
        )
        .unwrap();
    let destination = tempfile::tempdir().unwrap();
    let recovery = Store::open(destination.path()).unwrap();
    let restored = recovery
        .read_workspace_package(File::open(&package).unwrap())
        .unwrap();

    assert_ne!(restored.id, workspace.id);
    let old = recovery.page(&restored.id, &page.info.id).unwrap();
    assert_eq!(
        (old.info.title.as_str(), old.info.revision),
        ("快照时的修订", 2)
    );
    assert_eq!(
        store.page(&workspace.id, &page.info.id).unwrap().info.title,
        "备份后的新写入"
    );
    let mut bytes = Vec::new();
    recovery
        .copy_attachment(&restored.id, &page.info.id, &asset.id, &mut bytes)
        .unwrap();
    assert_eq!(bytes, original);
    let history = recovery
        .revision_attributions(&restored.id, &page.info.id)
        .unwrap();
    assert_eq!(history[&2].integration_id, issued.integration.id);
    let restored_grants = recovery.integrations(&restored.id).unwrap();
    assert_eq!(restored_grants.len(), 1);
    assert!(restored_grants[0].revoked_at.is_some());
    assert_eq!(restored_grants[0].expires_at, Some(expires_at));
    assert_eq!(
        recovery
            .integration_identity(&restored.id, &issued.token)
            .unwrap_err()
            .code,
        "UNAUTHORIZED"
    );
    assert_eq!(
        store
            .integration_identity(&workspace.id, &issued.token)
            .unwrap(),
        issued.integration.id
    );

    let mut damaged = fs::read(&package).unwrap();
    let position = damaged
        .windows(original.len())
        .position(|window| window == original)
        .unwrap();
    damaged[position] ^= 1;
    assert!(
        recovery
            .read_workspace_package(Cursor::new(damaged))
            .is_err()
    );
    assert_eq!(recovery.workspaces().unwrap(), vec![restored]);
}

#[test]
fn pre_upgrade_recovery_and_latest_recovery_both_leave_post_upgrade_edits_intact() {
    let source = tempfile::tempdir().unwrap();
    let store = Store::open(source.path()).unwrap();
    let workspace = store.create_workspace("待升级").unwrap();
    let page = store
        .create_page(&workspace.id, None, "旧版本正文", "file")
        .unwrap();
    let original = b"pre-upgrade fixture";
    let asset = store
        .add_attachment(
            &workspace.id,
            &page.info.id,
            "保留.txt",
            original.as_slice(),
        )
        .unwrap()
        .attachment;
    let db = rusqlite::Connection::open(source.path().join(&workspace.id).join("workspace.sqlite"))
        .unwrap();
    support::remove_v16_schema(&db);
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        15
    );
    let newer = store
        .save_page(
            &workspace.id,
            &page.info.id,
            1,
            PageDraft {
                title: "升级后写入".into(),
                blocks: vec![],
            },
        )
        .unwrap();
    assert_eq!(newer.revision, 2);
    let before = source
        .path()
        .join("upgrade-backups")
        .join(format!("{}-v15.zhiye-recovery.zip", workspace.id));
    let other = tempfile::tempdir().unwrap();
    let newest = other.path().join("latest.zhiye-recovery.zip");
    store.save_recovery_package(&workspace.id, &newest).unwrap();
    let restore_root = tempfile::tempdir().unwrap();
    let recovery = Store::open(restore_root.path()).unwrap();
    let earlier = recovery
        .read_workspace_package(File::open(before).unwrap())
        .unwrap();
    let current = recovery
        .read_workspace_package(File::open(newest).unwrap())
        .unwrap();
    assert_ne!(
        later_page(&recovery, &earlier.id, &page.info.id),
        later_page(&recovery, &current.id, &page.info.id)
    );
    assert_eq!(
        later_page(&recovery, &current.id, &page.info.id),
        "升级后写入"
    );
    assert_eq!(
        later_page(&store, &workspace.id, &page.info.id),
        "升级后写入"
    );
    let mut output = Vec::new();
    recovery
        .copy_attachment(&earlier.id, &page.info.id, &asset.id, &mut output)
        .unwrap();
    assert_eq!(output, original);
}

fn later_page(store: &Store, workspace: &str, page: &str) -> String {
    store.page(workspace, page).unwrap().info.title
}
