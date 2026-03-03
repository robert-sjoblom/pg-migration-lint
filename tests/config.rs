mod common;

#[test]
fn test_disabled_rules_suppresses_findings() {
    let changed = &["V002__violations.sql"];

    // Baseline: PGM501 should fire
    let findings_all = common::lint_fixture("all-rules", changed);
    let pgm501_all = findings_all
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .count();
    assert!(pgm501_all > 0, "PGM501 should fire without suppression");

    // With PGM501 disabled: no PGM501 findings
    let findings_disabled = common::lint_fixture_with_disabled("all-rules", changed, &["PGM501"]);
    let pgm501_disabled = findings_disabled
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .count();
    assert_eq!(pgm501_disabled, 0, "PGM501 should not fire when disabled");

    // Other rules should be unaffected
    let other_all = findings_all
        .iter()
        .filter(|f| f.rule_id.as_str() != "PGM501")
        .count();
    let other_disabled = findings_disabled
        .iter()
        .filter(|f| f.rule_id.as_str() != "PGM501")
        .count();
    assert_eq!(
        other_all, other_disabled,
        "Non-disabled rules should produce identical findings"
    );
}
