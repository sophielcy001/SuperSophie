// SPDX-License-Identifier: AGPL-3.0-or-later

use super::{
    Access,
    content::project_block,
    missing,
    mutations::{MAX_OPERATIONS, paragraph, partial_content},
};
use crate::model::new_id;
use crate::{Block, Error, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationBlockDraft {
    #[serde(rename = "type")]
    pub kind: String,
    pub payload: Value,
    pub children: Option<Vec<Self>>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum IntegrationBlockOperation {
    Append {
        parent_block_id: Option<String>,
        block: IntegrationBlockDraft,
    },
    Insert {
        parent_block_id: Option<String>,
        index: usize,
        block: IntegrationBlockDraft,
    },
    Update {
        block_id: String,
        block: IntegrationBlockDraft,
    },
    Delete {
        block_id: String,
    },
    Move {
        block_id: String,
        parent_block_id: Option<String>,
        index: usize,
    },
}

impl IntegrationBlockOperation {
    pub fn append_text(&self) -> Option<&str> {
        match self {
            Self::Append {
                parent_block_id: None,
                block,
            } if block.children.is_none() => block.text(),
            _ => None,
        }
    }
}

impl IntegrationBlockDraft {
    fn text(&self) -> Option<&str> {
        if self.kind != "paragraph" || self.payload.as_object()?.len() != 1 {
            return None;
        }
        self.payload["text"].as_str()
    }

    fn materialize(self, previous: Option<Block>, new_ids: &mut Vec<String>) -> Result<Block> {
        let payload = if let Some(text) = self.text() {
            if text.contains('\0') {
                return Err(invalid_structure());
            }
            paragraph(text).payload
        } else {
            self.payload
        };
        let (id, original_children) = if let Some(previous) = previous {
            (previous.id, previous.children)
        } else {
            let id = new_id();
            new_ids.push(id.clone());
            (id, Vec::new())
        };
        let children = match self.children {
            Some(children) => children
                .into_iter()
                .map(|child| child.materialize(None, new_ids))
                .collect::<Result<_>>()?,
            None => original_children,
        };
        Ok(Block {
            id,
            kind: self.kind,
            schema_version: 1,
            payload,
            children,
            extra: Default::default(),
        })
    }
}

pub(super) fn apply_operations(
    db: &Connection,
    access: &Access,
    page_id: &str,
    roots: &mut Vec<Block>,
    operations: Vec<IntegrationBlockOperation>,
) -> Result<Vec<String>> {
    if operations.len() > MAX_OPERATIONS {
        return Err(Error::new(
            "TOO_MANY_OPERATIONS",
            "每批最多可以提交 100 个块操作。",
        ));
    }
    if operations.is_empty() {
        return Err(Error::new(
            "INVALID_BLOCK_OPERATION",
            "请提供至少一项块操作。",
        ));
    }
    let mut edit = BlockEditor {
        db,
        access,
        page_id,
        roots,
        new_ids: Vec::new(),
    };
    for operation in operations {
        edit.apply(operation)?;
    }
    Ok(edit.new_ids)
}

struct BlockEditor<'a> {
    db: &'a Connection,
    access: &'a Access,
    page_id: &'a str,
    roots: &'a mut Vec<Block>,
    new_ids: Vec<String>,
}

impl BlockEditor<'_> {
    fn apply(&mut self, operation: IntegrationBlockOperation) -> Result<()> {
        use IntegrationBlockOperation::{Append, Delete, Insert, Move, Update};
        match operation {
            Append {
                parent_block_id,
                block,
            } => self.place(parent_block_id.as_deref(), None, block),
            Insert {
                parent_block_id,
                index,
                block,
            } => self.place(parent_block_id.as_deref(), Some(index), block),
            Update { block_id, block } => {
                let path = self.authorized_path(&block_id, true)?;
                let (index, parent) = path.split_last().expect("a block path is nonempty");
                let original = groups_at_mut(self.roots, parent)[*index].clone();
                let replacement = block.materialize(Some(original), &mut self.new_ids)?;
                self.validate_input(&replacement)?;
                validate_placement(parent_kind(self.roots, parent), &replacement)?;
                groups_at_mut(self.roots, parent)[*index] = replacement;
                self.validate_ancestors(parent)
            }
            Delete { block_id } => {
                let path = self.authorized_path(&block_id, true)?;
                let (index, parent) = path.split_last().expect("a block path is nonempty");
                groups_at_mut(self.roots, parent).remove(*index);
                self.validate_ancestors(parent)
            }
            Move {
                block_id,
                parent_block_id,
                index,
            } => {
                let source = self.authorized_path(&block_id, true)?;
                let destination = self.parent_path(parent_block_id.as_deref())?;
                if destination.starts_with(&source) {
                    return Err(Error::new(
                        "INVALID_BLOCK_OPERATION",
                        "不能把块移动到自己的子块中。",
                    ));
                }
                let (index_before, old_parent) =
                    source.split_last().expect("a block path is nonempty");
                let old_parent_id = last_block(self.roots, old_parent).map(|p| p.id.clone());
                let moved = groups_at_mut(self.roots, old_parent).remove(*index_before);
                let destination = path_to_parent(self.roots, parent_block_id.as_deref())?;
                self.insert_at(&destination, Some(index), moved)?;
                self.validate_ancestors(&path_to_parent(self.roots, old_parent_id.as_deref())?)
            }
        }
    }

    fn place(
        &mut self,
        parent: Option<&str>,
        index: Option<usize>,
        block: IntegrationBlockDraft,
    ) -> Result<()> {
        let destination = self.parent_path(parent)?;
        let block = block.materialize(None, &mut self.new_ids)?;
        self.validate_input(&block)?;
        self.insert_at(&destination, index, block)
    }

    fn insert_at(&mut self, parent: &[usize], index: Option<usize>, block: Block) -> Result<()> {
        validate_placement(parent_kind(self.roots, parent), &block)?;
        let siblings = groups_at_mut(self.roots, parent);
        let index = index.unwrap_or(siblings.len());
        if index > siblings.len() {
            return Err(Error::new(
                "INVALID_BLOCK_OPERATION",
                "插入位置超出同级块范围。",
            ));
        }
        siblings.insert(index, block);
        self.validate_ancestors(parent)
    }

    fn parent_path(&self, id: Option<&str>) -> Result<Vec<usize>> {
        id.map_or_else(|| Ok(Vec::new()), |id| self.authorized_path(id, false))
    }

    fn authorized_path(&self, id: &str, complete: bool) -> Result<Vec<usize>> {
        let path = find_path(self.roots, id).ok_or_else(missing)?;
        let projected = project_block(self.db, self.access, self.page_id, &self.roots[path[0]])?;
        let target = projected.find(id).ok_or_else(missing)?;
        if target.kind == "restricted" || complete && target.restricted {
            return Err(partial_content());
        }
        Ok(path)
    }

    fn validate_input(&self, block: &Block) -> Result<()> {
        let visible = project_block(self.db, self.access, self.page_id, block)?;
        if visible.kind == "restricted"
            && !matches!(block.kind.as_str(), "attachmentImage" | "attachmentFile")
            && crate::references::block_target(block).is_none()
            && crate::collections::views::view_target(block).is_none()
        {
            return Err(invalid_structure());
        }
        if visible.restricted {
            return Err(partial_content());
        }
        validate_tree(block)
    }

    fn validate_ancestors(&self, parent: &[usize]) -> Result<()> {
        let mut group = self.roots.as_slice();
        for &index in parent {
            let block = &group[index];
            validate_children(block)?;
            group = &block.children;
        }
        Ok(())
    }
}

