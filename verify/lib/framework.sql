-- framework.sql: Shared helpers for PostgreSQL behavior verification tests
-- Loaded before each test file via psql.

-- Result tracking table
DROP TABLE IF EXISTS _verify_results CASCADE;
CREATE TABLE _verify_results (
    id serial PRIMARY KEY,
    test_file text,
    label text NOT NULL,
    passed bool NOT NULL,
    detail text DEFAULT ''
);

-- Current test file name (set by runner before each test)
-- Uses a temp table so each session gets its own value.
CREATE OR REPLACE FUNCTION _set_test_file(filename text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    PERFORM set_config('verify.test_file', filename, false);
END;
$$;

CREATE OR REPLACE FUNCTION _get_test_file() RETURNS text
LANGUAGE sql AS $$
    SELECT current_setting('verify.test_file', true);
$$;

--------------------------------------------------------------------------------
-- Basic assertions
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION assert_true(condition bool, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, COALESCE(condition, false),
            CASE WHEN COALESCE(condition, false) THEN 'OK' ELSE 'condition was false or NULL' END);
END;
$$;

CREATE OR REPLACE FUNCTION assert_false(condition bool, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, NOT COALESCE(condition, true),
            CASE WHEN NOT COALESCE(condition, true) THEN 'OK' ELSE 'condition was true' END);
END;
$$;

CREATE OR REPLACE FUNCTION assert_eq(actual text, expected text, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, actual = expected,
            CASE WHEN actual = expected THEN 'OK'
                 ELSE format('expected %s, got %s', expected, actual) END);
END;
$$;

CREATE OR REPLACE FUNCTION assert_eq(actual bigint, expected bigint, label text) RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, actual = expected,
            CASE WHEN actual = expected THEN 'OK'
                 ELSE format('expected %s, got %s', expected, actual) END);
END;
$$;

--------------------------------------------------------------------------------
-- Table rewrite detection via event trigger
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION rewrite_trap_setup() RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    -- Clean up any previous trap
    DROP EVENT TRIGGER IF EXISTS _rewrite_trap;
    DROP TABLE IF EXISTS _verify_rewrite_log;

    CREATE TABLE _verify_rewrite_log(happened bool NOT NULL DEFAULT false);
    INSERT INTO _verify_rewrite_log VALUES (false);

    CREATE OR REPLACE FUNCTION _on_rewrite() RETURNS event_trigger
    LANGUAGE plpgsql AS $fn$
    BEGIN
        UPDATE _verify_rewrite_log SET happened = true;
    END;
    $fn$;

    CREATE EVENT TRIGGER _rewrite_trap ON table_rewrite
        EXECUTE FUNCTION _on_rewrite();
END;
$$;

CREATE OR REPLACE FUNCTION rewrite_trap_fired() RETURNS bool
LANGUAGE plpgsql AS $$
BEGIN
    RETURN (SELECT happened FROM _verify_rewrite_log LIMIT 1);
END;
$$;

CREATE OR REPLACE FUNCTION rewrite_trap_reset() RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    UPDATE _verify_rewrite_log SET happened = false;
END;
$$;

CREATE OR REPLACE FUNCTION rewrite_trap_teardown() RETURNS void
LANGUAGE plpgsql AS $$
BEGIN
    DROP EVENT TRIGGER IF EXISTS _rewrite_trap;
    DROP TABLE IF EXISTS _verify_rewrite_log;
END;
$$;

--------------------------------------------------------------------------------
-- Lock conflict probes via dblink
--
-- These functions run DDL in the current transaction, then use dblink to open
-- a second connection that attempts a conflicting operation with a short
-- lock_timeout. If the probe times out, the DDL holds a conflicting lock.
--------------------------------------------------------------------------------

--------------------------------------------------------------------------------
-- EXPLAIN helper — captures EXPLAIN output as a single text string
-- (EXPLAIN cannot be used as a subquery; this function works around that)
--------------------------------------------------------------------------------

CREATE OR REPLACE FUNCTION get_explain(query text) RETURNS text
LANGUAGE plpgsql AS $$
DECLARE
    result text := '';
    rec record;
