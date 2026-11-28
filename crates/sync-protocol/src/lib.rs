// SPDX-License-Identifier: AGPL-3.0-or-later

//! Wire format for Zhiye personal sync (protocol 1).
//!
//! The service orders every accepted change. Clients send each object together
//! with the server version their edit was based on. A stale base version is a
//! conflict and never overwrites the newer server state.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PROTOCOL_VERSION: u32 = 1;
/// Maximum operations in one push request.
pub const MAX_PUSH_OPERATIONS: usize = 200;
/// Default and maximum number of changes in one pull page.
pub const DEFAULT_PULL_LIMIT: u32 = 200;
pub const MAX_PULL_LIMIT: u32 = 500;
/// Upper bound for the serialized blocks of one page, matching the local editor limit.
pub const MAX_PAGE_BLOCK_BYTES: usize = 1024 * 1024;
/// Upper bound for one request body.
pub const MAX_REQUEST_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_TITLE_CHARS: usize = 200;
pub const MAX_NAME_CHARS: usize = 64;
/// Largest attachment original, matching the local import limit.
pub const MAX_BLOB_BYTES: u64 = 100 * 1024 * 1024;
pub const MAX_ATTACHMENTS_PER_PAGE: usize = 1000;
/// Hashes per missing-blob query.
pub const MAX_BLOB_QUERY: usize = 1000;
/// Media types accepted for attachment originals.
pub const ATTACHMENT_MEDIA_TYPES: [&str; 7] = [
    "image/png",
    "image/jpeg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "application/pdf",
    "application/octet-stream",
];

/// The synchronized state of an ordinary page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncPage {
    pub parent_id: Option<String>,
    pub title: String,
    pub icon: String,
    pub blocks: Vec<Value>,
    pub trash_root_id: Option<String>,
    /// Attachments owned by the page. Originals are transferred separately by SHA-256.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SyncAttachment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<SyncCover>,
    /// Present when the page is a database record; the parent is the database's container page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record: Option<SyncRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncRecord {
    pub created_at: i64,
    /// Property values keyed by property ID, in the local storage format.
    pub properties: serde_json::Map<String, Value>,
}

/// A database definition. Its container page is synchronized as a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncCollection {
    pub container_page_id: String,
    /// Property definitions, in the local storage format.
    pub properties: Vec<Value>,
    /// Saved views, in the local storage format.
    pub views: Vec<Value>,
}

