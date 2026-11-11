-- @claim: Within one CREATE TABLE, an index-backed constraint identical to an earlier one is dropped, not suffixed
-- @claim: So a int UNIQUE, UNIQUE (a) gives one constraint t_a_key; PRIMARY KEY + UNIQUE on one column gives only t_pkey
-- @claim: Separate statements (or two ADD UNIQUE in one ALTER TABLE) are not deduplicated and get t_a_key, t_a_key1, t_a_key2
-- @claim: CHECK and FK duplicates are never deduplicated; they get _check1, _fkey1, numbered in textual order
-- @claim: CREATE INDEX skips a generated name already taken by any relation in the schema: t_a_idx1, then t_a_idx2
-- @claim: An explicit name earlier in a CREATE TABLE pushes a later generated name to suffix 1; a generated name earlier makes a later identical explicit name fail
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE TABLE dedup_unique (a int UNIQUE, UNIQUE (a));
SELECT assert_eq(constraint_names('app.dedup_unique', 'u'), 'dedup_unique_a_key',
    'a int UNIQUE, UNIQUE (a): one constraint, no _key1');
SELECT assert_eq(index_names('app.dedup_unique'), 'app.dedup_unique_a_key',
    'a int UNIQUE, UNIQUE (a): one index');

CREATE TABLE dedup_pk (a int PRIMARY KEY UNIQUE);
SELECT assert_eq(constraint_names('app.dedup_pk', 'p') || ' / ' || constraint_names('app.dedup_pk', 'u'),
    'dedup_pk_pkey / <none>',
    'PRIMARY KEY UNIQUE on one column: only the PK survives');

CREATE TABLE dedup_named (a int UNIQUE, CONSTRAINT named_u UNIQUE (a));
SELECT assert_eq(constraint_names('app.dedup_named', 'u'), 'named_u',
    'unnamed UNIQUE plus identical named UNIQUE: only the named one survives');

CREATE TABLE dedup_exclude (a int, EXCLUDE USING btree (a WITH =), EXCLUDE USING btree (a WITH =));
SELECT assert_eq(constraint_names('app.dedup_exclude', 'x'), 'dedup_exclude_a_excl',
    'two identical EXCLUDE in one CREATE TABLE: one constraint');

CREATE TABLE dedup_differs (a int UNIQUE, b int, UNIQUE (a) INCLUDE (b), UNIQUE (a) DEFERRABLE);
SELECT assert_eq(constraint_names('app.dedup_differs', 'u'), 'dedup_differs_a_b_key,dedup_differs_a_key,dedup_differs_a_key1',
    'UNIQUE (a) differing in INCLUDE or DEFERRABLE is not deduplicated');

CREATE TABLE seq_unique (a int);
ALTER TABLE seq_unique ADD UNIQUE (a);
ALTER TABLE seq_unique ADD UNIQUE (a);
ALTER TABLE seq_unique ADD UNIQUE (a);
SELECT assert_eq(constraint_names('app.seq_unique', 'u'), 'seq_unique_a_key,seq_unique_a_key1,seq_unique_a_key2',
    'three separate ADD UNIQUE (a): _key, _key1, _key2');

CREATE TABLE seq_one_alter (a int);
ALTER TABLE seq_one_alter ADD UNIQUE (a), ADD UNIQUE (a);
SELECT assert_eq(constraint_names('app.seq_one_alter', 'u'), 'seq_one_alter_a_key,seq_one_alter_a_key1',
    'two ADD UNIQUE (a) in one ALTER TABLE are not deduplicated: _key, _key1');

CREATE TABLE seq_check (a int CHECK (a > 0), CHECK (a > 0));
SELECT assert_eq(constraint_names('app.seq_check', 'c'), 'seq_check_a_check,seq_check_a_check1',
    'identical CHECKs are not deduplicated: _check, _check1');

CREATE TABLE seq_check_order (a int, b int, CHECK (a < b), CHECK (true), CHECK (b > a + 1));
SELECT assert_eq(
    coalesce((SELECT string_agg(conname || '=' || pg_get_constraintdef(oid), '; ' ORDER BY conname)
                FROM pg_constraint WHERE conrelid = to_regclass('app.seq_check_order')), '<none>'),
    'seq_check_order_check=CHECK ((a < b)); seq_check_order_check1=CHECK (true); seq_check_order_check2=CHECK ((b > (a + 1)))',
    'suffixes follow textual order: _check, _check1, _check2');

CREATE TABLE ref (id int PRIMARY KEY);
CREATE TABLE seq_fk (a int REFERENCES ref, FOREIGN KEY (a) REFERENCES ref);
SELECT assert_eq(constraint_names('app.seq_fk', 'f'), 'seq_fk_a_fkey,seq_fk_a_fkey1',
    'duplicate FKs are not deduplicated: _fkey, _fkey1');

CREATE TABLE seq_index (a int);
CREATE INDEX ON seq_index (a);
CREATE INDEX ON seq_index (a);
CREATE INDEX ON seq_index (a);
SELECT assert_eq(index_names('app.seq_index'), 'app.seq_index_a_idx,app.seq_index_a_idx1,app.seq_index_a_idx2',
    'three CREATE INDEX ON t (a): _idx, _idx1, _idx2');

CREATE TABLE ci (a int);
CREATE TABLE ci_a_idx (x int);
CREATE INDEX ON ci (a);
SELECT assert_eq(index_names('app.ci'), 'app.ci_a_idx1',
    'CREATE INDEX skips a name taken by a table: ci_a_idx1');

CREATE TABLE ci2 (a int);
CREATE TABLE ci2_a_idx (x int);
CREATE INDEX ci2_a_idx1 ON ci2_a_idx (x);
CREATE INDEX ON ci2 (a);
SELECT assert_eq(index_names('app.ci2'), 'app.ci2_a_idx2',
    'with ci2_a_idx (table) and ci2_a_idx1 (index on another table) taken: ci2_a_idx2');

CREATE TABLE explicit_first (CONSTRAINT explicit_first_a_check CHECK (a < 10), a int CHECK (a > 0));
SELECT assert_eq(
    coalesce((SELECT string_agg(conname || '=' || pg_get_constraintdef(oid), '; ' ORDER BY conname)
                FROM pg_constraint WHERE conrelid = to_regclass('app.explicit_first')), '<none>'),
    'explicit_first_a_check=CHECK ((a < 10)); explicit_first_a_check1=CHECK ((a > 0))',
    'explicit name earlier in the statement: the later generated CHECK gets _check1');

DO $$
BEGIN
    CREATE TABLE generated_first (a int CHECK (a > 0), CONSTRAINT generated_first_a_check CHECK (a < 10));
    PERFORM assert_true(false, 'generated CHECK name before an identical explicit name should fail');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_eq(SQLSTATE || ' ' || SQLERRM,
        '42710 check constraint "generated_first_a_check" already exists',
        'generated CHECK name before an identical explicit name: CREATE TABLE fails with 42710');
END;
$$;

DO $$
BEGIN
    CREATE TABLE generated_first_u (a int UNIQUE, b int, CONSTRAINT generated_first_u_a_key UNIQUE (b));
    PERFORM assert_true(false, 'generated UNIQUE name before an identical explicit name should fail');
EXCEPTION WHEN OTHERS THEN
    PERFORM assert_eq(SQLSTATE || ' ' || SQLERRM,
        '42P07 relation "generated_first_u_a_key" already exists',
        'generated UNIQUE name before an identical explicit name: CREATE TABLE fails with 42P07');
END;
$$;
