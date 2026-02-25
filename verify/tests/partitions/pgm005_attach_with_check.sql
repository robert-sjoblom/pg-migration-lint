-- @claim: ATTACH PARTITION without pre-validated CHECK scans entire child (PGM005)
-- @claim: Adding CHECK NOT VALID + VALIDATE + ATTACH avoids full scan under ACCESS EXCLUSIVE
-- @min_version: 14

-- Setup
CREATE TABLE test_att(id int, ts date) PARTITION BY RANGE(ts);
CREATE TABLE test_att_existing PARTITION OF test_att
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

-- Create standalone table to attach
CREATE TABLE test_att_new(id int, ts date);
INSERT INTO test_att_new SELECT g, '2025-06-15'::date
    FROM generate_series(1, 10000) g;

-- Test 1: ATTACH without pre-existing CHECK succeeds but must scan
-- (We can't easily prove the scan happens, but we verify it works)
ALTER TABLE test_att ATTACH PARTITION test_att_new
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_inherits WHERE inhparent = 'test_att'::regclass),
    2::bigint,
    'ATTACH PARTITION succeeds (2 partitions)'
);

-- Detach for next test
ALTER TABLE test_att DETACH PARTITION test_att_new;

-- Test 2: Safe 3-step pattern: CHECK NOT VALID → VALIDATE → ATTACH
-- Step 1: Add CHECK NOT VALID (instant, no scan)
ALTER TABLE test_att_new ADD CONSTRAINT chk_range
    CHECK (ts >= '2025-01-01' AND ts < '2026-01-01') NOT VALID;

-- Verify constraint exists but not validated
SELECT assert_true(
    NOT (SELECT convalidated FROM pg_constraint WHERE conname = 'chk_range'),
    'CHECK NOT VALID is marked as not validated'
);

-- Step 2: VALIDATE (scans with weaker lock)
ALTER TABLE test_att_new VALIDATE CONSTRAINT chk_range;

SELECT assert_true(
    (SELECT convalidated FROM pg_constraint WHERE conname = 'chk_range'),
    'VALIDATE CONSTRAINT marks it as validated'
);

-- Step 3: ATTACH — PostgreSQL recognizes the validated CHECK and skips scan
ALTER TABLE test_att ATTACH PARTITION test_att_new
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_inherits WHERE inhparent = 'test_att'::regclass),
    2::bigint,
    'ATTACH with pre-validated CHECK succeeds'
);

-- Verify all data accessible through parent
SELECT assert_eq(
    (SELECT count(*)::bigint FROM test_att WHERE ts >= '2025-01-01'),
    10000::bigint,
    'All child data accessible through parent after ATTACH'
);

-- Cleanup
DROP TABLE test_att;
