-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Retain idempotency receipts until their normal expiry. The replay path
-- checks the resulting page again, so a retry cannot recreate deleted data.
DELETE FROM integration_proposals WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM revisions WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM block_ids WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM page_links WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);

DELETE FROM derived_job_references WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL)
    OR source_page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM derived_job_relations WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL)
    OR target_page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM derived_job_computed WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM computed_record_before WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM computed_schema_before WHERE collection_id IN (SELECT id FROM deleted_collections);

DELETE FROM feed_versions WHERE kind = 'asset' AND object_id IN (
    SELECT a.id FROM attachments a JOIN pages p ON p.id = a.page_id WHERE p.trash_root_id IS NOT NULL
);
DELETE FROM feed_versions WHERE kind = 'collection' AND object_id IN (SELECT id FROM deleted_collections);
DELETE FROM attachments WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM relation_edges WHERE source_page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM collection_records WHERE page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);

DELETE FROM import_receipts
WHERE (kind = 'csvExisting' AND destination IN (SELECT id FROM deleted_collections))
   OR (kind <> 'csvExisting' AND destination IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL));
DELETE FROM collections WHERE container_page_id IN (SELECT id FROM pages WHERE trash_root_id IS NOT NULL);
DELETE FROM pages WHERE trash_root_id IS NOT NULL;
