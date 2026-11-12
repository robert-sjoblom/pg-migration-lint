-- @claim: A generated constraint name avoids every constraint name in the table's schema, not just the table's own
-- @claim: This holds for CHECK, FK, UNIQUE and PK, across constraint kinds, and counts domain constraints too
-- @claim: Constraints and relations in a different schema are ignored: no suffix
-- @claim: Index-backed constraints (PK, UNIQUE, EXCLUDE) also avoid relation names (pg_class) in the schema; plain types do not count
-- @claim: CHECK and FK names ignore relation names entirely
-- @claim: An unnamed CREATE INDEX ignores constraint names; it only avoids relations
-- @claim: An explicit CHECK name may repeat another table's constraint name; an explicit index-backed name may not repeat a relation name
-- @min_version: 14

CREATE SCHEMA app;
CREATE SCHEMA other;
SET search_path = app, public;

CREATE TABLE ref (id int PRIMARY KEY);

-- same schema, another table
CREATE TABLE bar_check (a int, CONSTRAINT foo_check_a_check CHECK (a > 0));
CREATE TABLE foo_check (a int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.foo_check', 'c'), 'foo_check_a_check1',
    'CHECK: another table in the schema owns foo_check_a_check, so foo_check gets foo_check_a_check1');

CREATE TABLE bar_fk (a int, CONSTRAINT foo_fk_a_fkey FOREIGN KEY (a) REFERENCES ref);
CREATE TABLE foo_fk (a int REFERENCES ref);
SELECT assert_eq(constraint_names('app.foo_fk', 'f'), 'foo_fk_a_fkey1',
    'FK: another table in the schema owns foo_fk_a_fkey, so foo_fk gets foo_fk_a_fkey1');

CREATE TABLE bar_uq (a int, CONSTRAINT foo_uq_a_key UNIQUE (a));
CREATE TABLE foo_uq (a int UNIQUE);
SELECT assert_eq(constraint_names('app.foo_uq', 'u'), 'foo_uq_a_key1',
    'UNIQUE: another table in the schema owns foo_uq_a_key, so foo_uq gets foo_uq_a_key1');
SELECT assert_eq(constraint_index('app.foo_uq', 'foo_uq_a_key1'), 'app.foo_uq_a_key1',
    'UNIQUE: the suffixed constraint and its index share the name');

-- across constraint kinds
CREATE TABLE bar_kind1 (a int, CONSTRAINT foo_kind1_a_key CHECK (a > 0));
CREATE TABLE foo_kind1 (a int UNIQUE);
SELECT assert_eq(constraint_names('app.foo_kind1', 'u'), 'foo_kind1_a_key1',
    'UNIQUE avoids a CHECK named foo_kind1_a_key on another table');

CREATE TABLE bar_kind2 (a int, CONSTRAINT foo_kind2_a_check UNIQUE (a));
CREATE TABLE foo_kind2 (a int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.foo_kind2', 'c'), 'foo_kind2_a_check1',
    'CHECK avoids a UNIQUE named foo_kind2_a_check on another table');

CREATE TABLE bar_kind3 (a int, CONSTRAINT foo_kind3_pkey CHECK (a > 0));
CREATE TABLE foo_kind3 (a int PRIMARY KEY);
SELECT assert_eq(constraint_names('app.foo_kind3', 'p'), 'foo_kind3_pkey1',
    'PK avoids a CHECK named foo_kind3_pkey on another table');

CREATE DOMAIN positive_int AS int CONSTRAINT foo_domain_a_check CHECK (VALUE > 0);
CREATE TABLE foo_domain (a int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.foo_domain', 'c'), 'foo_domain_a_check1',
    'CHECK avoids a domain constraint name in the same schema');

-- a realistic collision: the table/column split is invisible after joining with _
CREATE TABLE app_user (id int UNIQUE);
CREATE TABLE app (user_id int UNIQUE);
SELECT assert_eq(constraint_names('app.app_user', 'u') || ' / ' || constraint_names('app.app', 'u'),
    'app_user_id_key / app_user_id_key1',
    'app_user(id) and app(user_id) both generate app_user_id_key; the later one gets app_user_id_key1');

-- a different schema
CREATE TABLE other.bar_iso (a int,
    CONSTRAINT foo_iso_a_check CHECK (a > 0),
    CONSTRAINT foo_iso_a_key UNIQUE (a),
    CONSTRAINT foo_iso_a_fkey FOREIGN KEY (a) REFERENCES app.ref);
