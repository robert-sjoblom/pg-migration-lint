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
    // V002, V003, and V004 are changed; V001 is just replayed as baseline.
    // This ensures tables from V001 are in catalog_before but NOT in
    // tables_created_in_change, so rules that check for pre-existing tables
    // (PGM001, PGM002, PGM009, PGM010, PGM011) will fire.
    // V004 introduces "Don't Do This" type anti-patterns (PGM101-PGM105).
    let findings = lint_fixture(
        "all-rules",
        &["V002__violations.sql", "V003__more_violations.sql", "V004__dont_do_this_types.sql"],
    );
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    for expected in &["PGM001", "PGM002", "PGM003", "PGM004", "PGM005",
                       "PGM006", "PGM007", "PGM009", "PGM010", "PGM011", "PGM012",
                       "PGM101", "PGM102", "PGM103", "PGM104", "PGM105"] {
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
    // Only V002, V003, and V004 are changed; V001 just replays.
    let findings = lint_fixture(
        "suppressed",
        &["V002__suppressed.sql", "V003__suppressed.sql", "V004__suppressed.sql"],
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
    // Only V001 is "changed" -- it creates new tables, so pre-existing-table
    // rules (PGM001, PGM009, PGM010, PGM011) should NOT fire. However,
    // PGM004 fires for the 'events' table which has no primary key.
    let findings = lint_fixture("all-rules", &["V001__baseline.sql"]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        !rule_ids.contains("PGM001"),
        "PGM001 should not fire for baseline-only. Got:\n  {}",
        format_findings(&findings)
    );
    // PGM004 fires for the events table (no PK)
    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 finding (PGM004 for events), got {}: {:?}",
        findings.len(),
        findings.iter().map(|f| format!("{}: {}", f.rule_id, f.message)).collect::<Vec<_>>()
    );
    assert_eq!(findings[0].rule_id, "PGM004");
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
// "Don't Do This" rules (PGM101-PGM105)
// ---------------------------------------------------------------------------

#[test]
fn test_pgm101_timestamp_without_tz() {
    let findings = lint_fixture(
        "all-rules",
        &["V004__dont_do_this_types.sql"],
    );
    let pgm101: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM101").collect();

    assert!(!pgm101.is_empty(), "Expected PGM101 findings for 'timestamp' without time zone");
    assert!(
        pgm101.iter().any(|f| f.message.to_lowercase().contains("timestamp")),
        "PGM101 message should mention 'timestamp'. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_pgm102_timestamptz_zero_precision() {
    let findings = lint_fixture(
        "all-rules",
        &["V004__dont_do_this_types.sql"],
    );
    let pgm102: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM102").collect();

    assert!(!pgm102.is_empty(), "Expected PGM102 findings for 'timestamptz(0)'");
    assert!(
        pgm102.iter().any(|f| f.message.contains("0") || f.message.to_lowercase().contains("precision")),
        "PGM102 message should mention precision or (0). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_pgm103_char_n_type() {
    let findings = lint_fixture(
        "all-rules",
        &["V004__dont_do_this_types.sql"],
    );
    let pgm103: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM103").collect();

    assert!(!pgm103.is_empty(), "Expected PGM103 findings for 'char(n)'");
    assert!(
        pgm103.iter().any(|f| f.message.to_lowercase().contains("char")),
        "PGM103 message should mention 'char'. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_pgm104_money_type() {
    let findings = lint_fixture(
        "all-rules",
        &["V004__dont_do_this_types.sql"],
    );
    let pgm104: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM104").collect();

    assert!(!pgm104.is_empty(), "Expected PGM104 findings for 'money' type");
    assert!(
        pgm104.iter().any(|f| f.message.to_lowercase().contains("money")),
        "PGM104 message should mention 'money'. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_pgm105_serial_type() {
    let findings = lint_fixture(
        "all-rules",
        &["V004__dont_do_this_types.sql"],
    );
    let pgm105: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM105").collect();

    assert!(!pgm105.is_empty(), "Expected PGM105 findings for 'serial' type");
    assert!(
        pgm105.iter().any(|f| f.message.to_lowercase().contains("serial")
            || f.message.to_lowercase().contains("identity")),
        "PGM105 message should mention 'serial' or 'identity'. Got:\n  {}",
        format_findings(&findings)
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

    // 001: 4, 002: 3, 003: 3, 004: 3, 005: 3, 006: 2, 007: 3, 008: 3, 009: 4, 010: 4, 011: 3 = 35
    assert_eq!(
        raw_units.len(),
        35,
        "Expected 35 changesets across all XML files, got {}",
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

// ---------------------------------------------------------------------------
// XML: "Don't Do This" rules (PGM101, PGM103, PGM104)
// ---------------------------------------------------------------------------

#[test]
fn test_xml_lint_011_pgm101_timestamp() {
    // Lint only 011 changesets. 001-010 are replayed as history.
    // 011-add-event-timestamp adds a TIMESTAMP column -> PGM101
    let findings = lint_xml_fixture("liquibase-xml", &[
        "011-add-event-timestamp",
    ]);
    let pgm101: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM101").collect();

    assert!(
        !pgm101.is_empty(),
        "Expected PGM101 for TIMESTAMP column. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_xml_lint_011_pgm103_char_n() {
    // 011-add-country-code adds a CHAR(3) column -> PGM103
    let findings = lint_xml_fixture("liquibase-xml", &[
        "011-add-country-code",
    ]);
    let pgm103: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM103").collect();

    assert!(
        !pgm103.is_empty(),
        "Expected PGM103 for CHAR(3) column. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_xml_lint_011_pgm104_money() {
    // 011-add-balance adds a MONEY column -> PGM104
    let findings = lint_xml_fixture("liquibase-xml", &[
        "011-add-balance",
    ]);
    let pgm104: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM104").collect();

    assert!(
        !pgm104.is_empty(),
        "Expected PGM104 for MONEY column. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_xml_lint_all_includes_dont_do_this_rules() {
    // Verify the "Don't Do This" rules fire when all changesets are linted
    let findings = lint_xml_fixture("liquibase-xml", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    assert!(
        rule_ids.contains("PGM101"),
        "Expected PGM101 (timestamp without tz). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        rule_ids.contains("PGM103"),
        "Expected PGM103 (char(n)). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        rule_ids.contains("PGM104"),
        "Expected PGM104 (money). Got:\n  {}",
        format_findings(&findings)
    );
}

// ===========================================================================
// Go-migrate fixture integration tests
// ===========================================================================

// ---------------------------------------------------------------------------
// Go-migrate: Parse all migrations
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_parses_all_migrations() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/go-migrate/migrations");
    let loader = SqlLoader;
    let history = loader.load(&[base]).expect("Failed to load go-migrate fixture");
    // 13 up files + 3 down files + 1 comment-only down file = 17 total
    assert_eq!(
        history.units.len(),
        17,
        "Should have 17 migration units (13 up + 3 down + 1 comment-only down)"
    );
}

#[test]
fn test_gomigrate_up_down_detection() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/go-migrate/migrations");
    let loader = SqlLoader;
    let history = loader.load(&[base]).expect("Failed to load go-migrate fixture");

    let up_count = history.units.iter().filter(|u| !u.is_down).count();
    let down_count = history.units.iter().filter(|u| u.is_down).count();

    assert_eq!(up_count, 14, "Should have 14 up migrations");
    assert_eq!(down_count, 3, "Should have 3 down migrations");
}

// ---------------------------------------------------------------------------
// Go-migrate: Lint all, verify key rules fire
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_lint_all_finds_violations() {
    // When all files are changed, tables_created_in_change includes everything,
    // so only rules that don't check tables_created_in_change will fire.
    let findings = lint_fixture("go-migrate", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM003 should fire (FK without covering index on orders.assigned_user_id)
    assert!(
        rule_ids.contains("PGM003"),
        "Expected PGM003 (FK without index). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM004 should fire (audit_log has no PK when first created in 000007)
    assert!(
        rule_ids.contains("PGM004"),
        "Expected PGM004 (table without PK). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM007 should fire (volatile defaults: now(), gen_random_uuid())
    assert!(
        rule_ids.contains("PGM007"),
        "Expected PGM007 (volatile default). Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: Down migration severity capping (PGM008)
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_down_migration_capped() {
    // Lint only the last down migration which creates an index without CONCURRENTLY.
    // It targets the orders table which is pre-existing in catalog_before.
    // PGM001 would normally fire as CRITICAL, but since it's a down migration,
    // all findings should be capped to INFO severity.
    let findings = lint_fixture(
        "go-migrate",
        &["000015_drop_index_no_concurrently.down.sql"],
    );

    assert!(
        !findings.is_empty(),
        "Down migration should produce findings (PGM001 capped to INFO)"
    );

    for finding in &findings {
        assert_eq!(
            finding.severity,
            pg_migration_lint::rules::Severity::Info,
            "All findings in down migration should be capped to INFO. Got {} with severity {}",
            finding.rule_id,
            finding.severity
        );
    }
}

// ---------------------------------------------------------------------------
// Go-migrate: Changed-file filtering
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_changed_file_filtering() {
    // Only lint baseline files (000001-000005). Since these create new tables,
    // they are all in tables_created_in_change. Rules requiring pre-existing
    // tables (PGM001, PGM010, PGM012) should NOT fire.
    let findings = lint_fixture(
        "go-migrate",
        &[
            "000001_create_users.up.sql",
            "000002_create_accounts.up.sql",
            "000003_create_orders.up.sql",
            "000004_create_order_items.up.sql",
            "000005_create_settings.up.sql",
        ],
    );

    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM001 should NOT fire (no CREATE INDEX without CONCURRENTLY in baseline)
    assert!(
        !rule_ids.contains("PGM001"),
        "PGM001 should not fire for baseline. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM010 should NOT fire (no ADD COLUMN NOT NULL in baseline)
    assert!(
        !rule_ids.contains("PGM010"),
        "PGM010 should not fire for baseline. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM001 fires when indexes are changed
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm001_fires() {
    // Only 000006 is changed. Tables from 000001-000005 are replayed as
    // history (pre-existing). 000006 creates indexes WITHOUT CONCURRENTLY.
    let findings = lint_fixture(
        "go-migrate",
        &["000006_add_indexes_no_concurrently.up.sql"],
    );
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(
        pgm001.len(),
        2,
        "Expected 2 PGM001 findings for indexes on users and orders. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM003 fires for FK without index
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm003_fires() {
    // Replay 000001-000007 as history, lint 000008 (adds FK without index).
    let findings = lint_fixture(
        "go-migrate",
        &["000008_add_fk_without_index.up.sql"],
    );
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert!(
        !pgm003.is_empty(),
        "Expected at least 1 PGM003 finding for orders.assigned_user_id FK. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm003.iter().any(|f| f.message.contains("assigned_user_id")),
        "PGM003 should mention assigned_user_id. Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM004 fires for table without PK
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm004_fires() {
    // Replay 000001-000006, lint 000007 (creates audit_log without PK).
    let findings = lint_fixture(
        "go-migrate",
        &["000007_create_audit_log.up.sql"],
    );
    let pgm004: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM004").collect();

    assert_eq!(
        pgm004.len(),
        1,
        "Expected 1 PGM004 finding for audit_log. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm004[0].message.contains("audit_log"),
        "PGM004 should mention audit_log. Got: {}",
        pgm004[0].message
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM007 fires for volatile defaults
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm007_fires() {
    // Replay 000001-000008, lint 000009 (adds volatile defaults).
    let findings = lint_fixture(
        "go-migrate",
        &["000009_add_volatile_defaults.up.sql"],
    );
    let pgm007: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM007").collect();

    assert_eq!(
        pgm007.len(),
        2,
        "Expected 2 PGM007 findings for now() and gen_random_uuid(). Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM010 fires for NOT NULL without default
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm010_fires() {
    // Replay 000001-000009, lint 000010 (ADD COLUMN NOT NULL no default).
    let findings = lint_fixture(
        "go-migrate",
        &["000010_add_not_null_no_default.up.sql"],
    );
    let pgm010: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM010").collect();

    assert_eq!(
        pgm010.len(),
        1,
        "Expected 1 PGM010 finding for users.role. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm010[0].message.contains("role"),
        "PGM010 should mention 'role'. Got: {}",
        pgm010[0].message
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM012 fires for ADD PRIMARY KEY without unique
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_pgm012_fires() {
    // Replay 000001-000011, skip 000012.down.sql, lint 000012.up.sql
    // (ADD PRIMARY KEY on audit_log without prior unique constraint).
    let findings = lint_fixture(
        "go-migrate",
        &["000012_add_primary_key_no_unique.up.sql"],
    );
    let pgm012: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM012").collect();

    assert_eq!(
        pgm012.len(),
        1,
        "Expected 1 PGM012 finding for audit_log. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm012[0].message.contains("audit_log"),
        "PGM012 should mention audit_log. Got: {}",
        pgm012[0].message
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: Clean migrations produce no violations
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_clean_files_no_violations() {
    // Lint only 000013 and 000014 (clean migrations).
    // 000013 uses CONCURRENTLY, 000014 adds nullable/default columns.
    let findings = lint_fixture(
        "go-migrate",
        &[
            "000013_add_concurrently_index.up.sql",
            "000014_add_order_notes.up.sql",
        ],
    );

    // Filter out PGM007 findings from 000013 — it indexes tables that have
    // volatile defaults in their catalog state, but PGM007 only fires for
    // column defs in the current file, not for pre-existing columns.
    // Similarly, PGM006 checks CONCURRENTLY + run_in_transaction.
    // 000013 has CONCURRENTLY but SqlLoader sets run_in_transaction=true,
    // so PGM006 will fire for the CONCURRENTLY indexes.
    let non_pgm006: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id != "PGM006")
        .collect();

    assert!(
        non_pgm006.is_empty(),
        "Clean migrations should have no findings (except PGM006 for CONCURRENTLY in txn). Got:\n  {}",
        format_findings(&non_pgm006.iter().map(|f| (*f).clone()).collect::<Vec<_>>())
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: Multi-file changed set with targeted violations
// ---------------------------------------------------------------------------

#[test]
fn test_gomigrate_multi_file_changed_set() {
    // Lint 000006-000010 as changed (000001-000005 as history).
    let findings = lint_fixture(
        "go-migrate",
        &[
            "000006_add_indexes_no_concurrently.up.sql",
            "000007_create_audit_log.up.sql",
            "000008_add_fk_without_index.up.sql",
            "000009_add_volatile_defaults.up.sql",
            "000010_add_not_null_no_default.up.sql",
        ],
    );
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM001 fires for 000006 (indexes on pre-existing tables)
    assert!(
        rule_ids.contains("PGM001"),
        "Expected PGM001. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM003 fires for 000008 (FK without index)
    assert!(
        rule_ids.contains("PGM003"),
        "Expected PGM003. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM004 fires for 000007 (audit_log without PK)
    assert!(
        rule_ids.contains("PGM004"),
        "Expected PGM004. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM007 fires for 000009 (volatile defaults)
    assert!(
        rule_ids.contains("PGM007"),
        "Expected PGM007. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM010 fires for 000010 (NOT NULL no default)
    assert!(
        rule_ids.contains("PGM010"),
        "Expected PGM010. Got:\n  {}",
        format_findings(&findings)
    );

    // Should have a reasonable number of findings
    assert!(
        findings.len() >= 6,
        "Expected at least 6 findings from 000006-000010, got {}: \n  {}",
        findings.len(),
        format_findings(&findings)
    );
    assert!(
        findings.len() <= 30,
        "Expected at most 30 findings from 000006-000010, got {}",
        findings.len()
    );
}
