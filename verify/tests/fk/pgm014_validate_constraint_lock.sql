-- @claim: VALIDATE CONSTRAINT uses SHARE UPDATE EXCLUSIVE lock (PGM014)
-- @claim: This allows reads and writes while validation is in progress
-- @min_version: 14

-- Setup
CREATE TABLE test_vp(id int PRIMARY KEY);
CREATE TABLE test_vc(id int, parent_id int);
INSERT INTO test_vp SELECT g FROM generate_series(1, 1000) g;
INSERT INTO test_vc SELECT g, g FROM generate_series(1, 1000) g;

-- Add FK NOT VALID first
ALTER TABLE test_vc ADD CONSTRAINT fk_valid
    FOREIGN KEY (parent_id) REFERENCES test_vp(id) NOT VALID;

-- Test 1: VALIDATE CONSTRAINT should allow INSERT (ShareUpdateExclusive vs RowExclusive)
SELECT assert_lock_allows(
    'ALTER TABLE test_vc VALIDATE CONSTRAINT fk_valid',
    'INSERT INTO test_vc VALUES (9999, 1)',
    'VALIDATE CONSTRAINT allows INSERT (ShareUpdateExclusive vs RowExclusive)'
);

-- Re-add NOT VALID for next test (drop and re-add since it's now validated)
ALTER TABLE test_vc DROP CONSTRAINT fk_valid;
ALTER TABLE test_vc ADD CONSTRAINT fk_valid2
    FOREIGN KEY (parent_id) REFERENCES test_vp(id) NOT VALID;

-- Test 2: VALIDATE CONSTRAINT should allow SELECT
SELECT assert_lock_allows(
    'ALTER TABLE test_vc VALIDATE CONSTRAINT fk_valid2',
    'SELECT count(*) FROM test_vc',
    'VALIDATE CONSTRAINT allows SELECT (ShareUpdateExclusive vs AccessShare)'
);

-- Cleanup
DROP TABLE test_vc;
DROP TABLE test_vp;
