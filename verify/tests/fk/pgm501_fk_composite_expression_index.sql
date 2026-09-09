-- @claim: A composite index (a, expr(b)) still lets an equality lookup on (a, b)
--   avoid a Seq Scan, using `a` as an Index Cond and applying `b` as a Filter,
--   even though only expr(b) -- not b itself -- is indexed.
-- @min_version: 14

CREATE TABLE parent_t(id int PRIMARY KEY);
CREATE TABLE child_expr(id int, a int, b text,
    FOREIGN KEY(a) REFERENCES parent_t(id));

INSERT INTO parent_t SELECT g FROM generate_series(1, 1000) g;
INSERT INTO child_expr
    SELECT g, ((g - 1) % 1000) + 1, 'user' || g || '@example.com'
    FROM generate_series(1, 100000) g;
ANALYZE parent_t;
ANALYZE child_expr;

-- Baseline: no index at all -> Seq Scan
SELECT assert_explain_contains(
    'SELECT 1 FROM child_expr WHERE a = 1 AND b = ''user1@example.com''',
    'Seq Scan',
    'No index: (a, b) lookup uses Seq Scan'
);

-- Composite index on (a, lower(b)) -- `b` itself is never indexed, only lower(b) is
CREATE INDEX idx_a_lower_b ON child_expr (a, lower(b));
ANALYZE child_expr;

SELECT assert_explain_contains(
    'SELECT 1 FROM child_expr WHERE a = 1 AND b = ''user1@example.com''',
    'Index',
    'idx(a, lower(b)): leading plain column a still avoids Seq Scan'
);

SELECT assert_explain_contains(
    'SELECT 1 FROM child_expr WHERE a = 1 AND b = ''user1@example.com''',
    'Filter',
    'idx(a, lower(b)): b is applied as a Filter, not an index condition, since only lower(b) is indexed'
);

DROP INDEX idx_a_lower_b;
ANALYZE child_expr;

-- Sanity: dropping the index reverts to Seq Scan, confirming the index (not
-- statistics or some other artifact) is what changed the plan above.
SELECT assert_explain_contains(
    'SELECT 1 FROM child_expr WHERE a = 1 AND b = ''user1@example.com''',
    'Seq Scan',
    'Without the index, reverts to Seq Scan'
);

DROP TABLE child_expr;
DROP TABLE parent_t;
