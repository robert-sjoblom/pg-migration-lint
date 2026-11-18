-- @claim: DROP INDEX of the index behind a PRIMARY KEY, UNIQUE or EXCLUDE constraint fails with 2BP01 "cannot drop index ... because constraint ... requires it" (PGM027)
-- @claim: IF EXISTS, CASCADE and CONCURRENTLY do not change that
-- @claim: DROP INDEX naming a constraint's index and a plain index fails as a whole; the plain index stays
-- @claim: The failed statement leaves the constraint and its index in place
-- @min_version: 14

CREATE SCHEMA app;
SET search_path = app, public;

CREATE FUNCTION statement_error(stmt text) RETURNS text
LANGUAGE plpgsql AS $$
BEGIN
    EXECUTE stmt;
    RETURN '<no error>';
EXCEPTION WHEN OTHERS THEN
    RETURN SQLSTATE || ' ' || SQLERRM;
END;
$$;

CREATE TABLE orders (
    id int,
    code text,
    slot int,
    ref int,
    CONSTRAINT orders_pkey PRIMARY KEY (id),
    CONSTRAINT orders_code_key UNIQUE (code),
    CONSTRAINT orders_slot_excl EXCLUDE USING btree (slot WITH =));
CREATE UNIQUE INDEX orders_ref_idx ON orders (ref);

SELECT assert_eq(statement_error('DROP INDEX orders_pkey'),
    '2BP01 cannot drop index orders_pkey because constraint orders_pkey on table orders requires it',
    'PRIMARY KEY: DROP INDEX of its index fails');
SELECT assert_eq(statement_error('DROP INDEX orders_code_key'),
    '2BP01 cannot drop index orders_code_key because constraint orders_code_key on table orders requires it',
    'UNIQUE: DROP INDEX of its index fails');
SELECT assert_eq(statement_error('DROP INDEX orders_slot_excl'),
    '2BP01 cannot drop index orders_slot_excl because constraint orders_slot_excl on table orders requires it',
    'EXCLUDE: DROP INDEX of its index fails');
SELECT assert_eq(statement_error('DROP INDEX IF EXISTS orders_code_key'),
    '2BP01 cannot drop index orders_code_key because constraint orders_code_key on table orders requires it',
    'IF EXISTS does not make it succeed');
SELECT assert_eq(statement_error('DROP INDEX orders_pkey CASCADE'),
    '2BP01 cannot drop index orders_pkey because constraint orders_pkey on table orders requires it',
    'CASCADE does not drop the constraint along');
SELECT assert_eq(statement_error('DROP INDEX orders_ref_idx, orders_code_key'),
    '2BP01 cannot drop index orders_code_key because constraint orders_code_key on table orders requires it',
    'a DROP INDEX naming a constraint''s index and a plain index fails');

-- CONCURRENTLY cannot run inside the function's transaction, so it runs at top level.
DROP INDEX CONCURRENTLY orders_pkey;
SELECT assert_eq(:'LAST_ERROR_SQLSTATE' || ' ' || :'LAST_ERROR_MESSAGE',
    '2BP01 cannot drop index orders_pkey because constraint orders_pkey on table orders requires it',
    'CONCURRENTLY does not make it succeed');

SELECT assert_eq(
    constraint_names('app.orders', 'p') || ' ' || constraint_names('app.orders', 'u') || ' '
        || constraint_names('app.orders', 'x') || ' ' || index_names('app.orders'),
    'orders_pkey orders_code_key orders_slot_excl app.orders_code_key,app.orders_pkey,app.orders_ref_idx,app.orders_slot_excl',
    'the failed drops leave every constraint and index in place, the plain index included');
