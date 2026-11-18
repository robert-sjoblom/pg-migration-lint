-- @claim: DROP COLUMN drops an expression index that references the column anywhere in its expression: under COLLATE, in or as an array subscript, in ARRAY[...], an IN list, BETWEEN bounds, IS TRUE, a named function argument, ROW(...) or a field selection
-- @claim: DROP COLUMN keeps an expression index that does not reference the column
-- @claim: DROP COLUMN drops a unique expression index the same way
-- @claim: DROP COLUMN drops an EXCLUDE constraint with a plain-column element on it, but fails with 2BP01 when an EXCLUDE expression element references the column (lower(e), or lower(e) COLLATE "C"); CASCADE then drops the constraint
-- @claim: RENAME COLUMN keeps an expression index, its expression follows the new name, and dropping the renamed column then drops the index
-- @claim: A qualified reference ((t.a)) is indexed as the plain column a, with no expression
-- @claim: An index on a whole-row reference ((t)) survives DROP COLUMN
-- @claim: DROP COLUMN drops a partial index whose WHERE clause references the column
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TYPE pair AS (x int, y int);

CREATE TABLE ex_collate (email text);
CREATE INDEX ex_collate_idx ON ex_collate ((lower(email) COLLATE "C"));
ALTER TABLE ex_collate DROP COLUMN email;
SELECT assert_eq(index_names('app.ex_collate'), '<none>',
    'lower(email) COLLATE "C": dropping email drops the index');

CREATE TABLE ex_collate_column (email text);
CREATE INDEX ex_collate_column_idx ON ex_collate_column ((email COLLATE "C"));
ALTER TABLE ex_collate_column DROP COLUMN email;
SELECT assert_eq(index_names('app.ex_collate_column'), '<none>',
    '(email COLLATE "C"): dropping email drops the index');

CREATE TABLE ex_subscript (arr int[], i int);
CREATE INDEX ex_subscript_idx ON ex_subscript ((arr[1]));
ALTER TABLE ex_subscript DROP COLUMN arr;
SELECT assert_eq(index_names('app.ex_subscript'), '<none>',
    'arr[1]: dropping arr drops the index');

CREATE TABLE ex_subscript_column (arr int[], i int);
CREATE INDEX ex_subscript_column_idx ON ex_subscript_column ((arr[i]));
ALTER TABLE ex_subscript_column DROP COLUMN i;
SELECT assert_eq(index_names('app.ex_subscript_column'), '<none>',
    'arr[i]: dropping the subscript column i drops the index');

CREATE TABLE ex_array (a int, b int);
CREATE INDEX ex_array_idx ON ex_array ((ARRAY[a, b]));
ALTER TABLE ex_array DROP COLUMN b;
SELECT assert_eq(index_names('app.ex_array'), '<none>',
    'ARRAY[a, b]: dropping b drops the index');

CREATE TABLE ex_in_list (a int, b int);
CREATE INDEX ex_in_list_idx ON ex_in_list ((a IN (b, 1)));
ALTER TABLE ex_in_list DROP COLUMN b;
SELECT assert_eq(index_names('app.ex_in_list'), '<none>',
    'a IN (b, 1): dropping b drops the index');

CREATE TABLE ex_between (a int, b int);
CREATE INDEX ex_between_idx ON ex_between ((a BETWEEN 0 AND b));
ALTER TABLE ex_between DROP COLUMN b;
SELECT assert_eq(index_names('app.ex_between'), '<none>',
    'a BETWEEN 0 AND b: dropping the bound b drops the index');

CREATE TABLE ex_is_true (flag bool);
CREATE INDEX ex_is_true_idx ON ex_is_true ((flag IS TRUE));
ALTER TABLE ex_is_true DROP COLUMN flag;
SELECT assert_eq(index_names('app.ex_is_true'), '<none>',
    'flag IS TRUE: dropping flag drops the index');

CREATE TABLE ex_named_argument (d int);
CREATE INDEX ex_named_argument_idx ON ex_named_argument ((make_interval(days => d)));
ALTER TABLE ex_named_argument DROP COLUMN d;
SELECT assert_eq(index_names('app.ex_named_argument'), '<none>',
    'make_interval(days => d): dropping d drops the index');

CREATE TABLE ex_row (a int, b int);
CREATE INDEX ex_row_idx ON ex_row ((ROW(a, b)::pair));
ALTER TABLE ex_row DROP COLUMN b;
SELECT assert_eq(index_names('app.ex_row'), '<none>',
    'ROW(a, b)::pair: dropping b drops the index');

