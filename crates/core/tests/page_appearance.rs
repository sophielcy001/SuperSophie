// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::Value;
use std::fs::File;
use std::io::{Cursor, Read};
use zhiye_core::{PageDraft, Store};

mod support;

const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 156, 99, 56, 150, 102, 246, 31, 0, 5,
    186, 2, 98, 77, 127, 216, 92, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

#[test]
fn page_appearance_survives_body_save_restart_and_history_restore() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("页面外观").unwrap().id;
    let page = store.create_page(&space, None, "读书", "file").unwrap();
    let id = &page.info.id;
    let asset = store
        .add_image(&space, id, "封面.png", PNG)
        .unwrap()
        .attachment;

    let styled = store
        .set_page_appearance(&space, id, page.info.revision, "book", Some(&asset.id))
        .unwrap();
    assert_eq!(styled.info.id, *id);
    assert_eq!(styled.info.revision, 2);
    assert_eq!(styled.info.icon, "book");
    assert_eq!(styled.cover.as_ref().unwrap().media_type, "image/png");
    assert_eq!(styled.cover.as_ref().unwrap().attachment_id, asset.id);
    assert_eq!(
        store
            .set_page_appearance(&space, id, 2, "book", Some(&asset.id))
            .unwrap(),
        styled
    );
    assert_eq!(
        store
            .set_page_appearance(&space, id, 1, "folder", None)
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    store
        .save_page(
            &space,
            id,
            2,
            PageDraft {
                title: "新笔记".into(),
                blocks: vec![],
            },
        )
        .unwrap();
    drop(store);

    let store = Store::open(root.path()).unwrap();
    let saved = store.page(&space, id).unwrap();
    assert_eq!(saved.info.title, "新笔记");
    assert_eq!(saved.cover, styled.cover);
    let cleared = store
        .set_page_appearance(&space, id, 3, "file", None)
        .unwrap();
    assert_eq!(cleared.cover, None);
    assert_eq!(store.attachments(&space, id).unwrap(), vec![asset]);
    let restored = store.restore_version(&space, id, 4, 2).unwrap();
    assert_eq!(restored.info.revision, 5);
    assert_eq!(restored.info.icon, styled.info.icon);
    assert_eq!(restored.cover, styled.cover);
    assert_eq!(store.history(&space, id).unwrap().len(), 5);
}

#[test]
fn rejects_invalid_cover_without_writing_a_revision() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("封面校验").unwrap().id;
    let page = store.create_page(&space, None, "原页", "file").unwrap();
    let sibling = store.create_page(&space, None, "其他页", "file").unwrap();
    let foreign = store
        .add_image(&space, &sibling.info.id, "图.png", PNG)
        .unwrap()
        .attachment;
    let text = store
        .add_attachment(&space, &page.info.id, "说明.txt", "说明".as_bytes())
        .unwrap()
        .attachment;
    for asset in [foreign, text] {
        let error = store
            .set_page_appearance(&space, &page.info.id, 1, "book", Some(&asset.id))
            .unwrap_err();
        assert_eq!(error.code, "PAGE_COVER");
    }
    assert_eq!(
        store
            .set_page_appearance(&space, &page.info.id, 1, "\n", None)
            .unwrap_err()
            .code,
        "INVALID_ICON"
    );
    assert_eq!(store.history(&space, &page.info.id).unwrap(), vec![page]);
}

#[test]
fn covers_round_trip_through_zip_recovery_and_templates() {
    let root = tempfile::tempdir().unwrap();
    let transfer = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("封面归档").unwrap().id;
    let page = store.create_page(&space, None, "模板页", "file").unwrap();
    let id = &page.info.id;
    let mut old = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut old).unwrap();
    assert_eq!(archive_version(old.get_ref()), 5);
    old.set_position(0);
    let legacy = store.read_workspace_archive(old).unwrap();
    assert_eq!(store.page(&legacy.id, id).unwrap(), page);

    let asset = store
        .add_image(&space, id, "图.png", PNG)
        .unwrap()
        .attachment;
    let styled = store
        .set_page_appearance(&space, id, 1, "book", Some(&asset.id))
        .unwrap();
    let template = store
        .save_template(&space, id, "有封面的模板", None)
        .unwrap();
    let cleared = store
        .set_page_appearance(&space, id, 2, "file", None)
        .unwrap();
    let recovery_file = transfer.path().join("cover.zhiye-recovery.zip");
    store.save_recovery_package(&space, &recovery_file).unwrap();
    let mut archive = Cursor::new(Vec::new());
    store.write_workspace_archive(&space, &mut archive).unwrap();
    assert_eq!(archive_version(archive.get_ref()), 6);
    archive.set_position(0);

    let copies = [
        store.read_workspace_archive(archive).unwrap(),
        store
            .read_workspace_package(File::open(recovery_file).unwrap())
            .unwrap(),
    ];
    for copy in copies {
        assert_eq!(store.page(&copy.id, id).unwrap(), cleared);
        let history = store.history(&copy.id, id).unwrap();
        assert_eq!(history[1], styled);
        let restored = store.restore_version(&copy.id, id, 3, 2).unwrap();
        assert_eq!(restored.cover, styled.cover);
        let from_template = store
            .apply_template(&copy.id, &template.id, None, None)
            .unwrap();
        let cloned_asset = &from_template.cover.unwrap().attachment_id;
        assert_ne!(cloned_asset, &asset.id);
        assert_eq!(from_template.info.icon, "book");
        let mut bytes = Vec::new();
        store
            .copy_attachment(&copy.id, &from_template.info.id, cloned_asset, &mut bytes)
            .unwrap();
        assert_eq!(bytes, PNG);
    }
}

#[test]
fn upgrades_existing_page_and_preserves_sqlite_16_recovery() {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    let space = store.create_workspace("旧空间").unwrap().id;
    let old = store
        .create_page(&space, None, "已有页面", "folder")
        .unwrap();
    let file = root.path().join(&space).join("workspace.sqlite");
    drop(store);
    support::remove_v17_schema(&rusqlite::Connection::open(file).unwrap());

    let store = Store::open(root.path()).unwrap();
    assert_eq!(store.page(&space, &old.info.id).unwrap(), old);
    let newer = store
        .set_page_appearance(&space, &old.info.id, 1, "book", None)
        .unwrap();
    assert_eq!(newer.info.revision, 2);
    let backup = root
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v16.zhiye-recovery.zip"));
    let earlier = store
        .read_workspace_package(File::open(backup).unwrap())
        .unwrap();
    assert_eq!(store.page(&earlier.id, &old.info.id).unwrap(), old);
    assert_eq!(store.page(&space, &old.info.id).unwrap(), newer);
}

fn archive_version(bytes: &[u8]) -> u64 {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut manifest = String::new();
    archive
        .by_name("manifest.json")
        .unwrap()
        .read_to_string(&mut manifest)
        .unwrap();
    let manifest: Value = serde_json::from_str(&manifest).unwrap();
    manifest["formatVersion"].as_u64().unwrap()
}
