// SPDX-License-Identifier: AGPL-3.0-or-later

use crate::{Error, Result, Store};
use rusqlite::{Connection, MAIN_DB, OpenFlags, Transaction, TransactionBehavior};
use std::{fs, path::Path};

pub(crate) const VERSION: u32 = 21;

pub(crate) fn version(db: &Connection) -> Result<u32> {
    Ok(db.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

pub(crate) fn apply(tx: &Transaction<'_>, current: u32, target: u32) -> Result<()> {
    let steps: [fn(&Transaction<'_>) -> Result<()>; VERSION as usize - 1] = [
        migrate_v1,
        migrate_v2,
        migrate_v3,
        migrate_v4,
        migrate_v5,
        migrate_v6,
        migrate_v7,
        crate::references::migrate,
        migrate_v9,
        migrate_v10,
        migrate_v11,
        migrate_v12,
        migrate_v13,
        migrate_v14,
        migrate_v15,
        migrate_v16,
        migrate_v17,
        migrate_v18,
        migrate_v19,
        migrate_v20,
    ];
    if current == 0 || current > target || target > VERSION {
        return Err(Error::new(
            "FORMAT_VERSION",
            "此版本的知页不能打开该工作空间。",
        ));
    }
    for step in &steps[current as usize - 1..target as usize - 1] {
        step(tx)?;
    }
    Ok(())
}

pub(crate) fn upgrade(db: &mut Connection, store: &Store, id: &str) -> Result<()> {
    let version = version(db)?;
    match version {
        VERSION => Ok(()),
        1..VERSION => {
            let saved = snapshot(db, store.data_root(), id, version)?;
            crate::backup::preserve_upgrade(store, id, version, &saved)?;
            let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
            if self::version(&tx)? != version {
                return Err(Error::new(
                    "FORMAT_VERSION",
                    "升级准备期间数据格式已经变化。",
                ));
            }
            apply(&tx, version, VERSION)?;
            tx.commit()?;
            Ok(())
        }
        _ => Err(Error::new(
            "FORMAT_VERSION",
            "此版本的知页不能打开该工作空间。",
        )),
    }
}

fn snapshot(db: &Connection, root: &Path, id: &str, version: u32) -> Result<std::path::PathBuf> {
    let backups = root.join("upgrade-backups");
    fs::create_dir_all(&backups)?;
    if fs::canonicalize(&backups)? != backups {
        return Err(Error::new(
            "WORKSPACE_PATH",
            "升级备份目录不能指向其他位置。",
        ));
    }
    let target = backups.join(format!("{id}-v{version}.sqlite"));
    let staging = tempfile::NamedTempFile::new_in(&backups)?;
    db.backup(
        MAIN_DB,
        staging.path(),
        None::<fn(rusqlite::backup::Progress)>,
    )?;
    let saved_version: u32 = Connection::open_with_flags(
        staging.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )?
    .query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if saved_version != version {
        return Err(Error::new(
            "FORMAT_VERSION",
            "升级备份期间数据格式已经变化。",
        ));
    }
    staging.as_file().sync_all()?;
    staging
        .persist(&target)
        .map_err(|error| Error::from(error.error))?;
    Ok(target)
}

pub(crate) fn migrate_v1(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE attachments (
        id TEXT PRIMARY KEY,
        page_id TEXT NOT NULL REFERENCES pages(id),
        name TEXT NOT NULL,
        sha256 TEXT NOT NULL CHECK(length(sha256) = 64),
        bytes INTEGER NOT NULL CHECK(bytes >= 0),
        media_type TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        UNIQUE(page_id, name, sha256)
    );
    CREATE INDEX attachment_page ON attachments(page_id);
    CREATE INDEX attachment_hash ON attachments(sha256);
    PRAGMA user_version = 2;",
    )?;
    Ok(())
}

pub(crate) fn migrate_v2(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE collections (
            id TEXT PRIMARY KEY,
            container_page_id TEXT NOT NULL UNIQUE REFERENCES pages(id),
            schema_revision INTEGER NOT NULL CHECK(schema_revision > 0),
            properties TEXT NOT NULL CHECK(json_type(properties) = 'array'),
            views TEXT NOT NULL CHECK(json_type(views) = 'array')
        );
        CREATE TABLE collection_records (
            page_id TEXT PRIMARY KEY REFERENCES pages(id),
            collection_id TEXT NOT NULL REFERENCES collections(id),
            created_at INTEGER NOT NULL CHECK(created_at >= 0),
            properties TEXT NOT NULL CHECK(json_type(properties) = 'object')
        );
        CREATE INDEX collection_record_membership ON collection_records(collection_id, page_id);
        PRAGMA user_version = 3;",
    )?;
    Ok(())
}

pub(crate) fn migrate_v3(tx: &Transaction<'_>) -> Result<()> {
    crate::search::install(tx)?;
    tx.pragma_update(None, "user_version", 4)?;
    Ok(())
}

pub(crate) fn migrate_v4(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE workspace_inbox (
            slot INTEGER PRIMARY KEY CHECK (slot = 1),
            page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE
        );
        INSERT INTO workspace_inbox (slot, page_id)
            SELECT 1, id FROM pages
            WHERE parent_id IS NULL AND icon = 'inbox' AND trash_root_id IS NULL
            ORDER BY rowid LIMIT 1;
        CREATE TABLE favorites (
            page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
            added_at INTEGER NOT NULL CHECK (added_at >= 0)
        );
        CREATE TABLE recent_pages (
            page_id TEXT PRIMARY KEY REFERENCES pages(id) ON DELETE CASCADE,
            visited_at INTEGER NOT NULL CHECK (visited_at >= 0)
        );
        CREATE TABLE import_receipts (
            kind TEXT NOT NULL CHECK (kind IN ('markdown', 'csvNew', 'csvExisting')),
            destination TEXT NOT NULL,
            name TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            output_page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
            PRIMARY KEY (kind, destination, name, sha256)
        );
        CREATE TABLE templates (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            collection_id TEXT REFERENCES collections(id) ON DELETE CASCADE,
            page_count INTEGER NOT NULL CHECK (page_count BETWEEN 1 AND 64),
            snapshot TEXT NOT NULL
        );
        PRAGMA user_version = 5;",
    )?;
    Ok(())
}