CREATE TABLE ex_field (p pair);
CREATE INDEX ex_field_idx ON ex_field (((p).x));
ALTER TABLE ex_field DROP COLUMN p;
SELECT assert_eq(index_names('app.ex_field'), '<none>',
    '(p).x: dropping p drops the index');

CREATE TABLE ex_unrelated (arr int[], i int, a int);
CREATE INDEX ex_unrelated_idx ON ex_unrelated ((arr[i]));
ALTER TABLE ex_unrelated DROP COLUMN a;
SELECT assert_eq(index_names('app.ex_unrelated'), 'app.ex_unrelated_idx',
    'arr[i]: dropping a column the expression does not reference keeps the index');

CREATE TABLE ex_unique (email text);
CREATE UNIQUE INDEX ex_unique_idx ON ex_unique ((lower(email) COLLATE "C"));
ALTER TABLE ex_unique DROP COLUMN email;
SELECT assert_eq(index_names('app.ex_unique'), '<none>',
    'UNIQUE (lower(email) COLLATE "C"): dropping email drops the unique index');

CREATE FUNCTION statement_error(stmt text) RETURNS text
LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN '<no error>';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE;
END;
$$;

CREATE TABLE ex_exclude_column (e text,
    CONSTRAINT ex_exclude_column_e_excl EXCLUDE USING btree (e WITH =));
ALTER TABLE ex_exclude_column DROP COLUMN e;
SELECT assert_eq(constraint_names('app.ex_exclude_column', 'x') || ' ' || index_names('app.ex_exclude_column'),
    '<none> <none>',
    'EXCLUDE (e WITH =): dropping e drops the constraint and its index');

CREATE TABLE ex_exclude_function (e text,
    CONSTRAINT ex_exclude_function_e_excl EXCLUDE USING btree (lower(e) WITH =));
SELECT assert_eq(statement_error('ALTER TABLE ex_exclude_function DROP COLUMN e'), '2BP01',
    'EXCLUDE (lower(e) WITH =): DROP COLUMN e fails with 2BP01, the constraint depends on e');

CREATE TABLE ex_exclude_collate (e text,
    CONSTRAINT ex_exclude_collate_e_excl EXCLUDE USING btree ((lower(e) COLLATE "C") WITH =));
SELECT assert_eq(statement_error('ALTER TABLE ex_exclude_collate DROP COLUMN e'), '2BP01',
    'EXCLUDE ((lower(e) COLLATE "C") WITH =): DROP COLUMN e fails with 2BP01, the constraint depends on e');
ALTER TABLE ex_exclude_collate DROP COLUMN e CASCADE;
SELECT assert_eq(constraint_names('app.ex_exclude_collate', 'x') || ' ' || index_names('app.ex_exclude_collate'),
    '<none> <none>',
    'EXCLUDE ((lower(e) COLLATE "C") WITH =): DROP COLUMN e CASCADE drops the constraint and its index');

CREATE TABLE ex_rename (arr int[], i int);
CREATE INDEX ex_rename_idx ON ex_rename ((arr[i]));
ALTER TABLE ex_rename RENAME COLUMN i TO pos;
SELECT assert_eq(index_names('app.ex_rename'), 'app.ex_rename_idx',
    'arr[i]: renaming i keeps the index');
SELECT assert_true(pg_get_indexdef('app.ex_rename_idx'::regclass) LIKE '%arr[pos]%',
    'arr[i]: after RENAME COLUMN i TO pos the index expression reads arr[pos]');
ALTER TABLE ex_rename DROP COLUMN pos;
SELECT assert_eq(index_names('app.ex_rename'), '<none>',
    'arr[i]: dropping the renamed column pos drops the index');

CREATE TABLE ex_qualified (a int, b int);
CREATE INDEX ex_qualified_idx ON ex_qualified ((ex_qualified.a));
SELECT assert_true(
    (SELECT indexprs IS NULL AND indkey::text = '1'
       FROM pg_index WHERE indexrelid = 'app.ex_qualified_idx'::regclass),
    '((t.a)): indexed as the plain column a, with no expression');

CREATE TABLE ex_whole_row (a int, b int);
CREATE INDEX ex_whole_row_idx ON ex_whole_row ((ex_whole_row));
ALTER TABLE ex_whole_row DROP COLUMN a;
SELECT assert_eq(index_names('app.ex_whole_row'), 'app.ex_whole_row_idx',
    '((t)) whole-row reference: dropping a column keeps the index');

CREATE TABLE ex_partial (a int, b int);
CREATE INDEX ex_partial_idx ON ex_partial (a) WHERE b > 0;
ALTER TABLE ex_partial DROP COLUMN b;
SELECT assert_eq(index_names('app.ex_partial'), '<none>',
    '(a) WHERE b > 0: dropping b drops the partial index');
