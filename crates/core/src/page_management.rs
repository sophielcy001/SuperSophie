// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::collections;
use crate::integrations::derived::{ComputedChange, ReferenceChange, enqueue};
use crate::integrations::feed::{MoveImpact, RangeSnapshot, record_archive};
use crate::store::{
    commit_revision, ensure_active_info, ensure_revision, read_page, read_page_info,
};
use crate::{Error, PageInfo, Result, Store};
use rusqlite::{Connection, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageVersion {
    pub id: String,
    pub revision: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PageMoves {
    pub pages: Vec<PageInfo>,
    pub impacts: Vec<MoveImpact>,
}

impl Store {
    pub fn move_page(
        &self,
        workspace: &str,
        id: &str,
        revision: u32,
        parent: Option<&str>,
    ) -> Result<PageInfo> {
        let selected = [PageVersion {
            id: id.into(),
            revision,
        }];
        self.move_pages(workspace, &selected, parent)
            .map(|mut pages| pages.remove(0))
    }

    pub fn move_pages(
        &self,
        workspace: &str,
        selection: &[PageVersion],
        parent: Option<&str>,
    ) -> Result<Vec<PageInfo>> {
        Ok(self
            .move_pages_with_impact(workspace, selection, parent)?
            .pages)
    }

    pub fn move_pages_with_impact(
        &self,
        workspace: &str,
        selection: &[PageVersion],
        parent: Option<&str>,
    ) -> Result<PageMoves> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let roots = active_roots(&tx, checked_selection(&tx, selection)?)?;
        if roots.iter().all(|page| page.parent_id.as_deref() == parent) {
            return Ok(PageMoves {
                pages: roots,
                impacts: Vec::new(),
            });
        }
        validate_destination(&tx, &roots, parent)?;
        let ranges = RangeSnapshot::capture(&tx)?;
        let mut moved = Vec::with_capacity(roots.len());
        for root in roots {
            if root.parent_id.as_deref() == parent {
                moved.push(root);
                continue;
            }
            let mut page = read_page(&tx, &root.id)?;
            if page.properties.is_some() {
                return Err(collection_move());
            }
            page.info.parent_id = parent.map(str::to_owned);
            commit_revision(&tx, &mut page)?;
            moved.push(page.info);
        }
        let impacts = ranges.finish(&tx)?;
        tx.commit()?;
        Ok(PageMoves {
            pages: moved,
            impacts,
        })
    }

    pub fn archive_page(&self, workspace: &str, id: &str, revision: u32) -> Result<()> {
        self.archive_pages(
            workspace,
            &[PageVersion {
                id: id.into(),
                revision,
            }],
        )
        .map(|_| ())
    }

    pub fn archive_pages(&self, workspace: &str, selection: &[PageVersion]) -> Result<usize> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let roots = active_roots(&tx, checked_selection(&tx, selection)?)?;
        let subtrees = roots
            .into_iter()
            .map(|root| {
                let ids = active_subtree(&tx, &root.id)?;
                Ok((root.id, ids))
            })
            .collect::<Result<Vec<_>>>()?;
        let count = archive_subtrees(&tx, &subtrees)?.len();
        tx.commit()?;
        Ok(count)
    }

    pub fn restore_page(&self, workspace: &str, id: &str, revision: u32) -> Result<()> {
        self.restore_pages(
            workspace,
            &[PageVersion {
                id: id.into(),
                revision,
            }],
        )
        .map(|_| ())
    }

    pub fn restore_pages(&self, workspace: &str, selection: &[PageVersion]) -> Result<usize> {
        let mut db = self.connection(workspace)?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let roots = checked_selection(&tx, selection)?;
        let restoring: HashSet<_> = roots.iter().map(|root| root.id.as_str()).collect();
        let subtrees = {
            let mut query = tx.prepare("SELECT id FROM pages WHERE trash_root_id = ?1")?;
            roots
                .iter()
                .map(|root| {
                    query
                        .query_map([&root.id], |row| row.get::<_, String>(0))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let affected: Vec<_> = subtrees.iter().flatten().cloned().collect();
        let derived = ReferenceChange::visibility(&tx, &affected)?;
        let computed = ComputedChange::visibility(&tx, &affected)?;
        let mut ranges = None;
        for (root, ids) in roots.iter().zip(subtrees) {
            if root.trash_root_id.as_deref() != Some(root.id.as_str()) {
                return Err(Error::new(
                    "NOT_TRASH_ROOT",
                    "请从回收站恢复这组页面的根页面。",
                ));
            }
            let parent = root
                .parent_id
                .as_deref()
                .map(|id| read_page_info(&tx, id))
                .transpose()?;
            let parent_stays_archived = parent
                .as_ref()
                .and_then(|parent| parent.trash_root_id.as_deref())
                .is_some_and(|archive| !restoring.contains(archive));
            if parent_stays_archived
                && collections::for_parent(&tx, root.parent_id.as_deref())?.is_some()
            {
                return Err(Error::new(
                    "COLLECTION_TRASHED",
                    "请先恢复记录所属的数据库。",
                ));
            }
            if parent_stays_archived && ranges.is_none() {
                ranges = Some(RangeSnapshot::capture(&tx)?);
            }
            for id in ids {
                let mut page = read_page(&tx, &id)?;
                page.info.trash_root_id = None;
                if id == root.id && parent_stays_archived {
                    page.info.parent_id = None;
                }
                commit_revision(&tx, &mut page)?;
            }
        }
        if let Some(ranges) = ranges {
            ranges.finish(&tx)?;
        }
        enqueue(&tx, derived, computed)?;
        tx.commit()?;
        Ok(affected.len())
    }
}

pub(crate) fn active_subtree(db: &Connection, root: &str) -> Result<Vec<String>> {
    let mut query = db.prepare(
        "WITH RECURSIVE subtree(id) AS (
             SELECT id FROM pages WHERE id = ?1 AND trash_root_id IS NULL
             UNION SELECT p.id FROM pages p JOIN subtree ON p.parent_id = subtree.id
             WHERE p.trash_root_id IS NULL
         ) SELECT id FROM subtree",
    )?;
    Ok(query
        .query_map([root], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub(crate) fn archive_subtrees(
    tx: &Transaction<'_>,
    subtrees: &[(String, Vec<String>)],
) -> Result<Vec<PageInfo>> {
    let affected: Vec<_> = subtrees
        .iter()
        .flat_map(|(_, ids)| ids.iter().cloned())
        .collect();
    let derived = ReferenceChange::visibility(tx, &affected)?;
    let computed = ComputedChange::visibility(tx, &affected)?;
    record_archive(tx, &affected)?;
    let mut revisions = Vec::with_capacity(affected.len());
    for (root, ids) in subtrees {
        for id in ids {
            let mut page = read_page(tx, id)?;
            page.info.trash_root_id = Some(root.clone());
            commit_revision(tx, &mut page)?;
            revisions.push(page.info);
        }
    }
    enqueue(tx, derived, computed)?;
    Ok(revisions)
}

fn checked_selection(db: &Connection, selection: &[PageVersion]) -> Result<Vec<PageInfo>> {
    if selection.is_empty() {
        return Err(Error::new("PAGE_SELECTION", "请先选择页面。"));
    }
    let mut pages = Vec::<PageInfo>::with_capacity(selection.len());
    let mut positions = HashMap::<&str, usize>::new();
    for item in selection {
        if let Some(index) = positions.get(item.id.as_str()) {
            ensure_revision(&pages[*index], item.revision)?;
            continue;
        }
        let page = read_page_info(db, &item.id)?;
        ensure_revision(&page, item.revision)?;
        positions.insert(&item.id, pages.len());
        pages.push(page);
    }
    Ok(pages)
}

fn active_roots(db: &Connection, pages: Vec<PageInfo>) -> Result<Vec<PageInfo>> {
    let selected: HashSet<_> = pages.iter().map(|page| page.id.clone()).collect();
    let mut parents: HashMap<_, _> = pages
        .iter()
        .map(|page| (page.id.clone(), page.parent_id.clone()))
        .collect();
    let mut roots = Vec::new();
    for page in pages {
        ensure_active_info(&page)?;
        let mut path = HashSet::from([page.id.clone()]);
        let mut parent = page.parent_id.clone();
        let mut covered = false;
        while let Some(id) = parent {
            if !path.insert(id.clone()) {
                return Err(page_cycle());
            }
            if selected.contains(&id) {
                covered = true;
                break;
            }
            parent = match parents.get(&id) {
                Some(parent) => parent.clone(),
                None => {
                    let parent = read_page_info(db, &id)?.parent_id;
                    parents.insert(id, parent.clone());
                    parent
                }
            };
        }
        if !covered {
            roots.push(page);
        }
    }
    if roots.is_empty() {
        return Err(page_cycle());
    }
    Ok(roots)
}

fn validate_destination(db: &Connection, roots: &[PageInfo], parent: Option<&str>) -> Result<()> {
    let roots: HashSet<_> = roots.iter().map(|page| page.id.as_str()).collect();
    let mut ancestor = parent.map(str::to_owned);
    let mut visited = HashSet::new();
    while let Some(id) = ancestor {
        if roots.contains(id.as_str()) || !visited.insert(id.clone()) {
            return Err(page_cycle());
        }
        let target = read_page_info(db, &id)?;
        ensure_active_info(&target)?;
        ancestor = target.parent_id;
    }
    if collections::for_parent(db, parent)?.is_some() {
        return Err(collection_move());
    }
    Ok(())
}

fn page_cycle() -> Error {
    Error::new("PAGE_CYCLE", "不能把页面移动到自身或自己的子页面。")
}

fn collection_move() -> Error {
    Error::new("COLLECTION_MOVE", "数据库记录随所属数据库移动。")
}
