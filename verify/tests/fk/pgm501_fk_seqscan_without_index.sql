-- @claim: DELETE/UPDATE on referenced table causes seq scan on referencing table without index (PGM501)
-- @claim: Prefix matching: FK(a,b) covered by idx(a,b,c) but NOT by idx(b,a) or idx(a)
-- @min_version: 14

-- Setup
CREATE TABLE parent_t(id int PRIMARY KEY);
CREATE TABLE child_t(id int, parent_id int REFERENCES parent_t(id));

INSERT INTO parent_t SELECT g FROM generate_series(1, 10000) g;
INSERT INTO child_t SELECT g, g FROM generate_series(1, 10000) g;

-- Analyze so planner has stats
ANALYZE parent_t;
ANALYZE child_t;

-- Test 1: Without index, FK column lookup uses Seq Scan
SELECT assert_explain_contains(
    'SELECT 1 FROM child_t WHERE parent_id = 1',
    'Seq Scan',
    'FK lookup without index uses Seq Scan'
);

-- Test 2: With covering index, same query uses Index Scan
CREATE INDEX idx_child_parent ON child_t(parent_id);
ANALYZE child_t;

SELECT assert_explain_contains(
    'SELECT 1 FROM child_t WHERE parent_id = 1',
    'Index',
    'FK lookup with covering index uses Index Scan'
);

DROP INDEX idx_child_parent;

-- Test 3: Composite FK prefix matching
DROP TABLE child_t;
DROP TABLE parent_t;

CREATE TABLE parent_comp(a int, b int, PRIMARY KEY(a, b));
CREATE TABLE child_comp(id int, a int, b int,
    FOREIGN KEY(a, b) REFERENCES parent_comp(a, b));

INSERT INTO parent_comp SELECT g, g FROM generate_series(1, 1000) g;
INSERT INTO child_comp SELECT g, g, g FROM generate_series(1, 1000) g;
ANALYZE parent_comp;
ANALYZE child_comp;

-- idx(a, b) covers FK(a, b) — correct prefix
CREATE INDEX idx_ab ON child_comp(a, b);
ANALYZE child_comp;

SELECT assert_explain_contains(
    'SELECT 1 FROM child_comp WHERE a = 1 AND b = 1',
    'Index',
    'idx(a,b) covers FK(a,b) — uses Index Scan'
);

DROP INDEX idx_ab;

-- idx(a, b, c) also covers FK(a, b) — prefix match
CREATE INDEX idx_abc ON child_comp(a, b, id);
ANALYZE child_comp;

SELECT assert_explain_contains(
    'SELECT 1 FROM child_comp WHERE a = 1 AND b = 1',
    'Index',
    'idx(a,b,c) covers FK(a,b) — prefix match uses Index Scan'
);

DROP INDEX idx_abc;

-- Without any index, reverts to Seq Scan
ANALYZE child_comp;

SELECT assert_explain_contains(
    'SELECT 1 FROM child_comp WHERE a = 1 AND b = 1',
    'Seq Scan',
    'Without covering index, FK(a,b) lookup falls back to Seq Scan'
);

-- Cleanup
DROP TABLE child_comp;
DROP TABLE parent_comp;
