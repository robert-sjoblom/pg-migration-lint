//! Integration tests for the full lint pipeline.
#![allow(dead_code)]

use pg_migration_lint::LintPipeline;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::normalize;
use pg_migration_lint::rules::{Finding, RuleId, dedup_findings};
use pg_migration_lint::suppress::parse_suppressions;
use std::collections::HashSet;
use std::path::PathBuf;

pub const APPLY_SUPPRESSIONS: bool = false;
pub const SKIP_SUPPRESSIONS: bool = true;

/// Return all non-baseline `.sql` filenames in a fixture's migrations dir.
/// V001 is always the baseline (replayed but not linted), so it is excluded.
pub fn changed_files_for(fixture_name: &str) -> Vec<String> {
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
pub fn lint_fixture<S: AsRef<str>>(fixture_name: &str, changed_filenames: &[S]) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        &[],
        &[],
        APPLY_SUPPRESSIONS,
    )
}

/// Run the lint pipeline but skip applying suppressions.
/// Returns raw findings before any suppression filtering.
pub fn lint_fixture_no_suppress<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        &[],
        &[],
        SKIP_SUPPRESSIONS,
    )
}

/// Run the lint pipeline with specifically disabled rules
pub fn lint_fixture_with_disabled(
    fixture_name: &str,
    changed_filenames: &[&str],
    disabled_rules: &[&str],
) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        &[],
        disabled_rules,
        APPLY_SUPPRESSIONS,
    )
}

/// Run the lint pipeline on a fixture repo with only specific rules.
/// If `only_rules` is empty, all rules are run.
pub fn lint_fixture_rules<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
    only_rules: &[&str],
) -> Vec<Finding> {
    lint_fixture_inner(
        fixture_name,
        changed_filenames,
        "public",
        only_rules,
        &[],
        APPLY_SUPPRESSIONS,
    )
}

/// Shared implementation for all lint_fixture variants.
pub fn lint_fixture_inner<S: AsRef<str>>(
    fixture_name: &str,
    changed_filenames: &[S],
    default_schema: &str,
    only_rules: &[&str],
    disabled_rules: &[&str],
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

    let disabled: HashSet<&str> = disabled_rules.iter().copied().collect();
    let active_rules: Vec<RuleId> = RuleId::lint_rules()
        .filter(|r| {
            (only_rules.is_empty() || only_rules.contains(&r.as_str()))
                && !disabled.contains(r.as_str())
        })
        .collect();

    let mut pipeline = LintPipeline::new();
    let mut all_findings: Vec<Finding> = Vec::new();

    for unit in &history.units {
        let is_changed = changed.is_empty() || changed.contains(&unit.source_file);

        if is_changed {
            let mut unit_findings = pipeline.lint(unit, &active_rules);

            if !skip_suppress {
                let source = std::fs::read_to_string(&unit.source_file).unwrap_or_default();
                let suppressions = parse_suppressions(&source);
                unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));
            }
            dedup_findings(&mut unit_findings);

            all_findings.extend(unit_findings);
        } else {
            pipeline.replay(unit);
        }
    }

    all_findings
}

/// Helper: format findings for debug output in assertion messages.
pub fn format_findings(findings: &[Finding]) -> String {
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
pub fn normalize_findings(findings: Vec<Finding>, fixture_name: &str) -> Vec<Finding> {
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
