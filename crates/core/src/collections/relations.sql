-- SPDX-License-Identifier: AGPL-3.0-or-later

CREATE TABLE relation_edges (
    source_page_id TEXT NOT NULL REFERENCES collection_records(page_id),
    property_id TEXT NOT NULL,
    target_page_id TEXT NOT NULL REFERENCES collection_records(page_id) DEFERRABLE INITIALLY DEFERRED,
    PRIMARY KEY (source_page_id, property_id, target_page_id)
) WITHOUT ROWID;
CREATE INDEX relation_edges_target ON relation_edges(target_page_id, source_page_id, property_id);

CREATE TABLE derived_job_relations (
    job_id INTEGER NOT NULL REFERENCES derived_jobs(id) ON DELETE CASCADE,
    page_id TEXT NOT NULL,
    property_id TEXT NOT NULL,
    target_page_id TEXT NOT NULL,
    PRIMARY KEY (job_id, page_id, property_id, target_page_id)
) WITHOUT ROWID;

CREATE TRIGGER relation_edges_insert AFTER INSERT ON collection_records
WHEN EXISTS (SELECT 1 FROM json_each(new.properties)) BEGIN
    INSERT INTO relation_edges
    SELECT new.page_id, property.key, member.value
    FROM collections c JOIN json_each(c.properties) definition
    JOIN json_each(new.properties) property ON property.key = json_extract(definition.value, '$.id')
    JOIN json_each(CASE WHEN property.type = 'array' THEN property.value ELSE '[]' END) member
    WHERE c.id = new.collection_id AND json_extract(definition.value, '$.type') = 'relation';
END;

CREATE TRIGGER relation_edges_update AFTER UPDATE OF properties ON collection_records
WHEN old.properties <> new.properties BEGIN
    DELETE FROM relation_edges WHERE source_page_id = new.page_id;
    INSERT INTO relation_edges
    SELECT new.page_id, property.key, member.value
    FROM collections c JOIN json_each(c.properties) definition
    JOIN json_each(new.properties) property ON property.key = json_extract(definition.value, '$.id')
    JOIN json_each(CASE WHEN property.type = 'array' THEN property.value ELSE '[]' END) member
    WHERE c.id = new.collection_id AND json_extract(definition.value, '$.type') = 'relation';
END;
