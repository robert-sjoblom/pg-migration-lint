-- @claim: ADD UNIQUE acquires ACCESS EXCLUSIVE lock and builds a new index (PGM017)
-- @claim: ADD UNIQUE USING INDEX reuses existing index
-- @min_version: 14

-- Setup
CREATE TABLE test_uq(id int, val text);
INSERT INTO test_uq SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: ADD UNIQUE blocks SELECT (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'ALTER TABLE test_uq ADD CONSTRAINT uq_val UNIQUE (val)',
    'SELECT count(*) FROM test_uq',
    'ADD UNIQUE blocks SELECT (AccessExclusive)'
);

ALTER TABLE test_uq DROP CONSTRAINT IF EXISTS uq_val;

-- Test 2: ADD UNIQUE blocks INSERT
SELECT assert_lock_blocks(
    'ALTER TABLE test_uq ADD CONSTRAINT uq_val2 UNIQUE (val)',
    'INSERT INTO test_uq VALUES (999999, ''probe_unique'')',
    'ADD UNIQUE blocks INSERT (AccessExclusive)'
);

ALTER TABLE test_uq DROP CONSTRAINT IF EXISTS uq_val2;

-- Test 3: ADD UNIQUE USING INDEX reuses existing index
CREATE UNIQUE INDEX idx_uq_val ON test_uq(val);

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_uq'),
    1::bigint,
    'One index before ADD UNIQUE USING INDEX'
);

ALTER TABLE test_uq ADD CONSTRAINT uq_reuse UNIQUE USING INDEX idx_uq_val;

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_uq'),
    1::bigint,
    'ADD UNIQUE USING INDEX reuses existing index (still 1 index)'
);

-- Cleanup
DROP TABLE test_uq;
