//! Integration tests for the full lint pipeline.

use pg_migration_lint::IrNode;
use pg_migration_lint::catalog::Catalog;
use pg_migration_lint::catalog::replay;
use pg_migration_lint::input::MigrationLoader;
#[cfg(feature = "bridge-tests")]
use pg_migration_lint::input::MigrationUnit;
#[cfg(feature = "bridge-tests")]
use pg_migration_lint::input::RawMigrationUnit;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::normalize;
use pg_migration_lint::output::{Reporter, RuleInfo, SarifReporter, SonarQubeReporter};
use pg_migration_lint::rules::{Finding, LintContext, Rule, RuleRegistry, cap_for_down_migration};
use pg_migration_lint::suppress::parse_suppressions;
use std::collections::HashSet;
use std::path::PathBuf;

const APPLY_SUPPRESSIONS: bool = false;
const SKIP_SUPPRESSIONS: bool = true;

/// Return all non-baseline `.sql` filenames in a fixture's migrations dir.
/// V001 is always the baseline (replayed but not linted), so it is excluded.
fn changed_files_for(fixture_name: &str) -> Vec<String> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos")
        .join(fixture_name)
        .join("migrations");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read fixture dir {}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            if name.ends_with(".sql") && !name.starts_with("V001") {
                Some(name)
            } else {
                None
            }
        })
        .collect();
    names.sort();
    names
}

/// Run the full lint pipeline on a fixture repo.
/// If `changed_files` is empty, all files are linted.
fn lint_fixture<S: AsRef<str>>(fixture_name: &str, changed_filenames: &[S]) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        &[],
        APPLY_SUPPRESSIONS,
    )
}

/// Run the lint pipeline but skip applying suppressions.
/// Returns raw findings before any suppression filtering.
fn lint_fixture_no_suppress<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        &[],
        SKIP_SUPPRESSIONS,
    )
}

/// Run the lint pipeline on a fixture repo with only specific rules.
/// If `only_rules` is empty, all rules are run.
fn lint_fixture_rules<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
    only_rules: &[&str],
) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        only_rules,
        APPLY_SUPPRESSIONS,
    )
}

/// Shared implementation for all lint_fixture variants.
fn lint_fixture_inner<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
    default_schema: &str,
    only_rules: &[&str],
    skip_suppress: bool,
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

    let changed: HashSet<PathBuf> = changed_filenames
        .iter()
        .map(|f| base.join(f.as_ref()))
        .collect();

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
                    let key = ct.name.catalog_key().to_string();
                    if !(ct.if_not_exists && catalog_before.has_table(&key)) {
                        tables_created_in_change.insert(key);
                    }
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
                if only_rules.is_empty() || only_rules.contains(&rule.id().as_str()) {
                    unit_findings.extend(rule.check(&unit.statements, &ctx));
                }
            }

            if unit.is_down {
                cap_for_down_migration(&mut unit_findings);
            }

            if !skip_suppress {
                let source = std::fs::read_to_string(&unit.source_file).unwrap_or_default();
                let suppressions = parse_suppressions(&source);
                unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));
            }

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
    let findings = lint_fixture::<&str>("clean", &[]);
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
    // All migration files except V001 (baseline) are changed.
    // V001 is just replayed so its tables appear in catalog_before.
    // Every registered non-meta rule must fire at least once.
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // Build the expected set from the registry, excluding meta rules
    // (PGM9xx) which never produce findings on their own.
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    for rule in registry.iter() {
        let id = rule.id();
        if matches!(id, pg_migration_lint::rules::RuleId::Meta(_)) {
            continue;
        }
        assert!(
            rule_ids.contains(id.as_str()),
            "Rule {} is registered but did not fire. Add a violation to the all-rules fixture. Got:\n  {}",
            id,
            format_findings(&findings)
        );
    }
}

// ---------------------------------------------------------------------------
// Suppressed repo: all violations suppressed, expect 0 findings
// ---------------------------------------------------------------------------

