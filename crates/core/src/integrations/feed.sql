-- SPDX-License-Identifier: AGPL-3.0-or-later

ALTER TABLE integrations ADD COLUMN grant_revision INTEGER NOT NULL DEFAULT 1;
ALTER TABLE integrations ADD COLUMN range_revision INTEGER NOT NULL DEFAULT 1;

CREATE TRIGGER integration_grants_feed_insert AFTER INSERT ON integration_grants BEGIN
    UPDATE integrations SET grant_revision = grant_revision + 1 WHERE id = new.integration_id;
END;
CREATE TRIGGER integration_grants_feed_update AFTER UPDATE ON integration_grants BEGIN
    UPDATE integrations SET grant_revision = grant_revision + 1
    WHERE id IN (old.integration_id, new.integration_id);
END;
CREATE TRIGGER integration_grants_feed_delete AFTER DELETE ON integration_grants BEGIN
    UPDATE integrations SET grant_revision = grant_revision + 1 WHERE id = old.integration_id;
END;
CREATE TRIGGER integration_grants_feed_status AFTER UPDATE OF scopes, revoked_at ON integrations
WHEN old.scopes IS NOT new.scopes OR old.revoked_at IS NOT new.revoked_at BEGIN
    UPDATE integrations SET grant_revision = grant_revision + 1 WHERE id = new.id;
END;

CREATE TABLE feed_state (
    slot INTEGER PRIMARY KEY CHECK (slot = 1),
    floor INTEGER NOT NULL DEFAULT 0,
    pruned_at INTEGER NOT NULL
);
INSERT INTO feed_state (slot, pruned_at) VALUES (1, CAST(unixepoch('subsec') * 1000 AS INTEGER));

CREATE TABLE feed_versions (
    kind TEXT NOT NULL CHECK (kind IN ('asset', 'collection')),
    object_id TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision > 0),
    PRIMARY KEY (kind, object_id)
);

CREATE TABLE feed_events (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    event_id TEXT NOT NULL UNIQUE DEFAULT (lower(hex(randomblob(16)))),
    kind TEXT NOT NULL CHECK (kind IN ('asset', 'collection', 'page')),
    object_id TEXT NOT NULL,
    owner_page_id TEXT NOT NULL,
    action TEXT NOT NULL CHECK (action IN ('upsert', 'delete')),
    revision INTEGER NOT NULL CHECK (revision > 0),
    created_at INTEGER NOT NULL DEFAULT (CAST(unixepoch('subsec') * 1000 AS INTEGER))
);
CREATE INDEX feed_event_time ON feed_events(created_at, seq);
CREATE TABLE feed_tombstones (
    seq INTEGER NOT NULL REFERENCES feed_events(seq) ON DELETE CASCADE,
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    grant_revision INTEGER NOT NULL,
    range_revision INTEGER NOT NULL,
    PRIMARY KEY (seq, integration_id)
);

CREATE TRIGGER feed_record_version AFTER INSERT ON feed_events
WHEN new.kind <> 'page' BEGIN
    INSERT INTO feed_versions VALUES (new.kind, new.object_id, new.revision)
    ON CONFLICT (kind, object_id) DO UPDATE SET revision = excluded.revision;
END;

-- Keep at least 31 days. Prune once per day on a write. Keep a contiguous suffix so
-- a backwards clock adjustment cannot create a hole beyond the cursor floor.
CREATE TRIGGER feed_prune AFTER INSERT ON feed_events
WHEN new.created_at >= (SELECT pruned_at + 86400000 FROM feed_state WHERE slot = 1) BEGIN
    UPDATE feed_state
    SET floor = max(floor, (
        SELECT coalesce(min(seq), new.seq) - 1 FROM feed_events
        WHERE created_at >= new.created_at - 2678400000
    )), pruned_at = new.created_at
    WHERE slot = 1;
    DELETE FROM feed_events WHERE seq <= (SELECT floor FROM feed_state WHERE slot = 1);
END;

CREATE TRIGGER feed_page_insert AFTER INSERT ON pages WHEN new.trash_root_id IS NULL BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    VALUES ('page', new.id, new.id, 'upsert', new.revision);
END;
CREATE TRIGGER feed_page_update AFTER UPDATE OF revision ON pages
WHEN new.trash_root_id IS NULL AND old.revision <> new.revision BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    VALUES ('page', new.id, new.id, 'upsert', new.revision);
END;

CREATE TRIGGER feed_collection_insert AFTER INSERT ON collections
WHEN EXISTS (SELECT 1 FROM pages WHERE id = new.container_page_id AND trash_root_id IS NULL) BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    VALUES ('collection', new.id, new.container_page_id, 'upsert', 1);
END;
CREATE TRIGGER feed_collection_update AFTER UPDATE OF schema_revision ON collections
WHEN old.schema_revision <> new.schema_revision
 AND EXISTS (SELECT 1 FROM pages WHERE id = new.container_page_id AND trash_root_id IS NULL) BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    SELECT 'collection', new.id, new.container_page_id, 'upsert', coalesce(v.revision, 1) + 1
    FROM collections c LEFT JOIN feed_versions v ON v.kind = 'collection' AND v.object_id = c.id
    WHERE c.id = new.id;
END;
CREATE TRIGGER feed_collection_title AFTER UPDATE OF title, trash_root_id ON pages
WHEN new.trash_root_id IS NULL AND (old.trash_root_id IS NOT NULL OR old.title <> new.title) BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    SELECT 'collection', c.id, new.id, 'upsert', coalesce(v.revision, 1) + 1
    FROM collections c LEFT JOIN feed_versions v ON v.kind = 'collection' AND v.object_id = c.id
    WHERE c.container_page_id = new.id;
END;

CREATE TRIGGER feed_asset_insert AFTER INSERT ON attachments
WHEN EXISTS (SELECT 1 FROM pages WHERE id = new.page_id AND trash_root_id IS NULL) BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    VALUES ('asset', new.id, new.page_id, 'upsert', 1);
END;
CREATE TRIGGER feed_asset_update AFTER UPDATE OF name ON attachments
WHEN old.name <> new.name
 AND EXISTS (SELECT 1 FROM pages WHERE id = new.page_id AND trash_root_id IS NULL) BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    SELECT 'asset', new.id, new.page_id, 'upsert', coalesce(v.revision, 1) + 1
    FROM attachments a LEFT JOIN feed_versions v ON v.kind = 'asset' AND v.object_id = a.id
    WHERE a.id = new.id;
END;
CREATE TRIGGER feed_asset_restore AFTER UPDATE OF trash_root_id ON pages
WHEN old.trash_root_id IS NOT NULL AND new.trash_root_id IS NULL BEGIN
    INSERT INTO feed_events (kind, object_id, owner_page_id, action, revision)
    SELECT 'asset', a.id, new.id, 'upsert', coalesce(v.revision, 1) + 1
    FROM attachments a LEFT JOIN feed_versions v ON v.kind = 'asset' AND v.object_id = a.id
    WHERE a.page_id = new.id;
END;
