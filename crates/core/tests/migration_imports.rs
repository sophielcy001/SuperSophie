// SPDX-License-Identifier: AGPL-3.0-or-later

mod support;

use rusqlite::{Connection, params};
use std::{
    fs,
    io::{self, Cursor, Read, Seek, SeekFrom, Write},
    sync::atomic::{AtomicBool, Ordering},
};
use zhiye_core::{ImportFile, ImportOriginal, ImportOutcome, ImportStatus, Store};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const GUIDE: &str = "手册 11111111111111111111111111111111";
const NOTE: &str = "子页 22222222222222222222222222222222";
const DATABASE: &str = "事项 33333333333333333333333333333333";
const RECORD: &str = "记录甲 44444444444444444444444444444444";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfixture";

fn zip(entries: &[(&str, &[u8])]) -> Cursor<Vec<u8>> {
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        writer.start_file(*name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap()
}

fn outcome<'a>(report: &'a [ImportOutcome], name: &str) -> &'a ImportOutcome {
    report.iter().find(|item| item.name == name).unwrap()
}

fn original(store: &Store, space: &str, reference: &ImportOriginal) -> Vec<u8> {
    let mut bytes = Vec::new();
    let copied = store
        .copy_attachment(
            space,
            &reference.page_id,
            &reference.attachment_id,
            &mut bytes,
        )
        .unwrap();
    assert_eq!(copied.name, reference.name);
    bytes
}

#[test]
fn notion_import_unpacks_nested_parts_and_merges_unique_database_record_bodies() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let space = store.create_workspace("迁移测试").unwrap().id;
    let target = store
        .create_page(&space, None, "目标", "folder")
        .unwrap()
        .info
        .id;
    let guide_source = format!(
        "[转到子页]({}.md#标题)\n\n![截图]({}/assets/图.png)\n\n[表格]({}/{}.csv)",
        format!("{GUIDE}/{NOTE}").replace(' ', "%20"),
        GUIDE.replace(' ', "%20"),
        GUIDE.replace(' ', "%20"),
        DATABASE.replace(' ', "%20"),
    );
    let markdown = format!("{GUIDE}/{NOTE}.md");
    let csv = format!("{GUIDE}/{DATABASE}_all.csv");
    let filtered = format!("{GUIDE}/{DATABASE}.csv");
    let record = format!("{GUIDE}/{DATABASE}/{RECORD}.md");
    let raw = format!("{GUIDE}/legacy.canvas");
    let part1 = zip(&[
        (&format!("{GUIDE}.md"), guide_source.as_bytes()),
        (&markdown, b"# Title"),
        (&format!("{GUIDE}/assets/图.png"), PNG),
        (&raw, b"{\"format\":\"opaque\"}"),
        ("../unsafe-example.txt", b"ignored by path validation"),
    ])
    .into_inner();
    let row_data = "名称,状态\n记录甲,完成\n记录乙,排期\n";
    let part2 = zip(&[
        (&csv, row_data.as_bytes()),
        (&filtered, "名称,状态\n记录甲,完成\n".as_bytes()),
        (&record, "# 记录正文".as_bytes()),
    ])
    .into_inner();
    let archive = zip(&[
        ("Export-fictional-Part-1.zip", &part1),
        ("Export-fictional-Part-2.zip", &part2),
        ("index.html", b"<html>Catalog</html>"),
    ]);
    let cancel = AtomicBool::new(false);
    let import = |space: &str| {
        store
            .import_notion_zip(
                space,
                Some(&target),
                "Notion 示例.zip",
                archive.clone(),
                &cancel,
            )
            .unwrap()
    };
    let report = import(&space);
    let root = report[0].page_id.as_deref().unwrap();
    assert_eq!(
        store.latest_migration(&space).unwrap().unwrap().status,
        "completed"
    );
    assert_eq!(
        store.page(&space, root).unwrap().info.parent_id.as_deref(),
        Some(target.as_str())
    );
    assert_eq!(
        original(&store, &space, report[0].original.as_ref().unwrap()),
        archive.get_ref().as_slice()
    );
    assert_eq!(
        outcome(&report, "../unsafe-example.txt").status,
        ImportStatus::Unsupported
    );
    assert_eq!(outcome(&report, &filtered).status, ImportStatus::Preserved);
    assert_eq!(
        outcome(&report, &filtered).page_id,
        outcome(&report, &csv).page_id
    );
    assert_eq!(outcome(&report, &raw).status, ImportStatus::Preserved);
    assert_eq!(
        original(
            &store,
            &space,
            outcome(&report, &raw).original.as_ref().unwrap()
        ),
        b"{\"format\":\"opaque\"}"
    );
    assert!(report[0].notes.iter().any(|note| note.contains("2 个")));

    let guide = outcome(&report, &format!("{GUIDE}.md"));
    let guide_page = store
        .page(&space, guide.page_id.as_deref().unwrap())
        .unwrap();
    let note = outcome(&report, &markdown).page_id.as_deref().unwrap();
    let table = outcome(&report, &csv);
    assert_eq!(table.records, 2);
    let table_id = table.page_id.as_deref().unwrap();
    assert_eq!(guide_page.info.title, "手册");
    assert_eq!(guide_page.blocks[0].payload["inline"][0]["pageId"], note);
    assert_eq!(guide_page.blocks[1].kind, "attachmentImage");
    assert_eq!(
        guide_page.blocks[2].payload["inline"][0]["pageId"],
        table_id
    );
    assert!(guide.notes.iter().any(|note| note.contains("锚点")));
    let items = store.attachments(&space, &guide_page.info.id).unwrap();
    let asset = items.iter().find(|asset| asset.name == "图.png").unwrap();
    assert_eq!(guide_page.blocks[1].payload["attachmentId"], asset.id);
    let matched = store
        .page(
            &space,
            outcome(&report, &record).page_id.as_deref().unwrap(),
        )
        .unwrap();
    assert_eq!(matched.info.parent_id.as_deref(), Some(table_id));
    assert_eq!(matched.blocks[0].payload["inline"][0]["text"], "记录正文");
    let props = matched.properties.unwrap();
    assert_eq!(props.values().next().unwrap(), "完成");
    assert_eq!(
        store
            .pages(&space)
            .unwrap()
            .iter()
            .filter(|page| page.parent_id.as_deref() == Some(table_id))
            .count(),
        2
    );
    assert_eq!(
        original(&store, &space, table.original.as_ref().unwrap()),
        row_data.as_bytes()
    );

    let restored = {
        let mut zip = Cursor::new(Vec::new());
        store.write_workspace_archive(&space, &mut zip).unwrap();
        store.read_workspace_archive(zip).unwrap()
    };
    assert_eq!(import(&restored.id)[0].status, ImportStatus::Duplicate);
    assert_eq!(import(&space)[0].status, ImportStatus::Duplicate);
    assert_eq!(
        store.pages(&space).unwrap().len(),
        store.pages(&restored.id).unwrap().len()
    );
    assert_eq!(
        original(&store, &restored.id, guide.original.as_ref().unwrap()),
        guide_source.as_bytes()
    );
}

