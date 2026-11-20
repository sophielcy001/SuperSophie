// SPDX-License-Identifier: AGPL-3.0-or-later

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};
use uuid::Uuid;

pub type Result<T> = std::result::Result<T, Error>;
pub const MAX_BUNDLE_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize)]
pub struct Error {
    pub code: &'static str,
    pub message: String,
}

impl Error {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn storage_full() -> Self {
        Self::new(
            "STORAGE_FULL",
            "存储空间不足。请释放空间后重试，或改用有空间的磁盘。",
        )
    }

    fn storage_capacity(kind: std::io::ErrorKind) -> Option<Self> {
        match kind {
            std::io::ErrorKind::StorageFull => Some(Self::storage_full()),
            std::io::ErrorKind::QuotaExceeded => Some(Self::new(
                "STORAGE_FULL",
                "存储配额不足。请释放配额后重试，或改用有空间的位置。",
            )),
            _ => None,
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::storage_capacity(error.kind())
            .unwrap_or_else(|| Self::new("IO", format!("无法读写本地文件。{error}")))
    }
}

impl From<zip::result::ZipError> for Error {
    fn from(error: zip::result::ZipError) -> Self {
        if let zip::result::ZipError::Io(source) = &error
            && let Some(capacity) = Self::storage_capacity(source.kind())
        {
            return capacity;
        }
        Self::new("ARCHIVE", format!("无法读取或写入工作空间包。{error}"))
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DiskFull) => Self::storage_full(),
            Some(rusqlite::ErrorCode::CannotOpen) => Self::new(
                "DATABASE_OPEN",
                format!("无法打开本地数据库或事务文件。请检查磁盘空间和目录写入权限。{error}"),
            ),
            _ => Self::new("DATABASE", format!("本地数据库操作失败。{error}")),
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        error
            .io_error_kind()
            .and_then(Self::storage_capacity)
            .unwrap_or_else(|| Self::new("FORMAT", format!("无法读取数据格式。{error}")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Workspace {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Block {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub schema_version: u32,
    pub payload: Value,
    #[serde(default)]
    pub children: Vec<Block>,
    #[serde(flatten)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub id: String,
    pub parent_id: Option<String>,
    pub title: String,
    pub revision: u32,
    pub updated_at: i64,
    pub icon: String,
    pub trash_root_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Page {
    #[serde(flatten)]
    pub info: PageInfo,
    pub blocks: Vec<Block>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub properties: Option<BTreeMap<String, Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub historical_properties: Vec<crate::PropertyDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<PageCover>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PageCover {
    pub attachment_id: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageDraft {
    pub title: String,
    pub blocks: Vec<Block>,
}

pub(crate) fn new_id() -> String {
    Uuid::new_v4().to_string()
}

pub(crate) fn valid_id(id: &str) -> bool {
    Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}

pub(crate) fn name(input: &str, limit: usize, default: Option<&str>) -> Result<String> {
    let trimmed = input.trim();
    let value = if trimmed.is_empty() {
        default.unwrap_or("")
    } else {
        trimmed
    };
    if value.is_empty() || value.chars().count() > limit || value.chars().any(char::is_control) {
        return Err(Error::new(
            "INVALID_NAME",
            format!("名称需要 1 至 {limit} 个字符，且不能包含换行。"),
        ));
    }
    Ok(value.to_owned())
}

pub(crate) fn validate_blocks(blocks: &[Block]) -> Result<HashSet<&str>> {
    fn visit<'a>(blocks: &'a [Block], ids: &mut HashSet<&'a str>, depth: usize) -> Result<()> {
        for block in blocks {
            if depth > 24
                || ids.len() >= 20_000
                || !valid_id(&block.id)
                || !ids.insert(&block.id)
                || block.kind.is_empty()
                || block.kind.chars().count() > 80
                || block.kind.chars().any(char::is_control)
                || block.schema_version == 0
                || !block.payload.is_object()
            {
                return Err(Error::new(
                    "INVALID_BLOCK",
                    "块标识、结构或数据超出当前格式的支持范围。",
                ));
            }
            visit(&block.children, ids, depth + 1)?;
        }
        Ok(())
    }

    let mut ids = HashSet::new();
    visit(blocks, &mut ids, 1)?;
    if serde_json::to_vec(blocks)?.len() > 1024 * 1024 {
        return Err(Error::new("PAGE_TOO_LARGE", "单页正文最大为 1 MiB。"));
    }
    Ok(ids)
}

pub(crate) fn valid_icon(icon: &str) -> bool {
    !icon.is_empty() && icon.chars().count() <= 24 && !icon.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::Error;
    use std::io::{self, ErrorKind, Write};

    #[test]
    fn json_writer_preserves_storage_capacity_errors() {
        struct FailingWriter(ErrorKind);

        impl Write for FailingWriter {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(self.0.into())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        for (kind, code, message) in [
            (ErrorKind::StorageFull, "STORAGE_FULL", "存储空间不足"),
            (ErrorKind::QuotaExceeded, "STORAGE_FULL", "存储配额不足"),
            (ErrorKind::PermissionDenied, "FORMAT", "无法读取数据格式"),
        ] {
            let source = serde_json::to_writer_pretty(FailingWriter(kind), &[1]).unwrap_err();
            let error = Error::from(source);
            assert_eq!(error.code, code, "{kind:?}");
            assert!(error.message.contains(message), "{error}");
        }

        let malformed = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        assert_eq!(Error::from(malformed).code, "FORMAT");
    }
}
