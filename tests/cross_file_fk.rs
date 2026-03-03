mod common;

#[test]
fn test_fk_without_index_cross_file_only_fk_changed() {
    // Only V002 is changed. V001 is replayed as history (creates tables).
    // V002 adds FK on orders.customer_id but V003 (which adds the covering
    // index) has NOT been replayed yet. PGM501 should fire because
    // catalog_after has no covering index at this point.
    let findings =
        common::lint_fixture_rules("fk-with-later-index", &["V002__add_fk.sql"], &["PGM501"]);

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding for FK without index. Got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_fk_with_later_index_only_index_changed() {
    // Only V003 is changed. V001 and V002 are replayed as history.
    // The FK from V002 already exists in catalog_before, and V003 adds
    // the covering index. Since V002 is not being linted, no PGM501
    // should fire -- the FK was in a prior file, not in the current lint set.
    let findings =
        common::lint_fixture_rules("fk-with-later-index", &["V003__add_index.sql"], &["PGM501"]);

    assert!(
        findings.is_empty(),
        "PGM501 should NOT fire when only the index file is linted. Got:\n  {}",
        common::format_findings(&findings)
    );
}

#[test]
fn test_fk_cross_file_both_changed() {
    // Both V002 and V003 are changed. V001 is replayed as history.
    // When linting V002: FK is added but no covering index yet -> PGM501 fires.
    // When linting V003: index is added, no new FK in this file -> no PGM501.
    let findings = common::lint_fixture_rules(
        "fk-with-later-index",
        &["V002__add_fk.sql", "V003__add_index.sql"],
        &["PGM501"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding (from V002 only). Got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_fk_cross_file_all_changed() {
    // All files are changed (empty changed set). V001 creates tables (no FK,
    // no finding). V002 adds FK without covering index -> PGM501 fires.
    // V003 adds the covering index -> no additional PGM501.
    let findings = common::lint_fixture_rules::<&str>("fk-with-later-index", &[], &["PGM501"]);

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding (from V002). Got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
    );
}