impl SyncCollection {
    pub fn validate(&self, object_id: &str) -> Result<(), Invalid> {
        if !valid_id(&self.container_page_id) || self.container_page_id == object_id {
            return Err(Invalid("数据库的容器页面标识无效。"));
        }
        if self
            .properties
            .iter()
            .chain(&self.views)
            .any(|item| !item.is_object())
        {
            return Err(Invalid("数据库定义的格式无效。"));
        }
        let bytes = serde_json::to_vec(self).map_err(|_| Invalid("数据库定义的格式无效。"))?;
        if bytes.len() > MAX_PAGE_BLOCK_BYTES {
            return Err(Invalid("数据库定义最大为 1 MiB。"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncAttachment {
    pub id: String,
    pub name: String,
    pub sha256: String,
    pub bytes: u64,
    pub media_type: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SyncCover {
    pub attachment_id: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BlobQuery {
    pub hashes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MissingBlobs {
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase", deny_unknown_fields)]
pub enum ObjectChange {
    Upsert { page: SyncPage },
    Collection { collection: SyncCollection },
    Delete,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushOperation {
    /// Client-chosen UUID. Replaying the same operation returns the stored result.
    pub op_id: String,
    pub object_id: String,
    /// Server version this change was based on; 0 creates a new object.
    pub base_version: u64,
    pub change: ObjectChange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushRequest {
    pub protocol: u32,
    pub operations: Vec<PushOperation>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteChange {
    pub seq: u64,
    pub object_id: String,
    pub version: u64,
    pub device_id: String,
    pub op_id: String,
    pub change: ObjectChange,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase", deny_unknown_fields)]
pub enum PushResult {
    #[serde(rename_all = "camelCase")]
    Accepted {
        op_id: String,
        object_id: String,
        version: u64,
        seq: u64,
    },
    /// The base version is stale. `current` is the latest server state, if any.
    #[serde(rename_all = "camelCase")]
    Conflict {
        op_id: String,
        object_id: String,
        current: Option<Box<RemoteChange>>,
    },
}

impl PushResult {
    pub fn op_id(&self) -> &str {
        match self {
            Self::Accepted { op_id, .. } | Self::Conflict { op_id, .. } => op_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PushResponse {
    pub results: Vec<PushResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PullResponse {
    pub changes: Vec<RemoteChange>,
    /// Pass as `since` in the next request.
    pub next_seq: u64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceDraft {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceDraft {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeviceInfo {
    pub id: String,
    pub name: String,
    pub created_at: i64,
    pub last_seen_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Returned once at registration. Only a digest of `token` is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IssuedDevice {
    pub device: DeviceInfo,
    pub token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HealthInfo {
    pub protocol: u32,
    pub service: String,
    pub version: String,
}

/// A protocol-level validation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid(pub &'static str);

impl std::fmt::Display for Invalid {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for Invalid {}

/// Lowercase hyphenated UUID, the identifier form used by Zhiye.
pub fn valid_id(id: &str) -> bool {
    uuid::Uuid::parse_str(id).is_ok_and(|value| value.to_string() == id)
}

/// Lowercase hexadecimal SHA-256.
pub fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Attachment names follow the local file-name rules.
pub fn valid_attachment_name(name: &str) -> bool {
    !name.trim().is_empty()
        && name.chars().count() <= 255
        && name != "."
        && name != ".."
        && !name
            .chars()
            .any(|c| c.is_control() || "/\\:*?\"<>|".contains(c))
}

/// A trimmed display name without control characters.
pub fn valid_name(name: &str, limit: usize) -> bool {
    let count = name.chars().count();
    name.trim() == name && count > 0 && count <= limit && !name.chars().any(char::is_control)
}

impl SyncPage {
    pub fn validate(&self, object_id: &str) -> Result<(), Invalid> {
        if !valid_name(&self.title, MAX_TITLE_CHARS) {
            return Err(Invalid("页面标题需要 1 至 200 个字符，且不能包含换行。"));
        }
        if self.icon.is_empty()
            || self.icon.chars().count() > 24
            || self.icon.chars().any(char::is_control)
        {
            return Err(Invalid("页面图标无效。"));
        }
        for id in [&self.parent_id, &self.trash_root_id].into_iter().flatten() {
            if !valid_id(id) {
                return Err(Invalid("页面引用的标识无效。"));
            }
        }
        if self.parent_id.as_deref() == Some(object_id) {
            return Err(Invalid("页面不能成为自己的上级。"));
        }
        if self.blocks.iter().any(|block| !block.is_object()) {
            return Err(Invalid("页面正文块格式无效。"));
        }
        let bytes =
            serde_json::to_vec(&self.blocks).map_err(|_| Invalid("页面正文块格式无效。"))?;
        if bytes.len() > MAX_PAGE_BLOCK_BYTES {
            return Err(Invalid("单页正文最大为 1 MiB。"));
        }
        if self.attachments.len() > MAX_ATTACHMENTS_PER_PAGE {
            return Err(Invalid("单页最多同步 1000 个附件。"));
        }
        let mut ids = std::collections::HashSet::new();
        for attachment in &self.attachments {
            if !valid_id(&attachment.id)
                || !ids.insert(attachment.id.as_str())
                || !valid_attachment_name(&attachment.name)
                || !valid_sha256(&attachment.sha256)
                || attachment.bytes > MAX_BLOB_BYTES
                || attachment.created_at < 0
                || !ATTACHMENT_MEDIA_TYPES.contains(&attachment.media_type.as_str())
            {
                return Err(Invalid("页面附件的元数据无效。"));
            }
        }
        if let Some(cover) = &self.cover
            && !self
                .attachments
                .iter()
                .any(|item| item.id == cover.attachment_id && item.media_type == cover.media_type)
        {
            return Err(Invalid("页面封面需要引用本页附件。"));
        }
        if let Some(record) = &self.record {
            if record.created_at < 0 || self.parent_id.is_none() {
                return Err(Invalid("数据库记录需要所属数据库的容器页面。"));
            }
            if record.properties.keys().any(|key| !valid_id(key)) {
                return Err(Invalid("记录属性的标识无效。"));
            }
        }
        Ok(())
    }

    /// Originals this page needs on the receiving side.
    pub fn blob_hashes(&self) -> impl Iterator<Item = &str> {
        self.attachments.iter().map(|item| item.sha256.as_str())
    }
}

fn change_hashes(change: &ObjectChange) -> impl Iterator<Item = &str> {
    match change {
        ObjectChange::Upsert { page } => Some(page.blob_hashes()),
        ObjectChange::Collection { .. } | ObjectChange::Delete => None,
    }
    .into_iter()
    .flatten()
}

impl PushRequest {
    /// Distinct originals referenced by this request, in first-seen order.
    pub fn blob_hashes(&self) -> Vec<String> {
        distinct(
            self.operations
                .iter()
                .flat_map(|operation| change_hashes(&operation.change)),
        )
    }
}

impl PullResponse {
    /// Distinct originals referenced by these changes, in first-seen order.
    pub fn blob_hashes(&self) -> Vec<String> {
        distinct(
            self.changes
                .iter()
                .flat_map(|change| change_hashes(&change.change)),
        )
    }
}

fn distinct<'a>(hashes: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    hashes
        .filter(|hash| seen.insert(*hash))
        .map(str::to_owned)
        .collect()
}

impl PushOperation {
    pub fn validate(&self) -> Result<(), Invalid> {
        if !valid_id(&self.op_id) || !valid_id(&self.object_id) {
            return Err(Invalid("操作或对象标识无效。"));
        }
        match &self.change {
            ObjectChange::Upsert { page } => page.validate(&self.object_id),
            ObjectChange::Collection { collection } => collection.validate(&self.object_id),
            ObjectChange::Delete if self.base_version == 0 => {
                Err(Invalid("删除操作需要已同步的对象版本。"))
            }
            ObjectChange::Delete => Ok(()),
        }
    }
}

impl PushRequest {
    pub fn validate(&self) -> Result<(), Invalid> {
        if self.protocol != PROTOCOL_VERSION {
            return Err(Invalid("同步协议版本不受支持。"));
        }
        if self.operations.is_empty() || self.operations.len() > MAX_PUSH_OPERATIONS {
            return Err(Invalid("每次推送需要 1 至 200 项操作。"));
        }
        let mut seen = std::collections::HashSet::new();
        for operation in &self.operations {
            operation.validate()?;
            if !seen.insert(operation.op_id.as_str()) {
                return Err(Invalid("同一请求中的操作标识重复。"));
            }
        }
        let mut objects = std::collections::HashSet::new();
        if !self
            .operations
            .iter()
            .all(|operation| objects.insert(operation.object_id.as_str()))
        {
            return Err(Invalid("同一请求中每个对象只能出现一次。"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> SyncPage {
        SyncPage {
            parent_id: None,
            title: "读书笔记".into(),
            icon: "📄".into(),
            blocks: vec![json!({"id": "x", "type": "paragraph"})],
            trash_root_id: None,
            attachments: vec![],
            cover: None,
            record: None,
        }
    }

    fn attachment(sha: char) -> SyncAttachment {
        SyncAttachment {
            id: uuid::Uuid::new_v4().to_string(),
            name: "海湾.png".into(),
            sha256: sha.to_string().repeat(64),
            bytes: 12,
            media_type: "image/png".into(),
            created_at: 1,
        }
    }

    #[test]
    fn validates_records_and_collections() {
        let mut record = page();
        record.parent_id = Some("00000000-0000-4000-8000-000000000001".into());
        record.record = Some(SyncRecord {
            created_at: 5,
            properties: serde_json::Map::from_iter([(
                "00000000-0000-4000-8000-000000000003".to_owned(),
                json!(12.5),
            )]),
        });
        assert!(
            record
                .validate("00000000-0000-4000-8000-000000000002")
                .is_ok()
        );
        let mut orphan = record.clone();
        orphan.parent_id = None;
        assert!(
            orphan
                .validate("00000000-0000-4000-8000-000000000002")
                .is_err()
        );
        let mut bad_key = record.clone();
        bad_key
            .record
            .as_mut()
            .unwrap()
            .properties
            .insert("状态".into(), json!("x"));
        assert!(
            bad_key
                .validate("00000000-0000-4000-8000-000000000002")
                .is_err()
        );

        let collection = SyncCollection {
            container_page_id: "00000000-0000-4000-8000-000000000001".into(),
            properties: vec![json!({"id": "p", "type": "number"})],
            views: vec![json!({"id": "v"})],
        };
        let change = serde_json::to_value(ObjectChange::Collection {
            collection: collection.clone(),
        })
        .unwrap();
        assert_eq!(change["action"], "collection");
        assert!(
            collection
                .validate("00000000-0000-4000-8000-000000000009")
                .is_ok()
        );
        assert!(
            collection
                .validate("00000000-0000-4000-8000-000000000001")
                .is_err()
        );
        let mut broken = collection;
        broken.views.push(json!(1));
        assert!(
            broken
                .validate("00000000-0000-4000-8000-000000000009")
                .is_err()
        );
    }

    #[test]
    fn validates_attachments_and_lists_blobs() {
        let mut with_files = page();
        with_files.attachments = vec![attachment('a'), attachment('b'), attachment('a')];
        with_files.cover = Some(SyncCover {
            attachment_id: with_files.attachments[1].id.clone(),
            media_type: "image/png".into(),
        });
        let operation = operation(
            ObjectChange::Upsert {
                page: with_files.clone(),
            },
            0,
        );
        assert!(operation.validate().is_ok());
        let request = PushRequest {
            protocol: PROTOCOL_VERSION,
            operations: vec![operation],
        };
        assert_eq!(request.blob_hashes(), ["a".repeat(64), "b".repeat(64)]);

        let mut bad = with_files.clone();
        bad.cover = Some(SyncCover {
            attachment_id: uuid::Uuid::new_v4().to_string(),
            media_type: "image/png".into(),
        });
        assert!(
            bad.validate("00000000-0000-4000-8000-000000000009")
                .is_err()
        );
        for broken in [
            |item: &mut SyncAttachment| item.sha256 = "A".repeat(64),
            |item: &mut SyncAttachment| item.name = "a/b".into(),
            |item: &mut SyncAttachment| item.media_type = "text/html".into(),
            |item: &mut SyncAttachment| item.bytes = MAX_BLOB_BYTES + 1,
        ] {
            let mut bad = with_files.clone();
            broken(&mut bad.attachments[0]);
            assert!(
                bad.validate("00000000-0000-4000-8000-000000000009")
                    .is_err()
            );
        }
        // Pages without attachments keep the original wire form.
        let plain = serde_json::to_value(page()).unwrap();
        assert!(plain.get("attachments").is_none() && plain.get("cover").is_none());
    }

    fn operation(change: ObjectChange, base_version: u64) -> PushOperation {
        PushOperation {
            op_id: uuid::Uuid::new_v4().to_string(),
            object_id: uuid::Uuid::new_v4().to_string(),
            base_version,
            change,
        }
    }

    #[test]
    fn wire_format_is_stable_camel_case() {
        let op = PushOperation {
            op_id: "00000000-0000-4000-8000-000000000001".into(),
            object_id: "00000000-0000-4000-8000-000000000002".into(),
            base_version: 3,
            change: ObjectChange::Delete,
        };
        assert_eq!(
            serde_json::to_value(&op).unwrap(),
            json!({
                "opId": "00000000-0000-4000-8000-000000000001",
                "objectId": "00000000-0000-4000-8000-000000000002",
                "baseVersion": 3,
                "change": {"action": "delete"}
            })
        );
        let accepted = PushResult::Accepted {
            op_id: "a".into(),
            object_id: "b".into(),
            version: 1,
            seq: 9,
        };
        assert_eq!(
            serde_json::to_value(&accepted).unwrap(),
            json!({"status": "accepted", "opId": "a", "objectId": "b", "version": 1, "seq": 9})
        );
        let upsert = serde_json::to_value(ObjectChange::Upsert { page: page() }).unwrap();
        assert_eq!(upsert["action"], "upsert");
        assert_eq!(upsert["page"]["trashRootId"], Value::Null);
    }

    #[test]
    fn rejects_invalid_requests() {
        let valid = operation(ObjectChange::Upsert { page: page() }, 0);
        assert!(valid.validate().is_ok());
        assert!(operation(ObjectChange::Delete, 0).validate().is_err());

        let mut untrimmed = page();
        untrimmed.title = " 标题".into();
        assert!(
            operation(ObjectChange::Upsert { page: untrimmed }, 0)
                .validate()
                .is_err()
        );

        let mut own_parent = valid.clone();
        if let ObjectChange::Upsert { page } = &mut own_parent.change {
            page.parent_id = Some(own_parent.object_id.clone());
        }
        assert!(own_parent.validate().is_err());

        let mut huge = page();
        huge.blocks = vec![json!({"text": "字".repeat(MAX_PAGE_BLOCK_BYTES / 3 + 1)})];
        assert!(
            operation(ObjectChange::Upsert { page: huge }, 0)
                .validate()
                .is_err()
        );

        let request = |operations| PushRequest {
            protocol: PROTOCOL_VERSION,
            operations,
        };
        assert!(request(vec![]).validate().is_err());
        assert!(request(vec![valid.clone()]).validate().is_ok());
        let mut same_object = operation(ObjectChange::Upsert { page: page() }, 0);
        same_object.object_id = valid.object_id.clone();
        assert!(
            request(vec![valid.clone(), same_object])
                .validate()
                .is_err()
        );
        assert!(
            PushRequest {
                protocol: 2,
                operations: vec![valid]
            }
            .validate()
            .is_err()
        );
    }
}