#[test]
fn obsidian_import_reports_ambiguous_links_and_keeps_downgraded_originals() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("workspaces")).unwrap();
    let space = store.create_workspace("笔记").unwrap().id;
    let vault = directory.path().join("资料库");
    for path in ["左", "右", "assets", ".obsidian"] {
        fs::create_dir_all(vault.join(path)).unwrap();
    }
    let source = "---\ntags: [测试]\n---\n\n[[左/重名|左笔记]]、[[重名]]、[[缺页]]。\n\n![[assets/图.png|200]]\n\n![[左/重名]]\n\n> [!NOTE]\n> 说明";
    fs::write(vault.join("入口.md"), source).unwrap();
    fs::write(vault.join("左/重名.md"), "[说明](../assets/文件.pdf)").unwrap();
    fs::write(vault.join("右/重名.md"), "# 另一个页面").unwrap();
    fs::write(vault.join("assets/图.png"), PNG).unwrap();
    fs::write(vault.join("assets/文件.pdf"), b"%PDF-1.7\nfixture").unwrap();
    fs::write(vault.join(".obsidian/config.json"), "{}").unwrap();
    fs::write(vault.join("已损坏.md"), [0xff]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(vault.join("入口.md"), vault.join("别名.md")).unwrap();
    let cancel = AtomicBool::new(false);
    let report = store
        .import_obsidian_vault(&space, None, &vault, &cancel)
        .unwrap();
    let entry = outcome(&report, "入口.md");
    let left = outcome(&report, "左/重名.md");
    let page = store
        .page(&space, entry.page_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(entry.unmatched_links, 2, "{entry:?}");
    assert_eq!(page.blocks[0].kind, "codeBlock");
    assert_eq!(
        page.blocks[1].payload["inline"][0]["pageId"],
        left.page_id.as_deref().unwrap()
    );
    assert_eq!(page.blocks[2].kind, "attachmentImage");
    assert_eq!(page.blocks[2].payload["alt"], "图.png");
    assert_eq!(
        page.blocks[3].payload["inline"][0]["pageId"],
        left.page_id.as_deref().unwrap()
    );
    assert_eq!(page.blocks[4].kind, "callout");
    assert!(entry.notes.iter().any(|note| note.contains("尺寸")));
    assert!(entry.notes.iter().any(|note| note.contains("笔记嵌入")));
    let linked = store
        .page(&space, left.page_id.as_deref().unwrap())
        .unwrap();
    assert_eq!(linked.blocks[0].kind, "attachmentFile");
    assert_eq!(
        original(&store, &space, entry.original.as_ref().unwrap()),
        source.as_bytes()
    );
    assert_eq!(
        outcome(&report, ".obsidian").status,
        ImportStatus::Unsupported
    );
    #[cfg(unix)]
    assert_eq!(
        outcome(&report, "别名.md").status,
        ImportStatus::Unsupported
    );
    let invalid = outcome(&report, "已损坏.md");
    assert_eq!(invalid.status, ImportStatus::Preserved);
    assert_eq!(
        original(&store, &space, invalid.original.as_ref().unwrap()),
        [0xff]
    );
    assert_eq!(
        store
            .import_obsidian_vault(&space, None, &vault, &cancel)
            .unwrap()[0]
            .status,
        ImportStatus::Duplicate
    );
    fs::write(vault.join("入口.md"), "# 有修改").unwrap();
    assert_eq!(
        store
            .import_obsidian_vault(&space, None, &vault, &cancel)
            .unwrap()[0]
            .status,
        ImportStatus::Imported
    );
    assert_eq!(store.page(&space, &page.info.id).unwrap(), page);
}

struct CancelAfterRead<'a> {
    archive: Cursor<Vec<u8>>,
    cancel: &'a AtomicBool,
}

