mod common;

use pg_migration_lint::rules::RuleId;
use std::collections::HashSet;

#[test]
fn test_clean_repo_no_findings() {
    let findings = common::lint_fixture::<&str>("clean", &[]);
    assert!(
        findings.is_empty(),
        "Clean repo should have 0 findings but got {}: {:?}",
        findings.len(),
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_all_rules_trigger() {
    // All migration files except V001 (baseline) are changed.
    // V001 is just replayed so its tables appear in catalog_before.
    // Every registered non-meta rule must fire at least once.
    let changed = common::changed_files_for("all-rules");
    let findings = common::lint_fixture("all-rules", &changed);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // Every registered non-meta rule must fire at least once.
    for id in RuleId::lint_rules() {
        assert!(
            rule_ids.contains(id.as_str()),
            "Rule {} is registered but did not fire. Add a violation to the all-rules fixture. Got:\n  {}",
            id,
            common::format_findings(&findings)
        );
    }
}

#[test]
fn test_suppressed_repo_no_findings() {
    let changed = common::changed_files_for("suppressed");

    // First: verify every non-meta rule fires before suppression.
    // This ensures the suppressed fixture stays in sync with new rules.
    let raw_findings = common::lint_fixture_no_suppress("suppressed", &changed);
    let raw_rule_ids: HashSet<&str> = raw_findings.iter().map(|f| f.rule_id.as_str()).collect();

    let mut missing_rules: Vec<&str> = RuleId::lint_rules()
        .filter(|id| !raw_rule_ids.contains(id.as_str()))
        .map(|id| id.as_str())
        .collect();
    if !missing_rules.is_empty() {
        missing_rules.sort_unstable();
        let mut fired: Vec<&str> = raw_rule_ids.iter().copied().collect();
        fired.sort_unstable();
        panic!(
            "Rules registered but did not fire in suppressed fixture (pre-suppression).\n\
             Add a suppressed violation for each missing rule.\n\
             Missing ({}):\n  {}\n\
             Fired ({}):\n  {}",
            missing_rules.len(),
            missing_rules.join(", "),
            fired.len(),
            fired.join(", "),
        );
    }

    // Second: verify all findings are suppressed.
    let findings = common::lint_fixture("suppressed", &changed);
    assert!(
        findings.is_empty(),
        "Suppressed repo should have 0 findings but got {}: {:?}",
        findings.len(),
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn test_all_files_changed_fresh_repo() {
    // All files passed as changed — no prior history to replay.
    // catalog_before starts empty for V001. DDL-safety rules like PGM001
    // must not fire because all tables are new.
    let findings = common::lint_fixture(
        "clean",
        &["V001__create_users.sql", "V002__create_orders.sql"],
    );
    assert!(
        findings.is_empty(),
        "Fresh repo with all files changed should have 0 findings but got {}: {}",
        findings.len(),
        common::format_findings(&findings)
    );
}

#[test]
fn test_bridge_table_missing_covering_index_on_second_fk_column() {
    // Bridge table xy has PK (x_id, y_id) and FKs to both x and y.
    // The composite PK index covers x_id (leftmost prefix) but NOT y_id alone.
    // PGM501 should fire for y_id only. All tables are new.
    let findings =
        common::lint_fixture_rules("bridge-table", &["V001__bridge_table.sql"], &["PGM501"]);
    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding (y_id), got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("y_id"),
        "PGM501 should fire for y_id, got: {}",
        findings[0].message
    );
}

#[test]
fn test_only_changed_files_linted() {
    // Only V001 is "changed" -- it creates new tables, so pre-existing-table
    // rules (PGM001, PGM007, PGM008, PGM009) should NOT fire. However,
    // PGM502 fires for the 'events' table which has no primary key.
    let findings =
        common::lint_fixture_rules("all-rules", &["V001__baseline.sql"], &["PGM001", "PGM502"]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        !rule_ids.contains("PGM001"),
        "PGM001 should not fire for baseline-only. Got:\n  {}",
        common::format_findings(&findings)
    );
    assert!(
        rule_ids.contains("PGM502"),
        "PGM502 should fire for events table (no PK). Got:\n  {}",
        common::format_findings(&findings)
    );
}

#[test]
fn test_changed_file_sees_catalog_from_history() {
    // Only V002 is "changed" -- V001 was replayed but not linted.
    // V002 should still see the tables from V001 as pre-existing.
    let findings = common::lint_fixture("all-rules", &["V002__violations.sql"]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        rule_ids.contains("PGM001"),
        "PGM001 should fire for V002 against pre-existing tables. Got:\n  {}",
        common::format_findings(&findings)
    );
}
