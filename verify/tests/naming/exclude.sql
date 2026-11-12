-- @claim: An unnamed EXCLUDE is named {table}_{cols}_excl; its backing index has the same name in the table's schema
-- @claim: Works without extensions: btree with = on int, gist with && on int4range
-- @claim: Expression elements name as in CREATE INDEX: a function call gives its name, an operator expression gives expr
-- @claim: INCLUDE columns are appended to the column part; a WHERE predicate does not change the name
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ex_btree (a int, EXCLUDE USING btree (a WITH =));
SELECT assert_eq(constraint_names('app.ex_btree', 'x'), 'ex_btree_a_excl',
    'EXCLUDE USING btree (a WITH =) is {table}_{col}_excl');
SELECT assert_eq(constraint_index('app.ex_btree', 'ex_btree_a_excl'), 'app.ex_btree_a_excl',
    'EXCLUDE backing index has the constraint name, in the table schema');

CREATE TABLE ex_gist (r int4range, EXCLUDE USING gist (r WITH &&));
SELECT assert_eq(constraint_names('app.ex_gist', 'x'), 'ex_gist_r_excl',
    'EXCLUDE USING gist (r WITH &&) is {table}_{col}_excl');

CREATE TABLE ex_multi (a int, b int, EXCLUDE USING btree (a WITH =, b WITH =));
SELECT assert_eq(constraint_names('app.ex_multi', 'x'), 'ex_multi_a_b_excl',
    'multi-column EXCLUDE is {table}_{c1}_{c2}_excl');

CREATE TABLE ex_function (e text, EXCLUDE USING btree (lower(e) WITH =));
SELECT assert_eq(constraint_names('app.ex_function', 'x'), 'ex_function_lower_excl',
    'EXCLUDE on lower(e) is {table}_lower_excl');

CREATE TABLE ex_operator (a int, EXCLUDE USING btree ((a + 1) WITH =));
SELECT assert_eq(constraint_names('app.ex_operator', 'x'), 'ex_operator_expr_excl',
    'EXCLUDE on (a + 1) is {table}_expr_excl');

CREATE TABLE ex_include (a int, b int, EXCLUDE USING btree (a WITH =) INCLUDE (b));
SELECT assert_eq(constraint_names('app.ex_include', 'x'), 'ex_include_a_b_excl',
    'EXCLUDE (a WITH =) INCLUDE (b) is {table}_a_b_excl');

CREATE TABLE ex_where (a int, EXCLUDE USING btree (a WITH =) WHERE (a > 0));
SELECT assert_eq(constraint_names('app.ex_where', 'x'), 'ex_where_a_excl',
    'EXCLUDE ... WHERE keeps {table}_{col}_excl');

CREATE TABLE ex_alter (a int);
ALTER TABLE ex_alter ADD EXCLUDE USING btree (a WITH =);
SELECT assert_eq(constraint_names('app.ex_alter', 'x'), 'ex_alter_a_excl',
    'ALTER TABLE ADD EXCLUDE is {table}_{col}_excl');
