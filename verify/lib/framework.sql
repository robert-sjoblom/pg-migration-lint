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