BEGIN
    FOR rec IN EXECUTE 'EXPLAIN ' || query LOOP
        result := result || rec."QUERY PLAN" || ' ';
    END LOOP;
    RETURN result;
END;
$$;

CREATE OR REPLACE FUNCTION assert_explain_contains(query text, pattern text, label text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    plan_text text;
BEGIN
    plan_text := get_explain(query);
    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, position(pattern in plan_text) > 0,
            CASE WHEN position(pattern in plan_text) > 0
                 THEN format('OK — plan contains "%s"', pattern)
                 ELSE format('FAIL — plan does not contain "%s": %s', pattern, plan_text) END);
END;
$$;

CREATE EXTENSION IF NOT EXISTS dblink;

-- _wrap_probe: wraps any SQL (including SELECT) into a DO block so dblink_exec
-- can execute it without complaining about result-returning statements.
CREATE OR REPLACE FUNCTION _wrap_probe(sql text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
    SELECT format('DO $probe$ BEGIN EXECUTE %L; END; $probe$', sql);
$$;

-- assert_lock_blocks: expects the probe to FAIL (lock timeout)
-- ddl_sql:   the DDL to execute in the current transaction (holds lock)
-- probe_sql: the SQL to attempt from a second connection (should be blocked)
CREATE OR REPLACE FUNCTION assert_lock_blocks(
    ddl_sql text,
    probe_sql text,
    label text
) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    probe_failed bool := false;
    err_msg text;
BEGIN
    -- Execute DDL in current transaction
    EXECUTE ddl_sql;

    -- Clean up any stale probe connection
    BEGIN
        PERFORM dblink_disconnect('probe');
    EXCEPTION WHEN OTHERS THEN
        NULL;
    END;

    -- Open probe connection
    PERFORM dblink_connect('probe', format(
        'dbname=%s user=postgres password=test',
        current_database()
    ));
    PERFORM dblink_exec('probe', 'SET lock_timeout = ''100ms''');

    -- Try probe — should fail with lock timeout
    -- Wrap in DO block to handle SELECT statements (dblink_exec rejects results)
    BEGIN
        PERFORM dblink_exec('probe', _wrap_probe(probe_sql));
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS err_msg = MESSAGE_TEXT;
        probe_failed := true;
    END;

    PERFORM dblink_disconnect('probe');

    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, probe_failed,
            CASE WHEN probe_failed
                 THEN format('OK — probe blocked: %s', err_msg)
                 ELSE 'FAIL — probe succeeded (lock did NOT block)' END);
END;
$$;

-- assert_lock_allows: expects the probe to SUCCEED
CREATE OR REPLACE FUNCTION assert_lock_allows(
    ddl_sql text,
    probe_sql text,
    label text
) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
    probe_failed bool := false;
    err_msg text;
BEGIN
    -- Execute DDL in current transaction
    EXECUTE ddl_sql;

    -- Clean up any stale probe connection
    BEGIN
        PERFORM dblink_disconnect('probe');
    EXCEPTION WHEN OTHERS THEN
        NULL;
    END;

    -- Open probe connection
    PERFORM dblink_connect('probe', format(
        'dbname=%s user=postgres password=test',
        current_database()
    ));
    PERFORM dblink_exec('probe', 'SET lock_timeout = ''100ms''');

    -- Try probe — should succeed
    -- Wrap in DO block to handle SELECT statements (dblink_exec rejects results)
    BEGIN
        PERFORM dblink_exec('probe', _wrap_probe(probe_sql));
    EXCEPTION WHEN OTHERS THEN
        GET STACKED DIAGNOSTICS err_msg = MESSAGE_TEXT;
        probe_failed := true;
    END;

    PERFORM dblink_disconnect('probe');

    INSERT INTO _verify_results(test_file, label, passed, detail)
    VALUES (_get_test_file(), label, NOT probe_failed,
            CASE WHEN NOT probe_failed
                 THEN 'OK — probe succeeded (lock allows operation)'
                 ELSE format('FAIL — probe blocked: %s', err_msg) END);
END;
$$;
