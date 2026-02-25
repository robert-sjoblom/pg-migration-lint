-- @claim: DETACH PARTITION CONCURRENTLY available in PG14+ (PGM004)
-- @claim: Regular DETACH requires ACCESS EXCLUSIVE on parent
-- @min_version: 14

-- Setup
CREATE TABLE test_dc(id int, ts date) PARTITION BY RANGE(ts);
CREATE TABLE test_dc_2024 PARTITION OF test_dc
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE test_dc_2025 PARTITION OF test_dc
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
INSERT INTO test_dc VALUES (1, '2024-06-15'), (2, '2025-06-15');

-- Test 1: DETACH CONCURRENTLY works (PG14+)
ALTER TABLE test_dc DETACH PARTITION test_dc_2024 CONCURRENTLY;

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_inherits WHERE inhparent = 'test_dc'::regclass),
    1::bigint,
    'DETACH CONCURRENTLY successfully removes partition (1 remaining)'
);

-- Test 2: Detached table still exists as standalone
SELECT assert_eq(
    (SELECT count(*)::bigint FROM test_dc_2024),
    1::bigint,
    'Detached partition data is preserved as standalone table'
);

-- Test 3: Parent no longer routes to detached partition
INSERT INTO test_dc VALUES (3, '2025-06-15');

SELECT assert_eq(
    (SELECT count(*)::bigint FROM test_dc),
    2::bigint,
    'Parent only has rows from remaining partition'
);

-- Cleanup
DROP TABLE test_dc_2024;
DROP TABLE test_dc;
