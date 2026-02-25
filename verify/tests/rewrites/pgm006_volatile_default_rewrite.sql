-- @claim: ADD COLUMN with volatile default forces table rewrite (PGM006)
-- @claim: ADD COLUMN with non-volatile default does NOT rewrite on PG11+ (lazy apply)
-- @min_version: 14

-- Setup rewrite detection
SELECT rewrite_trap_setup();

CREATE TABLE test_vd(id int, val text);
INSERT INTO test_vd SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: Non-volatile default should NOT rewrite (PG11+)
SELECT rewrite_trap_reset();
ALTER TABLE test_vd ADD COLUMN created_at timestamptz DEFAULT '2024-01-01'::timestamptz;

SELECT assert_false(
    rewrite_trap_fired(),
    'Non-volatile literal default does NOT rewrite (PG11+ lazy apply)'
);

-- Test 2: Volatile default (clock_timestamp) SHOULD rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_vd ADD COLUMN updated_at timestamptz DEFAULT clock_timestamp();

SELECT assert_true(
    rewrite_trap_fired(),
    'Volatile default (clock_timestamp) DOES rewrite'
);

-- Test 3: Volatile default (random) SHOULD rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_vd ADD COLUMN rand_val double precision DEFAULT random();

SELECT assert_true(
    rewrite_trap_fired(),
    'Volatile default (random) DOES rewrite'
);

-- Test 4: Volatile default (gen_random_uuid) SHOULD rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_vd ADD COLUMN uuid_val uuid DEFAULT gen_random_uuid();

SELECT assert_true(
    rewrite_trap_fired(),
    'Volatile default (gen_random_uuid) DOES rewrite'
);

-- Test 5: Non-volatile function default should NOT rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_vd ADD COLUMN zero_val int DEFAULT 0;

SELECT assert_false(
    rewrite_trap_fired(),
    'Non-volatile literal 0 default does NOT rewrite'
);

-- Cleanup
SELECT rewrite_trap_teardown();
DROP TABLE test_vd;
