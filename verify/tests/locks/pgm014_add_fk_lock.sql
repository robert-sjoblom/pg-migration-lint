-- @claim: ADD FOREIGN KEY without NOT VALID acquires SHARE ROW EXCLUSIVE lock (PGM014)
-- @claim: ADD FK NOT VALID allows invalid data to exist, constraint marked unvalidated
-- @min_version: 14

-- Setup
CREATE TABLE test_parent(id int PRIMARY KEY);
CREATE TABLE test_child(id int, parent_id int);
INSERT INTO test_parent SELECT g FROM generate_series(1, 100) g;
INSERT INTO test_child SELECT g, g FROM generate_series(1, 100) g;

-- Test 1: ADD FK blocks INSERT on child table (ShareRowExclusive vs RowExclusive)
SELECT assert_lock_blocks(
    'ALTER TABLE test_child ADD CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES test_parent(id)',
    'INSERT INTO test_child VALUES (999, 1)',
    'ADD FK blocks INSERT on child (ShareRowExclusive)'
);

ALTER TABLE test_child DROP CONSTRAINT IF EXISTS fk_parent;

-- Test 2: ADD FK allows SELECT on child table (ShareRowExclusive vs AccessShare)
SELECT assert_lock_allows(
    'ALTER TABLE test_child ADD CONSTRAINT fk_parent2 FOREIGN KEY (parent_id) REFERENCES test_parent(id)',
    'SELECT count(*) FROM test_child',
    'ADD FK allows SELECT on child (ShareRowExclusive vs AccessShare)'
);

ALTER TABLE test_child DROP CONSTRAINT IF EXISTS fk_parent2;

-- Test 3: ADD FK NOT VALID succeeds even with invalid data
INSERT INTO test_child VALUES (999, 99999);  -- no matching parent

DO $$
BEGIN
    ALTER TABLE test_child ADD CONSTRAINT fk_nv FOREIGN KEY (parent_id)
        REFERENCES test_parent(id) NOT VALID;
    PERFORM assert_true(true, 'ADD FK NOT VALID succeeds with invalid data');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_true(false, 'ADD FK NOT VALID succeeds with invalid data');
END;
$$;

-- Test 4: NOT VALID constraint is marked as not validated in pg_constraint
SELECT assert_true(
    NOT (SELECT convalidated FROM pg_constraint WHERE conname = 'fk_nv'),
    'NOT VALID FK is marked as not validated in pg_constraint'
);

-- Cleanup
DROP TABLE test_child;
DROP TABLE test_parent;
