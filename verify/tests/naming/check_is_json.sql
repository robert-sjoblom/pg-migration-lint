-- @claim: From PG 16, a column tested with IS JSON counts toward an unnamed CHECK's name like any other referenced column
-- @min_version: 16

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ck_is_json (doc text, CHECK (doc IS JSON OBJECT));
SELECT assert_eq(constraint_names('app.ck_is_json', 'c'), 'ck_is_json_doc_check',
    'CHECK (doc IS JSON OBJECT) references doc: {table}_{col}_check');

CREATE TABLE ck_is_json_two (doc text, alt text, CHECK (doc IS JSON OR alt IS JSON));
SELECT assert_eq(constraint_names('app.ck_is_json_two', 'c'), 'ck_is_json_two_check',
    'two columns tested with IS JSON give {table}_check');
