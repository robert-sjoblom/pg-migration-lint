-- @claim: ADD CHECK without NOT VALID acquires ACCESS EXCLUSIVE lock and scans table (PGM015)
-- @min_version: 14

-- Setup
CREATE TABLE test_chk(id int, val int);
INSERT INTO test_chk SELECT g, g FROM generate_series(1, 1000) g;

-- Test 1: ADD CHECK blocks SELECT (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'ALTER TABLE test_chk ADD CONSTRAINT chk_val CHECK (val > 0)',
    'SELECT count(*) FROM test_chk',
    'ADD CHECK blocks SELECT (AccessExclusive)'
);

ALTER TABLE test_chk DROP CONSTRAINT IF EXISTS chk_val;

-- Test 2: ADD CHECK blocks INSERT
SELECT assert_lock_blocks(
    'ALTER TABLE test_chk ADD CONSTRAINT chk_val2 CHECK (val > 0)',
    'INSERT INTO test_chk VALUES (9999, 1)',
    'ADD CHECK blocks INSERT (AccessExclusive)'
);

ALTER TABLE test_chk DROP CONSTRAINT IF EXISTS chk_val2;

-- Test 3: ADD CHECK NOT VALID should succeed even with violating data
INSERT INTO test_chk VALUES (9999, -1);  -- violates val > 0

DO $$
BEGIN
    ALTER TABLE test_chk ADD CONSTRAINT chk_nv CHECK (val > 0) NOT VALID;
    PERFORM assert_true(true, 'ADD CHECK NOT VALID succeeds with violating data');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_true(false, 'ADD CHECK NOT VALID succeeds with violating data');
END;
$$;

-- Verify constraint exists but not validated
SELECT assert_true(
    NOT (SELECT convalidated FROM pg_constraint WHERE conname = 'chk_nv'),
    'NOT VALID CHECK is marked unvalidated'
);

-- Cleanup
DROP TABLE test_chk;
