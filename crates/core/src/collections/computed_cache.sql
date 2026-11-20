-- SPDX-License-Identifier: AGPL-3.0-or-later

CREATE TABLE computed_jobs (
    job_id INTEGER PRIMARY KEY REFERENCES derived_jobs(id) ON DELETE CASCADE,
    seeds TEXT NOT NULL,
    observers TEXT NOT NULL,
    source_sequence INTEGER NOT NULL,
    expanded INTEGER NOT NULL DEFAULT 0 CHECK (expanded IN (0, 1))
);
CREATE INDEX computed_jobs_sequence ON computed_jobs(source_sequence);

-- A NULL record state marks creation. Unchanged inputs can be read from
-- the live tables, and the first later change supplies the old state.
CREATE TABLE computed_record_before (
    page_id TEXT NOT NULL,
    job_id INTEGER NOT NULL REFERENCES computed_jobs(job_id) ON DELETE CASCADE,
    state TEXT,
    PRIMARY KEY (page_id, job_id)
) WITHOUT ROWID;
CREATE INDEX computed_record_before_job ON computed_record_before(job_id);

CREATE TABLE computed_schema_before (
    collection_id TEXT NOT NULL,
    job_id INTEGER NOT NULL REFERENCES computed_jobs(job_id) ON DELETE CASCADE,
    properties TEXT NOT NULL,
    dependencies TEXT NOT NULL,
    PRIMARY KEY (collection_id, job_id)
) WITHOUT ROWID;
CREATE INDEX computed_schema_before_job ON computed_schema_before(job_id);

CREATE TABLE computed_cache (
    page_id TEXT NOT NULL REFERENCES pages(id) ON DELETE CASCADE,
    reader TEXT NOT NULL,
    grant_revision INTEGER NOT NULL,
    range_revision INTEGER NOT NULL,
    source_sequence INTEGER NOT NULL,
    computed_at INTEGER NOT NULL,
    representation TEXT NOT NULL,
    values_json TEXT NOT NULL,
    inputs_json TEXT NOT NULL,
    input_hash TEXT NOT NULL,
    PRIMARY KEY (page_id, reader)
) WITHOUT ROWID;
