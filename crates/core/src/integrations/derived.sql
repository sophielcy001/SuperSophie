-- SPDX-License-Identifier: AGPL-3.0-or-later

ALTER TABLE feed_events ADD COLUMN derived INTEGER NOT NULL DEFAULT 0
    CHECK (derived = 0 OR (derived = 1 AND kind = 'page' AND action = 'upsert'));

CREATE TABLE feed_derived_audience (
    seq INTEGER NOT NULL REFERENCES feed_events(seq) ON DELETE CASCADE,
    integration_id TEXT NOT NULL REFERENCES integrations(id) ON DELETE CASCADE,
    grant_revision INTEGER NOT NULL,
    range_revision INTEGER NOT NULL,
    PRIMARY KEY (seq, integration_id)
) WITHOUT ROWID;

CREATE TABLE derived_jobs (
    id INTEGER PRIMARY KEY,
    before_state TEXT NOT NULL,
    after_state TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    retry_at INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX derived_jobs_retry ON derived_jobs(retry_at, id);

CREATE TABLE derived_job_references (
    job_id INTEGER NOT NULL REFERENCES derived_jobs(id) ON DELETE CASCADE,
    page_id TEXT NOT NULL,
    block_id TEXT NOT NULL,
    source_page_id TEXT NOT NULL,
    source_block_id TEXT NOT NULL,
    PRIMARY KEY (job_id, page_id, block_id)
) WITHOUT ROWID;
