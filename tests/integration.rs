//! Integration tests for the full lint pipeline.

use pg_migration_lint::catalog::replay;
use pg_migration_lint::catalog::Catalog;
use pg_migration_lint::input::liquibase_xml::XmlFallbackLoader;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::input::{MigrationLoader, MigrationUnit};
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

// ===========================================================================
// Liquibase XML integration tests
// ===========================================================================

/// Run the full lint pipeline on a Liquibase XML fixture repo.
/// If `changed_ids` is empty, all changesets are linted.
/// Uses changeset IDs (not file names) for filtering.
fn lint_xml_fixture(fixture_name: &str, changed_ids: &[&str]) -> Vec<Finding> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos")
        .join(fixture_name)
        .join("changelog/master.xml");

    let loader = XmlFallbackLoader;
    let raw_units = loader.load(&base).expect("Failed to load XML fixture");

    // Convert RawMigrationUnit -> MigrationUnit
    let units: Vec<MigrationUnit> = raw_units
        .into_iter()
        .map(|r| r.into_migration_unit())
        .collect();

    let changed_set: HashSet<String> = changed_ids.iter().map(|s| s.to_string()).collect();

    let mut catalog = Catalog::new();
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();

    for unit in &units {
        let is_changed = changed_set.is_empty() || changed_set.contains(&unit.id);

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

            // Note: suppressions don't apply to XML since comments are in XML not SQL
            all_findings.extend(unit_findings);
        } else {
            replay::apply(&mut catalog, unit);
        }
    }

    all_findings
}

// ---------------------------------------------------------------------------
// XML: Parse all changesets
// ---------------------------------------------------------------------------

#[test]
fn test_xml_parses_all_changesets() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

    let loader = XmlFallbackLoader;
    let raw_units = loader.load(&base).expect("Failed to load XML fixture");

    // 001: 4, 002: 3, 003: 3, 004: 3, 005: 3, 006: 2, 007: 3, 008: 3, 009: 4, 010: 4 = 32
    assert_eq!(
        raw_units.len(),
        32,
        "Expected 32 changesets across all XML files, got {}",
        raw_units.len()
    );
}

// ---------------------------------------------------------------------------
// XML: Lint all changesets, verify key violations fire
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_all_finds_violations() {
    let findings = lint_xml_fixture("liquibase-xml", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM003 should fire (FKs without covering indexes in 005, 006, 007)
    assert!(
        rule_ids.contains("PGM003"),
        "Expected PGM003. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM004 should fire (tables without PKs in 006)
    assert!(
        rule_ids.contains("PGM004"),
        "Expected PGM004. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM007 should fire (volatile defaults in 007)
    assert!(
        rule_ids.contains("PGM007"),
        "Expected PGM007. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// XML: Lint only 004 changesets (create indexes on pre-existing tables)
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_004_only() {
    // Files 001-003 are replayed as history, only 004 changesets are linted.
    // 004 creates 3 indexes WITHOUT CONCURRENTLY on pre-existing tables -> PGM001
    let findings = lint_xml_fixture("liquibase-xml", &[
        "004-add-users-email-index",
        "004-add-subscriptions-account-index",
        "004-add-products-composite-index",
    ]);
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(
        pgm001.len(),
        3,
        "Expected 3 PGM001 findings for 3 non-concurrent indexes. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// XML: Lint only 005 changesets (add FKs)
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_005_only() {
    // Files 001-004 replayed, only 005 changesets are linted.
    // orders.user_id FK has no covering index -> PGM003
    // subscriptions.account_id FK: idx_subscriptions_account_id was created in 004 -> should NOT fire
    // orders.account_id FK: idx_orders_account_id was created in 002 -> should NOT fire
    let findings = lint_xml_fixture("liquibase-xml", &[
        "005-add-fk-orders-user",
        "005-add-fk-subscriptions-account",
        "005-add-fk-orders-account",
    ]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    // orders.user_id has no covering index -> PGM003
    // subscriptions.account_id had idx_subscriptions_account_id created in 004 -> no PGM003
    // orders.account_id has idx_orders_account_id from 002 -> no PGM003
    assert!(
        pgm003.len() >= 1,
        "Expected at least 1 PGM003 finding for orders.user_id FK. Got:\n  {}",
        format_findings(&findings)
    );

    // Verify orders.user_id FK fires
    let user_id_finding = pgm003.iter().any(|f| f.message.contains("user_id"));
    assert!(
        user_id_finding,
        "Expected PGM003 for orders.user_id FK. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// XML: Lint only 006 changesets (tables without PKs)
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_006_only() {
    // Files 001-005 replayed, only 006 changesets are linted.
    // event_log has UNIQUE NOT NULL on event_id -> PGM005 fires (not PGM004, since
    // the tool considers UNIQUE NOT NULL as functionally equivalent to a PK).
    // subscription_invoices has no PK -> PGM004
    // subscription_invoices has FK without covering index -> PGM003
    let findings = lint_xml_fixture("liquibase-xml", &[
        "006-create-event-log",
        "006-create-subscription-invoices",
    ]);

    let pgm004: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM004").collect();
    assert_eq!(
        pgm004.len(),
        1,
        "Expected 1 PGM004 finding for subscription_invoices. Got:\n  {}",
        format_findings(&findings)
    );

    let pgm005: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM005").collect();
    assert_eq!(
        pgm005.len(),
        1,
        "Expected 1 PGM005 finding for event_log (UNIQUE NOT NULL instead of PK). Got:\n  {}",
        format_findings(&findings)
    );

    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();
    assert!(
        pgm003.len() >= 1,
        "Expected PGM003 for subscription_invoices.subscription_id FK. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// XML: Lint only 008 changesets (add NOT NULL columns without defaults)
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_008_only() {
    // Files 001-007 replayed, only 008 changesets are linted.
    // accounts.region: NOT NULL, no default -> PGM010
    // orders.priority: NOT NULL, WITH default (0) -> clean
    // products.product_type: NOT NULL, no default -> PGM010
    let findings = lint_xml_fixture("liquibase-xml", &[
        "008-add-region-to-accounts",
        "008-add-priority-to-orders",
        "008-add-category-to-products",
    ]);
    let pgm010: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM010").collect();

    assert_eq!(
        pgm010.len(),
        2,
        "Expected 2 PGM010 findings for accounts.region and products.product_type. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// XML: Lint changesets 004-008 as changed, verify reasonable finding count
// ---------------------------------------------------------------------------

#[test]
fn test_xml_finding_count_reasonable() {
    // Changesets 004-008 as changed, 001-003 as history.
    let findings = lint_xml_fixture("liquibase-xml", &[
        // 004
        "004-add-users-email-index",
        "004-add-subscriptions-account-index",
        "004-add-products-composite-index",
        // 005
        "005-add-fk-orders-user",
        "005-add-fk-subscriptions-account",
        "005-add-fk-orders-account",
        // 006
        "006-create-event-log",
        "006-create-subscription-invoices",
        // 007
        "007-create-price-plans",
        "007-add-timestamps-to-products",
        "007-create-price-plan-products",
        // 008
        "008-add-region-to-accounts",
        "008-add-priority-to-orders",
        "008-add-category-to-products",
    ]);

    assert!(
        findings.len() >= 8,
        "Expected at least 8 findings from 004-008, got {}: \n  {}",
        findings.len(),
        format_findings(&findings)
    );
    assert!(
        findings.len() <= 40,
        "Expected at most 40 findings from 004-008, got {}",
        findings.len()
    );
}
