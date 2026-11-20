// SPDX-License-Identifier: AGPL-3.0-or-later

use super::Access;
use crate::store::now;
use crate::{Error, Result};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use chacha20poly1305::{
    KeyInit, XChaCha20Poly1305, XNonce,
    aead::{Aead, Payload},
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

const DOMAIN: &[u8] = b"zhiye.resource-feed.v1";
const NONCE_BYTES: usize = 24;
const PAYLOAD_BYTES: usize = 384;
const TOKEN_BYTES: usize = NONCE_BYTES + PAYLOAD_BYTES + 16;
const TOKEN_LENGTH: usize = (TOKEN_BYTES * 4).div_ceil(3);
pub(super) const CATALOG_TTL: i64 = 15 * 60 * 1_000;
pub(super) const CHANGES_TTL: i64 = 31 * 24 * 60 * 60 * 1_000;

#[derive(Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub(super) enum Position {
    Catalog { kind: String, id: String },
    Changes { after: i64, sweep: Option<Sweep> },
}

#[derive(Deserialize, Serialize)]
pub(super) struct Sweep {
    pub high_water: i64,
    pub watermark: String,
}

#[derive(Deserialize, Serialize)]
struct Cursor {
    subject: String,
    grant: i64,
    range: i64,
    expires_at: i64,
    position: Position,
}

pub(super) struct CursorCodec<'a> {
    cipher: XChaCha20Poly1305,
    access: &'a Access,
}

impl<'a> CursorCodec<'a> {
    pub fn new(db: &Connection, access: &'a Access) -> Result<Self> {
        let key: Vec<u8> = db.query_row(
            "SELECT query_cursor_key FROM workspace_state WHERE slot = 1",
            [],
            |row| row.get(0),
        )?;
        Ok(Self {
            cipher: XChaCha20Poly1305::new_from_slice(&key).map_err(|_| crypto_error())?,
            access,
        })
    }

    pub fn seal(&self, position: Position, expires_at: i64) -> Result<String> {
        let mut payload = serde_json::to_vec(&Cursor {
            subject: self.access.id.clone(),
            grant: self.access.grant_revision,
            range: self.access.range_revision,
            expires_at,
            position,
        })?;
        if payload.len() > PAYLOAD_BYTES {
            return Err(crypto_error());
        }
        // Fixed length keeps the size of the private scan position out of the token.
        payload.resize(PAYLOAD_BYTES, b' ');
        let mut nonce = [0; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| crypto_error())?;
        let encrypted = self
            .cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: &payload,
                    aad: DOMAIN,
                },
            )
            .map_err(|_| crypto_error())?;
        let mut output = Vec::with_capacity(TOKEN_BYTES);
        output.extend_from_slice(&nonce);
        output.extend(encrypted);
        Ok(URL_SAFE_NO_PAD.encode(output))
    }

    pub fn open(&self, token: &str) -> Result<(Position, i64)> {
        if token.len() != TOKEN_LENGTH {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
        if bytes.len() != TOKEN_BYTES {
            return Err(invalid());
        }
        let (nonce, encrypted) = bytes.split_at(NONCE_BYTES);
        let plaintext = self
            .cipher
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: encrypted,
                    aad: DOMAIN,
                },
            )
            .map_err(|_| invalid())?;
        let cursor: Cursor = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;
        if cursor.subject != self.access.id
            || cursor.grant != self.access.grant_revision
            || cursor.range != self.access.range_revision
        {
            return Err(invalid());
        }
        if cursor.expires_at <= now() {
            return Err(expired());
        }
        Ok((cursor.position, cursor.expires_at))
    }
}

pub(super) fn invalid() -> Error {
    Error::new(
        "CURSOR_INVALID",
        "编目或变更游标无效，或授权范围已变化。请重新编目。",
    )
}

pub(super) fn expired() -> Error {
    Error::new(
        "CURSOR_EXPIRED",
        "编目或变更游标已超过可续读时间。请重新编目。",
    )
}

fn crypto_error() -> Error {
    Error::new("CURSOR_GENERATION", "无法生成工作空间游标。")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn authenticated_tokens_keep_a_fixed_length_and_check_expiry() {
        let access = Access {
            id: crate::model::new_id(),
            scopes: BTreeSet::new(),
            last_used_at: None,
            grant_revision: 2,
            range_revision: 1,
        };
        let codec = CursorCodec {
            cipher: XChaCha20Poly1305::new(&[42; 32].into()),
            access: &access,
        };
        let token = |after, expires_at| {
            codec
                .seal(Position::Changes { after, sweep: None }, expires_at)
                .unwrap()
        };
        let current = now();
        let expired_token = token(0, current - 1);
        let active = token(i64::MAX, current + CHANGES_TTL);
        assert_eq!(expired_token.len(), active.len());
        assert_eq!(
            codec.open(&expired_token).err().unwrap().code,
            "CURSOR_EXPIRED"
        );
        assert!(matches!(
            codec.open(&active).unwrap().0,
            Position::Changes {
                after: i64::MAX,
                ..
            }
        ));
        let mut altered = URL_SAFE_NO_PAD.decode(active).unwrap();
        altered[NONCE_BYTES] ^= 1;
        assert_eq!(
            codec
                .open(&URL_SAFE_NO_PAD.encode(altered))
                .err()
                .unwrap()
                .code,
            "CURSOR_INVALID"
        );
    }
}
