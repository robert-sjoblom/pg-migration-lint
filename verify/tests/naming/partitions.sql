-- @claim: A PARTITION OF child gets its own generated index-backed names from the child table name: {child}_pkey, {child}_{cols}_key
-- @claim: An index created on the parent, named or not, appears on each child as {child}_{cols}_idx
-- @claim: Child indexes and index-backed constraints live in the child's schema
-- @claim: CHECK and FK constraints on a child keep the parent's constraint name
-- @claim: ATTACH PARTITION creates missing indexes with generated child names and keeps an existing matching index's name
-- @claim: Child index names avoid relation collisions the usual way ({child}_pkey1)
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA other;
SET search_path = app, public;

CREATE TABLE customers (id int PRIMARY KEY);
CREATE TABLE ev (
    id int,
    ts date,
    c int CHECK (c > 0),
    customer_id int REFERENCES customers,
    u int,
    PRIMARY KEY (id, ts),
    UNIQUE (u, ts)
) PARTITION BY RANGE (ts);
SELECT assert_eq(constraint_names('app.ev', 'p'), 'ev_pkey', 'partitioned parent PK is {parent}_pkey');

CREATE TABLE ev_2024 PARTITION OF ev FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE INDEX ON ev (c);
CREATE INDEX named_parent_idx ON ev (u);
SELECT assert_eq(index_names('app.ev'), 'app.ev_c_idx,app.ev_pkey,app.ev_u_ts_key,app.named_parent_idx',
    'parent indexes: ev_pkey, ev_u_ts_key, ev_c_idx, named_parent_idx');
SELECT assert_eq(constraint_names('app.ev_2024', 'p'), 'ev_2024_pkey', 'child PK constraint is {child}_pkey');
SELECT assert_eq(constraint_index('app.ev_2024', 'ev_2024_pkey'), 'app.ev_2024_pkey', 'child PK index is {child}_pkey');
SELECT assert_eq(constraint_names('app.ev_2024', 'u'), 'ev_2024_u_ts_key', 'child UNIQUE is {child}_{cols}_key');
SELECT assert_eq(index_names('app.ev_2024'),
    'app.ev_2024_c_idx,app.ev_2024_pkey,app.ev_2024_u_idx,app.ev_2024_u_ts_key',
    'child created before CREATE INDEX on parent: ev_2024_c_idx, and ev_2024_u_idx for named_parent_idx');
SELECT assert_eq(constraint_names('app.ev_2024', 'c'), 'ev_c_check', 'child CHECK keeps the parent name ev_c_check');
SELECT assert_eq(constraint_names('app.ev_2024', 'f'), 'ev_customer_id_fkey', 'child FK keeps the parent name ev_customer_id_fkey');

CREATE TABLE ev_2025 PARTITION OF ev FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
SELECT assert_eq(index_names('app.ev_2025'),
    'app.ev_2025_c_idx,app.ev_2025_pkey,app.ev_2025_u_idx,app.ev_2025_u_ts_key',
    'child created after CREATE INDEX on parent: same generated names');

CREATE TABLE other.ev_2026 PARTITION OF ev FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
SELECT assert_eq(index_names('other.ev_2026'),
    'other.ev_2026_c_idx,other.ev_2026_pkey,other.ev_2026_u_idx,other.ev_2026_u_ts_key',
    'child in schema other: indexes in other, named from the child table');
SELECT assert_eq(
    coalesce((SELECT string_agg(n.nspname || '.' || k.conname, ',' ORDER BY k.conname)
                FROM pg_constraint k JOIN pg_namespace n ON n.oid = k.connamespace
               WHERE k.conrelid = to_regclass('other.ev_2026') AND k.contype IN ('p', 'u')), '<none>'),
    'other.ev_2026_pkey,other.ev_2026_u_ts_key',
    'child in schema other: index-backed constraints in other');

CREATE TABLE ev_2027 (id int NOT NULL, ts date NOT NULL, c int, customer_id int, u int,
    CONSTRAINT ev_c_check CHECK (c > 0));
ALTER TABLE ev ATTACH PARTITION ev_2027 FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');
SELECT assert_eq(index_names('app.ev_2027'),
    'app.ev_2027_c_idx,app.ev_2027_pkey,app.ev_2027_u_idx,app.ev_2027_u_ts_key',
    'ATTACH of a table without indexes: indexes created with generated child names');
SELECT assert_eq(constraint_names('app.ev_2027', 'f'), 'ev_customer_id_fkey',
    'ATTACH: the cloned FK keeps the parent name');

CREATE TABLE ev_2028 (id int NOT NULL, ts date NOT NULL, c int, customer_id int, u int,
    CONSTRAINT ev_c_check CHECK (c > 0));
CREATE INDEX my_c_idx ON ev_2028 (c);
ALTER TABLE ev ATTACH PARTITION ev_2028 FOR VALUES FROM ('2028-01-01') TO ('2029-01-01');
SELECT assert_eq(index_names('app.ev_2028'),
    'app.ev_2028_pkey,app.ev_2028_u_idx,app.ev_2028_u_ts_key,app.my_c_idx',
    'ATTACH of a table with a matching index: my_c_idx is kept, the rest are generated');

CREATE TABLE ev_2029_pkey (x int);
CREATE TABLE ev_2029 PARTITION OF ev FOR VALUES FROM ('2029-01-01') TO ('2030-01-01');
SELECT assert_eq(constraint_names('app.ev_2029', 'p'), 'ev_2029_pkey1',
    'child PK avoids a relation named ev_2029_pkey: ev_2029_pkey1');
