-- @claim: CREATE INDEX onto a name a relation in the table's schema already has fails with 42P07 "relation ... already exists" (PGM027): an index of another table or of the same table, the index of a PRIMARY KEY, UNIQUE or EXCLUDE constraint, or a table
-- @claim: CONCURRENTLY does not change that, and the failed CONCURRENTLY build leaves no index behind
-- @claim: Inside a transaction block, CONCURRENTLY fails with 25001 first (PGM003)
-- @claim: A sequence, a view, or the index PostgreSQL creates on a partition takes the name too
-- @claim: The name is looked up in the table's schema only; a relation of that name in another schema leaves it free
-- @claim: On a table that does not exist, it fails with 42P01 instead
-- @claim: IF NOT EXISTS onto a taken name succeeds and creates nothing, whatever relation has the name
-- @claim: The failed statements change nothing
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA other;
SET search_path = app, public;

CREATE FUNCTION statement_error(stmt text) RETURNS text
LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN '<no error>';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE || ' ' || SQLERRM;
END;
$$;

CREATE TABLE archive (
    id int,
    code text,
    slot int,
    CONSTRAINT archive_pkey PRIMARY KEY (id),
    CONSTRAINT archive_code_key UNIQUE (code),
    CONSTRAINT archive_slot_excl EXCLUDE USING btree (slot WITH =));
CREATE INDEX archive_slot_idx ON archive (slot);
CREATE TABLE orders (id int, status text);
CREATE INDEX orders_status_idx ON orders (status);
CREATE TABLE other.remote (r int);
CREATE INDEX remote_idx ON other.remote (r);

SELECT assert_eq(statement_error('CREATE INDEX archive_slot_idx ON orders (status)'),
    '42P07 relation "archive_slot_idx" already exists',
    'an index of another table has the name');
SELECT assert_eq(statement_error('CREATE INDEX orders_status_idx ON orders (id)'),
    '42P07 relation "orders_status_idx" already exists',
    'an index of the same table has the name');
SELECT assert_eq(statement_error('CREATE INDEX archive_pkey ON orders (id)'),
    '42P07 relation "archive_pkey" already exists',
    'a PRIMARY KEY''s index has the name');
SELECT assert_eq(statement_error('CREATE UNIQUE INDEX archive_code_key ON orders (status)'),
    '42P07 relation "archive_code_key" already exists',
    'a UNIQUE constraint''s index has the name');
SELECT assert_eq(statement_error('CREATE INDEX archive_slot_excl ON orders (id)'),
    '42P07 relation "archive_slot_excl" already exists',
    'an EXCLUDE constraint''s index has the name');
SELECT assert_eq(statement_error('CREATE INDEX archive ON orders (id)'),
    '42P07 relation "archive" already exists',
    'a table has the name');
CREATE SEQUENCE archive_seq;
CREATE VIEW archive_view AS SELECT id FROM archive;
CREATE TABLE events (id int, at date) PARTITION BY RANGE (at);
CREATE TABLE events_2024 PARTITION OF events FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
CREATE INDEX events_at_idx ON events (at);
SELECT assert_eq(index_names('app.events_2024'), 'app.events_2024_at_idx',
    'the index on the partitioned table creates one on the partition, with a generated name');
SELECT assert_eq(statement_error('CREATE INDEX archive_seq ON orders (id)'),
    '42P07 relation "archive_seq" already exists',
    'a sequence has the name');
SELECT assert_eq(statement_error('CREATE INDEX archive_view ON orders (id)'),
    '42P07 relation "archive_view" already exists',
    'a view has the name');
SELECT assert_eq(statement_error('CREATE INDEX events_2024_at_idx ON orders (id)'),
    '42P07 relation "events_2024_at_idx" already exists',
    'the index created on a partition has the name');
SELECT assert_eq(statement_error('CREATE INDEX archive_slot_idx ON made_outside_migrations (id)'),
    '42P01 relation "made_outside_migrations" does not exist',
    'a missing table fails first');

-- CONCURRENTLY cannot run inside the function's transaction, so it runs at top level.
CREATE INDEX CONCURRENTLY archive_slot_idx ON orders (status);
SELECT assert_eq(:'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE',
    '42P07 relation "archive_slot_idx" already exists',
    'CONCURRENTLY: an index of another table has the name');
CREATE INDEX CONCURRENTLY archive ON orders (status);
SELECT assert_eq(:'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE',
    '42P07 relation "archive" already exists',
    'CONCURRENTLY: a table has the name');
BEGIN;
CREATE INDEX CONCURRENTLY archive_slot_idx ON orders (status);
ROLLBACK;
SELECT assert_eq(:'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE',
    '25001 CREATE INDEX CONCURRENTLY cannot run inside a transaction block',
    'CONCURRENTLY in a transaction block fails on the transaction before the name');

SELECT assert_eq(statement_error('CREATE INDEX IF NOT EXISTS archive_slot_idx ON orders (status)'), '<no error>',
    'IF NOT EXISTS: an index of another table has the name');
SELECT assert_eq(statement_error('CREATE INDEX IF NOT EXISTS orders_status_idx ON orders (id)'), '<no error>',
    'IF NOT EXISTS: an index of the same table has the name');
SELECT assert_eq(statement_error('CREATE INDEX IF NOT EXISTS archive_code_key ON orders (status)'), '<no error>',
    'IF NOT EXISTS: a UNIQUE constraint''s index has the name');
SELECT assert_eq(statement_error('CREATE INDEX IF NOT EXISTS archive ON orders (id)'), '<no error>',
    'IF NOT EXISTS: a table has the name');

SELECT assert_eq(index_names('app.orders') || ' ' || index_names('app.archive'),
    'app.orders_status_idx app.archive_code_key,app.archive_pkey,app.archive_slot_excl,app.archive_slot_idx',
    'the failed and skipped statements leave every index where it was');
SELECT assert_eq(
    (SELECT string_agg(c.relname || ':' || c.relkind::text, ',' ORDER BY c.relname)
       FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
      WHERE n.nspname = 'app' AND c.relname NOT LIKE 'events%' AND c.relkind IN ('r', 'i')),
    'archive:r,archive_code_key:i,archive_pkey:i,archive_slot_excl:i,archive_slot_idx:i,orders:r,orders_status_idx:i',
    'the failed CONCURRENTLY builds leave no index behind');

SELECT assert_eq(statement_error('CREATE INDEX remote_idx ON orders (id)'), '<no error>',
    'a relation of that name in another schema leaves the name free');
SELECT assert_eq(index_names('app.orders') || ' ' || index_names('other.remote'),
    'app.orders_status_idx,app.remote_idx other.remote_idx',
    'the index is created in the table''s schema');
