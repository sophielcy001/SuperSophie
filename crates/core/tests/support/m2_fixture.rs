// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, time::Instant};
use uuid::Uuid;
use zhiye_core::{
    Block, FilterOperator, PageDraft, PropertyDraft, PropertyKind, RecordDraft, Result,
    SortDirection, Store, ViewFilter, ViewSettings, ViewSort,
};

pub const RECORDS: usize = 10_000;
pub const ATTACHMENTS: usize = 1_000;
pub const LOGICAL_SHA256: &str = "107fb70e9bcd39ca6db88459050a780d703540e4ad36b9e5de1098297a03e875";
pub const SEARCHES: [(&str, usize); 8] = [
    ("笔", 20),
    ("笔记", 20),
    ("阅读计划", 20),
    ("SQLite WAL", 20),
    ("::", 20),
    ("run(\"声音\")", 20),
    ("kb-07216-4", 1),
    ("不存在的匹配", 0),
];

const TEXTS: [&str; 10] = [
    "知识库笔记与阅读计划，每日整理资料。",
    "数据库事务 SQLite WAL 记录混合内容。",
    "编程笔记 api::run(\"声音\")，记录 Rust 类型。",
    "Reference guide for searchable notes and office tools.",
    "数据恢复、附件验证和定期归档。",
    "实验笔记：短词召回和精确定位。",
    "项目进度、相关任务与评审记录。",
    "文献收藏、研究方法及检索问题。",
    "SQLite queries and full-text search on Chinese blocks.",
    "本地备份与更新记录。",
];

pub struct Fixture {
    pub collection: String,
    pub container: String,
    pub view: String,
    pub count: u32,
    pub sha256: String,
    pub seed_ms: f64,
}

pub fn paragraph(text: &str) -> Block {
    Block {
        id: Uuid::new_v4().to_string(),
        kind: "paragraph".into(),
        schema_version: 1,
        payload: json!({"inline": [{"type": "text", "text": text}]}),
        children: vec![],
        extra: Default::default(),
    }
}

pub fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1_000.0
}

pub fn seed(store: &Store, workspace: &str) -> Result<Fixture> {
    let (database, container) = store.create_collection(workspace, None, "知识库基线")?;
    let current = store.collection(workspace, &database.id)?;
    let schema = store.save_property(
        workspace,
        &database.id,
        current.schema_revision,
        PropertyDraft {
            id: None,
            name: "序号".into(),
            kind: PropertyKind::Number,
            options: vec![],
        },
    )?;
    let status = &schema.properties[0];
    let sequence = &schema.properties[1].id;
    let started = Instant::now();
    let mut hash = Sha256::new();
    for number in 0..RECORDS {
        let title = format!("资料 {number:05}");
        let record = store.create_page(workspace, Some(&container.info.id), &title, "file")?;
        let option = number % status.options.len();
        hash.update(format!("{title} {option}\n"));
        let record = store.save_record(
            workspace,
            &database.id,
            &record.info.id,
            record.info.revision,
            RecordDraft {
                title: title.clone(),
                schema_revision: schema.schema_revision,
                properties: BTreeMap::from([
                    (status.id.clone(), json!(status.options[option].id)),
                    (sequence.clone(), json!(number)),
                ]),
            },
        )?;
        let blocks = TEXTS
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let text = format!("{text} 标识 kb-{number:05}-{index}。");
                hash.update(text.as_bytes());
                paragraph(&text)
            })
            .collect();
        store.save_page(
            workspace,
            &record.info.id,
            record.info.revision,
            PageDraft { title, blocks },
        )?;
        if number < ATTACHMENTS {
            let mut payload = [b'a'; 1024];
            payload[..8].copy_from_slice(&(number as u64).to_le_bytes());
            hash.update(payload);
            store.add_attachment(workspace, &record.info.id, "基线原件.bin", &payload[..])?;
        }
    }
    let seed_ms = elapsed_ms(started);
    let view = &schema.views[0];
    store.save_view(
        workspace,
        &schema.id,
        &view.id,
        view.revision,
        ViewSettings {
            filter: Some(ViewFilter {
                field_id: status.id.clone(),
                operator: FilterOperator::Equal,
                value: Some(json!(status.options[2].id)),
            }),
            sort: Some(ViewSort {
                field_id: sequence.clone(),
                direction: SortDirection::Desc,
            }),
            group_by: None,
        },
    )?;
    Ok(Fixture {
        collection: schema.id,
        container: container.info.id,
        view: view.id.clone(),
        count: (RECORDS / 3) as u32,
        sha256: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
        seed_ms,
    })
}

pub fn block_count() -> usize {
    TEXTS.len() * RECORDS
}