#[test]
fn test_suppressed_repo_no_findings() {
    let changed = changed_files_for("suppressed");

    // First: verify every non-meta rule fires before suppression.
    // This ensures the suppressed fixture stays in sync with new rules.
    let raw_findings = lint_fixture_no_suppress("suppressed", &changed);
    let raw_rule_ids: HashSet<&str> = raw_findings.iter().map(|f| f.rule_id.as_str()).collect();

    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    for rule in registry.iter() {
        let id = rule.id();
        if matches!(id, pg_migration_lint::rules::RuleId::Meta(_)) {
            continue;
        }
        assert!(
            raw_rule_ids.contains(id.as_str()),
            "Rule {} is registered but did not fire in the suppressed fixture (pre-suppression). \
             Add a suppressed violation for it. Got:\n  {}",
            id,
            format_findings(&raw_findings)
        );
    }

    // Second: verify all findings are suppressed.
    let findings = lint_fixture("suppressed", &changed);
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
    // rules (PGM001, PGM007, PGM008, PGM009) should NOT fire. However,
    // PGM502 fires for the 'events' table which has no primary key.
    let findings = lint_fixture_rules("all-rules", &["V001__baseline.sql"], &["PGM001", "PGM502"]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();
    assert!(
        !rule_ids.contains("PGM001"),
        "PGM001 should not fire for baseline-only. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        rule_ids.contains("PGM502"),
        "PGM502 should fire for events table (no PK). Got:\n  {}",
        format_findings(&findings)
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
    let findings = lint_fixture_rules("all-rules", &["V002__violations.sql"], &["PGM001"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm501_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V002__violations.sql"], &["PGM501"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm502_finding_details() {
    // V002 creates audit_log without PK. Since V002 is changed, audit_log
    // is in tables_created_in_change, but PGM502 does not check that set --
    // it only checks catalog_after for has_primary_key.
    let findings = lint_fixture_rules("all-rules", &["V002__violations.sql"], &["PGM502"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm002_finding_details() {
    // V003 drops idx_customers_email WITHOUT CONCURRENTLY.
    // V001 is replayed as baseline (creates the index), V002 and V003 are changed.
    let findings = lint_fixture_rules(
        "all-rules",
        &["V002__violations.sql", "V003__more_violations.sql"],
        &["PGM002"],
    );
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm503_finding_details() {
    // V003 creates the 'settings' table with UNIQUE NOT NULL but no PK.
    let findings = lint_fixture_rules("all-rules", &["V003__more_violations.sql"], &["PGM503"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm003_finding_details() {
    // V003 uses CREATE INDEX CONCURRENTLY inside a transaction (SqlLoader
    // sets run_in_transaction=true by default).
    let findings = lint_fixture_rules("all-rules", &["V003__more_violations.sql"], &["PGM003"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_all_rules_changed_files_all_empty() {
    // When all files are changed (empty changed_files), tables created in V001
    // are in tables_created_in_change, so PGM001/006/007/008/009 won't fire for
    // those tables. But PGM501, PGM502, PGM503, PGM003 should still fire.
    let findings = lint_fixture::<&str>("all-rules", &[]);

    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // These rules do NOT check tables_created_in_change, so they fire regardless
    assert!(
        rule_ids.contains("PGM501"),
        "PGM501 should fire even with all files changed"
    );
    assert!(
        rule_ids.contains("PGM502"),
        "PGM502 should fire even with all files changed"
    );
    assert!(
        rule_ids.contains("PGM503"),
        "PGM503 should fire even with all files changed"
    );
    // PGM003 fires because it only checks run_in_transaction + concurrent flag
    assert!(
        rule_ids.contains("PGM003"),
        "PGM003 should fire even with all files changed"
    );
}

// ---------------------------------------------------------------------------
// "Don't Do This" rules (PGM101-PGM105)
// ---------------------------------------------------------------------------

#[test]
fn test_pgm101_timestamp_without_tz() {
    let findings = lint_fixture_rules("all-rules", &["V004__dont_do_this_types.sql"], &["PGM101"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm102_timestamptz_zero_precision() {
    let findings = lint_fixture_rules("all-rules", &["V004__dont_do_this_types.sql"], &["PGM102"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm103_char_n_type() {
    let findings = lint_fixture_rules("all-rules", &["V004__dont_do_this_types.sql"], &["PGM103"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm104_money_type() {
    let findings = lint_fixture_rules("all-rules", &["V004__dont_do_this_types.sql"], &["PGM104"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm105_serial_type() {
    let findings = lint_fixture_rules("all-rules", &["V004__dont_do_this_types.sql"], &["PGM105"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// New rules: PGM013-PGM505, PGM106
// ---------------------------------------------------------------------------

#[test]
fn test_pgm013_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V005__new_violations.sql"], &["PGM013"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm014_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V005__new_violations.sql"], &["PGM014"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm015_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V005__new_violations.sql"], &["PGM015"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm504_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V005__new_violations.sql"], &["PGM504"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm505_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V005__new_violations.sql"], &["PGM505"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm106_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V006__json_type.sql"], &["PGM106"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm402_finding_details() {
    // PGM402 fires on CREATE TABLE and CREATE INDEX without IF NOT EXISTS.
    // V002 has both: CREATE TABLE audit_log + CREATE INDEX idx_products_name.
    let findings = lint_fixture_rules("all-rules", &["V002__violations.sql"], &["PGM402"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm403_finding_details() {
    let findings = lint_fixture_rules(
        "all-rules",
        &["V008__if_not_exists_redundant.sql"],
        &["PGM403"],
    );
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

#[test]
fn test_pgm018_finding_details() {
    let findings = lint_fixture_rules("all-rules", &["V010__cluster.sql"], &["PGM018"]);
    let findings = normalize_findings(findings, "all-rules");
    insta::assert_yaml_snapshot!(findings);
}

// ---------------------------------------------------------------------------
// Regression: CREATE TABLE IF NOT EXISTS no-op must not mask existing-table rules
// ---------------------------------------------------------------------------

/// V008 contains `CREATE TABLE IF NOT EXISTS customers` which is a no-op (the
/// table exists since V001). V011 contains `CLUSTER customers ...`. When both
/// are in the changed set, PGM018 must still fire on `customers` because it is
/// an existing table — the IF NOT EXISTS no-op must not add it to
/// `tables_created_in_change`.
#[test]
fn test_if_not_exists_noop_does_not_mask_existing_table_rules() {
    let findings = lint_fixture_rules(
        "all-rules",
        &[
            "V008__if_not_exists_redundant.sql",
            "V011__cluster_customers.sql",
        ],
        &["PGM018"],
    );
    assert_eq!(
        findings.len(),
        1,
        "PGM018 should fire on CLUSTER customers even when V008's \
         CREATE TABLE IF NOT EXISTS customers is in the same change set.\n\
         Findings: {}",
        format_findings(&findings),
    );
    assert!(
        findings[0].message.contains("customers"),
        "Finding should be about the 'customers' table, got: {}",
        findings[0].message,
    );
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
    let findings = lint_fixture::<&str>("enterprise", &[]);
    let rule_ids: HashSet<&str> = findings.iter().map(|f| f.rule_id.as_str()).collect();

    // PGM501 should fire (FKs without covering indexes in V005, V006, V010, V021, V029)
    assert!(
        rule_ids.contains("PGM501"),
        "Expected PGM501. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM502 should fire (many tables without PKs in V003, V015, V020, V021)
    assert!(
        rule_ids.contains("PGM502"),
        "Expected PGM502. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v007_only() {
    // V001-V006 are replayed as history, V007 is the only changed file.
    // V007 creates indexes WITHOUT CONCURRENTLY on pre-existing tables → PGM001
    let findings = lint_fixture_rules(
        "enterprise",
        &["V007__create_index_no_concurrently.sql"],
        &["PGM001"],
    );

    assert_eq!(
        findings.len(),
        3,
        "Expected 3 PGM001 findings for 3 non-concurrent indexes. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v023_only() {
    // V001-V022 replayed, V023 is changed: DROP INDEX without CONCURRENTLY → PGM002
    let findings = lint_fixture_rules(
        "enterprise",
        &["V023__drop_index_no_concurrently.sql"],
        &["PGM002"],
    );

    assert!(
        !findings.is_empty(),
        "Expected PGM002 for DROP INDEX without CONCURRENTLY. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v008_only() {
    // V001-V007 replayed, V008 is changed: ADD COLUMN NOT NULL without default → PGM008
    let findings = lint_fixture_rules(
        "enterprise",
        &["V008__add_not_null_column.sql"],
        &["PGM008"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected 1 PGM008 for NOT NULL without default. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_lint_v013_only() {
    // V001-V012 replayed, V013 is changed: ALTER COLUMN TYPE → PGM007
    let findings = lint_fixture_rules("enterprise", &["V013__alter_column_type.sql"], &["PGM007"]);

    assert!(
        !findings.is_empty(),
        "Expected PGM007 for ALTER COLUMN TYPE. Got:\n  {}",
        format_findings(&findings)
    );
}

// ===========================================================================
// Enterprise changed-files mode: incremental lint behavior
// ===========================================================================

#[test]
fn test_enterprise_changed_file_volatile_defaults() {
    // Lint only V022 (add volatile defaults). V001-V021 are replayed as history.
    // V022 adds columns with gen_random_uuid() and now() defaults -> PGM006.
    let findings = lint_fixture_rules(
        "enterprise",
        &["V022__add_volatile_defaults.sql"],
        &["PGM006", "PGM501"],
    );

    let pgm006: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM006")
        .collect();
    assert!(
        !pgm006.is_empty(),
        "Expected PGM006 for volatile defaults in V022. Got:\n  {}",
        format_findings(&findings)
    );

    // V022 does not add any foreign keys, so PGM501 should not fire
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire for V022 (no FK creation). Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_enterprise_changed_files_reduces_fk_noise() {
    // Full run should produce PGM501 findings (FKs without covering indexes)
    let findings_full = lint_fixture::<&str>("enterprise", &[]);
    let pgm501_full: Vec<&Finding> = findings_full
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        !pgm501_full.is_empty(),
        "Full run should have PGM501 findings. Got:\n  {}",
        format_findings(&findings_full)
    );

    // Targeting only V014 (drop column, no FK creation) should have 0 PGM501
    let findings_v014 = lint_fixture("enterprise", &["V014__drop_column.sql"]);
    let pgm501_v014: Vec<&Finding> = findings_v014
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501_v014.is_empty(),
        "V014 (drop column) should have 0 PGM501 findings. Got:\n  {}",
        format_findings(&findings_v014)
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
        unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));

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

// ---------------------------------------------------------------------------
// Go-migrate: Down migration severity capping (PGM901)
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

// ---------------------------------------------------------------------------
// Go-migrate: PGM001 fires when indexes are changed
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: PGM003 fires for FK without index
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: PGM004 fires for table without PK
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: PGM007 fires for volatile defaults
// ---------------------------------------------------------------------------

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
        "Expected 2 PGM006 findings for now() and gen_random_uuid(). Got:\n  {}",
        format_findings(&findings)
    );
}

// ---------------------------------------------------------------------------
// Go-migrate: PGM010 fires for NOT NULL without default
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: PGM012 fires for ADD PRIMARY KEY without unique
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: Clean migrations produce no violations
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Go-migrate: Multi-file changed set with targeted violations
// ---------------------------------------------------------------------------

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

// ===========================================================================
// SARIF output integration tests
// ===========================================================================

#[test]
fn test_sarif_output_valid_structure() {
    // Run the all-rules fixture through the full pipeline, emit SARIF, and
    // verify the output is valid SARIF 2.1.0 with correct structure.
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);
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
    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let known_rules: HashSet<&str> = registry.iter().map(|r| r.id().as_str()).collect();
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
            r["ruleId"].as_str() == Some(finding.rule_id.as_str())
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
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);
    assert!(
        !findings.is_empty(),
        "All-rules fixture should produce findings"
    );

    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new(RuleInfo::from_registry(&registry));
    reporter
        .emit(&findings, dir.path())
        .expect("emit SonarQube JSON");

    let content =
        std::fs::read_to_string(dir.path().join("findings.json")).expect("read SonarQube file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    // Verify top-level structure: both rules and issues arrays
    let rules = parsed["rules"]
        .as_array()
        .expect("rules should be an array");
    let issues = parsed["issues"]
        .as_array()
        .expect("issues should be an array");
    assert_eq!(
        issues.len(),
        findings.len(),
        "SonarQube issues count should match findings count"
    );

    // Verify rules array has required fields
    for rule in rules {
        assert_eq!(
            rule["engineId"], "pg-migration-lint",
            "All rules must have engineId 'pg-migration-lint'"
        );
        assert!(rule["id"].is_string(), "rule must have id");
        assert!(rule["name"].is_string(), "rule must have name");
        assert!(
            rule["cleanCodeAttribute"].is_string(),
            "rule must have cleanCodeAttribute"
        );
        assert!(rule["type"].is_string(), "rule must have type");
        assert!(rule["severity"].is_string(), "rule must have severity");
        let impacts = rule["impacts"].as_array().expect("impacts array");
        assert!(!impacts.is_empty(), "rule must have at least one impact");
    }

    // Verify each issue has the required fields
    let known_rules: HashSet<&str> = registry.iter().map(|r| r.id().as_str()).collect();

    for issue in issues {
        // ruleId
        let rule_id = issue["ruleId"].as_str().expect("ruleId should be a string");
        assert!(
            known_rules.contains(rule_id),
            "SonarQube ruleId '{}' should be a known rule",
            rule_id
        );

        // effortMinutes
        assert!(
            issue["effortMinutes"].is_u64(),
            "issue must have effortMinutes"
        );

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

    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let dir = tempfile::tempdir().expect("tempdir");
    let reporter = SonarQubeReporter::new(RuleInfo::from_registry(&registry));
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
            issue["ruleId"].as_str() == Some(finding.rule_id.as_str())
                && issue["primaryLocation"]["message"].as_str() == Some(&finding.message)
        });
        assert!(
            matching.is_some(),
            "Finding {} with message '{}' should appear in SonarQube output",
            finding.rule_id,
            finding.message
        );

        let matched = matching.unwrap();

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
    let changed = changed_files_for("all-rules");
    let findings = lint_fixture("all-rules", &changed);

    let dir_sarif = tempfile::tempdir().expect("sarif tempdir");
    let dir_sonar = tempfile::tempdir().expect("sonar tempdir");

    let sarif_reporter = SarifReporter::new();
    sarif_reporter
        .emit(&findings, dir_sarif.path())
        .expect("emit SARIF");

    let mut registry = RuleRegistry::new();
    registry.register_defaults();
    let sonar_reporter = SonarQubeReporter::new(RuleInfo::from_registry(&registry));
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
    // index) has NOT been replayed yet. PGM501 should fire because
    // catalog_after has no covering index at this point.
    let findings = lint_fixture_rules("fk-with-later-index", &["V002__add_fk.sql"], &["PGM501"]);

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding for FK without index. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_fk_with_later_index_only_index_changed() {
    // Only V003 is changed. V001 and V002 are replayed as history.
    // The FK from V002 already exists in catalog_before, and V003 adds
    // the covering index. Since V002 is not being linted, no PGM501
    // should fire -- the FK was in a prior file, not in the current lint set.
    let findings = lint_fixture_rules("fk-with-later-index", &["V003__add_index.sql"], &["PGM501"]);

    assert!(
        findings.is_empty(),
        "PGM501 should NOT fire when only the index file is linted. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_fk_cross_file_both_changed() {
    // Both V002 and V003 are changed. V001 is replayed as history.
    // When linting V002: FK is added but no covering index yet -> PGM501 fires.
    // When linting V003: index is added, no new FK in this file -> no PGM501.
    let findings = lint_fixture_rules(
        "fk-with-later-index",
        &["V002__add_fk.sql", "V003__add_index.sql"],
        &["PGM501"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding (from V002 only). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
    );
}

#[test]
fn test_fk_cross_file_all_changed() {
    // All files are changed (empty changed set). V001 creates tables (no FK,
    // no finding). V002 adds FK without covering index -> PGM501 fires.
    // V003 adds the covering index -> no additional PGM501.
    let findings = lint_fixture_rules::<&str>("fk-with-later-index", &[], &["PGM501"]);

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM501 finding (from V002). Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("customer_id"),
        "PGM501 message should mention 'customer_id'. Got: {}",
        findings[0].message
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
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
        &["PGM001", "PGM501"],
    );

    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();

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

    // PGM501 should NOT fire: the covering index is in V002.
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index present). Got:\n  {}",
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
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V002__add_fk_and_index.sql"],
        &["PGM001", "PGM501", "PGM014"],
    );
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();

    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index in same file). Got:\n  {}",
        format_findings(&findings)
    );

    // PGM001 fires exactly once for CREATE INDEX on pre-existing orders table
    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();
    assert_eq!(
        pgm001.len(),
        1,
        "Expected exactly 1 PGM001 finding for CREATE INDEX on pre-existing orders. Got:\n  {}",
        format_findings(&findings)
    );

    // PGM014 fires for the FK without NOT VALID on pre-existing orders table
    let pgm014: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM014")
        .collect();
    assert_eq!(
        pgm014.len(),
        1,
        "Expected exactly 1 PGM014 finding for FK without NOT VALID. Got:\n  {}",
        format_findings(&findings)
    );
}

#[test]
fn test_schema_qualified_pgm001_fires() {
    // Lint only V003. V001 and V002 are replayed as history.
    // myschema.customers exists in catalog_before (from V001).
    // V003 creates index on myschema.customers without CONCURRENTLY -> PGM001.
    let findings = lint_fixture_rules(
        "schema-qualified",
        &["V003__alter_schema_table.sql"],
        &["PGM001"],
    );

    assert_eq!(
        findings.len(),
        1,
        "Expected exactly 1 PGM001 finding. Got:\n  {}",
        format_findings(&findings)
    );
    assert!(
        findings[0].message.contains("myschema.customers"),
        "PGM001 message should mention 'myschema.customers'. Got: {}",
        findings[0].message
    );
    assert!(
        findings[0].message.contains("CONCURRENTLY"),
        "PGM001 message should mention CONCURRENTLY. Got: {}",
        findings[0].message
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
    let findings = lint_fixture_inner(
        "schema-qualified",
        &["V002__add_fk_and_index.sql", "V003__alter_schema_table.sql"],
        "myschema",
        &["PGM001", "PGM501"],
        APPLY_SUPPRESSIONS,
    );

    // PGM001 should fire for the myschema.customers index (V003)
    let pgm001: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM001")
        .collect();
    assert!(
        pgm001
            .iter()
            .any(|f| f.message.contains("myschema.customers")),
        "Expected PGM001 for myschema.customers index. Got:\n  {}",
        format_findings(&findings)
    );

    // No PGM501 (covering index exists for the FK)
    let pgm501: Vec<&Finding> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .collect();
    assert!(
        pgm501.is_empty(),
        "PGM501 should not fire (covering index present). Got:\n  {}",
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
        .filter(|r| !disabled.contains(r.id().as_str()))
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
                    let key = ct.name.catalog_key().to_string();
                    if !(ct.if_not_exists && catalog_before.has_table(&key)) {
                        tables_created_in_change.insert(key);
                    }
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
            unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));

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

    // Baseline: PGM501 should fire
    let findings_all = lint_fixture("all-rules", changed);
    let pgm501_all = findings_all
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM501")
        .count();
    assert!(pgm501_all > 0, "PGM501 should fire without suppression");

    // With PGM501 disabled: no PGM501 findings
    let findings_disabled = lint_fixture_with_disabled("all-rules", changed, &["PGM501"]);
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

// ===========================================================================
// Bridge JAR integration tests (feature-gated)
// ===========================================================================

#[cfg(feature = "bridge-tests")]
use pg_migration_lint::input::liquibase_bridge::{BridgeLoader, resolve_source_paths};
#[cfg(feature = "bridge-tests")]
use pg_migration_lint::input::liquibase_updatesql::UpdateSqlLoader;

#[cfg(feature = "bridge-tests")]
fn bridge_jar_path() -> PathBuf {
    if let Ok(path) = std::env::var("BRIDGE_JAR_PATH") {
        PathBuf::from(path)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bridge/target/liquibase-bridge-1.0.0.jar")
    }
}

/// Shared replay+lint logic for Liquibase loaders (bridge and update-sql).
///
/// Converts raw migration units into `MigrationUnit`s, normalizes schemas,
/// replays catalog history, and runs the full rule engine on changed units.
/// The only difference between bridge and update-sql is HOW they produce
/// `raw_units`; everything after that is identical.
#[cfg(feature = "bridge-tests")]
fn lint_loaded_units(raw_units: Vec<RawMigrationUnit>, changed_ids: &[&str]) -> Vec<Finding> {
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
                    let key = ct.name.catalog_key().to_string();
                    if !(ct.if_not_exists && catalog_before.has_table(&key)) {
                        tables_created_in_change.insert(key);
                    }
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
            unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));

            all_findings.extend(unit_findings);
        } else {
            replay::apply(&mut catalog, unit);
        }
    }

    all_findings
}

#[cfg(feature = "bridge-tests")]
fn lint_via_bridge(changed_ids: &[&str]) -> Vec<Finding> {
    let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");
    let base_dir = master_xml.parent().unwrap();

    let loader = BridgeLoader::new(bridge_jar_path());
    let mut raw_units = loader.load(&master_xml).expect("Failed to load via bridge");
    resolve_source_paths(&mut raw_units, base_dir);

    lint_loaded_units(raw_units, changed_ids)
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

    // The fixture contains 39 changesets. The bridge may produce fewer if
    // some changesets generate no SQL. Assert a reasonable range.
    assert!(
        raw_units.len() >= 30 && raw_units.len() <= 45,
        "Expected 30-45 changesets from bridge, got {}",
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

// ---------------------------------------------------------------------------
// Bridge: Lint only 010 changesets (drop index / drop table / truncate)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_bridge_lint_010_only() {
    let mut findings = lint_via_bridge(&[
        "010-truncate-event-log",
        "010-drop-unused-indexes",
        "010-drop-event-log",
        "010-drop-index-if-exists",
        "010-drop-table-if-exists",
    ]);
    sort_findings(&mut findings);

    // The plain DROP INDEX / DROP TABLE should trigger PGM401
    let pgm401: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM401")
        .collect();
    assert_eq!(
        pgm401.len(),
        2,
        "Expected exactly 2 PGM401 findings (DROP INDEX + DROP TABLE without IF EXISTS).\n\
         The IF EXISTS variants must NOT fire.\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );

    // TRUNCATE TABLE event_log CASCADE should trigger PGM203 + PGM204
    let pgm203_bridge: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM203")
        .collect();
    assert_eq!(
        pgm203_bridge.len(),
        1,
        "Expected exactly 1 PGM203 finding (TRUNCATE TABLE on existing table).\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );
    let pgm204_bridge: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM204")
        .collect();
    assert_eq!(
        pgm204_bridge.len(),
        1,
        "Expected exactly 1 PGM204 finding (TRUNCATE TABLE CASCADE on existing table).\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ===========================================================================
// Liquibase update-sql E2E tests (feature-gated)
// ===========================================================================

#[cfg(feature = "bridge-tests")]
fn liquibase_binary_path() -> PathBuf {
    PathBuf::from(
        std::env::var("PG_LINT_LIQUIBASE_PATH").unwrap_or_else(|_| "liquibase".to_string()),
    )
}

#[cfg(feature = "bridge-tests")]
fn lint_via_updatesql(changed_ids: &[&str]) -> Vec<Finding> {
    let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");
    let base_dir = master_xml.parent().unwrap();

    let loader = UpdateSqlLoader::new(liquibase_binary_path());
    let mut raw_units = loader
        .load(&master_xml)
        .expect("Failed to load via update-sql");
    resolve_source_paths(&mut raw_units, base_dir);

    lint_loaded_units(raw_units, changed_ids)
}

// ---------------------------------------------------------------------------
// Update-sql: Parse all changesets
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_parses_all_changesets() {
    let master_xml = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/repos/liquibase-xml/changelog/master.xml");

    let loader = UpdateSqlLoader::new(liquibase_binary_path());
    let raw_units = loader
        .load(&master_xml)
        .expect("Failed to load via update-sql");

    assert!(
        !raw_units.is_empty(),
        "Update-sql should produce at least one changeset"
    );

    // The fixture contains 39 changesets. update-sql may produce fewer if
    // some changesets generate no SQL. Assert a reasonable range.
    assert!(
        raw_units.len() >= 30 && raw_units.len() <= 45,
        "Expected 30-45 changesets from update-sql, got {}",
        raw_units.len()
    );
}

// ---------------------------------------------------------------------------
// Update-sql: Lint all changesets, snapshot all findings
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_lint_all_findings() {
    let mut findings = lint_via_updatesql(&[]);
    sort_findings(&mut findings);

    // NOTE: The update-sql path produces 2 extra PGM003 findings compared to
    // the bridge snapshot. This is because update-sql cannot detect
    // `runInTransaction="false"` from the XML -- it always assumes
    // `run_in_transaction: true`. As a result, CONCURRENTLY-in-transaction
    // warnings (PGM003) fire for changesets 009 and 010 that the bridge
    // correctly suppresses (since it knows those changesets run outside a
    // transaction).
    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}

// ---------------------------------------------------------------------------
// Update-sql: Lint only 004 changesets (create indexes on pre-existing tables)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_lint_004_only() {
    let mut findings = lint_via_updatesql(&[
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
// Update-sql: Lint only 005 changesets (add FKs)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_lint_005_only() {
    let mut findings = lint_via_updatesql(&[
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
// Update-sql: Lint only 006 changesets (tables without PKs)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_lint_006_only() {
    let mut findings =
        lint_via_updatesql(&["006-create-event-log", "006-create-subscription-invoices"]);
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
// Update-sql: Lint only 010 changesets (drop index / drop table / truncate)
// ---------------------------------------------------------------------------

#[cfg(feature = "bridge-tests")]
#[test]
fn test_updatesql_lint_010_only() {
    let mut findings = lint_via_updatesql(&[
        "010-truncate-event-log",
        "010-drop-unused-indexes",
        "010-drop-event-log",
        "010-drop-index-if-exists",
        "010-drop-table-if-exists",
    ]);
    sort_findings(&mut findings);

    // The plain DROP INDEX / DROP TABLE should trigger PGM401
    let pgm401_usql: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM401")
        .collect();
    assert_eq!(
        pgm401_usql.len(),
        2,
        "Expected exactly 2 PGM401 findings (DROP INDEX + DROP TABLE without IF EXISTS).\n\
         The IF EXISTS variants must NOT fire.\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );

    // TRUNCATE TABLE event_log CASCADE should trigger PGM203 + PGM204
    let pgm203_usql: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM203")
        .collect();
    assert_eq!(
        pgm203_usql.len(),
        1,
        "Expected exactly 1 PGM203 finding (TRUNCATE TABLE on existing table).\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );
    let pgm204_usql: Vec<_> = findings
        .iter()
        .filter(|f| f.rule_id.as_str() == "PGM204")
        .collect();
    assert_eq!(
        pgm204_usql.len(),
        1,
        "Expected exactly 1 PGM204 finding (TRUNCATE TABLE CASCADE on existing table).\nAll findings: {:?}",
        findings
            .iter()
            .map(|f| format!("{}: {}", f.rule_id, f.message))
            .collect::<Vec<_>>()
    );

    insta::assert_yaml_snapshot!(findings, {
        "[].file" => insta::dynamic_redaction(|value, _path| {
            let s = value.as_str().unwrap();
            let filename = std::path::Path::new(s).file_name().unwrap().to_str().unwrap();
            filename.to_string()
        })
    });
}
