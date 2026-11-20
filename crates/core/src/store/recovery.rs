// SPDX-License-Identifier: AGPL-3.0-or-later

use super::*;
use crate::{Block, SearchRequest};
use serde_json::json;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;

const WORKER_CASE: &str = "ZHIYE_RECOVERY_WORKER_CASE";

fn draft(id: &str, title: &str, text: &str) -> PageDraft {
    PageDraft {
        title: title.into(),
        blocks: vec![Block {
            id: id.into(),
            kind: "paragraph".into(),
            schema_version: 1,
            payload: json!({"inline": [{"type": "text", "text": text}]}),
            children: vec![],
            extra: Default::default(),
        }],
    }
}

fn initial(store: &Store) -> (String, Page) {
    let workspace = store.create_workspace("故障演练").unwrap().id;
    let page = store.create_page(&workspace, None, "初稿", "file").unwrap();
    store
        .save_page(
            &workspace,
            &page.info.id,
            page.info.revision,
            draft(&new_id(), "已确认原稿", "山茶原文可以恢复"),
        )
        .unwrap();
    assert_eq!(store.advance_search_index(&workspace).unwrap().pending, 0);
    (
        workspace.clone(),
        store.page(&workspace, &page.info.id).unwrap(),
    )
}

fn search(store: &Store, workspace: &str, text: &str) -> crate::SearchResponse {
    store
        .search_text(
            workspace,
            &SearchRequest {
                query: text.into(),
                ..Default::default()
            },
        )
        .unwrap()
}

#[cfg(unix)]
#[test]
fn workspace_reopens_before_child_exec_completes() {
    use std::os::unix::{net::UnixStream, process::CommandExt};

    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let (workspace, page) = initial(&store);

    let (mut controller, mut inherited) = UnixStream::pair().unwrap();
    let timeout = Some(Duration::from_secs(10));
    controller.set_read_timeout(timeout).unwrap();
    inherited.set_read_timeout(timeout).unwrap();
    let mut command = Command::new(std::env::current_exe().unwrap());
    command.arg("--list").stdout(Stdio::null());
    // SAFETY: The child uses read and write on an inherited socket until exec.
    unsafe {
        command.pre_exec(move || {
            inherited.write_all(&[1])?;
            inherited.read_exact(&mut [0])?;
            Ok(())
        });
    }
    let child = std::thread::spawn(move || command.status());
    controller.read_exact(&mut [0]).unwrap();
    let competing = Store::open(directory.path());
    drop(store);
    let reopened = Store::open(directory.path());
    controller.write_all(&[1]).unwrap();
    assert!(child.join().unwrap().unwrap().success());

    assert!(matches!(competing, Err(error) if error.code == "WORKSPACE_BUSY"));
    let reopened = reopened.expect("a closed store should release its lock before the child execs");
    assert_eq!(reopened.page(&workspace, &page.info.id).unwrap(), page);
    let competing = Store::open(directory.path());
    assert!(matches!(competing, Err(error) if error.code == "WORKSPACE_BUSY"));
}

#[test]
fn sqlite_full_rolls_back_body_history_and_pending_index_then_allows_retry() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path()).unwrap();
    let (workspace, page) = initial(&store);
    let history = store.history(&workspace, &page.info.id).unwrap();
    let draft = draft(
        &page.blocks[0].id,
        "需要更多空间",
        &"可取回的中文草稿\n".repeat(8_000),
    );
    let mut db = store.connection(&workspace).unwrap();
    let pages: u32 = db
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    db.pragma_update(None, "max_page_count", pages).unwrap();
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let error = stage_edit(
        &tx,
        &page.info.id,
        page.info.revision,
        document_edit(draft.clone()).unwrap(),
    )
    .and_then(|_| tx.commit().map_err(Into::into))
    .unwrap_err();
    assert_eq!(error.code, "STORAGE_FULL");
    assert!(error.message.contains("存储空间不足"));
    drop(db);

    assert_eq!(store.page(&workspace, &page.info.id).unwrap(), page);
    assert_eq!(store.history(&workspace, &page.info.id).unwrap(), history);
    assert_eq!(search(&store, &workspace, "山茶原文").index.pending, 0);
    assert!(
        search(&store, &workspace, "可取回的中文草稿")
            .hits
            .is_empty()
    );
    let saved = store
        .save_page(&workspace, &page.info.id, page.info.revision, draft.clone())
        .unwrap();
    assert_eq!(saved.revision, page.info.revision + 1);
    assert_eq!(
        store.page(&workspace, &page.info.id).unwrap().blocks,
        draft.blocks
    );
    assert_eq!(
        search(&store, &workspace, "可取回的中文草稿").index.pending,
        1
    );
}

