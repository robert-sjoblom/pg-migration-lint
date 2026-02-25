-- @claim: SET NOT NULL requires ACCESS EXCLUSIVE lock (PGM013)
-- @min_version: 14

-- Setup
CREATE TABLE test_snn(id int, val text);
INSERT INTO test_snn SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: SET NOT NULL blocks SELECT (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'ALTER TABLE test_snn ALTER COLUMN val SET NOT NULL',
    'SELECT count(*) FROM test_snn',
    'SET NOT NULL blocks SELECT (AccessExclusive)'
);

-- Reset
ALTER TABLE test_snn ALTER COLUMN val DROP NOT NULL;

-- Test 2: SET NOT NULL blocks INSERT
SELECT assert_lock_blocks(
    'ALTER TABLE test_snn ALTER COLUMN val SET NOT NULL',
    'INSERT INTO test_snn VALUES (999999, ''probe'')',
    'SET NOT NULL blocks INSERT (AccessExclusive)'
);

-- Cleanup
DROP TABLE test_snn;