pub(crate) fn migrate_v5(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE integrations (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            token_hash BLOB NOT NULL UNIQUE CHECK(length(token_hash) = 32),
            fingerprint TEXT NOT NULL,
            scopes TEXT NOT NULL CHECK(json_type(scopes) = 'array'),
            created_at INTEGER NOT NULL,
            last_used_at INTEGER,
            revoked_at INTEGER
        );
        CREATE TABLE integration_grants (
            integration_id TEXT NOT NULL REFERENCES integrations(id),
            kind TEXT NOT NULL,
            resource_id TEXT NOT NULL,
            page_id TEXT REFERENCES pages(id) ON DELETE CASCADE,
            collection_id TEXT REFERENCES collections(id) ON DELETE CASCADE,
            PRIMARY KEY (integration_id, kind, resource_id),
            CHECK (
                (kind IN ('page', 'subtree') AND page_id IS NOT NULL AND
                    resource_id = page_id AND collection_id IS NULL)
                OR (kind = 'collection' AND collection_id IS NOT NULL AND
                    resource_id = collection_id AND page_id IS NULL)
            )
        );
        CREATE TABLE integration_operations (
            integration_id TEXT NOT NULL REFERENCES integrations(id),
            request_key TEXT NOT NULL,
            request_hash BLOB NOT NULL CHECK(length(request_hash) = 32),
            response TEXT NOT NULL CHECK(json_type(response) = 'object'),
            expires_at INTEGER NOT NULL,
            PRIMARY KEY (integration_id, request_key)
        );
        CREATE INDEX integration_operation_expiry ON integration_operations(expires_at);
        CREATE TABLE integration_revisions (
            page_id TEXT NOT NULL,
            revision INTEGER NOT NULL,
            integration_id TEXT NOT NULL REFERENCES integrations(id),
            operation_id TEXT NOT NULL,
            PRIMARY KEY (page_id, revision),
            FOREIGN KEY (page_id, revision) REFERENCES revisions(page_id, revision) ON DELETE CASCADE
        );
        PRAGMA user_version = 6;",
    )?;
    Ok(())
}

