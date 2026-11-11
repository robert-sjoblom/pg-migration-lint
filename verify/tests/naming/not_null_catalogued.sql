-- @claim: From PG 18, an unnamed NOT NULL is a pg_constraint row named {table}_{col}_not_null
-- @claim: The same name comes from inline NOT NULL, table-level NOT NULL col, SET NOT NULL, ADD COLUMN ... NOT NULL and PK columns
-- @claim: CONSTRAINT my_nn NOT NULL keeps my_nn; NOT NULL repeated on one column gives one constraint
-- @claim: CHECK (a IS NOT NULL) stays a CHECK named {table}_{col}_check
-- @claim: NOT NULL names share the schema-wide constraint namespace and get the usual suffix (_not_null1)
-- @claim: RENAME TO and RENAME COLUMN keep the name; a PARTITION OF child gets the parent's name
-- @claim: Truncation uses the label not_null (8 bytes): a 63-char table keeps 52 chars before _{col}_not_null
-- @min_version: 18

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE nn_inline (a int NOT NULL);
SELECT assert_eq(constraint_names('app.nn_inline', 'n'), 'nn_inline_a_not_null',
    'inline NOT NULL is {table}_{col}_not_null');
SELECT assert_eq(
    (SELECT conindid::bigint FROM pg_constraint WHERE conrelid = to_regclass('app.nn_inline') AND contype = 'n'),
    0::bigint,
    'a NOT NULL constraint has no backing index');

CREATE TABLE nn_table_level (a int, NOT NULL a);
SELECT assert_eq(constraint_names('app.nn_table_level', 'n'), 'nn_table_level_a_not_null',
    'table-level NOT NULL a is {table}_{col}_not_null');

CREATE TABLE nn_set (a int);
ALTER TABLE nn_set ALTER COLUMN a SET NOT NULL;
SELECT assert_eq(constraint_names('app.nn_set', 'n'), 'nn_set_a_not_null',
    'ALTER COLUMN SET NOT NULL is {table}_{col}_not_null');

CREATE TABLE nn_add_column (x int);
ALTER TABLE nn_add_column ADD COLUMN a int NOT NULL DEFAULT 0;
SELECT assert_eq(constraint_names('app.nn_add_column', 'n'), 'nn_add_column_a_not_null',
    'ADD COLUMN ... NOT NULL is {table}_{col}_not_null');

CREATE TABLE nn_pk (a int, b int, PRIMARY KEY (a, b));
SELECT assert_eq(constraint_names('app.nn_pk', 'n'), 'nn_pk_a_not_null,nn_pk_b_not_null',
    'PK columns each get {table}_{col}_not_null');
SELECT assert_eq(constraint_names('app.nn_pk', 'p'), 'nn_pk_pkey',
    'the PK itself is still {table}_pkey');

CREATE TABLE nn_named (a int CONSTRAINT my_nn NOT NULL, b int, CONSTRAINT my_table_nn NOT NULL b);
SELECT assert_eq(constraint_names('app.nn_named', 'n'), 'my_nn,my_table_nn',
    'named NOT NULL, column-level or table-level, keeps its name');

CREATE TABLE nn_repeat (a int NOT NULL NOT NULL);
SELECT assert_eq(constraint_names('app.nn_repeat', 'n'), 'nn_repeat_a_not_null',
    'NOT NULL NOT NULL on one column gives one constraint');

CREATE TABLE nn_check (a int CHECK (a IS NOT NULL));
SELECT assert_eq(constraint_names('app.nn_check', 'c') || ' / ' || constraint_names('app.nn_check', 'n'),
    'nn_check_a_check / <none>',
    'CHECK (a IS NOT NULL) stays a CHECK, no NOT NULL constraint');

CREATE TABLE nn_bar (a int, CONSTRAINT nn_foo_a_not_null CHECK (a > 0));
CREATE TABLE nn_foo (a int NOT NULL);
SELECT assert_eq(constraint_names('app.nn_foo', 'n'), 'nn_foo_a_not_null1',
    'NOT NULL avoids a constraint name on another table in the schema: _not_null1');

CREATE TABLE nn_old (id int PRIMARY KEY, a int NOT NULL);
ALTER TABLE nn_old RENAME TO nn_renamed;
ALTER TABLE nn_renamed RENAME COLUMN a TO z;
SELECT assert_eq(constraint_names('app.nn_renamed', 'n'), 'nn_old_a_not_null,nn_old_id_not_null',
    'RENAME TO and RENAME COLUMN keep the NOT NULL names');
CREATE TABLE nn_old (id int PRIMARY KEY, a int NOT NULL);
SELECT assert_eq(constraint_names('app.nn_old', 'n'), 'nn_old_a_not_null1,nn_old_id_not_null1',
    'a new table reusing the old name gets _not_null1');

CREATE TABLE nn_parent (a int NOT NULL, ts date) PARTITION BY RANGE (ts);
CREATE TABLE nn_child PARTITION OF nn_parent FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
SELECT assert_eq(constraint_names('app.nn_child', 'n'), 'nn_parent_a_not_null',
    'a PARTITION OF child gets the parent NOT NULL name');

CREATE TABLE n23456789012345678901234567890123456789012345678901234567890123 (a int NOT NULL);
SELECT assert_eq(
    constraint_names('app.n23456789012345678901234567890123456789012345678901234567890123', 'n'),
    'n234567890123456789012345678901234567890123456789012_a_not_null',
    '63-char table: NOT NULL keeps 52 chars of the table name');
