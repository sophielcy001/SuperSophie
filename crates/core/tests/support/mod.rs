// SPDX-License-Identifier: AGPL-3.0-or-later

use rusqlite::Connection;

#[allow(dead_code)]
pub fn remove_v20_schema(db: &Connection) {
    db.execute_batch("ALTER TABLE integrations DROP COLUMN expires_at; PRAGMA user_version = 19;")
        .unwrap();
}

#[allow(dead_code)]
pub fn remove_v19_schema(db: &Connection) {
    remove_v20_schema(db);
    db.execute_batch(
        "DROP TRIGGER attachment_rescue_blob;
         DROP TRIGGER template_insert_rescue_blobs;
         DROP TRIGGER template_update_rescue_blobs;
         DROP TABLE blob_quarantine;
         DROP TABLE deleted_records;
         DROP TABLE deleted_collections;
         PRAGMA user_version = 18;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v18_schema(db: &Connection) {
    remove_v19_schema(db);
    db.execute_batch(
        "ALTER TABLE migration_batches RENAME TO previous_migration_batches;
         CREATE TABLE migration_batches (
             id TEXT PRIMARY KEY,
             session_id TEXT NOT NULL,
             kind TEXT NOT NULL CHECK (kind IN ('notionZip','obsidianVault')),
             source TEXT NOT NULL,
             status TEXT NOT NULL CHECK (status IN ('preparing','completed','failed','cancelled')),
             output_page_id TEXT REFERENCES pages(id) ON DELETE SET NULL,
             detail TEXT
         );
         INSERT INTO migration_batches SELECT * FROM previous_migration_batches ORDER BY rowid;
         DROP TABLE previous_migration_batches;
         PRAGMA user_version = 17;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v17_schema(db: &Connection) {
    remove_v18_schema(db);
    db.execute_batch("ALTER TABLE pages DROP COLUMN cover; PRAGMA user_version = 16;")
        .unwrap();
}

#[allow(dead_code)]
pub fn remove_v16_schema(db: &Connection) {
    remove_v17_schema(db);
    db.execute_batch(
        "DELETE FROM derived_job_computed WHERE job_id IN (SELECT job_id FROM computed_jobs);
         DROP TABLE computed_cache; DROP TABLE computed_schema_before;
         DROP TABLE computed_record_before; DROP TABLE computed_jobs;
         PRAGMA user_version = 15;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v15_schema(db: &Connection) {
    remove_v16_schema(db);
    db.execute_batch(
        "DROP TABLE migration_batches;
         ALTER TABLE import_receipts RENAME TO previous_import_receipts;
         CREATE TABLE import_receipts (
            kind TEXT NOT NULL CHECK (kind IN ('markdown', 'csvNew', 'csvExisting')),
            destination TEXT NOT NULL,
            name TEXT NOT NULL,
            sha256 TEXT NOT NULL,
            output_page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
            PRIMARY KEY (kind, destination, name, sha256)
         );
         INSERT INTO import_receipts SELECT * FROM previous_import_receipts ORDER BY rowid;
         DROP TABLE previous_import_receipts;
         PRAGMA user_version = 14;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v14_schema(db: &Connection) {
    remove_v15_schema(db);
    db.execute_batch(
        "DROP TABLE pdf_text_entries; DROP TABLE pdf_text_fts; DROP TABLE pdf_index_documents;
         DROP TABLE pdf_index_state; DROP INDEX attachments_pdf; PRAGMA user_version = 13;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v12_schema(db: &Connection) {
    remove_v14_schema(db);
    db.execute_batch(
        "DROP TABLE derived_job_computed; DROP TABLE computed_dependencies; PRAGMA user_version = 11;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v11_schema(db: &Connection) {
    remove_v12_schema(db);
    db.execute_batch(
        "DROP TRIGGER relation_edges_insert; DROP TRIGGER relation_edges_update;
         DROP TABLE derived_job_relations; DROP TABLE relation_edges; PRAGMA user_version = 10;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v10_schema(db: &Connection) {
    remove_v11_schema(db);
    db.execute_batch(
        "DROP TABLE derived_job_references; DROP TABLE derived_jobs;
         DROP TABLE feed_derived_audience; ALTER TABLE feed_events DROP COLUMN derived;
         PRAGMA user_version = 9;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v9_schema(db: &Connection) {
    remove_v10_schema(db);
    db.execute_batch("DROP TABLE page_links; ALTER TABLE block_ids DROP COLUMN reference_anchor; PRAGMA user_version = 8;")
        .unwrap();
}

#[allow(dead_code)]
pub fn remove_v8_schema(db: &Connection) {
    remove_v9_schema(db);
    drop_triggers(
        db,
        "name GLOB 'feed_*' OR name GLOB 'integration_grants_feed_*'",
    );
    db.execute_batch(
        "DROP TABLE integration_proposals;
         DROP TABLE feed_tombstones; DROP TABLE feed_events; DROP TABLE feed_versions;
         DROP TABLE feed_state;
         ALTER TABLE integrations DROP COLUMN grant_revision;
         ALTER TABLE integrations DROP COLUMN range_revision;
         PRAGMA user_version = 7;",
    )
    .unwrap();
}

#[allow(dead_code)]
pub fn remove_v7_schema(db: &Connection) {
    remove_v8_schema(db);
    drop_triggers(db, "name GLOB 'workspace_sequence_*'");
    db.execute_batch("DROP TABLE workspace_state; PRAGMA user_version = 6;")
        .unwrap();
}

fn drop_triggers(db: &Connection, predicate: &str) {
    let triggers = {
        let mut statement = db
            .prepare(&format!(
                "SELECT name FROM sqlite_schema WHERE type = 'trigger' AND ({predicate})",
            ))
            .unwrap();
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap()
    };
    for trigger in triggers {
        db.execute_batch(&format!("DROP TRIGGER {trigger}"))
            .unwrap();
    }
}