fn path_to_parent(blocks: &[Block], id: Option<&str>) -> Result<Vec<usize>> {
    id.map_or_else(
        || Ok(Vec::new()),
        |id| find_path(blocks, id).ok_or_else(missing),
    )
}

fn find_path(blocks: &[Block], id: &str) -> Option<Vec<usize>> {
    fn walk(blocks: &[Block], id: &str, path: &mut Vec<usize>) -> bool {
        for (index, block) in blocks.iter().enumerate() {
            path.push(index);
            if block.id == id || walk(&block.children, id, path) {
                return true;
            }
            path.pop();
        }
        false
    }
    let mut path = Vec::new();
    walk(blocks, id, &mut path).then_some(path)
}

fn groups_at_mut<'a>(mut group: &'a mut Vec<Block>, path: &[usize]) -> &'a mut Vec<Block> {
    for &index in path {
        group = &mut group[index].children;
    }
    group
}

fn last_block<'a>(mut group: &'a [Block], path: &[usize]) -> Option<&'a Block> {
    let (&last, parents) = path.split_last()?;
    for &index in parents {
        group = &group[index].children;
    }
    group.get(last)
}

fn parent_kind<'a>(roots: &'a [Block], parent: &[usize]) -> Option<&'a str> {
    last_block(roots, parent).map(|block| block.kind.as_str())
}

