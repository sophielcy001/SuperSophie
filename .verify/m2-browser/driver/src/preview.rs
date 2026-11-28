// SPDX-License-Identifier: AGPL-3.0-or-later

//! First-run content for the local UI preview (`scripts/ui-preview.mjs`),
//! matching what the desktop app creates on a fresh install.

use serde_json::{Value, json};
use zhiye_core::{Block, PageDraft, Result, Store};

fn block(kind: &str, payload: Value, children: Vec<Block>) -> Block {
    Block {
        id: uuid::Uuid::new_v4().to_string(),
        kind: kind.into(),
        schema_version: 1,
        payload,
        children,
        extra: Default::default(),
    }
}

fn paragraph(text: &str) -> Block {
    block("paragraph", json!({"inline": [{"type": "text", "text": text}]}), vec![])
}

fn heading(text: &str) -> Block {
    block("heading", json!({"level": 2, "inline": [{"type": "text", "text": text}]}), vec![])
}

fn save(store: &Store, workspace: &str, id: &str, title: &str, blocks: Vec<Block>) -> Result<()> {
    store
        .save_page(workspace, id, 1, PageDraft { title: title.into(), blocks })
        .map(drop)
}

pub fn seed(store: &Store) -> Result<Value> {
    let workspace = store.create_workspace("我的知识库")?;
    let ws = workspace.id.as_str();
    let guide = store.create_page(ws, None, "开始使用", "sparkles")?;
    let notes = store.create_page(ws, None, "笔记", "folder")?;
    let reading = store.create_page(ws, Some(&notes.info.id), "阅读笔记", "book")?;
    let inbox = store.create_page(ws, None, "收集箱", "inbox")?;
    store.set_inbox(ws, &inbox.info.id)?;
    save(store, ws, &guide.info.id, "开始使用", vec![
        paragraph("把读到的资料、想法和待办放进同一个工作空间。你可以直接编辑这份页面，修改会自动保存到本机。"),
        heading("写下第一条笔记"),
        paragraph("点击侧栏的「新建页面」，为笔记起个名字。在空白行输入 /，或点击块左侧的 +，插入标题、列表、待办或代码块；选中文字即可设置格式。"),
        block("taskList", json!({}), vec![
            block("taskItem", json!({"checked": true}), vec![paragraph("打开工作空间")]),
            block("taskItem", json!({"checked": false}), vec![paragraph("新建一个页面，写下今天的想法")]),
        ]),
        heading("整理与查找"),
        block("paragraph", json!({"inline": [
            {"type": "text", "text": "在「笔记」下新建子页面，或从这里打开 "},
            {"type": "pageReference", "pageId": reading.info.id, "label": "阅读笔记"},
            {"type": "text", "text": "。按 Ctrl + K 可搜索当前工作空间。"}
        ]}), vec![]),
        heading("备份你的资料"),
        paragraph("从侧栏的「备份与恢复」导出工作空间包。导入时会创建独立副本，保留页面关系与历史修订。"),
    ])?;
    save(store, ws, &reading.info.id, "阅读笔记", vec![
        heading("资料来源"),
        paragraph("在这里记录书名、文章或链接。"),
        heading("我的理解"),
        paragraph("用自己的话概括关键内容，记录还需要验证的问题。"),
    ])?;
    save(store, ws, &inbox.info.id, "收集箱", vec![
        paragraph("暂存还没整理的想法。可以在这个页面下为每条线索新建子页面。"),
    ])?;
    let (collection, _) = store.create_collection(ws, None, "读书清单")?;
    for title in ["思考，快与慢", "原则", "置身事内"] {
        store.create_page(ws, Some(&collection.container_page_id), title, "book")?;
    }
    Ok(json!(workspace))
}
