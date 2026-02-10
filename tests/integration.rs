//! Integration tests for the full lint pipeline.

use pg_migration_lint::catalog::replay;
use pg_migration_lint::catalog::Catalog;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::input::MigrationLoader;
use pg_migration_lint::rules::{cap_for_down_migration, Finding, LintContext, RuleRegistry};
use pg_migration_lint::suppress::parse_suppressions;
use pg_migration_lint::IrNode;
use std::collections::HashSet;
use std::path::PathBuf;

/// Run the full lint pipeline on a fixture repo.
/// If `changed_files` is empty, all files are linted.
fn lint_fixture(fixture_name: &str, changed_filenames: &[&str]) -> Vec<Finding> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos")
        .join(fixture_name)
        .join("migrations");

    let loader = SqlLoader;
    let history = loader.load(&[base.clone()]).expect("Failed to load fixture");

    let changed: HashSet<PathBuf> = changed_filenames
        .iter()
        .map(|f| base.join(f))
        .collect();

    let mut catalog = Catalog::new();
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();

    for unit in &history.units {
        let is_changed = changed.is_empty()
            || changed.contains(&unit.source_file);

        if is_changed {
            let catalog_before = catalog.clone();
            replay::apply(&mut catalog, unit);

            for stmt in &unit.statements {
                if let IrNode::CreateTable(ct) = &stmt.node {
                    tables_created_in_change.insert(ct.name.catalog_key().to_string());
                }
            }

            let ctx = LintContext {
                catalog_before: &catalog_before,
                catalog_after: &catalog,
                tables_created_in_change: &tables_created_in_change,
                run_in_transaction: unit.run_in_transaction,
                is_down: unit.is_down,
                file: &unit.source_file,
            };

            let mut unit_findings: Vec<Finding> = Vec::new();
            for rule in registry.iter() {
                unit_findings.extend(rule.check(&unit.statements, &ctx));
            }

            if unit.is_down {
                cap_for_down_migration(&mut unit_findings);
            }

            let source = std::fs::read_to_string(&unit.source_file).unwrap_or_default();
            let suppressions = parse_suppressions(&source);
            unit_findings.retain(|f| !suppressions.is_suppressed(&f.rule_id, f.start_line));

            all_findings.extend(unit_findings);
        } else {
            replay::apply(&mut catalog, unit);
        }
    }

    all_findings
}

/// Helper: format findings for debug output in assertion messages.
fn format_findings(findings: &[Finding]) -> String {
    findings
        .iter()
        .map(|f| format!("{} {} (line {})", f.rule_id, f.message, f.start_line))
        .collect::<Vec<_>>()
        .join("\n  ")
}

// ---------------------------------------------------------------------------
// Clean repo: all migrations correct, expect 0 findings
// ---------------------------------------------------------------------------