fn validate_tree(block: &Block) -> Result<()> {
    validate_children(block)?;
    for child in &block.children {
        validate_tree(child)?;
    }
    Ok(())
}

fn validate_placement(parent: Option<&str>, child: &Block) -> Result<()> {
    let kind = child.kind.as_str();
    let valid = match parent {
        Some("bulletList" | "orderedList") => kind == "listItem",
        Some("taskList") => kind == "taskItem",
        Some("table") => kind == "tableRow",
        Some("tableRow") => matches!(kind, "tableCell" | "tableHeader"),
        Some(
            "blockquote" | "listItem" | "taskItem" | "tableCell" | "tableHeader" | "toggleBlock"
            | "callout",
        )
        | None => !matches!(
            kind,
            "tableRow" | "tableCell" | "tableHeader" | "listItem" | "taskItem"
        ),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(invalid_structure())
    }
}

fn validate_children(block: &Block) -> Result<()> {
    let basic = match block.kind.as_str() {
        "listItem" | "taskItem" => block
            .children
            .first()
            .is_some_and(|first| first.kind == "paragraph"),
        "tableRow" => true,
        "blockquote" | "bulletList" | "orderedList" | "taskList" | "table" | "tableCell"
        | "tableHeader" | "toggleBlock" | "callout" => !block.children.is_empty(),
        _ => block.children.is_empty(),
    };
    if !basic {
        return Err(invalid_structure());
    }
    for child in &block.children {
        validate_placement(Some(&block.kind), child)?;
    }
    if block.kind == "table" && !valid_table(&block.children) {
        return Err(invalid_structure());
    }
    Ok(())
}

fn spans(cell: &Block, axis: &str) -> Option<usize> {
    if let Some(value) = cell.payload.get(axis) {
        let len = value.as_u64()?;
        (1..=20_000).contains(&len).then_some(len as usize)
    } else {
        Some(1)
    }
}

fn valid_table(rows: &[Block]) -> bool {
    let Some(width) = rows[0].children.iter().try_fold(0_usize, |size, cell| {
        size.checked_add(spans(cell, "colspan")?)
    }) else {
        return false;
    };
    if width == 0 || width > 20_000 / rows.len() {
        return false;
    }
    let mut occupied = vec![0; width];
    for (row_index, row) in rows.iter().enumerate() {
        let mut column = 0;
        for cell in &row.children {
            let (Some(columns), Some(height)) = (spans(cell, "colspan"), spans(cell, "rowspan"))
            else {
                return false;
            };
            if cell
                .payload
                .get("colwidth")
                .is_some_and(|value| value.as_array().is_none_or(|a| a.len() != columns))
            {
                return false;
            }
            while column < width && occupied[column] > row_index {
                column += 1;
            }
            let end = column + columns;
            let bottom = row_index + height;
            if end > width
                || bottom > rows.len()
                || occupied[column..end].iter().any(|&used| used > row_index)
            {
                return false;
            }
            occupied[column..end].fill(bottom);
            column = end;
        }
        if occupied.iter().any(|&bottom| bottom <= row_index) {
            return false;
        }
    }
    true
}

fn invalid_structure() -> Error {
    Error::new(
        "INVALID_BLOCK_OPERATION",
        "块类型、内容或父子关系不符合当前编辑器格式。",
    )
}
