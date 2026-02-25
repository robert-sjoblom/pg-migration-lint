-- @claim: CLUSTER acquires ACCESS EXCLUSIVE lock and rewrites table (PGM018)
-- @min_version: 14

-- Setup
CREATE TABLE test_cl(id int, val text);
INSERT INTO test_cl SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
CREATE INDEX idx_cl_val ON test_cl(val);

-- Test 1: CLUSTER blocks SELECT (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'CLUSTER test_cl USING idx_cl_val',
    'SELECT count(*) FROM test_cl',
    'CLUSTER blocks SELECT (AccessExclusive)'
);

-- Test 2: CLUSTER blocks INSERT
SELECT assert_lock_blocks(
    'CLUSTER test_cl USING idx_cl_val',
    'INSERT INTO test_cl VALUES (999999, ''probe'')',
    'CLUSTER blocks INSERT (AccessExclusive)'
);

-- Cleanup
DROP TABLE test_cl;
