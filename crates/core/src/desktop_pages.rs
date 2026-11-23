// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::PageInfo;
use serde::{Serialize, Serializer, ser::SerializeSeq};

/// Desktop page metadata can use fixed-order rows to avoid repeating JSON field names.
pub struct DesktopPageList {
    pages: Vec<PageInfo>,
    compact: bool,
}

impl DesktopPageList {
    pub fn new(pages: Vec<PageInfo>, compact: bool) -> Self {
        Self { pages, compact }
    }
}

impl Serialize for DesktopPageList {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if !self.compact {
            return self.pages.serialize(serializer);
        }
        let mut rows = serializer.serialize_seq(Some(self.pages.len()))?;
        for page in &self.pages {
            rows.serialize_element(&(
                &page.id,
                &page.parent_id,
                &page.title,
                page.revision,
                page.updated_at,
                &page.icon,
                &page.trash_root_id,
            ))?;
        }
        rows.end()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, to_value};

    #[test]
    fn desktop_page_list_preserves_order_nulls_and_archived_metadata() {
        let root = PageInfo {
            id: "root".into(),
            parent_id: None,
            title: "根目录".into(),
            revision: 1,
            updated_at: 1_720_000_000_000,
            icon: "folder".into(),
            trash_root_id: None,
        };
        let archived = PageInfo {
            id: "child".into(),
            parent_id: Some(root.id.clone()),
            title: "归档页".into(),
            revision: 4,
            updated_at: 1_720_000_000_001,
            icon: "file".into(),
            trash_root_id: Some("child".into()),
        };
        let pages = vec![root, archived];
        assert_eq!(
            to_value(DesktopPageList::new(pages.clone(), false)).unwrap(),
            to_value(&pages).unwrap(),
        );
        assert_eq!(
            to_value(DesktopPageList::new(pages, true)).unwrap(),
            json!([
                [
                    "root",
                    null,
                    "根目录",
                    1,
                    1_720_000_000_000_i64,
                    "folder",
                    null
                ],
                [
                    "child",
                    "root",
                    "归档页",
                    4,
                    1_720_000_000_001_i64,
                    "file",
                    "child"
                ],
            ]),
        );
    }
}
