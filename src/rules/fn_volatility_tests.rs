//! Tests for the auto-generated fn_volatility module.

use super::fn_volatility::{self, FnVolatility};

/// The VOLATILE, STABLE, and IMMUTABLE arrays must be sorted for binary search.
#[test]
fn arrays_are_sorted() {
    // We can't access the private arrays directly, but we can validate that
    // lookup finds functions that we know exist. The real sort correctness
    // is tested by verifying that known functions in each category are found.
    // If the arrays were unsorted, binary search would miss entries.

    // Volatile functions at various positions in the alphabet
    for name in ["clock_timestamp", "gen_random_uuid", "random", "timeofday"] {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Volatile),
            "{name} should be found as Volatile (binary search correctness)"
        );
    }

    // Stable functions at various positions
    for name in [
        "now",
        "statement_timestamp",
        "transaction_timestamp",
        "txid_current",
    ] {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Stable),
            "{name} should be found as Stable (binary search correctness)"
        );
    }

    // Immutable functions at various positions
    for name in ["abs", "lower", "md5", "upper"] {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Immutable),
            "{name} should be found as Immutable (binary search correctness)"
        );
    }
}

/// Smoke tests for key volatile functions.
#[test]
fn volatile_functions() {
    let volatile_fns = [
        "random",
        "gen_random_uuid",
        "clock_timestamp",
        "timeofday",
        "nextval",
        "setval",
    ];
    for name in volatile_fns {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Volatile),
            "{name} should be Volatile"
        );
    }
}

/// Smoke tests for key stable functions.
#[test]
fn stable_functions() {
    let stable_fns = [
        "now",
        "statement_timestamp",
        "transaction_timestamp",
        "txid_current",
        "current_schema",
        "current_user",
    ];
    for name in stable_fns {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Stable),
            "{name} should be Stable"
        );
    }
}

/// Smoke tests for key immutable functions.
#[test]
fn immutable_functions() {
    let immutable_fns = ["abs", "lower", "upper", "md5", "btrim", "replace"];
    for name in immutable_fns {
        assert_eq!(
            fn_volatility::lookup(name),
            Some(FnVolatility::Immutable),
            "{name} should be Immutable"
        );
    }
}

/// Extension functions (not in pg_catalog) should return None.
#[test]
fn extension_functions_are_unknown() {
    // uuid-ossp functions are not in pg_catalog
    assert_eq!(
        fn_volatility::lookup("uuid_generate_v4"),
        None,
        "uuid_generate_v4 is an extension function, should be None"
    );
}

/// Unknown functions should return None.
#[test]
fn unknown_functions() {
    assert_eq!(
        fn_volatility::lookup("my_custom_function"),
        None,
        "user-defined functions should return None"
    );
    assert_eq!(
        fn_volatility::lookup("totally_made_up"),
        None,
        "non-existent functions should return None"
    );
}

/// Lookup should be case-insensitive.
#[test]
fn case_insensitive() {
    assert_eq!(fn_volatility::lookup("NOW"), Some(FnVolatility::Stable));
    assert_eq!(
        fn_volatility::lookup("Random"),
        Some(FnVolatility::Volatile)
    );
    assert_eq!(fn_volatility::lookup("ABS"), Some(FnVolatility::Immutable));
    // Extension functions are not in pg_catalog, so they return None
    assert_eq!(fn_volatility::lookup("UUID_GENERATE_V4"), None);
}
