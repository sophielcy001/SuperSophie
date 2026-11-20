-- SPDX-License-Identifier: AGPL-3.0-or-later

-- A local edge has an empty relation_id. Count rollups also have a reverse
-- edge with an empty input_id, which listens to visibility transitions only.
CREATE TABLE computed_dependencies (
    collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    input_id TEXT NOT NULL,
    output_id TEXT NOT NULL,
    relation_id TEXT NOT NULL,
    PRIMARY KEY (collection_id, relation_id, input_id, output_id)
) WITHOUT ROWID;

CREATE TABLE derived_job_computed (
    job_id INTEGER NOT NULL REFERENCES derived_jobs(id) ON DELETE CASCADE,
    page_id TEXT NOT NULL,
    audience TEXT NOT NULL,
    PRIMARY KEY (job_id, page_id)
) WITHOUT ROWID;
