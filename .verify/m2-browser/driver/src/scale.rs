use serde_json::{Value, json};
use std::{fs::File, path::Path, time::Instant};
use zhiye_core::{Block, Error, PageDraft, Result, Store};

fn elapsed(start: Instant) -> u128 {
    start.elapsed().as_millis()
}

pub fn seed(store: &Store) -> Result<Value> {
    let start = Instant::now();
    let space = store.create_workspace("千页组织验收")?;
    let mut roots = Vec::new();
    let mut target = None;
    for section in 1..=20 {
        let folder = store.create_page(&space.id, None, &format!("目录 {section:02}"), "folder")?;
        roots.push(folder.info.clone());
        let mut parent = folder.info.id.clone();
        for number in 1..=49 {
            let title = format!("资料 {section:02}·{number:02}");
            let page = store.create_page(&space.id, Some(&parent), &title, "file")?;
            let topic = if section == 20 && number == 49 {
                "千页验收青岚"
            } else {
                "千页合成资料"
            };
            let line = Block {
                id: format!("00000000-0000-4000-8000-{:012x}", section * 100 + number),
                kind: "paragraph".into(),
                schema_version: 1,
                payload: json!({"inline": [
                    {"type": "text", "text": format!("{topic} · 第 {section} 组第 {number} 页。")},
                    {"type": "pageReference", "pageId": folder.info.id, "label": folder.info.title}
                ]}),
                children: vec![],
                extra: Default::default(),
            };
            let saved = store.save_page(
                &space.id,
                &page.info.id,
                page.info.revision,
                PageDraft {
                    title,
                    blocks: vec![line],
                },
            )?;
            if number % 12 == 0 {
                parent = saved.id.clone();
            }
            if section == 20 && number == 49 {
                target = Some(store.page(&space.id, &saved.id)?);
            }
        }
    }
    Ok(
        json!({"workspace": space, "roots": roots, "target": target, "seedMs": elapsed(start),
        "pageCount": store.pages(&space.id)?.len()}),
    )
}

pub fn roundtrip(store: &Store, space: &str, backup: &Path) -> Result<Value> {
    let start = Instant::now();
    let expected = store.pages(space)?;
    let archive = backup.with_file_name("m1-thousand.zhiye.zip");
    store.write_workspace_archive(space, File::create(&archive)?)?;
    let archive_ms = elapsed(start);
    let root = backup.with_file_name("m1-restored-workspaces");
    let restore = Instant::now();
    let destination = Store::open(&root)?;
    let imported = destination.read_workspace_archive(File::open(&archive)?)?;
    drop(destination);
    let restored = Store::open(&root)?;
    let reopen_ms = elapsed(restore);
    let inspect = Instant::now();
    let mut restored_info = restored.pages(&imported.id)?;
    let mut expected_info = expected;
    restored_info.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    expected_info.sort_unstable_by(|a, b| a.id.cmp(&b.id));
    if expected_info.len() != 1000 || expected_info != restored_info || imported.id == space {
        return Err(Error::new(
            "VERIFY_SCALE",
            "恢复空间的页面身份、层级或数目不同。",
        ));
    }
    let mut blocks = 0;
    let mut references = 0;
    let mut revisions = 0;
    for info in &expected_info {
        let original = store.page(space, &info.id)?;
        let history = store.history(space, &info.id)?;
        if original != restored.page(&imported.id, &info.id)?
            || history != restored.history(&imported.id, &info.id)?
        {
            return Err(Error::new(
                "VERIFY_SCALE",
                format!("页面 {} 的正文或修订不同。", info.id),
            ));
        }
        for block in original.blocks {
            blocks += 1;
            references += block.payload["inline"].as_array().map_or(0, |items| {
                items
                    .iter()
                    .filter(|item| item["type"] == "pageReference")
                    .count()
            });
        }
        revisions += history.len();
    }
    Ok(
        json!({"pages": expected_info.len(), "topLevelBlocks": blocks, "pageReferences": references,
        "revisions": revisions, "independentRoot": true, "archiveMs": archive_ms,
        "restoreAndOpenMs": reopen_ms, "comparisonMs": elapsed(inspect),
        "archiveBytes": archive.metadata()?.len()}),
    )
}
