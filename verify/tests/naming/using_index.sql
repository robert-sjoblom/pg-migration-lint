-- @claim: ADD PRIMARY KEY USING INDEX idx (no constraint name) names the constraint idx; the index keeps its name
-- @claim: ADD UNIQUE USING INDEX idx behaves the same
-- @claim: ADD CONSTRAINT my_pk PRIMARY KEY USING INDEX idx renames the index to my_pk; the same for UNIQUE
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ui_pk (a int NOT NULL);
CREATE UNIQUE INDEX ui_pk_a_idx ON ui_pk (a);
ALTER TABLE ui_pk ADD PRIMARY KEY USING INDEX ui_pk_a_idx;
SELECT assert_eq(constraint_names('app.ui_pk', 'p'), 'ui_pk_a_idx',
    'ADD PRIMARY KEY USING INDEX: the constraint takes the index name, not {table}_pkey');
SELECT assert_eq(index_names('app.ui_pk'), 'app.ui_pk_a_idx',
    'ADD PRIMARY KEY USING INDEX: the index is not renamed');

CREATE TABLE ui_uq (a int);
CREATE UNIQUE INDEX ui_uq_a_idx ON ui_uq (a);
ALTER TABLE ui_uq ADD UNIQUE USING INDEX ui_uq_a_idx;
SELECT assert_eq(constraint_names('app.ui_uq', 'u'), 'ui_uq_a_idx',
    'ADD UNIQUE USING INDEX: the constraint takes the index name, not {table}_{col}_key');
SELECT assert_eq(index_names('app.ui_uq'), 'app.ui_uq_a_idx',
    'ADD UNIQUE USING INDEX: the index is not renamed');

CREATE TABLE ui_named_pk (a int NOT NULL);
CREATE UNIQUE INDEX ui_named_pk_idx ON ui_named_pk (a);
ALTER TABLE ui_named_pk ADD CONSTRAINT my_pk PRIMARY KEY USING INDEX ui_named_pk_idx;
SELECT assert_eq(constraint_names('app.ui_named_pk', 'p'), 'my_pk',
    'ADD CONSTRAINT my_pk PRIMARY KEY USING INDEX: the constraint is my_pk');
SELECT assert_eq(index_names('app.ui_named_pk'), 'app.my_pk',
    'ADD CONSTRAINT my_pk PRIMARY KEY USING INDEX: the index is renamed to my_pk');

CREATE TABLE ui_named_uq (a int);
CREATE UNIQUE INDEX ui_named_uq_idx ON ui_named_uq (a);
ALTER TABLE ui_named_uq ADD CONSTRAINT my_uq UNIQUE USING INDEX ui_named_uq_idx;
SELECT assert_eq(constraint_names('app.ui_named_uq', 'u'), 'my_uq',
    'ADD CONSTRAINT my_uq UNIQUE USING INDEX: the constraint is my_uq');
SELECT assert_eq(index_names('app.ui_named_uq'), 'app.my_uq',
    'ADD CONSTRAINT my_uq UNIQUE USING INDEX: the index is renamed to my_uq');
