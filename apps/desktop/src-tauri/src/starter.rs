// SPDX-License-Identifier: AGPL-3.0-or-later

use serde_json::{Value, json};
use zhiye_core::{Block, PageDraft, Result, Store};

/// The starter page names the search shortcut of the platform it was created on.
const SEARCH_HINT: &str = if cfg!(target_os = "macos") {
    "。按 ⌘K 可搜索当前工作空间。"
} else {
    "。按 Ctrl+K 可搜索当前工作空间。"
};

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
    block(
        "paragraph",
        json!({"inline": [{"type": "text", "text": text}]}),
        vec![],
    )
}

fn heading(text: &str) -> Block {
    block(
        "heading",
        json!({"level": 2, "inline": [{"type": "text", "text": text}]}),
        vec![],
    )
}

pub fn create(store: &Store) -> Result<()> {
    let workspace = store.create_workspace("我的知识库")?;
    let guide = store.create_page(&workspace.id, None, "开始使用", "sparkles")?;
    let notes = store.create_page(&workspace.id, None, "笔记", "folder")?;
    let reading = store.create_page(&workspace.id, Some(&notes.info.id), "阅读笔记", "book")?;
    let inbox = store.create_page(&workspace.id, None, "收集箱", "inbox")?;
    store.set_inbox(&workspace.id, &inbox.info.id)?;

    let guide_blocks = vec![
        paragraph(
            "把读到的资料、想法和待办放进同一个工作空间。你可以直接编辑这份页面，修改会自动保存到本机。",
        ),
        heading("写下第一条笔记"),
        paragraph(
            "点击侧栏的「新建页面」，为笔记起个名字。在空白行输入 /，或点击块左侧的 +，插入标题、列表、待办或代码块；选中文字即可设置格式。",
        ),
        block(
            "taskList",
            json!({}),
            vec![
                block(
                    "taskItem",
                    json!({"checked": true}),
                    vec![paragraph("打开工作空间")],
                ),
                block(
                    "taskItem",
                    json!({"checked": false}),
                    vec![paragraph("新建一个页面，写下今天的想法")],
                ),
            ],
        ),
        heading("整理与查找"),
        block(
            "paragraph",
            json!({"inline": [
                {"type":"text", "text":"在「笔记」下新建子页面，或从这里打开 "},
                {"type":"pageReference", "pageId":reading.info.id, "label":"阅读笔记"},
                {"type":"text", "text":SEARCH_HINT}
            ]}),
            vec![],
        ),
        heading("备份你的资料"),
        paragraph(
            "从侧栏的「备份与恢复」导出工作空间包。导入时会创建独立副本，保留页面关系与历史修订。",
        ),
    ];
    store.save_page(
        &workspace.id,
        &guide.info.id,
        1,
        PageDraft {
            title: guide.info.title,
            blocks: guide_blocks,
        },
    )?;
    store.save_page(
        &workspace.id,
        &reading.info.id,
        1,
        PageDraft {
            title: reading.info.title,
            blocks: vec![
                heading("资料来源"),
                paragraph("在这里记录书名、文章或链接。"),
                heading("我的理解"),
                paragraph("用自己的话概括关键内容，记录还需要验证的问题。"),
            ],
        },
    )?;
    store.save_page(
        &workspace.id,
        &inbox.info.id,
        1,
        PageDraft {
            title: inbox.info.title,
            blocks: vec![paragraph(
                "暂存还没整理的想法。可以在这个页面下为每条线索新建子页面。",
            )],
        },
    )?;
    Ok(())
}
