-- @claim: An unnamed CHECK is named from the distinct columns its expression references, not from where it is written
-- @claim: Exactly one referenced column gives {table}_{col}_check, column-level or table-level
-- @claim: Two or more referenced columns, or none (CHECK (true)), give {table}_check
-- @claim: A column-level CHECK that references another column, or no column, also gets {table}_check
-- @claim: A column referenced twice in one expression counts once
-- @claim: ALTER TABLE ADD CHECK and ADD COLUMN ... CHECK follow the same rule
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ck_column (a int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.ck_column', 'c'), 'ck_column_a_check',
    'column-level CHECK on its own column is {table}_{col}_check');

CREATE TABLE ck_table_one (a int, b int, CHECK (b > 0));
SELECT assert_eq(constraint_names('app.ck_table_one', 'c'), 'ck_table_one_b_check',
    'table-level CHECK referencing exactly one column is {table}_{col}_check');

CREATE TABLE ck_table_two (a int, b int, CHECK (a < b));
SELECT assert_eq(constraint_names('app.ck_table_two', 'c'), 'ck_table_two_check',
    'table-level CHECK referencing two columns is {table}_check');

CREATE TABLE ck_table_none (a int, CHECK (true));
SELECT assert_eq(constraint_names('app.ck_table_none', 'c'), 'ck_table_none_check',
    'table-level CHECK referencing no column is {table}_check');

CREATE TABLE ck_column_other (a int, b int CHECK (b > a));
SELECT assert_eq(constraint_names('app.ck_column_other', 'c'), 'ck_column_other_check',
    'column-level CHECK on b that also references a is {table}_check');

CREATE TABLE ck_column_foreign (a int, b int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.ck_column_foreign', 'c'), 'ck_column_foreign_a_check',
    'column-level CHECK written on b but referencing only a is {table}_a_check');

CREATE TABLE ck_column_none (a int CHECK (true));
SELECT assert_eq(constraint_names('app.ck_column_none', 'c'), 'ck_column_none_check',
    'column-level CHECK (true) is {table}_check, no column part');

CREATE TABLE ck_repeat (a int, CHECK (a + a > 0 AND a < 100));
SELECT assert_eq(constraint_names('app.ck_repeat', 'c'), 'ck_repeat_a_check',
    'a column referenced several times counts once: {table}_{col}_check');

CREATE TABLE ck_function (a int, CHECK (length(a::text) > 0));
SELECT assert_eq(constraint_names('app.ck_function', 'c'), 'ck_function_a_check',
    'a column inside a function call or cast still counts: {table}_{col}_check');

CREATE TABLE ck_between (a int, b int, c int, CHECK (a BETWEEN b AND c));
SELECT assert_eq(constraint_names('app.ck_between', 'c'), 'ck_between_check',
    'the bounds of BETWEEN count: a BETWEEN b AND c references three columns, {table}_check');

CREATE TABLE ck_in_list (a int, CHECK (0 IN (a, 1)));
SELECT assert_eq(constraint_names('app.ck_in_list', 'c'), 'ck_in_list_a_check',
    'a column inside an IN list counts: {table}_{col}_check');

CREATE TABLE ck_boolean_test (flag bool, CHECK (flag IS NOT FALSE));
SELECT assert_eq(constraint_names('app.ck_boolean_test', 'c'), 'ck_boolean_test_flag_check',
    'a column under IS NOT FALSE counts: {table}_{col}_check');

CREATE TABLE ck_subscript (arr int[], CHECK (arr[1] > 0));
SELECT assert_eq(constraint_names('app.ck_subscript', 'c'), 'ck_subscript_arr_check',
    'a subscripted column counts: {table}_{col}_check');

CREATE TABLE ck_alter (a int, b int);
ALTER TABLE ck_alter ADD CHECK (a > 0);
ALTER TABLE ck_alter ADD CHECK (a < b);
SELECT assert_eq(constraint_names('app.ck_alter', 'c'), 'ck_alter_a_check,ck_alter_check',
    'ALTER TABLE ADD CHECK: one column gives {table}_{col}_check, two give {table}_check');

CREATE TABLE ck_add_column (a int);
ALTER TABLE ck_add_column ADD COLUMN b int CHECK (b > 0);
ALTER TABLE ck_add_column ADD COLUMN c int CHECK (c > a);
SELECT assert_eq(constraint_names('app.ck_add_column', 'c'), 'ck_add_column_b_check,ck_add_column_check',
    'ADD COLUMN ... CHECK: own column only gives {table}_{col}_check, referencing another gives {table}_check');
