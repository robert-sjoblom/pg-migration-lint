//! Integration tests for the full lint pipeline.

use pg_migration_lint::IrNode;
use pg_migration_lint::catalog::Catalog;
use pg_migration_lint::catalog::replay;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::input::{MigrationLoader, MigrationUnit};
use pg_migration_lint::normalize;
use pg_migration_lint::output::{Reporter, SarifReporter, SonarQubeReporter};
use pg_migration_lint::rules::{Finding, LintContext, Rule, RuleRegistry, cap_for_down_migration};
use pg_migration_lint::suppress::parse_suppressions;
use std::collections::HashSet;
use std::path::PathBuf;

/// Run the full lint pipeline on a fixture repo.
/// If `changed_files` is empty, all files are linted.
fn lint_fixture(fixture_name: &str, changed_filenames: &[&str]) -> Vec<Finding> {
    lint_fixture_with_schema(fixture_name, changed_filenames, "public")
}

/// Run the full lint pipeline on a fixture repo with a custom default schema.
/// If `changed_files` is empty, all files are linted.
fn lint_fixture_with_schema(
    fixture_name: &str,
    changed_filenames: &[&str],
    default_schema: &str,
) -> Vec<Finding> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos")
        .join(fixture_name)
        .join("migrations");

    let loader = SqlLoader::default();
    let mut history = loader
        .load(std::slice::from_ref(&base))
        .expect("Failed to load fixture");

    normalize::normalize_schemas(&mut history.units, default_schema);

    let changed: HashSet<PathBuf> = changed_filenames.iter().map(|f| base.join(f)).collect();

    let mut catalog = Catalog::new();
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();

    for unit in &history.units {
        let is_changed = changed.is_empty() || changed.contains(&unit.source_file);

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

/// Normalize finding paths for snapshot stability across machines.
///
/// Strips the machine-specific prefix from each finding's file path, keeping
/// only the portion starting from `repos/{fixture_name}/...`.
fn normalize_findings(findings: Vec<Finding>, fixture_name: &str) -> Vec<Finding> {
    let marker = format!("repos/{}/", fixture_name);
    findings
        .into_iter()
        .map(|mut f| {
            let path_str = f.file.to_string_lossy().to_string();
            if let Some(pos) = path_str.find(&marker) {
                f.file = std::path::PathBuf::from(&path_str[pos..]);
            }
            f
        })
        .collect()
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
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );
}

// ---------------------------------------------------------------------------
// All-rules repo: one violation per rule
// ---------------------------------------------------------------------------

