-- @claim: Within one statement, a generated name avoids the names Postgres has already created in that statement, in creation order, not textual order
-- @claim: CREATE TABLE names its CHECKs first, then the PRIMARY KEY, then UNIQUE and EXCLUDE in textual order, then FOREIGN KEYs in textual order
-- @claim: ALTER TABLE names index-backed constraints first, in textual order, then CHECKs and FOREIGN KEYs together in textual order
-- @claim: ALTER TABLE attaches USING INDEX constraints before it creates any other index-backed constraint
-- @claim: ALTER TABLE names the constraints of every ADD COLUMN before those of any ADD CONSTRAINT, whatever the textual order
-- @claim: ALTER TABLE runs DROP CONSTRAINT before any ADD, so a name dropped in the same statement is free whatever the textual order
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE ref (id int PRIMARY KEY);

-- CREATE TABLE
CREATE TABLE ct_check_first (a int UNIQUE, CONSTRAINT ct_check_first_a_key CHECK (a > 0));
SELECT assert_eq(constraint_names('app.ct_check_first', 'u'), 'ct_check_first_a_key1',
    'CREATE TABLE: a CHECK written after a UNIQUE is still named first, so the UNIQUE gets _key1');

CREATE TABLE ct_pk_first (a int UNIQUE, b int, CONSTRAINT ct_pk_first_a_key PRIMARY KEY (b));
SELECT assert_eq(constraint_names('app.ct_pk_first', 'u'), 'ct_pk_first_a_key1',
    'CREATE TABLE: a PRIMARY KEY written after a UNIQUE is created first, so the UNIQUE gets _key1');

CREATE TABLE ct_unique_textual (a int, b int, CONSTRAINT ct_unique_textual_a_excl UNIQUE (b), EXCLUDE USING btree (a WITH =));
SELECT assert_eq(constraint_names('app.ct_unique_textual', 'x'), 'ct_unique_textual_a_excl1',
    'CREATE TABLE: a UNIQUE written before an EXCLUDE is created first, so the EXCLUDE gets _excl1');

DO $$
BEGIN
    CREATE TABLE ct_exclude_textual (a int, b int, EXCLUDE USING btree (a WITH =), CONSTRAINT ct_exclude_textual_a_excl UNIQUE (b));
    PERFORM assert_true(false, 'an EXCLUDE written before a UNIQUE holding its generated name should make CREATE TABLE fail');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_eq(SQLSTATE || ' ' || SQLERRM,
        '42P07 relation "ct_exclude_textual_a_excl" already exists',
        'CREATE TABLE: an EXCLUDE written before a UNIQUE takes the name first; the UNIQUE then fails with 42P07');
END;
$$;

CREATE TABLE ct_fk_after_index (a int REFERENCES ref, CONSTRAINT ct_fk_after_index_a_fkey UNIQUE (a));
SELECT assert_eq(constraint_names('app.ct_fk_after_index', 'f'), 'ct_fk_after_index_a_fkey1',
    'CREATE TABLE: a FOREIGN KEY written before a UNIQUE is named after it, so it gets _fkey1');

CREATE TABLE ct_fk_after_check (a int REFERENCES ref, CONSTRAINT ct_fk_after_check_a_fkey CHECK (a > 0));
SELECT assert_eq(constraint_names('app.ct_fk_after_check', 'f'), 'ct_fk_after_check_a_fkey1',
    'CREATE TABLE: a FOREIGN KEY written before a CHECK is named after it, so it gets _fkey1');

-- ALTER TABLE
CREATE TABLE at_index_first (a int, b int);
ALTER TABLE at_index_first ADD CHECK (a > 0), ADD CONSTRAINT at_index_first_a_check UNIQUE (b);
SELECT assert_eq(constraint_names('app.at_index_first', 'c'), 'at_index_first_a_check1',
    'ALTER TABLE: a UNIQUE written after a CHECK is created first, so the CHECK gets _check1');