pub(crate) fn migrate_v6(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE workspace_state (
            slot INTEGER PRIMARY KEY CHECK(slot = 1),
            sequence INTEGER NOT NULL CHECK(typeof(sequence) = 'integer' AND sequence >= 0),
            query_cursor_key BLOB NOT NULL CHECK(length(query_cursor_key) = 32),
            max_attachment_mib INTEGER NOT NULL DEFAULT 100 CHECK(max_attachment_mib BETWEEN 1 AND 100)
         );",
    )?;
    let mut key = [0_u8; 32];
    getrandom::fill(&mut key)
        .map_err(|_| Error::new("TOKEN_GENERATION", "无法初始化工作空间游标。"))?;
    tx.execute(
        "INSERT INTO workspace_state (slot, sequence, query_cursor_key) VALUES (1, 0, ?1)",
        [&key[..]],
    )?;

    // These tables change externally readable data or its visibility in the
    // same transaction. Search index maintenance and last-used timestamps do not.
    for table in [
        "pages",
        "collections",
        "collection_records",
        "attachments",
        "integration_grants",
    ] {
        for (event, suffix) in [
            ("INSERT", "insert"),
            ("UPDATE", "update"),
            ("DELETE", "delete"),
        ] {
            tx.execute_batch(&format!(
                "CREATE TRIGGER workspace_sequence_{table}_{suffix} AFTER {event} ON {table} BEGIN
                     UPDATE workspace_state SET sequence = sequence + 1 WHERE slot = 1;
                 END;"
            ))?;
        }
    }
    tx.execute_batch(
        "CREATE TRIGGER workspace_sequence_integration_status AFTER UPDATE OF scopes, revoked_at ON integrations
         WHEN old.scopes IS NOT new.scopes OR old.revoked_at IS NOT new.revoked_at BEGIN
             UPDATE workspace_state SET sequence = sequence + 1 WHERE slot = 1;
         END;
         PRAGMA user_version = 7;",
    )?;
    Ok(())
}

pub(crate) fn migrate_v7(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(include_str!("integrations/feed.sql"))?;
    tx.execute_batch(
        "CREATE TABLE integration_proposals (
            id TEXT PRIMARY KEY,
            integration_id TEXT NOT NULL REFERENCES integrations(id),
            page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
            base_revision INTEGER NOT NULL,
            base_etag TEXT NOT NULL,
            review_revision INTEGER NOT NULL DEFAULT 1 CHECK (review_revision > 0),
            operations TEXT NOT NULL CHECK (json_type(operations) = 'object'),
            candidate TEXT NOT NULL CHECK (json_type(candidate) = 'object'),
            status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'conflict', 'accepted', 'rejected')),
            conflict TEXT,
            created_at INTEGER NOT NULL,
            expires_at INTEGER NOT NULL,
            accepted_revision INTEGER,
            FOREIGN KEY (page_id, base_revision) REFERENCES revisions(page_id, revision)
        );
        CREATE INDEX proposal_by_page ON integration_proposals(page_id, created_at);",
    )?;
    tx.pragma_update(None, "user_version", 8)?;
    Ok(())
}

pub(crate) fn migrate_v9(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(include_str!("integrations/derived.sql"))?;
    tx.pragma_update(None, "user_version", 10)?;
    Ok(())
}

pub(crate) fn migrate_v10(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(include_str!("collections/relations.sql"))?;
    tx.pragma_update(None, "user_version", 11)?;
    Ok(())
}

pub(crate) fn migrate_v11(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(include_str!("collections/computed.sql"))?;
    tx.pragma_update(None, "user_version", 12)?;
    Ok(())
}

pub(crate) fn migrate_v12(tx: &Transaction<'_>) -> Result<()> {
    crate::collections::views::migrate(tx)?;
    tx.pragma_update(None, "user_version", 13)?;
    Ok(())
}

pub(crate) fn migrate_v13(tx: &Transaction<'_>) -> Result<()> {
    crate::search::pdf::install(tx)?;
    tx.pragma_update(None, "user_version", 14)?;
    Ok(())
}

