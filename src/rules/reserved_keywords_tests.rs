//! Tests for the auto-generated reserved_keywords module.

use super::reserved_keywords;

/// Smoke tests for well-known reserved keywords.
#[test]
fn known_reserved_keywords() {
    // Only keywords with catcode 'R' or 'T' in pg_get_keywords() — many common
    // SQL words (INSERT, UPDATE, DELETE, ALTER, DROP) are actually non-reserved.
    for word in [
        "select",
        "from",
        "where",
        "create",
        "table",
        "order",
        "group",
        "having",
        "join",
        "and",
        "or",
        "not",
        "null",
        "true",
        "false",
        "in",
        "like",
        "as",
        "with",
        "primary",
        "foreign",
        "unique",
        "check",
        "constraint",
        "default",
    ] {
        assert!(
            reserved_keywords::is_reserved(word),
            "{word} should be reserved"
        );
    }
}

/// Non-reserved words should return false.
/// Note: INSERT, UPDATE, DELETE, ALTER, DROP, INDEX are non-reserved in PostgreSQL
/// despite being common SQL keywords. BETWEEN has catcode 'C' (col_name_keyword)
/// and does not require quoting as an identifier.
#[test]
fn non_reserved_words() {
    for word in [
        "users",
        "orders",
        "status",
        "my_column",
        "totally_made_up",
        "id",
        "name",
        "insert",
        "update",
        "delete",
        "alter",
        "drop",
        "index",
        "between",
    ] {
        assert!(
            !reserved_keywords::is_reserved(word),
            "{word} should not be reserved"
        );
    }
}