CREATE TABLE at_fk_after_index (a int);
ALTER TABLE at_fk_after_index ADD FOREIGN KEY (a) REFERENCES ref, ADD CONSTRAINT at_fk_after_index_a_fkey UNIQUE (a);
SELECT assert_eq(constraint_names('app.at_fk_after_index', 'f'), 'at_fk_after_index_a_fkey1',
    'ALTER TABLE: a UNIQUE written after a FOREIGN KEY is created first, so the FOREIGN KEY gets _fkey1');

CREATE TABLE at_check_fk_textual (a int);
ALTER TABLE at_check_fk_textual ADD CONSTRAINT at_check_fk_textual_a_check FOREIGN KEY (a) REFERENCES ref, ADD CHECK (a > 0);
SELECT assert_eq(constraint_names('app.at_check_fk_textual', 'c'), 'at_check_fk_textual_a_check1',
    'ALTER TABLE: CHECK and FOREIGN KEY share a pass in textual order, so a CHECK after a FOREIGN KEY holding its name gets _check1');

DO $$
BEGIN
    CREATE TABLE at_pk_textual (a int, b int);
    ALTER TABLE at_pk_textual ADD UNIQUE (a), ADD CONSTRAINT at_pk_textual_a_key PRIMARY KEY (b);
    PERFORM assert_true(false, 'a UNIQUE written before a PRIMARY KEY holding its generated name should make ALTER TABLE fail');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_eq(SQLSTATE || ' ' || SQLERRM,
        '42P07 relation "at_pk_textual_a_key" already exists',
        'ALTER TABLE: index-backed constraints are created in textual order, PRIMARY KEY included; the PRIMARY KEY then fails with 42P07');
END;
$$;

CREATE TABLE at_drop_after (a int UNIQUE);
ALTER TABLE at_drop_after ADD UNIQUE (a), DROP CONSTRAINT at_drop_after_a_key;
SELECT assert_eq(constraint_names('app.at_drop_after', 'u'), 'at_drop_after_a_key',
    'ALTER TABLE: DROP CONSTRAINT written after ADD UNIQUE still runs first, so the new UNIQUE reuses the name');

CREATE TABLE at_drop_before (a int UNIQUE);
ALTER TABLE at_drop_before DROP CONSTRAINT at_drop_before_a_key, ADD UNIQUE (a);
SELECT assert_eq(constraint_names('app.at_drop_before', 'u'), 'at_drop_before_a_key',
    'ALTER TABLE: DROP CONSTRAINT then ADD UNIQUE reuses the dropped name');

CREATE TABLE at_using_first (a int, b int NOT NULL);
CREATE UNIQUE INDEX at_using_first_idx ON at_using_first (b);
ALTER TABLE at_using_first ADD UNIQUE (a), ADD CONSTRAINT at_using_first_a_key UNIQUE USING INDEX at_using_first_idx;
SELECT assert_eq(constraint_names('app.at_using_first', 'u'), 'at_using_first_a_key,at_using_first_a_key1',
    'ALTER TABLE: a USING INDEX constraint written after ADD UNIQUE is attached first, so the new UNIQUE gets _key1');
SELECT assert_eq(constraint_index('app.at_using_first', 'at_using_first_a_key'), 'app.at_using_first_a_key',
    'ALTER TABLE: the USING INDEX constraint renamed its index to at_using_first_a_key');

CREATE TABLE at_add_column_first (a int);
ALTER TABLE at_add_column_first ADD CHECK (a > 0), ADD COLUMN c int CONSTRAINT at_add_column_first_a_check CHECK (c > 0);
SELECT assert_eq(
    coalesce((SELECT string_agg(conname || '=' || pg_get_constraintdef(oid), '; ' ORDER BY conname)
                FROM pg_constraint WHERE conrelid = to_regclass('app.at_add_column_first')), '<none>'),
    'at_add_column_first_a_check=CHECK ((c > 0)); at_add_column_first_a_check1=CHECK ((a > 0))',
    'ALTER TABLE: an ADD COLUMN written after ADD CHECK has its CHECK named first, so ADD CHECK gets _check1');
