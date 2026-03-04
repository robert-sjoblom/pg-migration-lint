mod common;

use rstest::rstest;

#[rstest]
#[case::only_fk_changed(
    // Only V002 is changed. V001 is replayed as history (creates tables).
    // V002 adds FK on orders.customer_id but V003 (which adds the covering
    // index) has NOT been replayed yet. PGM501 should fire because
    // catalog_after has no covering index at this point.
    &["V002__add_fk.sql"],
    1,
    Some("customer_id"),
)]
#[case::only_index_changed(
    // Only V003 is changed. V001 and V002 are replayed as history.
    // The FK from V002 already exists in catalog_before, and V003 adds
    // the covering index. Since V002 is not being linted, no PGM501
    // should fire -- the FK was in a prior file, not in the current lint set.
    &["V003__add_index.sql"],
    0,
    None,
)]
#[case::both_changed(
    // Both V002 and V003 are changed. V001 is replayed as history.
    // When linting V002: FK is added but no covering index yet -> PGM501 fires.
    // When linting V003: index is added, no new FK in this file -> no PGM501.
    &["V002__add_fk.sql", "V003__add_index.sql"],
    1,
    Some("customer_id"),
)]
#[case::all_changed(
    // All files are changed (empty changed set). V001 creates tables (no FK,
    // no finding). V002 adds FK without covering index -> PGM501 fires.
    // V003 adds the covering index -> no additional PGM501.
    &[],
    1,
    Some("customer_id"),
)]
fn test_fk_cross_file(
    #[case] changed_files: &[&str],
    #[case] expected_count: usize,
    #[case] expected_message_substr: Option<&str>,
) {
    let findings = common::lint_fixture_rules("fk-with-later-index", changed_files, &["PGM501"]);

    assert_eq!(
        findings.len(),
        expected_count,
        "Expected exactly {expected_count} PGM501 finding(s). Got:\n  {}",
        common::format_findings(&findings)
    );

    if let Some(substr) = expected_message_substr {
        assert!(
            findings[0].message.contains(substr),
            "PGM501 message should mention '{substr}'. Got: {}",
            findings[0].message
        );
    }
}