CREATE TABLE other.foo_iso_pk_pkey (x int);
CREATE TABLE foo_iso (a int CHECK (a > 0) UNIQUE REFERENCES ref);
CREATE TABLE foo_iso_pk (a int PRIMARY KEY);
SELECT assert_eq(constraint_names('app.foo_iso', 'c'), 'foo_iso_a_check',
    'CHECK ignores a same-named constraint in another schema');
SELECT assert_eq(constraint_names('app.foo_iso', 'f'), 'foo_iso_a_fkey',
    'FK ignores a same-named constraint in another schema');
SELECT assert_eq(constraint_names('app.foo_iso', 'u'), 'foo_iso_a_key',
    'UNIQUE ignores a same-named constraint and index in another schema');
SELECT assert_eq(constraint_names('app.foo_iso_pk', 'p'), 'foo_iso_pk_pkey',
    'PK ignores a same-named relation in another schema');

-- relation names: index-backed constraints avoid them
CREATE TABLE rel_uq_a_key (x int);
CREATE TABLE rel_uq (a int UNIQUE);
SELECT assert_eq(constraint_names('app.rel_uq', 'u'), 'rel_uq_a_key1',
    'UNIQUE avoids a table named rel_uq_a_key');

CREATE TABLE rel_pk_pkey (x int);
CREATE TABLE rel_pk (a int PRIMARY KEY);
SELECT assert_eq(constraint_names('app.rel_pk', 'p'), 'rel_pk_pkey1',
    'PK avoids a table named rel_pk_pkey');

CREATE SEQUENCE rel_ex_a_excl;
CREATE TABLE rel_ex (a int, EXCLUDE USING btree (a WITH =));
SELECT assert_eq(constraint_names('app.rel_ex', 'x'), 'rel_ex_a_excl1',
    'EXCLUDE avoids a sequence named rel_ex_a_excl');

CREATE VIEW rel_view_a_key AS SELECT 1 AS x;
CREATE TABLE rel_view (a int UNIQUE);
SELECT assert_eq(constraint_names('app.rel_view', 'u'), 'rel_view_a_key1',
    'UNIQUE avoids a view named rel_view_a_key');

CREATE TYPE rel_composite_a_key AS (x int);
CREATE TABLE rel_composite (a int UNIQUE);
SELECT assert_eq(constraint_names('app.rel_composite', 'u'), 'rel_composite_a_key1',
    'UNIQUE avoids a composite type rel_composite_a_key (it has a pg_class row)');

CREATE TYPE rel_enum_a_key AS ENUM ('x');
CREATE TABLE rel_enum (a int UNIQUE);
SELECT assert_eq(constraint_names('app.rel_enum', 'u'), 'rel_enum_a_key',
    'UNIQUE ignores an enum type rel_enum_a_key (no pg_class row)');

-- relation names: CHECK and FK ignore them
CREATE TABLE rel_ck_a_check (x int);
CREATE TABLE rel_ck (a int CHECK (a > 0));
SELECT assert_eq(constraint_names('app.rel_ck', 'c'), 'rel_ck_a_check',
    'CHECK ignores a table named rel_ck_a_check');

CREATE TABLE rel_fk_holder (x int);
CREATE INDEX rel_fk_a_fkey ON rel_fk_holder (x);
CREATE TABLE rel_fk (a int REFERENCES ref);
SELECT assert_eq(constraint_names('app.rel_fk', 'f'), 'rel_fk_a_fkey',
    'FK ignores an index named rel_fk_a_fkey');

-- CREATE INDEX ignores constraint names
CREATE TABLE ix (a int, CONSTRAINT ix_a_idx CHECK (a > 0));
CREATE INDEX ON ix (a);
SELECT assert_eq(index_names('app.ix'), 'app.ix_a_idx',
    'CREATE INDEX ignores a CHECK constraint named ix_a_idx');

-- explicit names
CREATE TABLE ex_owner (a int CHECK (a > 0));
CREATE TABLE ex_copy (a int, CONSTRAINT ex_owner_a_check CHECK (a > 0));
SELECT assert_eq(constraint_names('app.ex_copy', 'c'), 'ex_owner_a_check',
    'an explicit CHECK name may repeat another table''s constraint name in the schema');

CREATE TABLE ex_uq_owner (a int UNIQUE);
DO $$
BEGIN
    CREATE TABLE ex_uq_copy (a int, CONSTRAINT ex_uq_owner_a_key UNIQUE (a));
    PERFORM assert_true(false, 'explicit UNIQUE name equal to an existing index name should fail');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_eq(SQLSTATE || ' ' || SQLERRM,
        '42P07 relation "ex_uq_owner_a_key" already exists',
        'an explicit UNIQUE name equal to an existing index name fails with 42P07');
END;
$$;
