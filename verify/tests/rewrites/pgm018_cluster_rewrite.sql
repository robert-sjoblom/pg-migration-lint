-- @claim: CLUSTER rewrites entire table and all indexes (PGM018)
-- @min_version: 14

-- Note: CLUSTER does NOT fire the table_rewrite event trigger (that's only for ALTER TABLE).
-- Instead, we detect rewrite by checking that relfilenode changes.

CREATE TABLE test_cl_rw(id int, val text);
INSERT INTO test_cl_rw SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
CREATE INDEX idx_cl_rw ON test_cl_rw(val);

-- Record relfilenode before CLUSTER
CREATE TEMP TABLE _before AS
    SELECT relfilenode FROM pg_class WHERE relname = 'test_cl_rw';

CLUSTER test_cl_rw USING idx_cl_rw;

-- Record relfilenode after CLUSTER
CREATE TEMP TABLE _after AS
    SELECT relfilenode FROM pg_class WHERE relname = 'test_cl_rw';

-- Test 1: relfilenode should have changed (table was rewritten to new file)
SELECT assert_true(
    (SELECT b.relfilenode != a.relfilenode
     FROM _before b, _after a),
    'CLUSTER changes relfilenode (table rewritten to new file)'
);

-- Test 2: Data should be intact after rewrite
SELECT assert_eq(
    (SELECT count(*)::bigint FROM test_cl_rw),
    1000::bigint,
    'All rows preserved after CLUSTER'
);

-- Cleanup
DROP TABLE test_cl_rw;
