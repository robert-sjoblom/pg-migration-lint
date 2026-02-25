-- @claim: CREATE INDEX acquires SHARE lock — blocks writes, allows reads (PGM001)
-- @min_version: 14

-- Setup: create a table with enough rows for the index build to hold the lock
CREATE TABLE test_ci(id int, val text);
INSERT INTO test_ci SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: CREATE INDEX should block INSERT (SHARE conflicts with RowExclusiveLock)
SELECT assert_lock_blocks(
    'CREATE INDEX idx_ci_val ON test_ci(val)',
    'INSERT INTO test_ci VALUES (999999, ''probe'')',
    'CREATE INDEX blocks INSERT (SHARE vs RowExclusive)'
);

-- Clean up for next test
DROP INDEX IF EXISTS idx_ci_val;

-- Test 2: CREATE INDEX should block UPDATE
SELECT assert_lock_blocks(
    'CREATE INDEX idx_ci_val2 ON test_ci(val)',
    'UPDATE test_ci SET val = ''changed'' WHERE id = 1',
    'CREATE INDEX blocks UPDATE (SHARE vs RowExclusive)'
);

DROP INDEX IF EXISTS idx_ci_val2;

-- Test 3: CREATE INDEX should allow SELECT (SHARE compatible with AccessShareLock)
SELECT assert_lock_allows(
    'CREATE INDEX idx_ci_val3 ON test_ci(val)',
    'SELECT count(*) FROM test_ci',
    'CREATE INDEX allows SELECT (SHARE vs AccessShare)'
);

DROP INDEX IF EXISTS idx_ci_val3;

-- Cleanup
DROP TABLE test_ci;
