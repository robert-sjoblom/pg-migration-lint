-- @claim: RENAME CONSTRAINT on a CHECK or FOREIGN KEY fails only when another constraint of the same table has the new name; other tables' constraint names and relation names are free
-- @claim: RENAME CONSTRAINT on a PRIMARY KEY, UNIQUE or EXCLUDE fails when a relation in the schema has the new name (42P07), and when another constraint of the same table has it (42710)
-- @claim: ALTER INDEX ... RENAME fails when a relation in the schema has the new name; an index backing a constraint also fails on a constraint name of its table
-- @claim: ALTER TABLE ... RENAME TO fails when a relation or a type in the schema has the new name; constraint names are free
-- @claim: Renaming onto the name the object already has fails
-- @claim: A failed rename leaves every name unchanged, the index included
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

CREATE TABLE neighbour (n int, CONSTRAINT neighbour_check CHECK (n > 0), CONSTRAINT neighbour_key UNIQUE (n));
CREATE INDEX neighbour_idx ON neighbour (n);
CREATE TABLE other.remote (r int);
CREATE INDEX remote_idx ON other.remote (r);
CREATE TYPE mood AS ENUM ('happy', 'sad');

CREATE TABLE con (
    a int,
    b int,
    CONSTRAINT con_pk PRIMARY KEY (a),
    CONSTRAINT con_uq UNIQUE (b),
    CONSTRAINT con_chk CHECK (a > 0),
    CONSTRAINT con_chk2 CHECK (b > 0),
    CONSTRAINT con_fk FOREIGN KEY (b) REFERENCES neighbour (n),
    CONSTRAINT con_excl EXCLUDE USING btree (b WITH =));
CREATE INDEX con_idx ON con (b);

-- RENAME CONSTRAINT on a CHECK or FOREIGN KEY
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_chk TO con_chk2'),
    '42710 constraint "con_chk2" for relation "con" already exists',
    'CHECK: another constraint name of the same table fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_chk TO con_uq'),
    '42710 constraint "con_uq" for relation "con" already exists',
    'CHECK: the name of an index-backed constraint of the same table fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_chk TO con_chk'),
    '42710 constraint "con_chk" for relation "con" already exists',
    'CHECK: its own name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_fk TO con_chk2'),
    '42710 constraint "con_chk2" for relation "con" already exists',
    'FOREIGN KEY: another constraint name of the same table fails');

SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_chk TO neighbour_check'), '<no error>',
    'CHECK: another table''s constraint name is free');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT neighbour_check TO neighbour_idx'), '<no error>',
    'CHECK: an index name is free');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT neighbour_idx TO neighbour'), '<no error>',
    'CHECK: a table name is free');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_fk TO neighbour_key'), '<no error>',
    'FOREIGN KEY: another table''s UNIQUE name is free, though its index has it');
SELECT assert_eq(constraint_names('app.con', 'c') || ' ' || constraint_names('app.con', 'f'),
    'con_chk2,neighbour neighbour_key',
    'the allowed CHECK and FOREIGN KEY renames took effect');
ALTER TABLE con RENAME CONSTRAINT neighbour TO con_chk;
ALTER TABLE con RENAME CONSTRAINT neighbour_key TO con_fk;

-- RENAME CONSTRAINT on a PRIMARY KEY, UNIQUE or EXCLUDE
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO neighbour_idx'),
    '42P07 relation "neighbour_idx" already exists',
    'UNIQUE: another table''s index name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO neighbour'),
    '42P07 relation "neighbour" already exists',
    'UNIQUE: a table name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO con_idx'),
    '42P07 relation "con_idx" already exists',
    'UNIQUE: a plain index name of the same table fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO con_uq'),
    '42P07 relation "con_uq" already exists',
    'UNIQUE: its own name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO con_chk2'),
    '42710 constraint "con_chk2" for relation "con" already exists',
    'UNIQUE: a CHECK name of the same table fails, though no relation has it');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_pk TO neighbour_key'),
    '42P07 relation "neighbour_key" already exists',
    'PRIMARY KEY: another table''s UNIQUE name fails, its index has it');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_excl TO con_pk'),
    '42P07 relation "con_pk" already exists',
    'EXCLUDE: the PRIMARY KEY name of the same table fails on the relation first');
