// SPDX-License-Identifier: AGPL-3.0-or-later

mod identities;

use super::{ArchiveData, extract_blob, insert_content, open_archive, validate_contents};
use crate::assets::{blob_directory, persist_blob};
use crate::bundle;
use crate::imports::{begin_batch, check_cancel, complete_batch, fail_batch};
use crate::store::new_page;
use crate::templates::{fit_merged_templates, template_attachments};
use crate::{MAX_BUNDLE_BYTES, Result, Store};
use rusqlite::TransactionBehavior;
use serde::Serialize;
use std::{
    io::{self, Read, Seek, SeekFrom},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveMerge {
    pub page_id: String,
    pub pages: usize,
    pub collections: usize,
    pub attachments: usize,
    pub revisions: usize,
    pub templates: usize,
    pub notices: Vec<String>,
}

impl ArchiveMerge {
    fn summary(&self) -> String {
        let mut summary = format!(
            "已合并 {} 页、{} 个数据库、{} 条附件记录、{} 条修订和 {} 个模板。",
            self.pages, self.collections, self.attachments, self.revisions, self.templates,
        );
        for notice in &self.notices {
            summary.push(' ');
            summary.push_str(notice);
        }
        summary
    }
}

impl Store {
    pub fn merge_workspace_package(
        &self,
        workspace: &str,
        source: &str,
        input: impl Read + Seek,
        cancel: &AtomicBool,
    ) -> Result<ArchiveMerge> {
        let mut db = self.connection(workspace)?;
        let batch = begin_batch(&db, &self.session_id, "nativeContent", source)?;
        let result = (|| {
            check_cancel(cancel)?;
            let mut input = CancellableInput { input, cancel };
            let (mut archive, mut data) = (|| {
                let mut prefix = [0; 4];
                input.read_exact(&mut prefix)?;
                input.rewind()?;
                if prefix == *b"PK\x03\x04" {
                    let (archive, data) = open_archive(input)?;
                    Ok((Some(archive), data))
                } else {
                    let mut bytes = Vec::new();
                    input
                        .take(MAX_BUNDLE_BYTES as u64 + 1)
                        .read_to_end(&mut bytes)?;
                    Ok((
                        None,
                        ArchiveData {
                            content: bundle::decode(&bytes)?,
                            attachments: Vec::new(),
                            databases: None,
                            library: None,
                            export: None,
                        },
                    ))
                }
            })()
            .map_err(|error| check_cancel(cancel).err().unwrap_or(error))?;
            let root = new_page(
                None,
                &format!("{}（导入）", data.content.workspace.name),
                "folder",
            )?;
            let templates = data
                .library
                .as_ref()
                .map_or(&[][..], |library| &library.templates);
            let mut report = ArchiveMerge {
                page_id: root.info.id.clone(),
                pages: data.content.pages.len(),
                collections: data.databases.as_ref().map_or(0, |db| db.definitions.len()),
                attachments: data.attachments.len() + template_attachments(templates).count(),
                revisions: data.content.revisions.len(),
                templates: templates.len(),
                notices: Vec::new(),
            };
            identities::remap(&mut data, root, &mut report.notices, cancel)?;
            let blobs = validate_contents(&data)?;
            let mut staged = Vec::new();
            let directory = if let Some(archive) = &mut archive
                && !blobs.is_empty()
            {
                let directory = blob_directory(&self.data_root().join(workspace), true)?;
                for attachment in blobs.into_values() {
                    check_cancel(cancel)?;
                    let file = extract_blob(archive, attachment, &directory)
                        .map_err(|error| check_cancel(cancel).err().unwrap_or(error))?;
                    staged.push((attachment.clone(), file));
                }
                Some(directory)
            } else {
                None
            };
            check_cancel(cancel)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            check_cancel(cancel)?;
            if let Some(library) = &mut data.library {
                let renamed = fit_merged_templates(&tx, &mut library.templates)?;
                if renamed > 0 {
                    report
                        .notices
                        .push(format!("{renamed} 个重名模板已添加编号。"));
                }
            }
            insert_content(&tx, &data)?;
            if let Some(directory) = directory {
                // Blobs are immutable and shared. A concurrent writer may already use the same
                // hash, so rollback DML without unlinking any published blob.
                for (attachment, file) in staged {
                    check_cancel(cancel)?;
                    persist_blob(file, &directory, &attachment)?;
                }
            }
            check_cancel(cancel)?;
            complete_batch(&tx, &batch, &report.page_id, Some(&report.summary()))?;
            tx.commit()?;
            Ok(report)
        })();
        if let Err(error) = &result {
            fail_batch(&db, &batch, error);
        }
        result
    }
}

struct CancellableInput<'a, R> {
    input: R,
    cancel: &'a AtomicBool,
}

impl<R> CancellableInput<'_, R> {
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::other("import cancelled"))
        } else {
            Ok(())
        }
    }
}

impl<R: Read> Read for CancellableInput<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.input.read(output)
    }
}

impl<R: Seek> Seek for CancellableInput<'_, R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.input.seek(position)
    }
}
