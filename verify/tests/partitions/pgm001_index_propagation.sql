-- @claim: CREATE INDEX on partitioned parent propagates to all partitions (PGM001)
-- @min_version: 14

-- Setup: partitioned table with multiple partitions
CREATE TABLE test_part(id int, ts date, val text) PARTITION BY RANGE(ts);
CREATE TABLE test_part_2023 PARTITION OF test_part
    FOR VALUES FROM ('2023-01-01') TO ('2024-01-01');
CREATE TABLE test_part_2024 PARTITION OF test_part
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE test_part_2025 PARTITION OF test_part
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

INSERT INTO test_part VALUES (1, '2023-06-15', 'a');
INSERT INTO test_part VALUES (2, '2024-06-15', 'b');
INSERT INTO test_part VALUES (3, '2025-06-15', 'c');

-- Test 1: CREATE INDEX on parent propagates to all children
CREATE INDEX idx_part_val ON test_part(val);

-- Check parent has index
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_part' AND indexdef LIKE '%val%'),
    1::bigint,
    'Parent has index on val'
);

-- Check each partition got the index
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_part_2023' AND indexdef LIKE '%val%'),
    1::bigint,
    'Partition 2023 got propagated index on val'
);

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_part_2024' AND indexdef LIKE '%val%'),
    1::bigint,
    'Partition 2024 got propagated index on val'
);

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_part_2025' AND indexdef LIKE '%val%'),
    1::bigint,
    'Partition 2025 got propagated index on val'
);

-- Test 2: New partition added AFTER index creation also gets the index
CREATE TABLE test_part_2026 PARTITION OF test_part
    FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_part_2026' AND indexdef LIKE '%val%'),
    1::bigint,
    'Newly attached partition automatically gets existing parent index'
);

-- Cleanup
DROP TABLE test_part;
