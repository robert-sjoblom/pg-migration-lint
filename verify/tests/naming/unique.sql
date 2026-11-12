-- @claim: An unnamed single-column UNIQUE is named {table}_{col}_key; its backing index has the same name in the table's schema
-- @claim: An unnamed multi-column UNIQUE is {table}_{c1}_{c2}_key, columns in constraint order, not table order
-- @claim: INCLUDE columns are appended to the column part: UNIQUE (a) INCLUDE (b) is {table}_a_b_key
-- @claim: ALTER TABLE ADD UNIQUE and ADD COLUMN ... UNIQUE follow the same rule
-- @claim: A named UNIQUE names its backing index after the constraint
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE uq_inline (a int UNIQUE);
SELECT assert_eq(constraint_names('app.uq_inline', 'u'), 'uq_inline_a_key',
    'inline UNIQUE is {table}_{col}_key');
SELECT assert_eq(constraint_index('app.uq_inline', 'uq_inline_a_key'), 'app.uq_inline_a_key',
    'inline UNIQUE backing index has the constraint name, in the table schema');

CREATE TABLE uq_table_level (a int, UNIQUE (a));
SELECT assert_eq(constraint_names('app.uq_table_level', 'u'), 'uq_table_level_a_key',
    'table-level single-column UNIQUE is {table}_{col}_key');

CREATE TABLE uq_multi (a int, b int, UNIQUE (a, b));
SELECT assert_eq(constraint_names('app.uq_multi', 'u'), 'uq_multi_a_b_key',
    'multi-column UNIQUE is {table}_{c1}_{c2}_key');
SELECT assert_eq(constraint_index('app.uq_multi', 'uq_multi_a_b_key'), 'app.uq_multi_a_b_key',
    'multi-column UNIQUE backing index has the constraint name');

CREATE TABLE uq_order (a int, b int, c int, UNIQUE (c, a));
SELECT assert_eq(constraint_names('app.uq_order', 'u'), 'uq_order_c_a_key',
    'UNIQUE column part follows the constraint column list order');

CREATE TABLE uq_include (a int, b int, UNIQUE (a) INCLUDE (b));
SELECT assert_eq(constraint_names('app.uq_include', 'u'), 'uq_include_a_b_key',
    'UNIQUE (a) INCLUDE (b) is {table}_a_b_key');

CREATE TABLE uq_alter (a int, b int);
ALTER TABLE uq_alter ADD UNIQUE (a, b);
SELECT assert_eq(constraint_names('app.uq_alter', 'u'), 'uq_alter_a_b_key',
    'ALTER TABLE ADD UNIQUE is {table}_{cols}_key');

CREATE TABLE uq_add_column (x int);
ALTER TABLE uq_add_column ADD COLUMN a int UNIQUE;
SELECT assert_eq(constraint_names('app.uq_add_column', 'u'), 'uq_add_column_a_key',
    'ALTER TABLE ADD COLUMN ... UNIQUE is {table}_{col}_key');
SELECT assert_eq(constraint_index('app.uq_add_column', 'uq_add_column_a_key'), 'app.uq_add_column_a_key',
    'ADD COLUMN UNIQUE backing index has the constraint name');

CREATE TABLE uq_named (a int, CONSTRAINT my_uq UNIQUE (a));
SELECT assert_eq(constraint_index('app.uq_named', 'my_uq'), 'app.my_uq',
    'named UNIQUE backing index is named after the constraint');