#[test]
fn test_clean_repo_no_findings() {
    let findings = lint_fixture("clean", &[]);
    assert!(
        findings.is_empty(),
        "Clean repo should have 0 findings but got {}: {:?}",
        findings.len(),
        findings.iter().map(|f| format!("{}: {}", f.rule_id, f.message)).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// All-rules repo: one violation per rule
// ---------------------------------------------------------------------------

#[test]
fn test_all_rules_trigger() {
    // Only V002 and V003 are changed; V001 is just replayed as baseline.
    // This ensures tables from V001 are in catalog_before but NOT in
    // tables_created_in_change, so rules that check for pre-existing tables
    // (PGM001, PGM002, PGM009, PGM010, PGM011) will fire.
    let findings = lint_fixture(
        "all-rules",
        &["V002__violations.sql", "V003__more_violations.sql"],
    );
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    for expected in &["PGM001", "PGM002", "PGM003", "PGM004", "PGM005",
                       "PGM006", "PGM007", "PGM009", "PGM010", "PGM011"] {
        assert!(
            rule_ids.contains(expected),
            "Expected {} finding but not found. Got:\n  {}",
            expected,
            format_findings(&findings)
        );
    }
}

// ---------------------------------------------------------------------------
// Suppressed repo: all violations suppressed, expect 0 findings
// ---------------------------------------------------------------------------

#[test]
fn test_suppressed_repo_no_findings() {
    // Only V002 and V003 are changed; V001 just replays.
    let findings = lint_fixture(
        "suppressed",
        &["V002__suppressed.sql", "V003__suppressed.sql"],
    );
    assert!(
        findings.is_empty(),
        "Suppressed repo should have 0 findings but got {}: {:?}",
        findings.len(),
        findings.iter().map(|f| format!("{}: {}", f.rule_id, f.message)).collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// Changed-file filtering: only lint the specified files
// ---------------------------------------------------------------------------

#[test]
fn test_only_changed_files_linted() {
    // Only V001 is "changed" -- it creates new tables, should have 0 findings
    let findings = lint_fixture("all-rules", &["V001__baseline.sql"]);
    assert!(
        findings.is_empty(),
        "Baseline-only should have 0 findings but got {}: {:?}",
        findings.len(),
        findings.iter().map(|f| format!("{}: {}", f.rule_id, f.message)).collect::<Vec<_>>()
    );
}

#[test]
fn test_changed_file_sees_catalog_from_history() {
    // Only V002 is "changed" -- V001 was replayed but not linted.
    // V002 should still see the tables from V001 as pre-existing.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        rule_ids.contains("PGM001"),
        "PGM001 should fire for V002 against pre-existing tables. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Verify specific rule details
// ---------------------------------------------------------------------------

#[test]
fn test_pgm001_finding_details() {
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(pgm001.len(), 1, "Expected exactly 1 PGM001 finding");
    assert!(
        pgm001[0].message.contains("products"),
        "PGM001 message should mention 'products' table"
    );
    assert!(
        pgm001[0].message.contains("CONCURRENTLY"),
        "PGM001 message should mention CONCURRENTLY"
    );
}

#[test]
fn test_pgm003_finding_details() {
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert_eq!(pgm003.len(), 1, "Expected exactly 1 PGM003 finding");
    assert!(
        pgm003[0].message.contains("customers"),
        "PGM003 message should mention 'customers' table"
    );
    assert!(
        pgm003[0].message.contains("customer_id"),
        "PGM003 message should mention 'customer_id' column"
    );
}

#[test]
fn test_pgm004_finding_details() {
    // V002 creates audit_log without PK. Since V002 is changed, audit_log
    // is in tables_created_in_change, but PGM004 does not check that set --
    // it only checks catalog_after for has_primary_key.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let pgm004: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM004").collect();

    assert_eq!(pgm004.len(), 1, "Expected exactly 1 PGM004 finding");
    assert!(
        pgm004[0].message.contains("audit_log"),
        "PGM004 message should mention 'audit_log' table"
    );
}

#[test]
fn test_all_rules_changed_files_all_empty() {
    // When all files are changed (empty changed_files), tables created in V001
    // are in tables_created_in_change, so PGM001/009/010/011 won't fire for
    // those tables. But PGM003, PGM004, PGM005, PGM006, PGM007 should still fire.
    let findings = lint_fixture("all-rules", &[]);

    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // These rules do NOT check tables_created_in_change, so they fire regardless
    assert!(
        rule_ids.contains("PGM003"),
        "PGM003 should fire even with all files changed"
    );
    assert!(
        rule_ids.contains("PGM004"),
        "PGM004 should fire even with all files changed"
    );
    assert!(
        rule_ids.contains("PGM005"),
        "PGM005 should fire even with all files changed"
    );
    assert!(
        rule_ids.contains("PGM007"),
        "PGM007 should fire even with all files changed"
    );
    // PGM006 fires because it only checks run_in_transaction + concurrent flag
    assert!(
        rule_ids.contains("PGM006"),
        "PGM006 should fire even with all files changed"
    );
}

// ---------------------------------------------------------------------------
// Enterprise fixture: realistic 30-file migration history
// ---------------------------------------------------------------------------

#[test]
fn test_enterprise_parses_all_migrations() {
    // Verify all 30 migrations load and parse without errors
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/enterprise/migrations");
    let loader = SqlLoader;
    let history = loader.load(&[base]).expect("Failed to load enterprise fixture");
    assert_eq!(history.units.len(), 30, "Should have 30 migration units");
}

#[test]
fn test_enterprise_lint_all_finds_violations() {
    let findings = lint_fixture("enterprise", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM003 should fire (FKs without covering indexes in V005, V006, V010, V021, V029)
    assert!(rule_ids.contains("PGM003"), "Expected PGM003. Got:\n  {}", format_findings(&findings));

    // PGM004 should fire (many tables without PKs in V003, V015, V020, V021)
    assert!(rule_ids.contains("PGM004"), "Expected PGM004. Got:\n  {}", format_findings(&findings));

    // PGM007 should fire (volatile defaults in V012, V018, V022, V027, V028, V029)
    assert!(rule_ids.contains("PGM007"), "Expected PGM007. Got:\n  {}", format_findings(&findings));
}

#[test]
fn test_enterprise_lint_v007_only() {
    // V001-V006 are replayed as history, V007 is the only changed file.
    // V007 creates indexes WITHOUT CONCURRENTLY on pre-existing tables → PGM001
    let findings = lint_fixture("enterprise", &["V007__create_index_no_concurrently.sql"]);
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(pgm001.len(), 3, "Expected 3 PGM001 findings for 3 non-concurrent indexes. Got:\n  {}",
        format_findings(&findings));
}

#[test]
fn test_enterprise_lint_v023_only() {
    // V001-V022 replayed, V023 is changed: DROP INDEX without CONCURRENTLY → PGM002
    let findings = lint_fixture("enterprise", &["V023__drop_index_no_concurrently.sql"]);
    let pgm002: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM002").collect();

    assert!(pgm002.len() >= 1, "Expected PGM002 for DROP INDEX without CONCURRENTLY. Got:\n  {}",
        format_findings(&findings));
}

#[test]
fn test_enterprise_lint_v008_only() {
    // V001-V007 replayed, V008 is changed: ADD COLUMN NOT NULL without default → PGM010
    let findings = lint_fixture("enterprise", &["V008__add_not_null_column.sql"]);
    let pgm010: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM010").collect();

    assert_eq!(pgm010.len(), 1, "Expected 1 PGM010 for NOT NULL without default. Got:\n  {}",
        format_findings(&findings));
}

#[test]
fn test_enterprise_lint_v013_only() {
    // V001-V012 replayed, V013 is changed: ALTER COLUMN TYPE → PGM009
    let findings = lint_fixture("enterprise", &["V013__alter_column_type.sql"]);
    let pgm009: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM009").collect();

    assert!(pgm009.len() >= 1, "Expected PGM009 for ALTER COLUMN TYPE. Got:\n  {}",
        format_findings(&findings));
}

#[test]
fn test_enterprise_finding_count_reasonable() {
    // Lint only V005-V015 as changed (V001-V004 as history).
    // This should produce a reasonable number of findings.
    let findings = lint_fixture("enterprise", &[
        "V005__add_support_user_fields.sql",
        "V006__add_order_status_columns.sql",
        "V007__create_index_no_concurrently.sql",
        "V008__add_not_null_column.sql",
        "V009__create_products_tables.sql",
        "V010__create_promotion_tables.sql",
        "V011__create_index_concurrently.sql",
        "V012__create_price_plans.sql",
        "V013__alter_column_type.sql",
        "V014__drop_column.sql",
        "V015__create_tables_without_pks.sql",
    ]);

    // Should have a significant number of findings
    assert!(findings.len() >= 10, "Expected at least 10 findings from V005-V015, got {}: \n  {}",
        findings.len(), format_findings(&findings));
    assert!(findings.len() <= 60, "Expected at most 60 findings from V005-V015, got {}",
        findings.len());
}
