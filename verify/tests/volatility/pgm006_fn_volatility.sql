-- @claim: Key PostgreSQL functions have expected provolatile in pg_proc (PGM006)
-- @min_version: 14

-- Helper: look up provolatile for a given function name.
-- For overloaded names, takes MAX (most volatile wins, matching our generator).
CREATE OR REPLACE FUNCTION _get_volatility(fn_name text) RETURNS char AS $$
    SELECT MAX(provolatile)
    FROM pg_proc
    WHERE proname = fn_name
      AND pronamespace = 'pg_catalog'::regnamespace
      AND prokind = 'f';
$$ LANGUAGE sql;

-- -----------------------------------------------------------------------
-- Volatile functions (provolatile = 'v')
-- -----------------------------------------------------------------------

SELECT assert_eq(
    _get_volatility('random'),
    'v',
    'random() should be volatile'
);

SELECT assert_eq(
    _get_volatility('clock_timestamp'),
    'v',
    'clock_timestamp() should be volatile'
);

SELECT assert_eq(
    _get_volatility('gen_random_uuid'),
    'v',
    'gen_random_uuid() should be volatile'
);

SELECT assert_eq(
    _get_volatility('nextval'),
    'v',
    'nextval() should be volatile'
);

SELECT assert_eq(
    _get_volatility('timeofday'),
    'v',
    'timeofday() should be volatile'
);

-- -----------------------------------------------------------------------
-- Stable functions (provolatile = 's')
-- -----------------------------------------------------------------------

SELECT assert_eq(
    _get_volatility('now'),
    's',
    'now() should be stable'
);

SELECT assert_eq(
    _get_volatility('statement_timestamp'),
    's',
    'statement_timestamp() should be stable'
);

SELECT assert_eq(
    _get_volatility('transaction_timestamp'),
    's',
    'transaction_timestamp() should be stable'
);

SELECT assert_eq(
    _get_volatility('txid_current'),
    's',
    'txid_current() should be stable'
);

-- -----------------------------------------------------------------------
-- Immutable functions (provolatile = 'i')
-- -----------------------------------------------------------------------

SELECT assert_eq(
    _get_volatility('abs'),
    'i',
    'abs() should be immutable'
);

SELECT assert_eq(
    _get_volatility('lower'),
    'i',
    'lower() should be immutable'
);

SELECT assert_eq(
    _get_volatility('md5'),
    'i',
    'md5() should be immutable'
);

-- Cleanup
DROP FUNCTION _get_volatility(text);
