-- @claim: DETACH PARTITION acquires ACCESS EXCLUSIVE lock on parent (PGM004)
-- @claim: DETACH PARTITION CONCURRENTLY uses SHARE UPDATE EXCLUSIVE (PG14+)
-- @min_version: 14

-- Setup: partitioned table
CREATE TABLE test_part(id int, ts date) PARTITION BY RANGE(ts);
CREATE TABLE test_part_2024 PARTITION OF test_part
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
INSERT INTO test_part VALUES (1, '2024-06-15');

-- Test 1: Plain DETACH blocks SELECT on parent (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'ALTER TABLE test_part DETACH PARTITION test_part_2024',
    'SELECT count(*) FROM test_part',
    'DETACH PARTITION blocks SELECT (AccessExclusive)'
);

-- Reattach for next test
ALTER TABLE test_part ATTACH PARTITION test_part_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

-- Test 2: Plain DETACH blocks INSERT on parent
SELECT assert_lock_blocks(
    'ALTER TABLE test_part DETACH PARTITION test_part_2024',
    'INSERT INTO test_part VALUES (2, ''2024-07-01'')',
    'DETACH PARTITION blocks INSERT (AccessExclusive)'
);

-- Reattach
ALTER TABLE test_part ATTACH PARTITION test_part_2024
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

-- Test 3: DETACH CONCURRENTLY uses weaker lock — cannot be tested with
-- lock probes since CONCURRENTLY cannot run inside a transaction.
-- Instead, verify it works and that the partition is detached.
ALTER TABLE test_part DETACH PARTITION test_part_2024 CONCURRENTLY;

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_inherits
     WHERE inhparent = 'test_part'::regclass),
    0::bigint,
    'DETACH CONCURRENTLY successfully detaches partition'
);

-- Cleanup
DROP TABLE test_part_2024;
DROP TABLE test_part;
