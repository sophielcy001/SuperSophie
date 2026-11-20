-- SPDX-License-Identifier: AGPL-3.0-or-later

-- Route through a union of live and pending historical edges. The worker
-- compares the two actual snapshots, so an extra route cannot create an event.
WITH RECURSIVE
    dependencies(collection_id, relation_id, input_id, output_id) AS MATERIALIZED (
        SELECT collection_id, relation_id, input_id, output_id FROM computed_dependencies
        UNION
        SELECT s.collection_id, json_extract(d.value, '$[0]'),
               json_extract(d.value, '$[1]'), json_extract(d.value, '$[2]')
        FROM computed_schema_before s JOIN json_each(s.dependencies) d
        WHERE s.job_id >= ?1
    ),
    previous_edges(page_id, collection_id, property_id, target_id) AS MATERIALIZED (
        SELECT DISTINCT s.page_id, json_extract(s.state, '$.collection'), f.key, member.value
        FROM computed_record_before s
        JOIN json_each(s.state, '$.properties') f
        JOIN json_each(CASE WHEN f.type='array' THEN f.value ELSE '[]' END) member
        WHERE s.job_id >= ?1 AND member.type='text'
    ),
    calculated_collections(id) AS MATERIALIZED (
        SELECT c.id FROM collections c WHERE EXISTS (
            SELECT 1 FROM json_each(c.properties) f WHERE json_type(f.value,'$.calculation')='object'
        )
        UNION
        SELECT s.collection_id FROM computed_schema_before s WHERE s.job_id>=?1 AND EXISTS (
            SELECT 1 FROM json_each(s.properties) f WHERE json_type(f.value,'$.calculation')='object'
        )
    ),
    affected(page_id, field_id) AS (
        SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]')
        FROM json_each(?2, '$.records')
        UNION
        SELECT r.page_id, json_extract(s.value, '$[1]')
        FROM json_each(?2, '$.schema') s
        JOIN collection_records r ON r.collection_id=json_extract(s.value, '$[0]')
        UNION
        SELECT a.page_id, d.output_id
        FROM affected a JOIN collection_records r ON r.page_id=a.page_id
        JOIN dependencies d ON d.collection_id=r.collection_id
            AND d.relation_id='' AND (a.field_id='' OR d.input_id=a.field_id)
        UNION
        SELECT e.source_page_id, d.output_id
        FROM affected a JOIN relation_edges e ON e.target_page_id=a.page_id
        JOIN collection_records r ON r.page_id=e.source_page_id
        JOIN dependencies d ON d.collection_id=r.collection_id
            AND d.relation_id=e.property_id AND (a.field_id='' OR d.input_id=a.field_id)
        UNION
        SELECT e.page_id, d.output_id
        FROM affected a JOIN previous_edges e ON e.target_id=a.page_id
        JOIN dependencies d ON d.collection_id=e.collection_id
            AND d.relation_id=e.property_id AND (a.field_id='' OR d.input_id=a.field_id)
    )
INSERT INTO derived_job_computed(job_id,page_id,audience)
SELECT DISTINCT ?1, a.page_id, '[]'
FROM affected a JOIN pages p ON p.id=a.page_id AND p.trash_root_id IS NULL
JOIN collection_records r ON r.page_id=a.page_id
JOIN calculated_collections c ON c.id=r.collection_id
