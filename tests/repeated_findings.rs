mod common;

use pg_migration_lint::rules::{Finding, RuleId, group_repeated_findings};
use pg_migration_lint::{Config, Rule};

fn group_with_default_threshold(findings: Vec<Finding>) -> Vec<Finding> {
    group_repeated_findings(findings, Config::default().output.group_threshold)
}

fn lines_of(finding: &Finding) -> Vec<usize> {
    finding.locations().map(|l| l.start_line).collect()
}

#[test]
fn test_twenty_drop_tables_without_if_exists_flood_one_finding_each() {
    let findings = common::lint_fixture_rules(
        "repeated-findings",
        &["V002__drop_legacy_reports.sql"],
        &["PGM401"],
    );

    assert_eq!(
        findings.len(),
        20,
        "Got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(findings.iter().all(|f| f.rule_id == RuleId::Pgm401));
}

#[test]
fn test_default_threshold_reports_the_flood_as_one_finding() {
    let findings = group_with_default_threshold(common::lint_fixture_rules(
        "repeated-findings",
        &["V002__drop_legacy_reports.sql"],
        &["PGM401"],
    ));

    assert_eq!(
        findings.len(),
        1,
        "Got:\n  {}",
        common::format_findings(&findings)
    );
    let group = &findings[0];
    assert_eq!(group.rule_id, RuleId::Pgm401);
    assert_eq!(group.severity, RuleId::Pgm401.default_severity());
    assert_eq!(lines_of(group), (1..=20).collect::<Vec<_>>());
    assert!(group.message.contains("'legacy_report_01'"));
    assert!(
        group.additional_locations[18]
            .message
            .contains("'legacy_report_20'")
    );
}

#[test]
fn test_default_threshold_groups_each_rule_separately() {
    let findings = group_with_default_threshold(common::lint_fixture_rules(
        "repeated-findings",
        &["V002__drop_legacy_reports.sql"],
        &["PGM201", "PGM401"],
    ));

    let rules: Vec<(RuleId, usize)> = findings
        .iter()
        .map(|f| (f.rule_id, f.locations().count()))
        .collect();
    assert_eq!(rules, vec![(RuleId::Pgm201, 20), (RuleId::Pgm401, 20)]);
}

#[test]
fn test_suppressed_statement_drops_out_of_its_group_before_counting() {
    let findings = group_with_default_threshold(common::lint_fixture_rules(
        "repeated-findings",
        &["V003__drop_archives.sql"],
        &["PGM201", "PGM401"],
    ));

    let drop_table_groups: Vec<Vec<usize>> = findings
        .iter()
        .filter(|f| f.rule_id == RuleId::Pgm201)
        .map(lines_of)
        .collect();
    assert_eq!(drop_table_groups, vec![vec![1, 2, 4, 5, 6]]);

    let missing_if_exists: Vec<Vec<usize>> = findings
        .iter()
        .filter(|f| f.rule_id == RuleId::Pgm401)
        .map(lines_of)
        .collect();
    assert_eq!(
        missing_if_exists,
        vec![vec![1], vec![2], vec![5], vec![6]],
        "the suppressed DROP TABLE on line 4 leaves 4 PGM401 findings, below the threshold of 5"
    );
}
