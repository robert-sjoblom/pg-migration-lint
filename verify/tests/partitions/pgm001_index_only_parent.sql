-- @claim: CREATE INDEX ON ONLY parent creates invalid parent-only index (PGM001)
-- @claim: Child indexes can be built CONCURRENTLY then attached to parent index
-- @min_version: 14

-- Setup
CREATE TABLE test_only(id int, ts date, val text) PARTITION BY RANGE(ts);
CREATE TABLE test_only_2024 PARTITION OF test_only
    FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE TABLE test_only_2025 PARTITION OF test_only
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

INSERT INTO test_only VALUES (1, '2024-06-15', 'a');
INSERT INTO test_only VALUES (2, '2025-06-15', 'b');

-- Test 1: CREATE INDEX ON ONLY creates parent index only
CREATE INDEX idx_only_val ON ONLY test_only(val);

-- Parent has the index
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_only' AND indexdef LIKE '%val%'),
    1::bigint,
    'ON ONLY creates index on parent'
);

-- Children do NOT have the index
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_only_2024' AND indexdef LIKE '%val%'),
    0::bigint,
    'ON ONLY does NOT create index on child partition 2024'
);

SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_only_2025' AND indexdef LIKE '%val%'),
    0::bigint,
    'ON ONLY does NOT create index on child partition 2025'
);

-- Test 2: Parent index should be marked as invalid (not ready)
SELECT assert_false(
    (SELECT indisvalid FROM pg_index WHERE indexrelid = 'idx_only_val'::regclass),
    'ON ONLY parent index is initially invalid'
);

-- Test 3: Build child indexes CONCURRENTLY, then attach
CREATE INDEX CONCURRENTLY idx_only_2024_val ON test_only_2024(val);
CREATE INDEX CONCURRENTLY idx_only_2025_val ON test_only_2025(val);

ALTER INDEX idx_only_val ATTACH PARTITION idx_only_2024_val;
ALTER INDEX idx_only_val ATTACH PARTITION idx_only_2025_val;

-- Now parent index should be valid
SELECT assert_true(
    (SELECT indisvalid FROM pg_index WHERE indexrelid = 'idx_only_val'::regclass),
    'Parent index becomes valid after all child indexes attached'
);

-- Cleanup
DROP TABLE test_only;
