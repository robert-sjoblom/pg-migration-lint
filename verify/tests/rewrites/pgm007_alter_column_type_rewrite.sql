-- @claim: ALTER COLUMN TYPE causes table rewrite for most type changes (PGM007)
-- @claim: varchar widening and varchar->text are safe (no rewrite)
-- @min_version: 14

-- Setup rewrite detection
SELECT rewrite_trap_setup();

CREATE TABLE test_act(
    id int,
    name varchar(50),
    code varchar(10),
    amount numeric(10,2),
    bits bit(8),
    vbits varbit(8)
);
INSERT INTO test_act VALUES (1, 'test', 'ABC', 99.99, B'10101010', B'1010');

-- Test 1: varchar widening should NOT rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN name TYPE varchar(100);

SELECT assert_false(
    rewrite_trap_fired(),
    'varchar(50)->varchar(100) does NOT rewrite (safe widening)'
);

-- Test 2: varchar to text should NOT rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN code TYPE text;

SELECT assert_false(
    rewrite_trap_fired(),
    'varchar(10)->text does NOT rewrite (safe cast)'
);

-- Test 3: numeric precision widening should NOT rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN amount TYPE numeric(20,2);

SELECT assert_false(
    rewrite_trap_fired(),
    'numeric(10,2)->numeric(20,2) does NOT rewrite (safe widening)'
);

-- Test 4: int to bigint SHOULD rewrite
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN id TYPE bigint;

SELECT assert_true(
    rewrite_trap_fired(),
    'int->bigint DOES rewrite (different binary format)'
);

-- Test 5: text to int SHOULD rewrite (type change)
-- Note: must drop any default before ALTER TYPE, otherwise PG errors on auto-cast of default
ALTER TABLE test_act ADD COLUMN str_num text;
UPDATE test_act SET str_num = '42';
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN str_num TYPE int USING str_num::int;

SELECT assert_true(
    rewrite_trap_fired(),
    'text->int DOES rewrite'
);

-- Test 6: bit(n) widening ERRORS (fixed-width mismatch) — NOT safe
-- NOTE: This disproves PGM007's claim that bit(N)->bit(M) is safe.
-- bit(n) is fixed-width, so existing 8-bit values can't fit in 16-bit slots.
DO $$
BEGIN
    ALTER TABLE test_act ALTER COLUMN bits TYPE bit(16);
    PERFORM assert_true(false, 'bit(8)->bit(16) errors without USING (fixed-width mismatch)');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_true(true, 'bit(8)->bit(16) errors without USING (fixed-width mismatch)');
END;
$$;

-- Test 7: varbit widening should NOT rewrite (variable-width is safe)
SELECT rewrite_trap_reset();
ALTER TABLE test_act ALTER COLUMN vbits TYPE varbit(16);

SELECT assert_false(
    rewrite_trap_fired(),
    'varbit(8)->varbit(16) does NOT rewrite (safe widening)'
);

-- Cleanup
SELECT rewrite_trap_teardown();
DROP TABLE test_act;