#[test]
fn test_all_rules_trigger() {
    // V002-V006 are changed; V001 is just replayed as baseline.
    // This ensures tables from V001 are in catalog_before but NOT in
    // tables_created_in_change, so rules that check for pre-existing tables
    // (PGM001, PGM002, PGM009, PGM010, PGM011, PGM016-PGM020) will fire.
    // V004 introduces "Don't Do This" type anti-patterns (PGM101-PGM105).
    // V005 introduces PGM016-PGM020 violations.
    // V006 introduces PGM108 (json type).
    let findings = lint_fixture(
        "all-rules",
        &[
            "V002__violations.sql",
            "V003__more_violations.sql",
            "V004__dont_do_this_types.sql",
            "V005__new_violations.sql",
            "V006__json_type.sql",
        ],
    );
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    for expected in &[
        "PGM001", "PGM002", "PGM003", "PGM004", "PGM005", "PGM006", "PGM007", "PGM009", "PGM010",
        "PGM011", "PGM012", "PGM013", "PGM014", "PGM015", "PGM016", "PGM017", "PGM018", "PGM019",
        "PGM020", "PGM101", "PGM102", "PGM103", "PGM104", "PGM105", "PGM108",
    ] {
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
    // Only V002-V006 are changed; V001 just replays.
    let findings = lint_fixture(
        "suppressed",
        &[
            "V002__suppressed.sql",
            "V003__suppressed.sql",
            "V004__suppressed.sql",
            "V005__suppressed.sql",
            "V006__suppressed.sql",
        ],
    );
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
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
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
    let pgm001: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM001")
        .collect();
    let pgm001 = normalize_findings(pgm001, "all-rules");
    insta::assert_yaml_snapshot!(pgm001);
}

#[test]
fn test_pgm003_finding_details() {
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let pgm003: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM003")
        .collect();
    let pgm003 = normalize_findings(pgm003, "all-rules");
    insta::assert_yaml_snapshot!(pgm003);
}

#[test]
fn test_pgm004_finding_details() {
    // V002 creates audit_log without PK. Since V002 is changed, audit_log
    // is in tables_created_in_change, but PGM004 does not check that set --
    // it only checks catalog_after for has_primary_key.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    let pgm004: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM004")
        .collect();
    let pgm004 = normalize_findings(pgm004, "all-rules");
    insta::assert_yaml_snapshot!(pgm004);
}

#[test]
fn test_pgm002_finding_details() {
    // V003 drops idx_customers_email WITHOUT CONCURRENTLY.
    // V001 is replayed as baseline (creates the index), V002 and V003 are changed.
    let findings = lint_fixture(
        "all-rules",
        &["V002__violations.sql", "V003__more_violations.sql"],
    );
    let pgm002: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM002")
        .collect();
    let pgm002 = normalize_findings(pgm002, "all-rules");
    insta::assert_yaml_snapshot!(pgm002);
}

#[test]
fn test_pgm005_finding_details() {
    // V003 creates the 'settings' table with UNIQUE NOT NULL but no PK.
    let findings = lint_fixture("all-rules", &["V003__more_violations.sql"]);
    let pgm005: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM005")
        .collect();
    let pgm005 = normalize_findings(pgm005, "all-rules");
    insta::assert_yaml_snapshot!(pgm005);
}

#[test]
fn test_pgm006_finding_details() {
    // V003 uses CREATE INDEX CONCURRENTLY inside a transaction (SqlLoader
    // sets run_in_transaction=true by default).
    let findings = lint_fixture("all-rules", &["V003__more_violations.sql"]);
    let pgm006: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM006")
        .collect();
    let pgm006 = normalize_findings(pgm006, "all-rules");
    insta::assert_yaml_snapshot!(pgm006);
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
    let findings = lint_fixture("all-rules", &["V004__dont_do_this_types.sql"]);
    let pgm101: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM101")
        .collect();
    let pgm101 = normalize_findings(pgm101, "all-rules");
    insta::assert_yaml_snapshot!(pgm101);
}

#[test]
fn test_pgm102_timestamptz_zero_precision() {
    let findings = lint_fixture("all-rules", &["V004__dont_do_this_types.sql"]);
    let pgm102: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM102")
        .collect();
    let pgm102 = normalize_findings(pgm102, "all-rules");
    insta::assert_yaml_snapshot!(pgm102);
}

#[test]
fn test_pgm103_char_n_type() {
    let findings = lint_fixture("all-rules", &["V004__dont_do_this_types.sql"]);
    let pgm103: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM103")
        .collect();
    let pgm103 = normalize_findings(pgm103, "all-rules");
    insta::assert_yaml_snapshot!(pgm103);
}

#[test]
fn test_pgm104_money_type() {
    let findings = lint_fixture("all-rules", &["V004__dont_do_this_types.sql"]);
    let pgm104: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM104")
        .collect();
    let pgm104 = normalize_findings(pgm104, "all-rules");
    insta::assert_yaml_snapshot!(pgm104);
}

#[test]
fn test_pgm105_serial_type() {
    let findings = lint_fixture("all-rules", &["V004__dont_do_this_types.sql"]);
    let pgm105: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM105")
        .collect();
    let pgm105 = normalize_findings(pgm105, "all-rules");
    insta::assert_yaml_snapshot!(pgm105);
}

// ---------------------------------------------------------------------------
// New rules: PGM016-PGM020, PGM108
// ---------------------------------------------------------------------------

#[test]
fn test_pgm016_finding_details() {
    let findings = lint_fixture("all-rules", &["V005__new_violations.sql"]);
    let pgm016: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM016")
        .collect();
    let pgm016 = normalize_findings(pgm016, "all-rules");
    insta::assert_yaml_snapshot!(pgm016);
}

#[test]
fn test_pgm017_finding_details() {
    let findings = lint_fixture("all-rules", &["V005__new_violations.sql"]);
    let pgm017: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM017")
        .collect();
    let pgm017 = normalize_findings(pgm017, "all-rules");
    insta::assert_yaml_snapshot!(pgm017);
}

#[test]
fn test_pgm018_finding_details() {
    let findings = lint_fixture("all-rules", &["V005__new_violations.sql"]);
    let pgm018: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM018")
        .collect();
    let pgm018 = normalize_findings(pgm018, "all-rules");
    insta::assert_yaml_snapshot!(pgm018);
}

#[test]
fn test_pgm019_finding_details() {
    let findings = lint_fixture("all-rules", &["V005__new_violations.sql"]);
    let pgm019: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM019")
        .collect();
    let pgm019 = normalize_findings(pgm019, "all-rules");
    insta::assert_yaml_snapshot!(pgm019);
}

#[test]
fn test_pgm020_finding_details() {
    let findings = lint_fixture("all-rules", &["V005__new_violations.sql"]);
    let pgm020: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM020")
        .collect();
    let pgm020 = normalize_findings(pgm020, "all-rules");
    insta::assert_yaml_snapshot!(pgm020);
}

#[test]
fn test_pgm108_finding_details() {
    let findings = lint_fixture("all-rules", &["V006__json_type.sql"]);
    let pgm108: Vec<_> = findings
        .into_iter()
        .filter(|f| f.rule_id == "PGM108")
        .collect();
    let pgm108 = normalize_findings(pgm108, "all-rules");
    insta::assert_yaml_snapshot!(pgm108);
}

// ---------------------------------------------------------------------------
// Enterprise fixture: realistic 31-file migration history
// ---------------------------------------------------------------------------

#[test]
fn test_enterprise_parses_all_migrations() {
    // Verify all 31 migrations load and parse without errors
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/enterprise/migrations");
    let loader = SqlLoader::default();
    let history = loader
        .load(&[base])
        .expect("Failed to load enterprise fixture");
    assert_eq!(history.units.len(), 31, "Should have 31 migration units");
}

#[test]
fn test_enterprise_lint_all_finds_violations() {
    let findings = lint_fixture("enterprise", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM003 should fire (FKs without covering indexes in V005, V006, V010, V021, V029)
    assert!(
        rule_ids.contains("PGM003"),
        "Expected PGM003. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM004 should fire (many tables without PKs in V003, V015, V020, V021)
    assert!(
        rule_ids.contains("PGM004"),
        "Expected PGM004. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM007 should fire (volatile defaults in V012, V018, V022, V027, V028, V029)
    assert!(
        rule_ids.contains("PGM007"),
        "Expected PGM007. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v007_only() {
    // V001-V006 are replayed as history, V007 is the only changed file.
    // V007 creates indexes WITHOUT CONCURRENTLY on pre-existing tables → PGM001
    let findings = lint_fixture("enterprise", &["V007__create_index_no_concurrently.sql"]);
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(
        pgm001.len(),
        3,
        "Expected 3 PGM001 findings for 3 non-concurrent indexes. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v023_only() {
    // V001-V022 replayed, V023 is changed: DROP INDEX without CONCURRENTLY → PGM002
    let findings = lint_fixture("enterprise", &["V023__drop_index_no_concurrently.sql"]);
    let pgm002: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM002").collect();

    assert!(
        !pgm002.is_empty(),
        "Expected PGM002 for DROP INDEX without CONCURRENTLY. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v008_only() {
    // V001-V007 replayed, V008 is changed: ADD COLUMN NOT NULL without default → PGM010
    let findings = lint_fixture("enterprise", &["V008__add_not_null_column.sql"]);
    let pgm010: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM010").collect();

    assert_eq!(
        pgm010.len(),
        1,
        "Expected 1 PGM010 for NOT NULL without default. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v013_only() {
    // V001-V012 replayed, V013 is changed: ALTER COLUMN TYPE → PGM009
    let findings = lint_fixture("enterprise", &["V013__alter_column_type.sql"]);
    let pgm009: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM009").collect();

    assert!(
        !pgm009.is_empty(),
        "Expected PGM009 for ALTER COLUMN TYPE. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_finding_count_reasonable() {
    // Lint only V005-V015 as changed (V001-V004 as history).
    // This should produce a reasonable number of findings.
    let findings = lint_fixture(
        "enterprise",
        &[
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
        ],
    );

    // Should have a significant number of findings.
    // Actual count is 45 as of 2026-02-11; allow ±3 for minor rule tuning.
    assert!(
        findings.len() >= 42,
        "Expected at least 42 findings from V005-V015, got {}: \n  {}",
        findings.len(),
        format_findings(&findings)
    );
    assert!(
        findings.len() <= 48,
        "Expected at most 48 findings from V005-V015, got {}:\n  {}",
        findings.len(),
        format_findings(&findings)
    );
}

/// Assert per-rule counts on the full enterprise run.
///
/// This test is a tripwire for regressions that silently change scope filters.
/// For example, if PGM013/014/015 are accidentally switched to
/// `ExcludeCreatedInChange`, their counts drop to 0 in the full run because
/// every table is in `tables_created_in_change`.
#[test]
fn test_enterprise_full_run_per_rule_counts() {
    use std::collections::HashMap;

    let findings = lint_fixture("enterprise", &[]);
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for f in &findings {
        *counts.entry(f.rule_id.as_str()).or_default() += 1;
    }

    // Baseline counts as of 2026-02-14 (31 migration files).
    // Each entry: (rule_id, expected_count).
    let expected: &[(&str, usize)] = &[
        ("PGM003", 18),
        ("PGM004", 9),
        ("PGM005", 8),
        ("PGM006", 7),
        ("PGM007", 32),
        ("PGM013", 2),
        ("PGM014", 1),
        ("PGM015", 3),
        ("PGM101", 19),
        ("PGM105", 22),
    ];

    for &(rule, expected_count) in expected {
        let actual = counts.get(rule).copied().unwrap_or(0);
        assert_eq!(
            actual,
            expected_count,
            "Rule {rule}: expected {expected_count} findings, got {actual}.\n  All findings:\n  {}",
            format_findings(&findings)
        );
    }

    assert_eq!(
        findings.len(),
        121,
        "Total finding count changed. Got {}:\n  {}",
        findings.len(),
        format_findings(&findings)
    );
}

// ===========================================================================
// Enterprise changed-files mode: incremental lint behavior
// ===========================================================================

#[test]
fn test_enterprise_changed_file_single_late_migration() {
    // Lint only V014 (drop column). V001-V013 are replayed as history.
    // All findings should come exclusively from V014.
    let findings_v014 = lint_fixture("enterprise", &["V014__drop_column.sql"]);

    // Every finding must originate from V014's file
    let v014_suffix = "V014__drop_column.sql";
    for finding in &findings_v014 {
        let path = finding.file.display().to_string();
        assert!(
            path.ends_with(v014_suffix),
            "All findings should be from V014, but found one from '{}': {} {}",
            path,
            finding.rule_id,
            finding.message
        );
    }

    // The incremental count for a single file should be less than the full run
    let findings_full = lint_fixture("enterprise", &[]);
    assert!(
        findings_v014.len() < findings_full.len(),
        "Single-file incremental ({}) should produce fewer findings than full run ({})",
        findings_v014.len(),
        findings_full.len()
    );
}

#[test]
fn test_enterprise_changed_file_volatile_defaults() {
    // Lint only V022 (add volatile defaults). V001-V021 are replayed as history.
    // V022 adds columns with gen_random_uuid() and now() defaults -> PGM007.
    let findings = lint_fixture("enterprise", &["V022__add_volatile_defaults.sql"]);

    let pgm007: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM007").collect();
    assert!(
        !pgm007.is_empty(),
        "Expected PGM007 for volatile defaults in V022. Got:\n  {}",
        format_findings(&findings)
    );

    // V022 does not add any foreign keys, so PGM003 should not fire
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();
    assert!(
        pgm003.is_empty(),
        "PGM003 should not fire for V022 (no FK creation). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_changed_files_reduces_fk_noise() {
    // Full run should produce PGM003 findings (FKs without covering indexes)
    let findings_full = lint_fixture("enterprise", &[]);
    let pgm003_full: Vec<&Finding> = findings_full
        .iter()
        .filter(|f| f.rule_id == "PGM003")
        .collect();
    assert!(
        !pgm003_full.is_empty(),
        "Full run should have PGM003 findings. Got:\n  {}",
        format_findings(&findings_full)
    );

    // Targeting only V014 (drop column, no FK creation) should have 0 PGM003
    let findings_v014 = lint_fixture("enterprise", &["V014__drop_column.sql"]);
    let pgm003_v014: Vec<&Finding> = findings_v014
        .iter()
        .filter(|f| f.rule_id == "PGM003")
        .collect();
    assert!(
        pgm003_v014.is_empty(),
        "V014 (drop column) should have 0 PGM003 findings. Got:\n  {}",
        format_findings(&findings_v014)
    );
}

#[test]
fn test_enterprise_full_vs_incremental_counts() {
    // Full lint (all files changed)
    let findings_full = lint_fixture("enterprise", &[]);

    // Incremental lint (only V014 + V022 changed)
    let findings_incremental = lint_fixture(
        "enterprise",
        &["V014__drop_column.sql", "V022__add_volatile_defaults.sql"],
    );

    assert!(
        !findings_full.is_empty(),
        "Full run should produce findings"
    );
    assert!(
        !findings_incremental.is_empty(),
        "Incremental run should produce findings (V014 triggers PGM011, V022 triggers PGM007)"
    );
    assert!(
        findings_incremental.len() < findings_full.len(),
        "Incremental count ({}) should be less than full count ({})",
        findings_incremental.len(),
        findings_full.len()
    );
}

// ===========================================================================
// Enterprise sliding-window test: lint each migration individually
// ===========================================================================

/// Lint each enterprise migration as the sole changed file, with all prior
/// migrations replayed as catalog history. This simulates the real CI workflow
/// where each PR introduces one new migration file.
///
/// A single snapshot captures the findings for every step, making it easy to
/// review what each migration triggers and catch regressions.
#[test]
fn test_enterprise_sliding_window() {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/enterprise/migrations");

    let loader = SqlLoader::default();
    let mut history = loader
        .load(std::slice::from_ref(&base))
        .expect("Failed to load enterprise fixture");

    normalize::normalize_schemas(&mut history.units, "public");

    let mut registry = RuleRegistry::new();
    registry.register_defaults();

    // Collect (filename, findings) for each step
    let mut steps: Vec<(String, Vec<Finding>)> = Vec::new();
    let mut catalog = Catalog::new();

    for (i, unit) in history.units.iter().enumerate() {
        let catalog_before = catalog.clone();
        replay::apply(&mut catalog, unit);

        // Track tables created in this single-file change
        let mut tables_created_in_change: HashSet<String> = HashSet::new();
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

        let filename = unit
            .source_file
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Sort findings deterministically within each step
        unit_findings.sort_by(|a, b| {
            a.rule_id
                .cmp(&b.rule_id)
                .then_with(|| a.start_line.cmp(&b.start_line))
        });

        steps.push((
            format!("step_{:02}_{}", i + 1, filename.trim_end_matches(".sql")),
            unit_findings,
        ));
    }

    // Build a structured snapshot: map of step name -> list of (rule_id, message)
    // Using a lightweight representation to keep the snapshot readable.
    let snapshot: Vec<_> = steps
        .iter()
        .filter(|(_, findings)| !findings.is_empty())
        .map(|(step_name, findings)| {
            let entries: Vec<_> = findings
                .iter()
                .map(|f| {
                    serde_json::json!({
                        "rule": f.rule_id,
                        "severity": format!("{:?}", f.severity),
                        "line": f.start_line,
                        "message": f.message,
                    })
                })
                .collect();
            serde_json::json!({
                "step": step_name,
                "findings": entries,
            })
        })
        .collect();

    insta::assert_yaml_snapshot!(snapshot);
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
    let findings = lint_fixture("go-migrate", &["000006_add_indexes_no_concurrently.up.sql"]);
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
    let findings = lint_fixture("go-migrate", &["000008_add_fk_without_index.up.sql"]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert!(
        !pgm003.is_empty(),
        "Expected at least 1 PGM003 finding for orders.assigned_user_id FK. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm003
            .iter()
            .any(|f| f.message.contains("assigned_user_id")),
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
    let findings = lint_fixture("go-migrate", &["000007_create_audit_log.up.sql"]);
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
    let findings = lint_fixture("go-migrate", &["000009_add_volatile_defaults.up.sql"]);
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
    let findings = lint_fixture("go-migrate", &["000010_add_not_null_no_default.up.sql"]);
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
    let findings = lint_fixture("go-migrate", &["000012_add_primary_key_no_unique.up.sql"]);
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
    let non_pgm006: Vec<&Finding> = findings.iter().filter(|f| f.rule_id != "PGM006").collect();

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

// ===========================================================================
// SARIF output integration tests
// ===========================================================================

#[test]
fn test_sarif_output_valid_structure() {
    // Run the all-rules fixture through the full pipeline, emit SARIF, and
    // verify the output is valid SARIF 2.1.0 with correct structure.
    let findings = lint_fixture(
        "all-rules",
        &[
            "V002__violations.sql",
            "V003__more_violations.sql",
            "V004__dont_do_this_types.sql",
            "V005__new_violations.sql",
            "V006__json_type.sql",
        ],
    );
    assert!(
        !findings.is_empty(),
        "All-rules fixture should produce findings"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SarifReporter::new();
    reporter.emit(&findings, dir.path()).expect("emit SARIF");

    let content =
        std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read SARIF file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    // Verify it's valid SARIF 2.1.0
    assert_eq!(
        parsed["$schema"],
        "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "SARIF $schema field must be the 2.1.0 schema URL"
    );
    assert_eq!(parsed["version"], "2.1.0", "SARIF version must be 2.1.0");

    // Verify runs array
    let runs = parsed["runs"].as_array().expect("runs should be an array");
    assert_eq!(runs.len(), 1, "Should have exactly 1 run");

    // Verify tool driver
    let driver = &runs[0]["tool"]["driver"];
    assert_eq!(driver["name"], "pg-migration-lint");
    assert!(driver["version"].is_string(), "driver should have version");
    assert!(
        driver["informationUri"].is_string(),
        "driver should have informationUri"
    );

    // Verify results count matches findings
    let results = runs[0]["results"]
        .as_array()
        .expect("results should be an array");
    assert_eq!(
        results.len(),
        findings.len(),
        "SARIF results count should match findings count"
    );

    // Verify all results have correct ruleIds from our rule set
    let known_rules: HashSet<&str> = [
        "PGM001", "PGM002", "PGM003", "PGM004", "PGM005", "PGM006", "PGM007", "PGM009", "PGM010",
        "PGM011", "PGM012", "PGM013", "PGM014", "PGM015", "PGM016", "PGM017", "PGM018", "PGM019",
        "PGM020", "PGM101", "PGM102", "PGM103", "PGM104", "PGM105", "PGM108",
    ]
    .into_iter()
    .collect();
    for result in results {
        let rule_id = result["ruleId"]
            .as_str()
            .expect("ruleId should be a string");
        assert!(
            known_rules.contains(rule_id),
            "SARIF result ruleId '{}' should be a known rule",
            rule_id
        );
    }

    // Verify file paths in results are not empty and reference SQL files
    for result in results {
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("artifactLocation.uri should be a string");
        assert!(
            !uri.is_empty(),
            "SARIF artifactLocation.uri should not be empty"
        );
        assert!(
            uri.contains(".sql"),
            "SARIF file paths should reference SQL files, got: {}",
            uri
        );
    }

    // Verify rules array has entries for distinct rule IDs in findings
    let rules = driver["rules"]
        .as_array()
        .expect("rules should be an array");
    let finding_rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert_eq!(
        rules.len(),
        finding_rule_ids.len(),
        "SARIF rules array should have one entry per distinct rule ID"
    );
    for rule in rules {
        assert!(rule["id"].is_string(), "Each rule must have an id");
        assert!(
            rule["shortDescription"]["text"].is_string(),
            "Each rule must have shortDescription.text"
        );
        assert!(
            rule["defaultConfiguration"]["level"].is_string(),
            "Each rule must have defaultConfiguration.level"
        );
        let level = rule["defaultConfiguration"]["level"].as_str().unwrap();
        assert!(
            ["error", "warning", "note"].contains(&level),
            "Rule level must be error, warning, or note; got: {}",
            level
        );
    }

    // Verify line numbers are positive and endLine >= startLine
    for result in results {
        let region = &result["locations"][0]["physicalLocation"]["region"];
        let start_line = region["startLine"]
            .as_u64()
            .expect("startLine should be a number");
        let end_line = region["endLine"]
            .as_u64()
            .expect("endLine should be a number");
        assert!(start_line >= 1, "startLine should be >= 1");
        assert!(end_line >= start_line, "endLine should be >= startLine");
    }

    // Verify SARIF levels map correctly to known values
    for result in results {
        let level = result["level"].as_str().expect("level should be a string");
        assert!(
            ["error", "warning", "note"].contains(&level),
            "Result level must be error, warning, or note; got: {}",
            level
        );
    }
}

#[test]
fn test_sarif_output_round_trip_from_fixture() {
    // A focused round-trip test: emit SARIF from a small changed-file set,
    // parse it back, and verify specific finding data survives serialization.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    assert!(!findings.is_empty(), "V002 should produce findings");

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SarifReporter::new();
    reporter.emit(&findings, dir.path()).expect("emit SARIF");

    let content =
        std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read SARIF file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("results array");

    // For each original finding, verify it appears in the SARIF output
    for finding in &findings {
        let matching = results.iter().find(|r| {
            r["ruleId"].as_str() == Some(&finding.rule_id)
                && r["message"]["text"].as_str() == Some(&finding.message)
        });
        assert!(
            matching.is_some(),
            "Finding {} with message '{}' should appear in SARIF output",
            finding.rule_id,
            finding.message
        );

        let matched = matching.unwrap();
        let loc = &matched["locations"][0]["physicalLocation"];
        assert_eq!(
            loc["region"]["startLine"].as_u64().unwrap() as usize,
            finding.start_line,
            "startLine mismatch for {}",
            finding.rule_id
        );
        assert_eq!(
            loc["region"]["endLine"].as_u64().unwrap() as usize,
            finding.end_line,
            "endLine mismatch for {}",
            finding.rule_id
        );
    }
}

// ===========================================================================
// SonarQube output integration tests
// ===========================================================================

#[test]
fn test_sonarqube_output_valid_structure() {
    // Run the all-rules fixture through the full pipeline, emit SonarQube JSON,
    // and verify the output has the correct Generic Issue Import structure.
    let findings = lint_fixture(
        "all-rules",
        &[
            "V002__violations.sql",
            "V003__more_violations.sql",
            "V004__dont_do_this_types.sql",
            "V005__new_violations.sql",
            "V006__json_type.sql",
        ],
    );
    assert!(
        !findings.is_empty(),
        "All-rules fixture should produce findings"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new();
    reporter
        .emit(&findings, dir.path())
        .expect("emit SonarQube JSON");

    let content =
        std::fs::read_to_string(dir.path().join("findings.json")).expect("read SonarQube file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    // Verify top-level structure
    let issues = parsed["issues"]
        .as_array()
        .expect("issues should be an array");
    assert_eq!(
        issues.len(),
        findings.len(),
        "SonarQube issues count should match findings count"
    );

    // Verify each issue has the required fields
    let known_rules: HashSet<&str> = [
        "PGM001", "PGM002", "PGM003", "PGM004", "PGM005", "PGM006", "PGM007", "PGM009", "PGM010",
        "PGM011", "PGM012", "PGM013", "PGM014", "PGM015", "PGM016", "PGM017", "PGM018", "PGM019",
        "PGM020", "PGM101", "PGM102", "PGM103", "PGM104", "PGM105", "PGM108",
    ]
    .into_iter()
    .collect();

    for issue in issues {
        // engineId
        assert_eq!(
            issue["engineId"], "pg-migration-lint",
            "All issues must have engineId 'pg-migration-lint'"
        );

        // ruleId
        let rule_id = issue["ruleId"].as_str().expect("ruleId should be a string");
        assert!(
            known_rules.contains(rule_id),
            "SonarQube ruleId '{}' should be a known rule",
            rule_id
        );

        // severity
        let severity = issue["severity"]
            .as_str()
            .expect("severity should be a string");
        assert!(
            ["BLOCKER", "CRITICAL", "MAJOR", "MINOR", "INFO"].contains(&severity),
            "Severity must be a valid SonarQube severity; got: {}",
            severity
        );

        // type
        assert_eq!(issue["type"], "BUG", "Issue type should be BUG");

        // primaryLocation
        let primary_location = &issue["primaryLocation"];
        assert!(
            primary_location["message"].is_string(),
            "primaryLocation must have a message"
        );
        let message = primary_location["message"]
            .as_str()
            .expect("message string");
        assert!(
            !message.is_empty(),
            "primaryLocation.message should not be empty"
        );

        let file_path = primary_location["filePath"]
            .as_str()
            .expect("filePath should be a string");
        assert!(!file_path.is_empty(), "filePath should not be empty");
        assert!(
            file_path.contains(".sql"),
            "SonarQube file paths should reference SQL files, got: {}",
            file_path
        );

        // textRange
        let text_range = &primary_location["textRange"];
        let start_line = text_range["startLine"]
            .as_u64()
            .expect("startLine should be a number");
        let end_line = text_range["endLine"]
            .as_u64()
            .expect("endLine should be a number");
        assert!(start_line >= 1, "startLine should be >= 1");
        assert!(end_line >= start_line, "endLine should be >= startLine");
    }
}

#[test]
fn test_sonarqube_output_round_trip_from_fixture() {
    // Focused round-trip: emit SonarQube JSON from a small changed-file set,
    // parse it back, and verify each finding's data survives serialization.
    let findings = lint_fixture("all-rules", &["V002__violations.sql"]);
    assert!(!findings.is_empty(), "V002 should produce findings");

    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new();
    reporter
        .emit(&findings, dir.path())
        .expect("emit SonarQube JSON");

    let content =
        std::fs::read_to_string(dir.path().join("findings.json")).expect("read SonarQube file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    let issues = parsed["issues"].as_array().expect("issues array");

    // For each original finding, verify it appears in the SonarQube output
    for finding in &findings {
        let matching = issues.iter().find(|issue| {
            issue["ruleId"].as_str() == Some(&finding.rule_id)
                && issue["primaryLocation"]["message"].as_str() == Some(&finding.message)
        });
        assert!(
            matching.is_some(),
            "Finding {} with message '{}' should appear in SonarQube output",
            finding.rule_id,
            finding.message
        );

        let matched = matching.unwrap();

        // Verify severity mapping
        let expected_severity = finding.severity.sonarqube_str();
        assert_eq!(
            matched["severity"].as_str().unwrap(),
            expected_severity,
            "Severity mismatch for {} finding",
            finding.rule_id
        );

        // Verify line numbers
        let text_range = &matched["primaryLocation"]["textRange"];
        assert_eq!(
            text_range["startLine"].as_u64().unwrap() as usize,
            finding.start_line,
            "startLine mismatch for {}",
            finding.rule_id
        );
        assert_eq!(
            text_range["endLine"].as_u64().unwrap() as usize,
            finding.end_line,
            "endLine mismatch for {}",
            finding.rule_id
        );

        // Verify file path is not empty
        let file_path = matched["primaryLocation"]["filePath"]
            .as_str()
            .expect("filePath string");
        assert!(
            !file_path.is_empty(),
            "filePath should not be empty for {}",
            finding.rule_id
        );
    }
}

#[test]
fn test_sarif_and_sonarqube_finding_counts_match() {
    // Both reporters should produce the same number of entries from the same findings.
    let findings = lint_fixture(
        "all-rules",
        &[
            "V002__violations.sql",
            "V003__more_violations.sql",
            "V004__dont_do_this_types.sql",
            "V005__new_violations.sql",
            "V006__json_type.sql",
        ],
    );

    let dir_sarif = tempfile::tempdir().expect("sarif tempdir");
    let dir_sonar = tempfile::tempdir().expect("sonar tempdir");

    let sarif_reporter = SarifReporter::new();
    sarif_reporter
        .emit(&findings, dir_sarif.path())
        .expect("emit SARIF");

    let sonar_reporter = SonarQubeReporter::new();
    sonar_reporter
        .emit(&findings, dir_sonar.path())
        .expect("emit SonarQube");

    let sarif_content =
        std::fs::read_to_string(dir_sarif.path().join("findings.sarif")).expect("read SARIF");
    let sonar_content =
        std::fs::read_to_string(dir_sonar.path().join("findings.json")).expect("read SonarQube");

    let sarif_parsed: serde_json::Value =
        serde_json::from_str(&sarif_content).expect("parse SARIF");
    let sonar_parsed: serde_json::Value =
        serde_json::from_str(&sonar_content).expect("parse SonarQube");

    let sarif_count = sarif_parsed["runs"][0]["results"]
        .as_array()
        .expect("SARIF results")
        .len();
    let sonar_count = sonar_parsed["issues"]
        .as_array()
        .expect("SonarQube issues")
        .len();

    assert_eq!(
        sarif_count, sonar_count,
        "SARIF result count ({}) should match SonarQube issue count ({})",
        sarif_count, sonar_count
    );
    assert_eq!(
        sarif_count,
        findings.len(),
        "Both should match the original findings count ({})",
        findings.len()
    );
}

// ===========================================================================
// Cross-file FK detection (PGM003) with fk-with-later-index fixture
// ===========================================================================

#[test]
fn test_fk_without_index_cross_file_only_fk_changed() {
    // Only V002 is changed. V001 is replayed as history (creates tables).
    // V002 adds FK on orders.customer_id but V003 (which adds the covering
    // index) has NOT been replayed yet. PGM003 should fire because
    // catalog_after has no covering index at this point.
    let findings = lint_fixture("fk-with-later-index", &["V002__add_fk.sql"]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert_eq!(
        pgm003.len(),
        1,
        "Expected exactly 1 PGM003 finding for FK without index. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm003[0].message.contains("customer_id"),
        "PGM003 message should mention 'customer_id'. Got: {}",
        pgm003[0].message
    );
}

#[test]
fn test_fk_with_later_index_only_index_changed() {
    // Only V003 is changed. V001 and V002 are replayed as history.
    // The FK from V002 already exists in catalog_before, and V003 adds
    // the covering index. Since V002 is not being linted, no PGM003
    // should fire -- the FK was in a prior file, not in the current lint set.
    let findings = lint_fixture("fk-with-later-index", &["V003__add_index.sql"]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert!(
        pgm003.is_empty(),
        "PGM003 should NOT fire when only the index file is linted. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_fk_cross_file_both_changed() {
    // Both V002 and V003 are changed. V001 is replayed as history.
    // When linting V002: FK is added but no covering index yet -> PGM003 fires.
    // When linting V003: index is added, no new FK in this file -> no PGM003.
    let findings = lint_fixture(
        "fk-with-later-index",
        &["V002__add_fk.sql", "V003__add_index.sql"],
    );
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert_eq!(
        pgm003.len(),
        1,
        "Expected exactly 1 PGM003 finding (from V002 only). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm003[0].message.contains("customer_id"),
        "PGM003 message should mention 'customer_id'. Got: {}",
        pgm003[0].message
    );
}

#[test]
fn test_fk_cross_file_all_changed() {
    // All files are changed (empty changed set). V001 creates tables (no FK,
    // no finding). V002 adds FK without covering index -> PGM003 fires.
    // V003 adds the covering index -> no additional PGM003.
    let findings = lint_fixture("fk-with-later-index", &[]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert_eq!(
        pgm003.len(),
        1,
        "Expected exactly 1 PGM003 finding (from V002). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm003[0].message.contains("customer_id"),
        "PGM003 message should mention 'customer_id'. Got: {}",
        pgm003[0].message
    );
}

// ===========================================================================
// Schema-qualified name integration tests
// ===========================================================================

#[test]
fn test_schema_qualified_no_collision() {
    // Lint V002 and V003 as changed; V001 is replayed as baseline.
    // V001 creates myschema.customers and (unqualified) orders.
    // After normalization: myschema.customers stays myschema.customers,
    // orders becomes public.orders. They must be distinct catalog entries.
    //
    // V002 adds FK + covering index (no PGM003).
    // V003 creates index on myschema.customers without CONCURRENTLY -> PGM001.
    // V002's index on orders also fires PGM001 (orders is pre-existing from V001).
    let findings = lint_fixture(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
    );

    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    // Two PGM001 findings: one for idx_orders_customer_id on public.orders,
    // one for idx_customers_name on myschema.customers.
    assert_eq!(
        pgm001.len(),
        2,
        "Expected 2 PGM001 findings (one per pre-existing table). Got:\n  {}",
        format_findings(&findings)
    );

    // Verify one mentions myschema.customers (explicitly qualified) and the other
    // mentions just 'orders' (unqualified — display_name omits the synthetic public. prefix).
    let mentions_myschema = pgm001
        .iter()
        .any(|f| f.message.contains("myschema.customers"));
    let mentions_orders = pgm001.iter().any(|f| f.message.contains("'orders'"));
    assert!(
        mentions_myschema,
        "Expected a PGM001 finding mentioning 'myschema.customers'. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        mentions_orders,
        "Expected a PGM001 finding mentioning 'orders' (without synthetic schema prefix). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM003 should NOT fire: the covering index is in V002.
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();
    assert!(
        pgm003.is_empty(),
        "PGM003 should not fire (covering index present). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_schema_qualified_cross_schema_fk() {
    // Lint only V002. V001 is replayed as history.
    // V002 adds FK on orders.customer_id referencing myschema.customers(id).
    // myschema.customers exists in catalog_before (from V001 replay).
    // The covering index idx_orders_customer_id is added in the same file.
    // Expect no PGM003 finding.
    // V002's CREATE INDEX on pre-existing orders fires PGM001.
    let findings = lint_fixture("schema-qualified", &["V002__add_fk_and_index.sql"]);
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();

    assert!(
        pgm003.is_empty(),
        "PGM003 should not fire (covering index in same file). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM001 fires exactly once for CREATE INDEX on pre-existing orders table
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();
    assert_eq!(
        pgm001.len(),
        1,
        "Expected exactly 1 PGM001 finding for CREATE INDEX on pre-existing orders. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM017 fires for the FK without NOT VALID on pre-existing orders table
    let pgm017: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM017").collect();
    assert_eq!(
        pgm017.len(),
        1,
        "Expected exactly 1 PGM017 finding for FK without NOT VALID. Got:\n  {}",
        format_findings(&findings)
    );

    // Total findings should be PGM001 + PGM017
    assert_eq!(
        findings.len(),
        pgm001.len() + pgm017.len(),
        "Total findings should equal PGM001 + PGM017 count (no unexpected findings). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_schema_qualified_pgm001_fires() {
    // Lint only V003. V001 and V002 are replayed as history.
    // myschema.customers exists in catalog_before (from V001).
    // V003 creates index on myschema.customers without CONCURRENTLY -> PGM001.
    let findings = lint_fixture("schema-qualified", &["V003__alter_schema_table.sql"]);
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();

    assert_eq!(
        pgm001.len(),
        1,
        "Expected exactly 1 PGM001 finding. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        pgm001[0].message.contains("myschema.customers"),
        "PGM001 message should mention 'myschema.customers'. Got: {}",
        pgm001[0].message
    );
    assert!(
        pgm001[0].message.contains("CONCURRENTLY"),
        "PGM001 message should mention CONCURRENTLY. Got: {}",
        pgm001[0].message
    );
}

#[test]
fn test_schema_qualified_custom_default_schema() {
    // Use default_schema = "myschema" instead of "public".
    // With this setting:
    //   - V001's unqualified `orders` normalizes to `myschema.orders`
    //   - V001's `myschema.customers` stays `myschema.customers`
    //   - V002's CREATE INDEX on `orders` targets `myschema.orders`
    //   - V003's CREATE INDEX on `myschema.customers` targets `myschema.customers`
    //
    // Lint V002 and V003 as changed; V001 is replayed as baseline.
    // Both tables are pre-existing (from V001 replay), so PGM001 fires
    // for both indexes.
    let findings = lint_fixture_with_schema(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
        "myschema",
    );

    // PGM001 should fire for the myschema.customers index (V003)
    let pgm001: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM001").collect();
    assert!(
        pgm001
            .iter()
            .any(|f| f.message.contains("myschema.customers")),
        "Expected PGM001 for myschema.customers index. Got:\n  {}",
        format_findings(&findings)
    );

    // No PGM003 (covering index exists for the FK)
    let pgm003: Vec<&Finding> = findings.iter().filter(|f| f.rule_id == "PGM003").collect();
    assert!(
        pgm003.is_empty(),
        "PGM003 should not fire (covering index present). Got:\n  {}",
        format_findings(&findings)
    );
}

// ===========================================================================
// Config-level rule suppression
// ===========================================================================

/// Run the lint pipeline with specific rules disabled.
fn lint_fixture_with_disabled(
    fixture_name: &str,
    changed_filenames: &[&str],
    disabled_rules: &[&str],
) -> Vec<Finding> {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos")
        .join(fixture_name)
        .join("migrations");

    let loader = SqlLoader::default();
    let mut history = loader
        .load(std::slice::from_ref(&base))
        .expect("Failed to load fixture");

    normalize::normalize_schemas(&mut history.units, "public");

    let changed: HashSet<PathBuf> = changed_filenames.iter().map(|f| base.join(f)).collect();
    let disabled: HashSet<&str> = disabled_rules.iter().copied().collect();

    let mut catalog = Catalog::new();
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let active_rules: Vec<&dyn Rule> = registry
        .iter()
        .filter(|r| !disabled.contains(r.id()))
        .collect();

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();

    for unit in &history.units {
        let is_changed = changed.is_empty() || changed.contains(&unit.source_file);

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
            for rule in &active_rules {
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

#[test]
fn test_disabled_rules_suppresses_findings() {
    let changed = &["V002__violations.sql"];

    // Baseline: PGM003 should fire
    let findings_all = lint_fixture("all-rules", changed);
    let pgm003_all = findings_all
        .iter()
        .filter(|f| f.rule_id == "PGM003")
        .count();
    assert!(pgm003_all > 0, "PGM003 should fire without suppression");

    // With PGM003 disabled: no PGM003 findings
    let findings_disabled = lint_fixture_with_disabled("all-rules", changed, &["PGM003"]);
    let pgm003_disabled = findings_disabled
        .iter()
        .filter(|f| f.rule_id == "PGM003")
        .count();
    assert_eq!(pgm003_disabled, 0, "PGM003 should not fire when disabled");

    // Other rules should be unaffected
    let other_all = findings_all
        .iter()
        .filter(|f| f.rule_id != "PGM003")
        .count();
    let other_disabled = findings_disabled
        .iter()
        .filter(|f| f.rule_id != "PGM003")
        .count();
    assert_eq!(
        other_all, other_disabled,
        "Non-disabled rules should produce identical findings"
    );
}

// ===========================================================================
// Bridge JAR integration tests (feature-gated)
// ===========================================================================

#[cfg(feature = "bridge-tests")]
use pg_migration_lint::input::liquibase_bridge::{BridgeLoader, resolve_source_paths};

#[cfg(feature = "bridge-tests")]
fn bridge_jar_path() -> PathBuf {
    if let Ok(path) = std::env::var("BRIDGE_JAR_PATH") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bridge/target/liquibase-bridge-1.0.0.jar")
    }
}

#[cfg(feature = "bridge-tests")]
fn lint_via_bridge(changed_ids: &[&str]) -> Vec<Finding> {
    let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");
    let base_dir = master_xml.parent().unwrap();

    let loader = BridgeLoader::new(bridge_jar_path());
    let mut raw_units = loader.load(&master_xml).expect("Failed to load via bridge");
    resolve_source_paths(&mut raw_units, base_dir);

    let mut units: Vec<MigrationUnit> = raw_units
        .into_iter()
        .map(|r| r.into_migration_unit())
        .collect();

    normalize::normalize_schemas(&mut units, "public");

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

#[cfg(feature = "bridge-tests")]
fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| {
        a.rule_id
            .cmp(&b.rule_id)
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.start_line.cmp(&b.start_line))
    });
}

// ---------------------------------------------------------------------------
// Bridge: Parse all changesets
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_parses_all_changesets() {
    let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

    let loader = BridgeLoader::new(bridge_jar_path());
    let raw_units = loader.load(&master_xml).expect("Failed to load via bridge");

    assert!(
        !raw_units.is_empty(),
        "Bridge should produce at least one changeset"
    );

    // The fixture contains 37 changesets. The bridge may produce fewer if
    // some changesets generate no SQL. Assert a reasonable range.
    assert!(
        raw_units.len() >= 30 && raw_units.len() <= 40,
        "Expected 30-40 changesets from bridge, got {}",
        raw_units.len()
    );
}

// ---------------------------------------------------------------------------
// Bridge: Lint all changesets, snapshot all findings
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_all_findings() {
    let mut findings = lint_via_bridge(&[]);
    sort_findings(&mut findings);

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ---------------------------------------------------------------------------
// Bridge: Lint only 004 changesets (create indexes on pre-existing tables)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_004_only() {
    let mut findings = lint_via_bridge(&[
        "004-add-users-email-index",
        "004-add-subscriptions-account-index",
        "004-add-products-composite-index",
    ]);
    sort_findings(&mut findings);

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ---------------------------------------------------------------------------
// Bridge: Lint only 005 changesets (add FKs)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_005_only() {
    let mut findings = lint_via_bridge(&[
        "005-add-fk-orders-user",
        "005-add-fk-subscriptions-account",
        "005-add-fk-orders-account",
    ]);
    sort_findings(&mut findings);

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ---------------------------------------------------------------------------
// Bridge: Lint only 006 changesets (tables without PKs)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_006_only() {
    let mut findings =
        lint_via_bridge(&["006-create-event-log", "006-create-subscription-invoices"]);
    sort_findings(&mut findings);

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ---------------------------------------------------------------------------
// Bridge: Lint only 008 changesets (add NOT NULL columns without defaults)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_008_only() {
    let mut findings = lint_via_bridge(&[
        "008-add-region-to-accounts",
        "008-add-priority-to-orders",
        "008-add-category-to-products",
    ]);
    sort_findings(&mut findings);

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}
