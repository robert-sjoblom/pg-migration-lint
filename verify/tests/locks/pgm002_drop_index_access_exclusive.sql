-- @claim: DROP INDEX acquires ACCESS EXCLUSIVE lock — blocks reads AND writes (PGM002)
-- @min_version: 14

-- Setup
CREATE TABLE test_di(id int, val text);
INSERT INTO test_di SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
CREATE INDEX idx_di_val ON test_di(val);

-- Test 1: DROP INDEX should block INSERT
SELECT assert_lock_blocks(
    'DROP INDEX idx_di_val',
    'INSERT INTO test_di VALUES (999999, ''probe'')',
    'DROP INDEX blocks INSERT (AccessExclusive vs RowExclusive)'
);

-- Recreate for next test
CREATE INDEX idx_di_val ON test_di(val);

-- Test 2: DROP INDEX should block SELECT (ACCESS EXCLUSIVE blocks everything)
SELECT assert_lock_blocks(
    'DROP INDEX idx_di_val',
    'SELECT count(*) FROM test_di',
    'DROP INDEX blocks SELECT (AccessExclusive vs AccessShare)'
);

-- Cleanup
DROP TABLE test_di;
