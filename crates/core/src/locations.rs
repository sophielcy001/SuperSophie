// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::assets::read_attachment;
use crate::model::valid_id;
use crate::search::pdf::{PdfTarget, valid_sha256};
use crate::store::{ensure_active, read_page, workspace_info};
use crate::{Error, Page, Result, Store, Workspace};
use rusqlite::params;
use serde::Serialize;
use std::fmt::{self, Display, Formatter};

const PREFIX: &str = "zhiye://open/workspaces/";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageLocation<'a> {
    workspace: &'a str,
    page: &'a str,
    target: Target<'a>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Target<'a> {
    Page,
    Block(&'a str),
    Pdf {
        attachment: &'a str,
        sha256: &'a str,
        page_number: u32,
    },
}

impl<'a> PageLocation<'a> {
    pub fn page(workspace: &'a str, page: &'a str) -> Self {
        Self {
            workspace,
            page,
            target: Target::Page,
        }
    }

    pub fn block(self, block: &'a str) -> Self {
        Self {
            target: Target::Block(block),
            ..self
        }
    }

    pub fn pdf(self, attachment: &'a str, sha256: &'a str, page_number: u32) -> Self {
        Self {
            target: Target::Pdf {
                attachment,
                sha256,
                page_number,
            },
            ..self
        }
    }

    pub fn parse(input: &'a str) -> Result<Self> {
        let path = input.strip_prefix(PREFIX).ok_or_else(invalid_location)?;
        let mut parts = path.split('/');
        let Some(workspace) = parts.next().filter(|id| valid_id(id)) else {
            return Err(invalid_location());
        };
        let (Some("pages"), Some(page)) = (parts.next(), parts.next().filter(|id| valid_id(id)))
        else {
            return Err(invalid_location());
        };
        let target = Self::page(workspace, page);
        match parts.next() {
            None => Ok(target),
            Some("blocks") => match (parts.next(), parts.next()) {
                (Some(block), None) if valid_id(block) => Ok(target.block(block)),
                _ => Err(invalid_location()),
            },
            Some("attachments") => {
                let (Some(attachment), Some("pdf"), Some(hash), Some(number), None) = (
                    parts.next(),
                    parts.next(),
                    parts.next(),
                    parts.next(),
                    parts.next(),
                ) else {
                    return Err(invalid_location());
                };
                let page_number = number.parse::<u32>().map_err(|_| invalid_location())?;
                if !valid_id(attachment)
                    || !valid_sha256(hash)
                    || page_number == 0
                    || number != page_number.to_string()
                {
                    return Err(invalid_location());
                }
                Ok(target.pdf(attachment, hash, page_number))
            }
            _ => Err(invalid_location()),
        }
    }
}

impl Display for PageLocation<'_> {
    fn fmt(&self, output: &mut Formatter<'_>) -> fmt::Result {
        write!(output, "{PREFIX}{}/pages/{}", self.workspace, self.page)?;
        match self.target {
            Target::Page => Ok(()),
            Target::Block(block) => write!(output, "/blocks/{block}"),
            Target::Pdf {
                attachment,
                sha256,
                page_number,
            } => write!(
                output,
                "/attachments/{attachment}/pdf/{sha256}/{page_number}"
            ),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLocation {
    pub workspace: Workspace,
    pub page: Page,
    pub block_id: Option<String>,
    pub missing_block: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pdf: Option<PdfTarget>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub missing_pdf: bool,
}

impl Store {
    pub fn resolve_location(&self, url: &str) -> Result<ResolvedLocation> {
        let location = PageLocation::parse(url)?;
        let mut db = self.connection(location.workspace)?;
        let tx = db.transaction()?;
        let workspace = workspace_info(&tx)?;
        let page = read_page(&tx, location.page)?;
        ensure_active(&page)?;
        let block_id = if let Target::Block(id) = location.target {
            let belongs: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM block_ids WHERE id = ?1 AND page_id = ?2)",
                params![id, location.page],
                |row| row.get(0),
            )?;
            belongs.then(|| id.to_owned())
        } else {
            None
        };
        let pdf = if let Target::Pdf {
            attachment,
            sha256,
            page_number,
        } = location.target
        {
            let target = PdfTarget {
                attachment_id: attachment.into(),
                sha256: sha256.into(),
                page_number,
            };
            match read_attachment(&tx, location.page, attachment) {
                Ok(file) if target.matches(&file) => Some(target),
                Ok(_) => None,
                Err(error) if error.code == "ATTACHMENT_NOT_FOUND" => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        Ok(ResolvedLocation {
            workspace,
            page,
            missing_block: matches!(location.target, Target::Block(_)) && block_id.is_none(),
            block_id,
            missing_pdf: matches!(location.target, Target::Pdf { .. }) && pdf.is_none(),
            pdf,
        })
    }
}

fn invalid_location() -> Error {
    Error::new("INVALID_LOCATION", "无法识别这条知页定位链接。")
}
