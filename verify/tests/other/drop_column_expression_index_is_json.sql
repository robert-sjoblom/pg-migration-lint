-- @claim: From PG 16, DROP COLUMN drops an expression index that tests the column with IS JSON
-- @min_version: 16

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ex_is_json (doc text);
CREATE INDEX ex_is_json_idx ON ex_is_json ((doc IS JSON));
ALTER TABLE ex_is_json DROP COLUMN doc;
SELECT assert_eq(index_names('app.ex_is_json'), '<none>',
    'doc IS JSON: dropping doc drops the index');
