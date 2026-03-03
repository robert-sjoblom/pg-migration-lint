use std::{collections::HashSet, path::PathBuf};

use pg_migration_lint::input::sql::SqlLoader;

use crate::common::{format_findings, lint_fixture, lint_fixture_rules};

mod common;

#[test]
fn test_gomigrate_parses_all_migrations() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/go-migrate/migrations");
    let loader = SqlLoader::default();
    let history = loader
        .load(&[base])
        .expect("Failed to load go-migrate fixture");
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
    let loader = SqlLoader::default();
    let history = loader
        .load(&[base])
        .expect("Failed to load go-migrate fixture");

    let up_count = history.units.iter().filter(|u| !u.is_down).count();
    let down_count = history.units.iter().filter(|u| u.is_down).count();

    assert_eq!(up_count, 14, "Should have 14 up migrations");
    assert_eq!(down_count, 3, "Should have 3 down migrations");
}

#[test]
fn test_gomigrate_lint_all_finds_violations() {
    // When all files are changed, tables_created_in_change includes everything,
    // so only rules that don't check tables_created_in_change will fire.
    let findings = lint_fixture::<&str>("go-migrate", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM501 should fire (FK without covering index on orders.assigned_user_id)
    assert!(
        rule_ids.contains("PGM501"),
        "Expected PGM501 (FK without index). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM502 should fire (audit_log has no PK when first created in 000007)
    assert!(
        rule_ids.contains("PGM502"),
        "Expected PGM502 (table without PK). Got:\n  {}",
        format_findings(&findings)
    );
}

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

#[test]
fn test_gomigrate_changed_file_filtering() {
    // Only lint baseline files (000001-000005). Since these create new tables,
    // they are all in tables_created_in_change. Rules requiring pre-existing
    // tables (PGM001, PGM008, PGM016) should NOT fire.
    let findings = lint_fixture_rules(
        "go-migrate",
        &[
            "000001_create_users.up.sql",
            "000002_create_accounts.up.sql",
            "000003_create_orders.up.sql",
            "000004_create_order_items.up.sql",
            "000005_create_settings.up.sql",
        ],
        &["PGM001", "PGM008"],
    );

    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM001 should NOT fire (no CREATE INDEX without CONCURRENTLY in baseline)
    assert!(
        !rule_ids.contains("PGM001"),
        "PGM001 should not fire for baseline. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM008 should NOT fire (no ADD COLUMN NOT NULL in baseline)
    assert!(
        !rule_ids.contains("PGM008"),
        "PGM008 should not fire for baseline. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_gomigrate_pgm001_fires() {
    // Only 000006 is changed. Tables from 000001-000005 are replayed as
    // history (pre-existing). 000006 creates indexes WITHOUT CONCURRENTLY.
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000006_add_indexes_no_concurrently.up.sql"],
        &["PGM001"],
    );

    assert_eq!(
        findings.len(),
        2,
        "Expected 2 PGM001 findings for indexes on users and orders. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_gomigrate_pgm501_fires() {
    // Replay 000001-000007 as history, lint 000008 (adds FK without index).
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000008_add_fk_without_index.up.sql"],
        &["PGM501"],
    );

    assert!(
        !findings.is_empty(),
        "Expected at least 1 PGM501 finding for orders.assigned_user_id FK. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings
            .iter()
            .any(|f| f.message.contains("assigned_user_id")),
        "PGM501 should mention assigned_user_id. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_gomigrate_pgm502_fires() {
    // Replay 000001-000006, lint 000007 (creates audit_log without PK).
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000007_create_audit_log.up.sql"],
        &["PGM502"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected 1 PGM502 finding for audit_log. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("audit_log"),
        "PGM502 should mention audit_log. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_gomigrate_pgm006_fires() {
    // Replay 000001-000008, lint 000009 (adds volatile defaults).
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000009_add_volatile_defaults.up.sql"],
        &["PGM006"],
    );

    assert_eq!(
        findings.len(),
        2,
        "Expected 2 PGM006 findings for clock_timestamp() and gen_random_uuid(). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_gomigrate_pgm008_fires() {
    // Replay 000001-000009, lint 000010 (ADD COLUMN NOT NULL no default).
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000010_add_not_null_no_default.up.sql"],
        &["PGM008"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected 1 PGM008 finding for users.role. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("role"),
        "PGM008 should mention 'role'. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_gomigrate_pgm016_fires() {
    // Replay 000001-000011, skip 000012.down.sql, lint 000012.up.sql
    // (ADD PRIMARY KEY on audit_log without prior unique constraint).
    let findings = lint_fixture_rules(
        "go-migrate",
        &["000012_add_primary_key_no_unique.up.sql"],
        &["PGM016"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected 1 PGM016 finding for audit_log. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("audit_log"),
        "PGM016 should mention audit_log. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_gomigrate_clean_files_no_violations() {
    // Lint only 000013 and 000014 (clean migrations) with rules that
    // should NOT fire on well-written migrations.
    let findings = lint_fixture_rules(
        "go-migrate",
        &[
            "000013_add_concurrently_index.up.sql",
            "000014_add_order_notes.up.sql",
        ],
        &["PGM001", "PGM501", "PGM502", "PGM006", "PGM008"],
    );

    assert!(
        findings.is_empty(),
        "Clean migrations should have no findings for PGM001/501/502/006/008. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_gomigrate_multi_file_changed_set() {
    // Lint 000006-000010 as changed (000001-000005 as history).
    let findings = lint_fixture_rules(
        "go-migrate",
        &[
            "000006_add_indexes_no_concurrently.up.sql",
            "000007_create_audit_log.up.sql",
            "000008_add_fk_without_index.up.sql",
            "000009_add_volatile_defaults.up.sql",
            "000010_add_not_null_no_default.up.sql",
        ],
        &["PGM001", "PGM501", "PGM502", "PGM006", "PGM008"],
    );
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM001 fires for 000006 (indexes on pre-existing tables)
    assert!(
        rule_ids.contains("PGM001"),
        "Expected PGM001. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM501 fires for 000008 (FK without index)
    assert!(
        rule_ids.contains("PGM501"),
        "Expected PGM501. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM502 fires for 000007 (audit_log without PK)
    assert!(
        rule_ids.contains("PGM502"),
        "Expected PGM502. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM006 fires for 000009 (volatile defaults)
    assert!(
        rule_ids.contains("PGM006"),
        "Expected PGM006. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM008 fires for 000010 (NOT NULL no default)
    assert!(
        rule_ids.contains("PGM008"),
        "Expected PGM008. Got:\n  {}",
        format_findings(&findings)
    );
}