impl Read for CancelAfterRead<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let count = self.archive.read(bytes)?;
        self.cancel.store(true, Ordering::Relaxed);
        Ok(count)
    }
}

impl Seek for CancelAfterRead<'_> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.archive.seek(position)
    }
}

#[test]
fn migration_failure_and_cancellation_leave_no_published_pages_and_can_retry() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let space = store.create_workspace("回滚").unwrap().id;
    let db = Connection::open(directory.path().join(&space).join("workspace.sqlite")).unwrap();
    let archive = zip(&[("有效.md", b"Test body")]);
    let cancel = AtomicBool::new(false);
    let stopped = store
        .import_notion_zip(
            &space,
            None,
            "样例.zip",
            CancelAfterRead {
                archive: archive.clone(),
                cancel: &cancel,
            },
            &cancel,
        )
        .unwrap_err();
    assert_eq!(stopped.code, "IMPORT_CANCELLED");
    assert_eq!(
        store.latest_migration(&space).unwrap().unwrap().status,
        "cancelled"
    );
    assert!(store.pages(&space).unwrap().is_empty());
    cancel.store(false, Ordering::Relaxed);
    let import = || store.import_notion_zip(&space, None, "样例.zip", archive.clone(), &cancel);
    db.execute_batch(
        "CREATE TRIGGER simulate_write_error BEFORE INSERT ON import_receipts
         BEGIN SELECT RAISE(ABORT, 'simulated write error'); END;",
    )
    .unwrap();
    assert_eq!(import().unwrap_err().code, "DATABASE");
    assert_eq!(
        store.latest_migration(&space).unwrap().unwrap().status,
        "failed"
    );
    for table in ["pages", "attachments", "import_receipts", "page_links"] {
        assert_eq!(
            db.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row
                .get::<_, u32>(0))
                .unwrap(),
            0
        );
    }
    db.execute_batch("DROP TRIGGER simulate_write_error")
        .unwrap();
    assert_eq!(import().unwrap()[0].status, ImportStatus::Imported);
    assert_eq!(store.pages(&space).unwrap().len(), 2);
    drop(store);
    db.execute(
        "INSERT INTO migration_batches(id,session_id,kind,source,status) VALUES (?1,?2,'notionZip','中断.zip','preparing')",
        params![uuid::Uuid::new_v4().to_string(), uuid::Uuid::new_v4().to_string()],
    ).unwrap();
    let store = Store::open(directory.path()).unwrap();
    let interrupted = store.latest_migration(&space).unwrap().unwrap();
    assert_eq!(interrupted.status, "interrupted");
    assert!(interrupted.page_id.is_none());
}

#[test]
fn sqlite_14_upgrade_keeps_an_existing_markdown_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let space = store.create_workspace("兼容").unwrap().id;
    let file = || ImportFile {
        name: "旧版.md".into(),
        bytes: b"v14".to_vec(),
    };
    let previous = store.import_markdown(&space, None, vec![file()]).unwrap();
    let db = Connection::open(directory.path().join(&space).join("workspace.sqlite")).unwrap();
    support::remove_v15_schema(&db);
    let again = store.import_markdown(&space, None, vec![file()]).unwrap();
    assert_eq!(again[0].status, ImportStatus::Duplicate);
    assert_eq!(again[0].page_id, previous[0].page_id);
    let snapshot = directory
        .path()
        .join("upgrade-backups")
        .join(format!("{space}-v14.sqlite"));
    let before = Connection::open(snapshot).unwrap();
    assert_eq!(
        before
            .pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        14
    );
    assert_eq!(
        db.pragma_query_value(None, "user_version", |row| row.get::<_, u32>(0))
            .unwrap(),
        22
    );
}
