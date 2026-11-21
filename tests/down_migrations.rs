mod common;

use pg_migration_lint::{Finding, RuleId, Severity};

const DROP_USERS_DOWN: &str = "V001__create_users_down.sql";
const INDEX_USERS_EMAIL: &str = "V002__index_users_email.sql";

fn findings_in<'a>(findings: &'a [Finding], file_name: &str) -> Vec<&'a Finding> {
    findings
        .iter()
        .filter(|finding| finding.file.ends_with(file_name))
        .collect()
}

fn severities_of(findings: &[&Finding], rule_id: RuleId) -> Vec<Severity> {
    findings
        .iter()
        .filter(|finding| finding.rule_id == rule_id)
        .map(|finding| finding.severity)
        .collect()
}

#[test]
fn a_replayed_down_does_not_drop_the_table_a_later_migration_indexes() {
    let findings = common::lint_fixture("down-migrations", &[INDEX_USERS_EMAIL]);

    assert_eq!(
        severities_of(&findings_in(&findings, INDEX_USERS_EMAIL), RuleId::Pgm001),
        [Severity::Critical],
        "users exists when V002 runs, so its CREATE INDEX needs CONCURRENTLY. Got:\n  {}",
        common::format_findings(&findings)
    );
}

#[test]
fn a_linted_down_does_not_drop_the_table_a_later_migration_indexes() {
    let findings = common::lint_fixture("down-migrations", &[DROP_USERS_DOWN, INDEX_USERS_EMAIL]);

    assert_eq!(
        severities_of(&findings_in(&findings, INDEX_USERS_EMAIL), RuleId::Pgm001),
        [Severity::Critical],
        "users exists when V002 runs, so its CREATE INDEX needs CONCURRENTLY. Got:\n  {}",
        common::format_findings(&findings)
    );
}

#[test]
fn linting_every_file_keeps_the_findings_that_need_the_table_the_down_drops() {
    let findings = common::lint_fixture::<&str>("down-migrations", &[]);

    assert_eq!(
        severities_of(&findings_in(&findings, INDEX_USERS_EMAIL), RuleId::Pgm508),
        [Severity::Info],
        "idx_users_email duplicates the index of the UNIQUE (email) constraint on users. Got:\n  {}",
        common::format_findings(&findings)
    );
}

#[test]
fn linting_every_file_caps_the_down_findings_to_info() {
    let findings = common::lint_fixture::<&str>("down-migrations", &[]);

    assert_eq!(
        severities_of(&findings_in(&findings, DROP_USERS_DOWN), RuleId::Pgm401),
        [Severity::Info],
        "DROP TABLE users without IF EXISTS reports PGM401, capped to INFO by PGM901. Got:\n  {}",
        common::format_findings(&findings)
    );
}
