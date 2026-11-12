-- @claim: On every version, NOT NULL and PRIMARY KEY columns have pg_attribute.attnotnull set
-- @claim: Before PG 18, a table's NOT NULL is stored only as attnotnull: no pg_constraint row and no name
-- @claim: From PG 18, each NOT NULL column also has a contype 'n' row in pg_constraint (names: not_null_catalogued.sql)
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE nn (id int PRIMARY KEY, a int NOT NULL, b int);
ALTER TABLE nn ALTER COLUMN b SET NOT NULL;

SELECT assert_eq(
    coalesce((SELECT string_agg(attname || '=' || attnotnull, ',' ORDER BY attnum)
                FROM pg_attribute WHERE attrelid = to_regclass('app.nn') AND attnum > 0), '<none>'),
    'id=true,a=true,b=true',
    'attnotnull is set for PK, inline NOT NULL and SET NOT NULL columns');

SELECT assert_eq(
    (SELECT count(*) FROM pg_constraint WHERE conrelid = to_regclass('app.nn') AND contype = 'n'),
    CASE WHEN current_setting('server_version_num')::int >= 180000 THEN 3 ELSE 0 END::bigint,
    'pg_constraint has one contype n row per NOT NULL column from PG 18, none before');

SELECT assert_eq(
    (SELECT count(*) FROM pg_constraint WHERE conrelid = to_regclass('app.nn') AND contype <> 'n'),
    1::bigint,
    'apart from NOT NULL rows, only the PK is in pg_constraint');