pub(crate) fn migrate_v14(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE import_receipts RENAME TO previous_import_receipts;
         CREATE TABLE import_receipts (
             kind TEXT NOT NULL CHECK (kind IN ('markdown','csvNew','csvExisting','notionZip','obsidianVault')),
             destination TEXT NOT NULL,
             name TEXT NOT NULL,
             sha256 TEXT NOT NULL,
             output_page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
             PRIMARY KEY (kind, destination, name, sha256)
         );
         INSERT INTO import_receipts SELECT * FROM previous_import_receipts ORDER BY rowid;
         DROP TABLE previous_import_receipts;
         CREATE TABLE migration_batches (
             id TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             kind TEXT NOT NULL CHECK (kind IN ('notionZip','obsidianVault')),
             source TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('preparing','completed','failed','cancelled')),
             output_page_id TEXT REFERENCES pages(id) ON DELETE SET NULL,
             detail TEXT
         );",
    )?;
    tx.pragma_update(None, "user_version", 15)?;
    Ok(())
}

pub(crate) fn migrate_v15(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(include_str!("collections/computed_cache.sql"))?;
    tx.pragma_update(None, "user_version", 16)?;
    Ok(())
}

fn migrate_v16(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch("ALTER TABLE pages ADD COLUMN cover TEXT;")?;
    tx.pragma_update(None, "user_version", 17)?;
    Ok(())
}

fn migrate_v17(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE migration_batches RENAME TO previous_migration_batches;
         CREATE TABLE migration_batches (
             id TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             kind TEXT NOT NULL CHECK (kind IN ('notionZip','obsidianVault','nativeContent')),
             source TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('preparing','completed','failed','cancelled')),
             output_page_id TEXT REFERENCES pages(id) ON DELETE SET NULL,
             detail TEXT
         );
         INSERT INTO migration_batches SELECT * FROM previous_migration_batches ORDER BY rowid;
         DROP TABLE previous_migration_batches;",
    )?;
    tx.pragma_update(None, "user_version", 18)?;
    Ok(())
}

fn migrate_v18(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "CREATE TABLE deleted_collections (
             id TEXT PRIMARY KEY NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE deleted_records (
             page_id TEXT PRIMARY KEY NOT NULL,
             collection_id TEXT NOT NULL
         ) WITHOUT ROWID;
         CREATE TABLE blob_quarantine (
             sha256 TEXT PRIMARY KEY NOT NULL CHECK(length(sha256) = 64),
             unreferenced_since INTEGER NOT NULL CHECK(unreferenced_since >= 0)
         ) WITHOUT ROWID;
         CREATE TRIGGER attachment_rescue_blob AFTER INSERT ON attachments BEGIN
             DELETE FROM blob_quarantine WHERE sha256 = new.sha256;
         END;
         CREATE TRIGGER template_insert_rescue_blobs AFTER INSERT ON templates BEGIN
             DELETE FROM blob_quarantine WHERE sha256 IN (
                 SELECT json_extract(a.value, '$.sha256')
                 FROM json_each(new.snapshot) p JOIN json_each(p.value, '$.attachments') a
             );
         END;
         CREATE TRIGGER template_update_rescue_blobs AFTER UPDATE OF snapshot ON templates BEGIN
             DELETE FROM blob_quarantine WHERE sha256 IN (
                 SELECT json_extract(a.value, '$.sha256')
                 FROM json_each(new.snapshot) p JOIN json_each(p.value, '$.attachments') a
             );
         END;",
    )?;
    tx.pragma_update(None, "user_version", 19)?;
    Ok(())
}

fn migrate_v19(tx: &Transaction<'_>) -> Result<()> {
    tx.execute_batch(
        "ALTER TABLE integrations ADD COLUMN expires_at INTEGER
         CHECK (expires_at IS NULL OR (typeof(expires_at) = 'integer' AND expires_at > created_at));",
    )?;
    tx.pragma_update(None, "user_version", 20)?;
    Ok(())
}

fn migrate_v20(tx: &Transaction<'_>) -> Result<()> {
    // New record revisions can preserve values with earlier property definitions.
    tx.pragma_update(None, "user_version", 21)?;
    Ok(())
}
