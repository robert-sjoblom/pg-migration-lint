-- @claim: An unnamed PRIMARY KEY is named {table}_pkey, inline or table-level, in CREATE TABLE or ALTER TABLE
-- @claim: The PK's backing index has the same name as the constraint and lives in the table's schema
-- @claim: A composite or INCLUDE primary key still has no column part in its name
-- @claim: A schema-qualified table gives {table}_pkey in that schema, with no schema part in the name
-- @claim: A named PK (CONSTRAINT my_pk PRIMARY KEY) names its backing index my_pk
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA analytics;
SET search_path = app, public;

CREATE TABLE pk_inline (id int PRIMARY KEY);
SELECT assert_eq(constraint_names('app.pk_inline', 'p'), 'pk_inline_pkey',
    'inline PRIMARY KEY in CREATE TABLE is {table}_pkey');
SELECT assert_eq(constraint_index('app.pk_inline', 'pk_inline_pkey'), 'app.pk_inline_pkey',
    'inline PK backing index has the constraint name, in the table schema');

CREATE TABLE pk_table_level (id int, PRIMARY KEY (id));
SELECT assert_eq(constraint_names('app.pk_table_level', 'p'), 'pk_table_level_pkey',
    'table-level PRIMARY KEY in CREATE TABLE is {table}_pkey');
SELECT assert_eq(constraint_index('app.pk_table_level', 'pk_table_level_pkey'), 'app.pk_table_level_pkey',
    'table-level PK backing index has the constraint name');

CREATE TABLE pk_composite (a int, b int, PRIMARY KEY (a, b));
SELECT assert_eq(constraint_names('app.pk_composite', 'p'), 'pk_composite_pkey',
    'composite PRIMARY KEY is {table}_pkey, no column part');

CREATE TABLE pk_include (id int, a int, PRIMARY KEY (id) INCLUDE (a));
SELECT assert_eq(constraint_names('app.pk_include', 'p'), 'pk_include_pkey',
    'PRIMARY KEY ... INCLUDE is {table}_pkey, no column part');

CREATE TABLE pk_add_column (x int);
ALTER TABLE pk_add_column ADD COLUMN id int PRIMARY KEY;
SELECT assert_eq(constraint_names('app.pk_add_column', 'p'), 'pk_add_column_pkey',
    'ALTER TABLE ADD COLUMN ... PRIMARY KEY is {table}_pkey');
SELECT assert_eq(constraint_index('app.pk_add_column', 'pk_add_column_pkey'), 'app.pk_add_column_pkey',
    'ADD COLUMN PK backing index has the constraint name');

CREATE TABLE pk_add_pk (id int);
ALTER TABLE pk_add_pk ADD PRIMARY KEY (id);
SELECT assert_eq(constraint_names('app.pk_add_pk', 'p'), 'pk_add_pk_pkey',
    'ALTER TABLE ADD PRIMARY KEY is {table}_pkey');
SELECT assert_eq(constraint_index('app.pk_add_pk', 'pk_add_pk_pkey'), 'app.pk_add_pk_pkey',
    'ADD PRIMARY KEY backing index has the constraint name');

-- search_path puts app first, so the schema can only come from the qualified name.
CREATE TABLE analytics.orders (id int PRIMARY KEY);
SELECT assert_eq(constraint_names('analytics.orders', 'p'), 'orders_pkey',
    'analytics.orders PK is orders_pkey, no schema part');
SELECT assert_eq(
    coalesce((SELECT n.nspname::text FROM pg_constraint k JOIN pg_namespace n ON n.oid = k.connamespace
               WHERE k.conrelid = to_regclass('analytics.orders') AND k.contype = 'p'), '<none>'),
    'analytics',
    'analytics.orders PK constraint lives in schema analytics');
SELECT assert_eq(constraint_index('analytics.orders', 'orders_pkey'), 'analytics.orders_pkey',
    'analytics.orders PK index is analytics.orders_pkey');
SELECT assert_false(to_regclass('app.orders_pkey') IS NOT NULL,
    'no orders_pkey relation is created in the search_path schema');

CREATE TABLE pk_named (id int, CONSTRAINT my_pk PRIMARY KEY (id));
SELECT assert_eq(constraint_names('app.pk_named', 'p'), 'my_pk',
    'named PK keeps its name');
SELECT assert_eq(constraint_index('app.pk_named', 'my_pk'), 'app.my_pk',
    'named PK backing index is named after the constraint');

CREATE TABLE pk_named_alter (id int);
ALTER TABLE pk_named_alter ADD CONSTRAINT alter_pk PRIMARY KEY (id);
SELECT assert_eq(constraint_index('app.pk_named_alter', 'alter_pk'), 'app.alter_pk',
    'ADD CONSTRAINT alter_pk PRIMARY KEY names its backing index alter_pk');
