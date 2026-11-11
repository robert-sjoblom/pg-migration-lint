-- @claim: CREATE INDEX without a name gives {table}_{cols}_idx, columns in index order, in the table's schema
-- @claim: A function-call element contributes the function name (last part if schema-qualified)
-- @claim: coalesce, greatest, least, nullif and ARRAY[] contribute their keyword, like function calls
-- @claim: A parenthesized column ((a)), a subscript (arr[1]) and a field selection ((p).x) contribute the column or field name
-- @claim: An operator or other non-function expression contributes expr
-- @claim: A cast contributes its operand's column or function name; a cast of anything else contributes the internal type name
-- @claim: CASE contributes its ELSE result's column or function name, and case otherwise; a cast of a CASE that gives case contributes the type name
-- @claim: COLLATE inside the element's expression does not change what the expression contributes
-- @claim: INCLUDE columns are appended; a repeated element gets a numeric suffix (a, a1)
-- @claim: WHERE, UNIQUE, USING, DESC, opclass, COLLATE and CONCURRENTLY do not change the name; the suffix stays idx
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA other;
SET search_path = app, public;

CREATE TABLE i_single (a int);
CREATE INDEX ON i_single (a);
SELECT assert_eq(index_names('app.i_single'), 'app.i_single_a_idx',
    'single column: {table}_{col}_idx in the table schema');

CREATE TABLE i_multi (a int, b int, c int);
CREATE INDEX ON i_multi (c, a);
SELECT assert_eq(index_names('app.i_multi'), 'app.i_multi_c_a_idx',
    'multi-column: {table}_{c1}_{c2}_idx in index column order');

CREATE TABLE other.i_qualified (a int);
CREATE INDEX ON other.i_qualified (a);
SELECT assert_eq(index_names('other.i_qualified'), 'other.i_qualified_a_idx',
    'index on other.t lands in schema other, no schema part in the name');

CREATE TABLE i_lower (email text);
CREATE INDEX ON i_lower (lower(email));
SELECT assert_eq(index_names('app.i_lower'), 'app.i_lower_lower_idx',
    'lower(email) contributes lower');

CREATE TABLE i_qualified_fn (email text);
CREATE INDEX ON i_qualified_fn (pg_catalog.lower(email));
SELECT assert_eq(index_names('app.i_qualified_fn'), 'app.i_qualified_fn_lower_idx',
    'pg_catalog.lower(email) contributes lower');

CREATE TABLE i_coalesce (a int, b int);
CREATE INDEX ON i_coalesce (coalesce(a, b));
SELECT assert_eq(index_names('app.i_coalesce'), 'app.i_coalesce_coalesce_idx',
    'coalesce(a, b) contributes coalesce');

CREATE TABLE i_case (a int);
CREATE INDEX ON i_case ((CASE WHEN a > 0 THEN 1 END));
SELECT assert_eq(index_names('app.i_case'), 'app.i_case_case_idx',
    'a CASE expression contributes case');

CREATE TABLE i_plus (a int, b int);
CREATE INDEX ON i_plus ((a + b));
SELECT assert_eq(index_names('app.i_plus'), 'app.i_plus_expr_idx',
    '(a + b) contributes expr');

CREATE TABLE i_jsonb (js jsonb);
CREATE INDEX ON i_jsonb ((js ->> 'k'));
SELECT assert_eq(index_names('app.i_jsonb'), 'app.i_jsonb_expr_idx',
    '(js ->> ''k'') contributes expr');

CREATE TABLE i_mixed (a int, b int, email text);
CREATE INDEX ON i_mixed (a, lower(email));
CREATE INDEX ON i_mixed (lower(email), (a + b), b);
SELECT assert_eq(index_names('app.i_mixed'), 'app.i_mixed_a_lower_idx,app.i_mixed_lower_expr_b_idx',
    'mixed columns and expressions: each element contributes its own part, in order');

CREATE TABLE i_cast_column (a int);
CREATE INDEX ON i_cast_column ((a::text));
SELECT assert_eq(index_names('app.i_cast_column'), 'app.i_cast_column_a_idx',
    '(a::text) contributes the column name a');

CREATE TABLE i_cast_expr (a int, b int, js jsonb);
CREATE INDEX ON i_cast_expr (((a + b)::text));
CREATE INDEX ON i_cast_expr ((CAST(a + b AS bigint)));
CREATE INDEX ON i_cast_expr (((js ->> 'k')::int));
SELECT assert_eq(index_names('app.i_cast_expr'),
    'app.i_cast_expr_int4_idx,app.i_cast_expr_int8_idx,app.i_cast_expr_text_idx',
    'cast of an expression contributes the internal type name: text, int8, int4');

CREATE TABLE i_cast_function (a int, b int, email text);
CREATE INDEX ON i_cast_function ((lower(email)::text));
CREATE INDEX ON i_cast_function ((coalesce(a, b)::text));
SELECT assert_eq(index_names('app.i_cast_function'),
    'app.i_cast_function_coalesce_idx,app.i_cast_function_lower_idx',
    'cast of a function call contributes the function name, not the type: lower, coalesce');

CREATE TABLE i_paren_column (a int);
CREATE INDEX ON i_paren_column ((a));
SELECT assert_eq(index_names('app.i_paren_column'), 'app.i_paren_column_a_idx',
    '((a)) is an expression element and contributes the column name a');

CREATE TABLE i_minmax (a int, b int);
CREATE INDEX ON i_minmax (greatest(a, b));
CREATE INDEX ON i_minmax (least(a, b));
SELECT assert_eq(index_names('app.i_minmax'), 'app.i_minmax_greatest_idx,app.i_minmax_least_idx',
    'greatest(a, b) contributes greatest, least(a, b) contributes least');

