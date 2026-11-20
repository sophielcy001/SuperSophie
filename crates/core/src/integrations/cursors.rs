// SPDX-License-Identifier: AGPL-3.0-or-later

use super::Access;
use crate::store::now;
use crate::{Error, PdfProgress, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use rusqlite::Connection;
use serde::Serialize;
use sha2::Sha256;

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 100;
const CURSOR_TTL_MS: i64 = 15 * 60 * 1_000;

pub(super) fn checked_limit(limit: Option<usize>) -> Result<usize> {
    let limit = limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(Error::new("QUERY_LIMIT", "分页大小需要在 1 到 100 之间。"));
    }
    Ok(limit)
}

#[derive(Debug, Serialize)]
pub struct IntegrationQueryPage<T> {
    pub items: Vec<T>,
    pub snapshot_revision: String,
    pub next_cursor: Option<String>,
    #[serde(skip)]
    pub pdf_index: Option<PdfProgress>,
}

pub(super) struct Pagination {
    key: Vec<u8>,
    snapshot: String,
    expires_at: i64,
    pub limit: usize,
    pub offset: usize,
}

impl Pagination {
    pub fn new(
        db: &Connection,
        access: &Access,
        limit: Option<usize>,
        cursor: Option<&str>,
        query: &impl Serialize,
    ) -> Result<Self> {
        let limit = checked_limit(limit)?;
        let (sequence, key): (i64, Vec<u8>) = db.query_row(
            "SELECT sequence, query_cursor_key FROM workspace_state WHERE slot = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let mut stamp = signer(&key, b"zhiye.query.snapshot.v1");
        stamp.update(&serde_json::to_vec(&(&access.id, sequence, limit, query))?);
        let snapshot = URL_SAFE_NO_PAD.encode(stamp.finalize().into_bytes());
        let mut paging = Self {
            key,
            snapshot,
            expires_at: now() + CURSOR_TTL_MS,
            limit,
            offset: 0,
        };
        if let Some(cursor) = cursor {
            paging.read_cursor(cursor)?;
        }
        Ok(paging)
    }

    fn read_cursor(&mut self, cursor: &str) -> Result<()> {
        if cursor.len() > 128 {
            return Err(invalid_cursor());
        }
        let mut parts = cursor.split('.');
        let (Some("1"), Some(offset), Some(expires), Some(tag), None) = (
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
            parts.next(),
        ) else {
            return Err(invalid_cursor());
        };
        let offset = offset.parse::<usize>().map_err(|_| invalid_cursor())?;
        let expires_at = expires.parse::<i64>().map_err(|_| invalid_cursor())?;
        if offset == 0 || expires_at <= now() {
            return Err(invalid_cursor());
        }
        let tag = URL_SAFE_NO_PAD.decode(tag).map_err(|_| invalid_cursor())?;
        self.cursor_mac(offset, expires_at)
            .verify_slice(&tag)
            .map_err(|_| invalid_cursor())?;
        self.offset = offset;
        self.expires_at = expires_at;
        Ok(())
    }

    fn cursor_mac(&self, offset: usize, expires_at: i64) -> Hmac<Sha256> {
        let mut mac = signer(&self.key, b"zhiye.query.cursor.v1");
        mac.update(self.snapshot.as_bytes());
        mac.update(&(offset as u64).to_be_bytes());
        mac.update(&expires_at.to_be_bytes());
        mac
    }

    pub fn finish<T>(self, mut items: Vec<T>) -> IntegrationQueryPage<T> {
        let next_cursor = if items.len() > self.limit {
            items.truncate(self.limit);
            let offset = self.offset + items.len();
            let tag = URL_SAFE_NO_PAD.encode(
                self.cursor_mac(offset, self.expires_at)
                    .finalize()
                    .into_bytes(),
            );
            Some(format!("1.{offset}.{}.{tag}", self.expires_at))
        } else {
            None
        };
        IntegrationQueryPage {
            items,
            snapshot_revision: self.snapshot,
            next_cursor,
            pdf_index: None,
        }
    }
}

fn signer(key: &[u8], domain: &[u8]) -> Hmac<Sha256> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts any key length");
    mac.update(domain);
    mac
}

fn invalid_cursor() -> Error {
    Error::new(
        "CURSOR_INVALID",
        "查询条件、工作空间或授权已更新，或游标已过期。请从第一页重新查询。",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_requires_a_live_expiration_even_with_a_valid_signature() {
        let at = |time| Pagination {
            key: vec![33; 32],
            snapshot: "same-snapshot".into(),
            expires_at: time,
            offset: 0,
            limit: 1,
        };
        let current = now();
        let expired = at(current - CURSOR_TTL_MS)
            .finish(vec![(), ()])
            .next_cursor
            .unwrap();
        let active = at(current + CURSOR_TTL_MS)
            .finish(vec![(), ()])
            .next_cursor
            .unwrap();
        let mut next = at(current + CURSOR_TTL_MS);
        assert_eq!(
            next.read_cursor(&expired).unwrap_err().code,
            "CURSOR_INVALID"
        );
        next.read_cursor(&active).unwrap();
        assert_eq!(next.offset, 1);
        assert_eq!(next.expires_at, current + CURSOR_TTL_MS);
    }
}