SELECT assert_eq(constraint_names('app.con', 'u') || ' ' || index_names('app.con'),
    'con_uq app.con_excl,app.con_idx,app.con_pk,app.con_uq',
    'a failed rename leaves the constraint and its index unchanged');

SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT con_uq TO neighbour_check'), '<no error>',
    'UNIQUE: another table''s CHECK name is free, no relation has it');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME CONSTRAINT neighbour_check TO remote_idx'), '<no error>',
    'UNIQUE: a relation name in another schema is free');
SELECT assert_eq(constraint_index('app.con', 'remote_idx'), 'app.remote_idx',
    'the allowed UNIQUE renames took the index along');
ALTER TABLE con RENAME CONSTRAINT remote_idx TO con_uq;

-- ALTER INDEX ... RENAME
SELECT assert_eq(statement_error('ALTER INDEX con_idx RENAME TO neighbour_idx'),
    '42P07 relation "neighbour_idx" already exists',
    'ALTER INDEX: another index name fails');
SELECT assert_eq(statement_error('ALTER INDEX con_idx RENAME TO neighbour'),
    '42P07 relation "neighbour" already exists',
    'ALTER INDEX: a table name fails');
SELECT assert_eq(statement_error('ALTER INDEX con_idx RENAME TO con_idx'),
    '42P07 relation "con_idx" already exists',
    'ALTER INDEX: its own name fails');
SELECT assert_eq(statement_error('ALTER INDEX con_pk RENAME TO con_chk2'),
    '42710 constraint "con_chk2" for relation "con" already exists',
    'ALTER INDEX: an index backing a constraint fails on a constraint name of its table');

SELECT assert_eq(statement_error('ALTER INDEX con_pk RENAME TO neighbour_check'), '<no error>',
    'ALTER INDEX: an index backing a constraint may take another table''s constraint name');
SELECT assert_eq(statement_error('ALTER INDEX con_idx RENAME TO con_chk2'), '<no error>',
    'ALTER INDEX: a plain index may take a constraint name of its own table');
SELECT assert_eq(statement_error('ALTER INDEX con_chk2 RENAME TO remote_idx'), '<no error>',
    'ALTER INDEX: a relation name in another schema is free');
SELECT assert_eq(constraint_names('app.con', 'p') || ' ' || index_names('app.con'),
    'neighbour_check app.con_excl,app.con_uq,app.neighbour_check,app.remote_idx',
    'the allowed index renames took effect');

-- ALTER TABLE ... RENAME TO
SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO neighbour'),
    '42P07 relation "neighbour" already exists',
    'RENAME TO: another table name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO neighbour_idx'),
    '42P07 relation "neighbour_idx" already exists',
    'RENAME TO: an index name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO neighbour_key'),
    '42P07 relation "neighbour_key" already exists',
    'RENAME TO: the name of an index-backed constraint fails, its index has it');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO con'),
    '42P07 relation "con" already exists',
    'RENAME TO: its own name fails');
SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO mood'),
    '42710 type "mood" already exists',
    'RENAME TO: a type name fails, the table''s row type would clash');
SELECT assert_eq(index_names('app.con'), 'app.con_excl,app.con_uq,app.neighbour_check,app.remote_idx',
    'a failed table rename leaves the table and its indexes unchanged');

SELECT assert_eq(statement_error('ALTER TABLE con RENAME TO con_chk2'), '<no error>',
    'RENAME TO: a constraint name is free');
SELECT assert_eq(statement_error('ALTER TABLE con_chk2 RENAME TO remote'), '<no error>',
    'RENAME TO: a table name in another schema is free');
SELECT assert_eq(index_names('app.remote'), 'app.con_excl,app.con_uq,app.neighbour_check,app.remote_idx',
    'the allowed table renames took effect');
