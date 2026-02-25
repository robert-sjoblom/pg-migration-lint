-- @claim: ADD COLUMN NOT NULL without DEFAULT fails if table has rows (PGM008)
-- @claim: ADD COLUMN NOT NULL DEFAULT <non-volatile> is safe on PG11+ (no rewrite)
-- @min_version: 14

-- Setup rewrite detection
SELECT rewrite_trap_setup();

CREATE TABLE test_acnn(id int, val text);
INSERT INTO test_acnn SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: ADD COLUMN NOT NULL without DEFAULT should fail on non-empty table
DO $$
BEGIN
    ALTER TABLE test_acnn ADD COLUMN new_col int NOT NULL;
    PERFORM assert_true(false, 'ADD COLUMN NOT NULL without DEFAULT should fail on non-empty table');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_true(true, 'ADD COLUMN NOT NULL without DEFAULT fails on non-empty table');
END;
$$;

-- Test 2: ADD COLUMN NOT NULL DEFAULT <literal> succeeds without rewrite (PG11+)
SELECT rewrite_trap_reset();
ALTER TABLE test_acnn ADD COLUMN status text NOT NULL DEFAULT 'active';

SELECT assert_false(
    rewrite_trap_fired(),
    'ADD COLUMN NOT NULL DEFAULT literal does NOT rewrite (PG11+ lazy apply)'
);

-- Verify column was added correctly
SELECT assert_eq(
    (SELECT status FROM test_acnn LIMIT 1),
    'active',
    'Default value is applied correctly to existing rows'
);

-- Test 3: ADD COLUMN NOT NULL DEFAULT <volatile> forces rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_acnn ADD COLUMN created_at timestamptz NOT NULL DEFAULT clock_timestamp();

SELECT assert_true(
    rewrite_trap_fired(),
    'ADD COLUMN NOT NULL DEFAULT volatile DOES rewrite'
);

-- Test 4: ADD COLUMN (nullable) with no default always succeeds (no rewrite)
SELECT rewrite_trap_reset();
ALTER TABLE test_acnn ADD COLUMN optional_col text;

SELECT assert_false(
    rewrite_trap_fired(),
    'ADD COLUMN (nullable, no default) does NOT rewrite'
);

-- Cleanup
SELECT rewrite_trap_teardown();
DROP TABLE test_acnn;
