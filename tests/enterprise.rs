use std::{collections::HashSet, path::PathBuf};

use pg_migration_lint::{
    Finding, LintPipeline, RuleId, input::sql::SqlLoader, normalize, suppress::parse_suppressions,
};
use rstest::rstest;

use crate::common::{format_findings, lint_fixture, lint_fixture_rules};

mod common;

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

/// Single-file, single-rule enterprise tests.
///
/// Each case replays all prior migrations as catalog history, then lints a
/// single changed file against one rule.
///
/// `expected_count`:
/// - `Some(n)` — assert exactly `n` findings
/// - `None`    — assert at least one finding (non-empty)
#[rstest]
#[case::v007_create_index_no_concurrently(
    "V007__create_index_no_concurrently.sql",
    "PGM001",
    Some(3)
)]
#[case::v023_drop_index_no_concurrently("V023__drop_index_no_concurrently.sql", "PGM002", None)]
#[case::v008_add_not_null_column("V008__add_not_null_column.sql", "PGM008", Some(1))]
#[case::v013_alter_column_type("V013__alter_column_type.sql", "PGM007", None)]
fn test_enterprise_single_file_single_rule(
    #[case] changed_file: &str,
    #[case] rule_id: &str,
    #[case] expected_count: Option<usize>,
) {
    let findings = lint_fixture_rules("enterprise", &[changed_file], &[rule_id]);

    match expected_count {
        Some(n) => {
            assert_eq!(
                findings.len(),
                n,
                "Expected {n} {rule_id} findings for {changed_file}. Got:\n  {}",
                format_findings(&findings)
            );
        }
        None => {
            assert!(
                !findings.is_empty(),
                "Expected at least one {rule_id} finding for {changed_file}. Got:\n  {}",
                format_findings(&findings)
            );
        }
    }
}

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

    let all_rules: Vec<RuleId> = RuleId::lint_rules().collect();

    // Collect (filename, findings) for each step
    let mut steps: Vec<(String, Vec<Finding>)> = Vec::new();

    for (i, unit) in history.units.iter().enumerate() {
        // Fresh pipeline per unit: replay all prior units, then lint the current one.
        let mut pipeline = LintPipeline::new();
        for prior in &history.units[..i] {
            pipeline.replay(prior);
        }
        let mut unit_findings = pipeline.lint(unit, &all_rules);

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