#[test]
fn killed_before_commit_rolls_back_and_killed_after_ack_keeps_revision() {
    for phase in ["staged", "acknowledged"] {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let (workspace, page) = initial(&store);
        drop(store);

        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "store::recovery::crash_worker",
                "--ignored",
                "--nocapture",
            ])
            .env(WORKER_CASE, phase)
            .env("ZHIYE_RECOVERY_WORKER_ROOT", directory.path())
            .env("ZHIYE_RECOVERY_WORKER_SPACE", &workspace)
            .env("ZHIYE_RECOVERY_WORKER_PAGE", &page.info.id)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let (sender, receiver) = mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(|line| line.ok()) {
                if line.starts_with("READY ") {
                    let _ = sender.send(line);
                }
            }
        });
        let ready = receiver.recv_timeout(Duration::from_secs(15));
        let journal = directory
            .path()
            .join(&workspace)
            .join("workspace.sqlite-journal");
        let journal_bytes = fs::metadata(journal).map_or(0, |metadata| metadata.len());
        let killed = child.kill();
        let status = child.wait().unwrap();
        reader.join().unwrap();
        assert_eq!(
            ready.unwrap(),
            format!("READY {phase} {}", page.info.revision + 1)
        );
        killed.unwrap();
        assert!(!status.success());

        let reopened = Store::open(directory.path()).unwrap();
        let latest = reopened.page(&workspace, &page.info.id).unwrap();
        let history = reopened.history(&workspace, &page.info.id).unwrap();
        assert_eq!(history[0], latest);
        let db = reopened.connection(&workspace).unwrap();
        let integrity: String = db
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");
        drop(db);
        if phase == "staged" {
            assert!(
                journal_bytes > 512,
                "the killed writer had spilled rollback pages"
            );
            assert_eq!(latest, page);
            assert_eq!(history.len(), page.info.revision as usize);
            assert_eq!(search(&reopened, &workspace, "山茶原文").index.pending, 0);
            assert!(search(&reopened, &workspace, "秋桂提交").hits.is_empty());
        } else {
            assert_eq!(latest.info.revision, page.info.revision + 1);
            assert_eq!(latest.blocks[0].id, page.blocks[0].id);
            assert_eq!(history[1], page);
            assert_eq!(history.len(), page.info.revision as usize + 1);
            assert_eq!(search(&reopened, &workspace, "秋桂提交").index.pending, 1);
            assert!(search(&reopened, &workspace, "山茶原文").hits.is_empty());
            assert_eq!(
                reopened.advance_search_index(&workspace).unwrap().pending,
                0
            );
            assert_eq!(
                reopened.advance_search_index(&workspace).unwrap().pending,
                0
            );
            let hits = search(&reopened, &workspace, "秋桂提交").hits;
            assert_eq!(hits.len(), 1);
            assert_eq!(
                hits[0].block_id.as_deref(),
                Some(page.blocks[0].id.as_str())
            );
            assert_eq!(
                reopened.history(&workspace, &page.info.id).unwrap(),
                history
            );
        }
    }
}

#[test]
#[ignore = "started in a child process by the recovery test"]
fn crash_worker() {
    let Ok(phase) = std::env::var(WORKER_CASE) else {
        return;
    };
    let root = std::env::var_os("ZHIYE_RECOVERY_WORKER_ROOT").unwrap();
    let workspace = std::env::var("ZHIYE_RECOVERY_WORKER_SPACE").unwrap();
    let id = std::env::var("ZHIYE_RECOVERY_WORKER_PAGE").unwrap();
    let store = Store::open(root).unwrap();
    let page = store.page(&workspace, &id).unwrap();
    if phase == "staged" {
        let mut db = store.connection(&workspace).unwrap();
        db.pragma_update(None, "cache_size", 3).unwrap();
        let tx = db
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .unwrap();
        let pending = draft(
            &page.blocks[0].id,
            "未确认稿",
            &"待提交的中文正文\n".repeat(8_000),
        );
        let staged = stage_edit(
            &tx,
            &id,
            page.info.revision,
            document_edit(pending).unwrap(),
        )
        .unwrap();
        let recorded: u32 = tx
            .query_row(
                "SELECT max(revision) FROM revisions WHERE page_id=?1",
                [&id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, staged.info.revision);
        wait_for_stop(&phase, recorded);
        drop(tx);
    } else {
        assert_eq!(phase, "acknowledged");
        let saved = store
            .save_page(
                &workspace,
                &id,
                page.info.revision,
                draft(&page.blocks[0].id, "已确认新稿", "秋桂提交后依然可检索"),
            )
            .unwrap();
        wait_for_stop(&phase, saved.revision);
    }
}

fn wait_for_stop(phase: &str, revision: u32) {
    println!("READY {phase} {revision}");
    std::io::stdout().flush().unwrap();
    let _ = std::io::stdin().read(&mut [0]);
}