CREATE TABLE i_nullif (a int, b int);
CREATE INDEX ON i_nullif (nullif(a, b));
SELECT assert_eq(index_names('app.i_nullif'), 'app.i_nullif_nullif_idx',
    'nullif(a, b) contributes nullif, though it parses as an operator expression');

CREATE TABLE i_array (a int, b int);
CREATE INDEX ON i_array ((ARRAY[a, b]));
SELECT assert_eq(index_names('app.i_array'), 'app.i_array_array_idx',
    'ARRAY[a, b] contributes array');

CREATE TABLE i_case_else (a int, b int);
CREATE INDEX ON i_case_else ((CASE WHEN a > 0 THEN 1 ELSE b END));
CREATE INDEX ON i_case_else ((CASE WHEN a > 0 THEN 1 ELSE abs(b) END));
CREATE INDEX ON i_case_else ((CASE WHEN a > 0 THEN 1 ELSE (a + b)::int END));
SELECT assert_eq(index_names('app.i_case_else'),
    'app.i_case_else_abs_idx,app.i_case_else_b_idx,app.i_case_else_case_idx',
    'CASE contributes its ELSE column or function (b, abs); an ELSE cast of an expression gives case');

CREATE TABLE i_cast_case (a int);
CREATE INDEX ON i_cast_case (((CASE WHEN a > 0 THEN 1 END)::text));
SELECT assert_eq(index_names('app.i_cast_case'), 'app.i_cast_case_text_idx',
    'a cast of a CASE without ELSE contributes the type name text, not case');

CREATE TABLE i_collate_expr (a int, b int, email text);
CREATE INDEX ON i_collate_expr ((email COLLATE "C"));
CREATE INDEX ON i_collate_expr ((lower(email) COLLATE "C"));
CREATE INDEX ON i_collate_expr (((a + b)::text COLLATE "C"));
SELECT assert_eq(index_names('app.i_collate_expr'),
    'app.i_collate_expr_email_idx,app.i_collate_expr_lower_idx,app.i_collate_expr_text_idx',
    'COLLATE inside the expression is transparent: email, lower, text');

CREATE TYPE pair AS (x int, y int);
CREATE TABLE i_indirection (arr int[], js jsonb, p pair);
CREATE INDEX ON i_indirection ((arr[1]));
CREATE INDEX ON i_indirection ((js['k']));
CREATE INDEX ON i_indirection (((p).x));
SELECT assert_eq(index_names('app.i_indirection'),
    'app.i_indirection_arr_idx,app.i_indirection_js_idx,app.i_indirection_x_idx',
    'a subscript contributes the column (arr, js); a field selection contributes the field (x)');

CREATE TABLE i_null_test (a int);
CREATE INDEX ON i_null_test ((a IS NULL));
SELECT assert_eq(index_names('app.i_null_test'), 'app.i_null_test_expr_idx',
    '(a IS NULL) contributes expr: an unhandled expression kind falls back to expr');

CREATE TABLE i_bool_expr (a int, b int);
CREATE INDEX ON i_bool_expr ((a > 0 AND b > 0));
SELECT assert_eq(index_names('app.i_bool_expr'), 'app.i_bool_expr_expr_idx',
    '(a > 0 AND b > 0) contributes expr');

CREATE TABLE i_partial (a int);
CREATE INDEX ON i_partial (a) WHERE a > 0;
SELECT assert_eq(index_names('app.i_partial'), 'app.i_partial_a_idx',
    'partial index: WHERE does not change the name');

CREATE TABLE i_unique (a int);
CREATE UNIQUE INDEX ON i_unique (a);
SELECT assert_eq(index_names('app.i_unique'), 'app.i_unique_a_idx',
    'CREATE UNIQUE INDEX keeps the idx suffix, not key');
SELECT assert_eq(constraint_names('app.i_unique', 'u'), '<none>',
    'CREATE UNIQUE INDEX creates no constraint');

CREATE TABLE i_include (a int, b int);
CREATE INDEX ON i_include (a) INCLUDE (b);
SELECT assert_eq(index_names('app.i_include'), 'app.i_include_a_b_idx',
    'INCLUDE columns are appended: (a) INCLUDE (b) is {table}_a_b_idx');

CREATE TABLE i_repeat (a int, email text);
CREATE INDEX ON i_repeat (a, a);
CREATE INDEX ON i_repeat (lower(email), lower(email));
SELECT assert_eq(index_names('app.i_repeat'), 'app.i_repeat_a_a1_idx,app.i_repeat_lower_lower1_idx',
    'a repeated element gets a numeric suffix: a_a1, lower_lower1');

CREATE TABLE i_hash (a text);
CREATE INDEX ON i_hash USING hash (a);
SELECT assert_eq(index_names('app.i_hash'), 'app.i_hash_a_idx', 'USING hash does not change the name');

CREATE TABLE i_desc (a int);
CREATE INDEX ON i_desc (a DESC NULLS LAST);
SELECT assert_eq(index_names('app.i_desc'), 'app.i_desc_a_idx', 'DESC NULLS LAST does not change the name');

CREATE TABLE i_opclass (a text);
CREATE INDEX ON i_opclass (a text_pattern_ops);
SELECT assert_eq(index_names('app.i_opclass'), 'app.i_opclass_a_idx', 'an opclass does not change the name');

CREATE TABLE i_collate (a text);
CREATE INDEX ON i_collate (a COLLATE "C");
SELECT assert_eq(index_names('app.i_collate'), 'app.i_collate_a_idx', 'COLLATE does not change the name');

CREATE TABLE i_concurrently (a int);
CREATE INDEX CONCURRENTLY ON i_concurrently (a);
SELECT assert_eq(index_names('app.i_concurrently'), 'app.i_concurrently_a_idx',
    'CONCURRENTLY does not change the name');
