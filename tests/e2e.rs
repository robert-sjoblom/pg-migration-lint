//! End-to-end tests that invoke the compiled `pg-migration-lint` binary as a subprocess.
//!
//! These tests exercise the full pipeline including CLI argument parsing, config loading,
//! output file generation, and exit codes.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Locate the compiled binary built by `cargo test`.
fn binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_pg-migration-lint"))
}

/// Root of the test fixtures directory.
fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/repos")
}

/// Path to a specific fixture repo.
fn fixture_path(name: &str) -> PathBuf {
    fixtures_dir().join(name)
}

/// Run the binary with the given arguments, returning the full Output.
fn run_lint(args: &[&str]) -> Output {
    Command::new(binary_path())
        .args(args)
        .output()
        .expect("failed to execute pg-migration-lint binary")
}

/// Write a minimal TOML config file pointing to the given migrations dir,
/// with output in the given output dir. Returns the path to the config file.
fn write_temp_config(
    dir: &Path,
    migrations_path: &str,
    output_dir: &str,
    formats: &[&str],
    fail_on: &str,
) -> PathBuf {
    let format_list = formats
        .iter()
        .map(|f| format!("\"{}\"", f))
        .collect::<Vec<_>>()
        .join(", ");

    let config_content = format!(
        r#"[migrations]
paths = ["{}"]
strategy = "filename_lexicographic"

[output]
formats = [{}]
dir = "{}"

[cli]
fail_on = "{}"
"#,
        migrations_path, format_list, output_dir, fail_on
    );

    let config_path = dir.join("pg-migration-lint.toml");
    std::fs::write(&config_path, config_content).expect("write config");
    config_path
}

/// Collect all .sql file paths in the migrations dir of a fixture, as absolute paths.
fn all_migration_files(fixture_name: &str) -> Vec<PathBuf> {
    let migrations_dir = fixture_path(fixture_name).join("migrations");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&migrations_dir)
        .expect("read migrations dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "sql").unwrap_or(false))
        .collect();
    files.sort();
    files
}

/// Join file paths into a comma-separated string for --changed-files.
fn comma_join(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(",")
}

// ===========================================================================
// Exit code tests
// ===========================================================================

#[test]
fn test_exit_0_clean_repo() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("clean").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "critical",
    );

    let files = all_migration_files("clean");
    let changed = comma_join(&files);

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Clean repo should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_exit_1_findings_above_threshold() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "info", // fail on INFO means any finding triggers exit 1
    );

    let changed = format!(
        "{},{},{}",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
        migrations_dir
            .join("V004__dont_do_this_types.sql")
            .display(),
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "All-rules repo with --fail-on info should exit 1. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_exit_0_findings_below_threshold() {
    // The all-rules fixture has findings with various severities.
    // The suppressed fixture has all findings suppressed, producing exit 0.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("suppressed").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "critical",
    );

    let changed = format!(
        "{},{},{}",
        migrations_dir.join("V002__suppressed.sql").display(),
        migrations_dir.join("V003__suppressed.sql").display(),
        migrations_dir.join("V004__suppressed.sql").display(),
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Suppressed repo should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_exit_2_bad_config_nonexistent() {
    let output = run_lint(&["--config", "nonexistent_config_12345.toml"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "Nonexistent config should exit 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Config file not found"),
        "Should mention config file not found. stderr: {}",
        stderr
    );
}

#[test]
fn test_exit_2_invalid_config_content() {
    // Create a file with invalid TOML content
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_path = tmp.path().join("bad.toml");
    std::fs::write(&config_path, "this is not { valid toml =").expect("write bad config");

    let output = run_lint(&["--config", &config_path.to_string_lossy()]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "Invalid TOML config should exit 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

// ===========================================================================
// Output file tests
// ===========================================================================

#[test]
fn test_sarif_output_file_created() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif"],
        "critical",
    );

    let changed = format!(
        "{},{}",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
    );

    let _output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "sarif",
    ]);

    let sarif_path = output_dir.join("findings.sarif");
    assert!(
        sarif_path.exists(),
        "SARIF file should be created at {}",
        sarif_path.display()
    );

    let content = std::fs::read_to_string(&sarif_path).expect("read SARIF file");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    // Verify it has the SARIF schema and version
    assert_eq!(
        parsed["$schema"],
        "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
        "SARIF $schema should be the 2.1.0 schema URL"
    );
    assert_eq!(parsed["version"], "2.1.0", "SARIF version should be 2.1.0");

    // Verify there are results
    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("results array");
    assert!(
        !results.is_empty(),
        "SARIF results should not be empty for all-rules fixture"
    );
}

#[test]
fn test_sonarqube_output_file_created() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sonarqube"],
        "critical",
    );

    let changed = format!(
        "{},{}",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
    );

    let _output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "sonarqube",
    ]);

    let sonar_path = output_dir.join("findings.json");
    assert!(
        sonar_path.exists(),
        "SonarQube JSON file should be created at {}",
        sonar_path.display()
    );

    let content = std::fs::read_to_string(&sonar_path).expect("read SonarQube JSON");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SonarQube JSON");

    let issues = parsed["issues"]
        .as_array()
        .expect("issues should be an array");
    assert!(
        !issues.is_empty(),
        "SonarQube issues should not be empty for all-rules fixture"
    );

    // Verify top-level rules array exists with engineId
    let rules = parsed["rules"]
        .as_array()
        .expect("rules should be an array");
    assert!(
        !rules.is_empty(),
        "SonarQube rules should not be empty for all-rules fixture"
    );
    for rule in rules {
        assert_eq!(rule["engineId"], "pg-migration-lint");
    }

    // Verify each issue has required fields (10.3+ slim format)
    for issue in issues {
        assert!(issue["ruleId"].is_string());
        assert!(issue["effortMinutes"].is_u64());
        assert!(issue["primaryLocation"]["message"].is_string());
        assert!(issue["primaryLocation"]["filePath"].is_string());
    }
}

#[test]
fn test_text_output_to_stdout() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "critical",
    );

    let changed = format!("{}", migrations_dir.join("V002__violations.sql").display(),);

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Text format writes findings to stdout; should contain PGM rule IDs
    assert!(
        stdout.contains("PGM001"),
        "Text output should contain PGM001. stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("CRITICAL"),
        "Text output should contain severity. stdout: {}",
        stdout
    );
}

// ===========================================================================
// CLI behavior tests
// ===========================================================================

#[test]
fn test_version_flag() {
    let output = run_lint(&["--version"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "--version should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("pg-migration-lint"),
        "--version output should contain 'pg-migration-lint'. stdout: {}",
        stdout
    );
}

#[test]
fn test_explain_known_rule() {
    let output = run_lint(&["--explain", "PGM001"]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "--explain PGM001 should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("CONCURRENTLY"),
        "--explain PGM001 should mention CONCURRENTLY. stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("PGM001"),
        "--explain PGM001 should mention PGM001. stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("CRITICAL"),
        "--explain PGM001 should show severity CRITICAL. stdout: {}",
        stdout
    );
}

#[test]
fn test_explain_unknown_rule() {
    let output = run_lint(&["--explain", "PGM999"]);

    assert_eq!(
        output.status.code(),
        Some(2),
        "--explain PGM999 should exit 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown rule"),
        "--explain PGM999 should say 'Unknown rule'. stderr: {}",
        stderr
    );
}

#[test]
fn test_no_config_falls_back_to_defaults() {
    // Run from a temp dir without any config file but WITH a db/migrations dir
    // containing at least one valid .sql file, so the default paths work.
    let tmp = tempfile::tempdir().expect("tempdir");
    let db_migrations = tmp.path().join("db/migrations");
    std::fs::create_dir_all(&db_migrations).expect("create db/migrations");

    // Write a simple clean migration so loading succeeds
    std::fs::write(
        db_migrations.join("V001__init.sql"),
        "CREATE TABLE test_table (id bigint PRIMARY KEY);\n",
    )
    .expect("write migration");

    let output = Command::new(binary_path())
        .current_dir(tmp.path())
        .args(["--format", "text"])
        .output()
        .expect("execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);

    // Should warn about missing config
    assert!(
        stderr.contains("Config file") && stderr.contains("not found"),
        "Should warn about missing config. stderr: {}",
        stderr
    );

    assert_eq!(
        output.status.code(),
        Some(0),
        "With valid default migrations dir, should exit 0. stderr: {}",
        stderr
    );
}

#[test]
fn test_changed_files_from_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "info",
    );

    // Write changed file paths to a temp file
    let changed_files_path = tmp.path().join("changed.txt");
    let changed_content = format!(
        "{}\n{}\n",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
    );
    std::fs::write(&changed_files_path, changed_content).expect("write changed files list");

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files-from",
        &changed_files_path.to_string_lossy(),
        "--format",
        "text",
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "--changed-files-from should work and find violations. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Should contain findings from V002 and V003
    assert!(
        stdout.contains("PGM001"),
        "Should find PGM001 violations. stdout: {}",
        stdout
    );
    assert!(
        stdout.contains("PGM002"),
        "Should find PGM002 violations. stdout: {}",
        stdout
    );
}

#[test]
fn test_fail_on_cli_override() {
    // Use --fail-on to override the config file's fail_on setting.
    // Config has fail_on = "critical", override with --fail-on info.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    // Config says fail_on = "blocker" -- no findings are BLOCKER, so exit 0
    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "blocker",
    );

    let changed = format!(
        "{},{}",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
    );

    // Without override, should exit 0 (no BLOCKER findings)
    let output_no_override = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);
    assert_eq!(
        output_no_override.status.code(),
        Some(0),
        "With fail_on=blocker and no blocker findings, should exit 0. stderr: {}",
        String::from_utf8_lossy(&output_no_override.stderr)
    );

    // With --fail-on info override, should exit 1 (there ARE info+ findings)
    let output_override = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
        "--fail-on",
        "info",
    ]);
    assert_eq!(
        output_override.status.code(),
        Some(1),
        "With --fail-on info override, should exit 1. stderr: {}",
        String::from_utf8_lossy(&output_override.stderr)
    );
}

// ===========================================================================
// Full pipeline E2E
// ===========================================================================

#[test]
fn test_full_pipeline_with_findings() {
    // Showcase E2E test: run against all-rules fixture, generate both SARIF
    // and SonarQube output, verify exit code, output files, and content.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif", "sonarqube"],
        "info",
    );

    let changed = format!(
        "{},{},{}",
        migrations_dir.join("V002__violations.sql").display(),
        migrations_dir.join("V003__more_violations.sql").display(),
        migrations_dir
            .join("V004__dont_do_this_types.sql")
            .display(),
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
    ]);

    // Exit code should be 1 (findings above INFO threshold)
    assert_eq!(
        output.status.code(),
        Some(1),
        "Full pipeline should exit 1 with findings. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Verify stderr mentions finding count
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("finding(s)"),
        "stderr should contain finding count summary. stderr: {}",
        stderr
    );

    // Verify SARIF output file
    let sarif_path = output_dir.join("findings.sarif");
    assert!(
        sarif_path.exists(),
        "SARIF file should exist at {}",
        sarif_path.display()
    );
    let sarif_content = std::fs::read_to_string(&sarif_path).expect("read SARIF");
    let sarif: serde_json::Value = serde_json::from_str(&sarif_content).expect("parse SARIF");

    let sarif_results = sarif["runs"][0]["results"]
        .as_array()
        .expect("SARIF results array");

    // Verify SonarQube output file
    let sonar_path = output_dir.join("findings.json");
    assert!(
        sonar_path.exists(),
        "SonarQube file should exist at {}",
        sonar_path.display()
    );
    let sonar_content = std::fs::read_to_string(&sonar_path).expect("read SonarQube");
    let sonar: serde_json::Value = serde_json::from_str(&sonar_content).expect("parse SonarQube");

    let sonar_issues = sonar["issues"].as_array().expect("SonarQube issues array");

    // Both should have the same count
    assert_eq!(
        sarif_results.len(),
        sonar_issues.len(),
        "SARIF and SonarQube should have equal result counts: SARIF={}, SonarQube={}",
        sarif_results.len(),
        sonar_issues.len()
    );

    // Should have found all the expected rules
    let sarif_rule_ids: std::collections::HashSet<&str> = sarif_results
        .iter()
        .map(|r| r["ruleId"].as_str().expect("ruleId"))
        .collect();

    for expected in &[
        "PGM001", "PGM002", "PGM003", "PGM004", "PGM005", "PGM006", "PGM007", "PGM009", "PGM010",
        "PGM011", "PGM012", "PGM101", "PGM102", "PGM103", "PGM104", "PGM105",
    ] {
        assert!(
            sarif_rule_ids.contains(expected),
            "Expected {} in SARIF results. Found: {:?}",
            expected,
            sarif_rule_ids
        );
    }
}

#[test]
fn test_full_pipeline_clean_repo_no_output_content() {
    // Verify that a clean repo produces empty results in output files.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("clean").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif"],
        "critical",
    );

    let files = all_migration_files("clean");
    let changed = comma_join(&files);

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
    ]);

    assert_eq!(output.status.code(), Some(0));

    let sarif_path = output_dir.join("findings.sarif");
    assert!(
        sarif_path.exists(),
        "SARIF file should be created even with no findings"
    );

    let sarif_content = std::fs::read_to_string(&sarif_path).expect("read SARIF");
    let sarif: serde_json::Value = serde_json::from_str(&sarif_content).expect("parse SARIF");
    let results = sarif["runs"][0]["results"]
        .as_array()
        .expect("results array");
    assert!(
        results.is_empty(),
        "Clean repo should produce empty SARIF results. Got {} results",
        results.len()
    );
}

#[test]
fn test_stderr_contains_summary() {
    // Verify that stderr always contains the summary line with finding count.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("clean").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "critical",
    );

    let files = all_migration_files("clean");
    let changed = comma_join(&files);

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
        "--format",
        "text",
    ]);

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pg-migration-lint:") && stderr.contains("finding(s)"),
        "stderr should contain summary. stderr: {}",
        stderr
    );
}

#[test]
fn test_enterprise_fixture_via_binary() {
    // Run the enterprise fixture (30 migrations) through the binary to
    // verify it handles a realistic migration history without errors.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("enterprise").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif"],
        "critical",
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--format",
        "sarif",
    ]);

    // The enterprise fixture has critical findings when all files are linted
    // (because all are "changed"), so exit 1 is expected.
    let exit_code = output.status.code().expect("exit code");
    assert!(
        exit_code == 0 || exit_code == 1,
        "Enterprise fixture should exit 0 or 1, not 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // SARIF file should exist
    let sarif_path = output_dir.join("findings.sarif");
    assert!(
        sarif_path.exists(),
        "SARIF file should exist after running enterprise fixture"
    );

    let sarif_content = std::fs::read_to_string(&sarif_path).expect("read SARIF");
    let sarif: serde_json::Value = serde_json::from_str(&sarif_content).expect("parse SARIF");
    assert_eq!(sarif["version"], "2.1.0");
}

#[test]
fn test_go_migrate_fixture_via_binary() {
    // Run the go-migrate fixture through the binary. It includes both .up.sql
    // and .down.sql files. Down migrations should produce INFO-level findings.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("go-migrate").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif"],
        "major",
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--format",
        "sarif",
    ]);

    let exit_code = output.status.code().expect("exit code");
    assert!(
        exit_code == 0 || exit_code == 1,
        "go-migrate fixture should exit 0 or 1, not 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sarif_path = output_dir.join("findings.sarif");
    assert!(sarif_path.exists(), "SARIF file should exist");
}

#[test]
fn test_multiple_format_outputs() {
    // Verify that passing multiple formats via config produces all output files.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    // Config with both sarif and sonarqube as default formats
    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif", "sonarqube"],
        "critical",
    );

    let changed = format!("{}", migrations_dir.join("V002__violations.sql").display(),);

    // Do NOT pass --format so the config's default formats are used
    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
    ]);

    let exit_code = output.status.code().expect("exit code");
    assert!(
        exit_code == 0 || exit_code == 1,
        "Should not exit 2. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Both files should be created
    assert!(
        output_dir.join("findings.sarif").exists(),
        "SARIF file should exist"
    );
    assert!(
        output_dir.join("findings.json").exists(),
        "SonarQube JSON file should exist"
    );
}

// ===========================================================================
// Changed-file path matching tests (exercise suffix matching in main.rs)
// ===========================================================================

#[test]
fn test_changed_files_relative_path_suffix_matching() {
    // Exercise the suffix-matching fallback in changed-file detection (main.rs lines 155-159).
    //
    // The config points to an absolute path for the migrations directory, so the
    // migration unit's source_file is absolute. We pass a RELATIVE path via
    // --changed-files that does NOT exist on disk (so canonicalize fails) but IS
    // a component-wise suffix of the absolute source_file path.
    //
    // This kills mutants:
    //   - `||` -> `&&` on the second disjunction
    //   - `>` -> `==` or `<` on cf.components().count() > 1
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "info",
    );

    // Use a relative path that is a suffix of the absolute source_file path.
    // "repos/all-rules/migrations/V002__violations.sql" does NOT exist on disk
    // (no "repos/" directory in the repo root), so canonicalize will fail.
    // The path has 4 components (> 1), enabling the suffix match.
    let relative_changed = "repos/all-rules/migrations/V002__violations.sql";

    // Verify our assumption: the relative path does not exist on disk
    assert!(
        !std::path::Path::new(relative_changed).exists(),
        "Relative path should NOT exist on disk for this test to be valid"
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        relative_changed,
        "--format",
        "text",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The suffix match should find V002__violations.sql, which contains PGM001
    // (CREATE INDEX without CONCURRENTLY on existing table).
    assert!(
        stdout.contains("PGM001"),
        "Suffix-matched file should produce PGM001 finding. stdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "Should exit 1 when suffix-matched file has findings above threshold. stderr: {}",
        stderr
    );
}

#[test]
fn test_changed_files_bare_filename_does_not_match() {
    // Verify that a bare filename (single path component) does NOT trigger
    // the suffix matching fallback. The guard `cf.components().count() > 1`
    // on line 159 of main.rs should prevent bare filenames from matching
    // across directories.
    //
    // This kills the `> 1` -> `>= 1` mutant
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["text"],
        "info",
    );

    // Pass ONLY a bare filename with no directory components.
    // "V002__violations.sql" has 1 component, so components().count() > 1 is false,
    // and the suffix match should NOT fire even though the absolute source_file
    // ends with this filename.
    let bare_filename = "V002__violations.sql";

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        bare_filename,
        "--format",
        "text",
    ]);

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    // The bare filename should NOT match, so no findings should be produced.
    // V002 contains PGM001 -- if it matched, we would see it.
    assert!(
        !stdout.contains("PGM001"),
        "Bare filename should NOT match via suffix. Expected no PGM001 finding.\nstdout: {}\nstderr: {}",
        stdout,
        stderr
    );

    // With no matched files, we get 0 findings -> exit 0
    assert_eq!(
        output.status.code(),
        Some(0),
        "Should exit 0 when bare filename does not match any migration unit. stderr: {}",
        stderr
    );

    // Verify that the summary confirms 0 findings
    assert!(
        stderr.contains("0 finding(s)"),
        "Should report 0 findings when no files matched. stderr: {}",
        stderr
    );
}

// ===========================================================================
// Selective mode: empty --changed-files writes report with 0 findings
// ===========================================================================

#[test]
fn test_empty_changed_files_writes_empty_report() {
    // When --changed-files is present but empty, selective mode is active:
    // no files are linted, but the output report file is still written.
    // This lets CI always invoke the tool and guarantees the report file exists.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sonarqube"],
        "critical",
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        "",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Empty --changed-files should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Report file should exist with 0 issues
    let sonar_path = output_dir.join("findings.json");
    assert!(
        sonar_path.exists(),
        "SonarQube findings.json should be created even with empty --changed-files"
    );

    let content = std::fs::read_to_string(&sonar_path).expect("read findings.json");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse JSON");
    let issues = parsed["issues"].as_array().expect("issues array");
    assert!(
        issues.is_empty(),
        "Empty --changed-files should produce 0 issues, got {}",
        issues.len()
    );
}

#[test]
fn test_empty_changed_files_from_writes_empty_report() {
    // Same as above but using --changed-files-from with an empty file.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sonarqube"],
        "critical",
    );

    let changed_file = tmp.path().join("changed.txt");
    std::fs::write(&changed_file, "").expect("write empty changed file");

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files-from",
        &changed_file.to_string_lossy(),
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Empty --changed-files-from should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sonar_path = output_dir.join("findings.json");
    assert!(
        sonar_path.exists(),
        "SonarQube findings.json should be created even with empty --changed-files-from"
    );
}

#[test]
fn test_empty_changed_files_writes_empty_sarif() {
    // Same as test_empty_changed_files_writes_empty_report but for SARIF output.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let config_path = write_temp_config(
        tmp.path(),
        &migrations_dir.to_string_lossy(),
        &output_dir.to_string_lossy(),
        &["sarif"],
        "critical",
    );

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        "",
    ]);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Empty --changed-files should exit 0. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sarif_path = output_dir.join("findings.sarif");
    assert!(
        sarif_path.exists(),
        "SARIF file should be created even with empty --changed-files"
    );

    let content = std::fs::read_to_string(&sarif_path).expect("read findings.sarif");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    assert_eq!(parsed["version"], "2.1.0");
    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("results array");
    assert!(
        results.is_empty(),
        "Empty --changed-files should produce 0 SARIF results, got {}",
        results.len()
    );
}

// ===========================================================================
// strip_prefix: finding paths get prefix stripped in reports
// ===========================================================================

#[test]
fn test_strip_prefix_in_sonarqube_output() {
    // Verify that output.strip_prefix removes the configured prefix from
    // finding file paths in the emitted report.
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    // Write a config with strip_prefix that matches the fixture path prefix.
    // The migration files are at e.g. .../tests/fixtures/repos/all-rules/migrations/V002__violations.sql
    // We'll strip everything up to and including "repos/" so the path becomes
    // all-rules/migrations/V002__violations.sql
    let prefix_to_strip = format!(
        "{}/",
        fixture_path("all-rules")
            .parent()
            .expect("parent")
            .to_string_lossy()
    );

    let config_content = format!(
        r#"[migrations]
paths = ["{}"]
strategy = "filename_lexicographic"

[output]
formats = ["sonarqube"]
dir = "{}"
strip_prefix = "{}"

[cli]
fail_on = "info"
"#,
        migrations_dir.to_string_lossy(),
        output_dir.to_string_lossy(),
        prefix_to_strip,
    );

    let config_path = tmp.path().join("pg-migration-lint.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let changed = format!("{}", migrations_dir.join("V002__violations.sql").display());

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "Should find violations. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sonar_path = output_dir.join("findings.json");
    let content = std::fs::read_to_string(&sonar_path).expect("read findings.json");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse JSON");

    let issues = parsed["issues"].as_array().expect("issues array");
    assert!(!issues.is_empty(), "Should have findings");

    // Verify the paths have been stripped
    for issue in issues {
        let file_path = issue["primaryLocation"]["filePath"]
            .as_str()
            .expect("filePath");
        assert!(
            file_path.starts_with("all-rules/migrations/"),
            "Expected stripped path starting with 'all-rules/migrations/', got: {}",
            file_path
        );
        assert!(
            !file_path.contains(&prefix_to_strip),
            "Path should not contain the stripped prefix. path: {}",
            file_path
        );
    }
}

#[test]
fn test_strip_prefix_in_sarif_output() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let migrations_dir = fixture_path("all-rules").join("migrations");
    let output_dir = tmp.path().join("output");

    let prefix_to_strip = format!(
        "{}/",
        fixture_path("all-rules")
            .parent()
            .expect("parent")
            .to_string_lossy()
    );

    let config_content = format!(
        r#"[migrations]
paths = ["{}"]
strategy = "filename_lexicographic"

[output]
formats = ["sarif"]
dir = "{}"
strip_prefix = "{}"

[cli]
fail_on = "info"
"#,
        migrations_dir.to_string_lossy(),
        output_dir.to_string_lossy(),
        prefix_to_strip,
    );

    let config_path = tmp.path().join("pg-migration-lint.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    let changed = format!("{}", migrations_dir.join("V002__violations.sql").display());

    let output = run_lint(&[
        "--config",
        &config_path.to_string_lossy(),
        "--changed-files",
        &changed,
    ]);

    assert_eq!(
        output.status.code(),
        Some(1),
        "Should find violations. stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let sarif_path = output_dir.join("findings.sarif");
    let content = std::fs::read_to_string(&sarif_path).expect("read findings.sarif");
    let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse SARIF JSON");

    let results = parsed["runs"][0]["results"]
        .as_array()
        .expect("results array");
    assert!(!results.is_empty(), "Should have findings");

    for result in results {
        let uri = result["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("uri");
        assert!(
            uri.starts_with("all-rules/migrations/"),
            "Expected stripped SARIF uri starting with 'all-rules/migrations/', got: {}",
            uri
        );
    }
}

// ===========================================================================
// Config path resolution: config in a different directory
// ===========================================================================

#[test]
fn test_config_paths_resolve_relative_to_config_dir() {
    // Place config in a subdirectory with relative paths to migrations and output.
    // Run the binary from a different CWD. Paths should resolve relative to the config dir.
    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path().join("project");
    std::fs::create_dir_all(&config_dir).expect("create config dir");

    // Create a migrations dir under the config dir
    let migrations_dir = config_dir.join("db/migrations");
    std::fs::create_dir_all(&migrations_dir).expect("create migrations dir");

    // Copy a clean migration into it
    std::fs::write(
        migrations_dir.join("V001__init.sql"),
        "CREATE TABLE test_table (id bigint PRIMARY KEY);\n",
    )
    .expect("write migration");

    // Write config with relative paths (relative to config dir)
    let config_content = r#"[migrations]
paths = ["db/migrations"]
strategy = "filename_lexicographic"

[output]
formats = ["sonarqube"]
dir = "build/output"

[cli]
fail_on = "critical"
"#;
    let config_path = config_dir.join("pg-migration-lint.toml");
    std::fs::write(&config_path, config_content).expect("write config");

    // Run from a DIFFERENT directory (tmp root, not config_dir)
    let output = Command::new(binary_path())
        .current_dir(tmp.path())
        .args([
            "--config",
            &config_path.to_string_lossy(),
            "--format",
            "sonarqube",
        ])
        .output()
        .expect("execute binary");

    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(
        output.status.code(),
        Some(0),
        "Config-relative paths should resolve correctly. stderr: {}",
        stderr
    );

    // Output dir should be under config_dir, not under CWD
    let expected_output = config_dir.join("build/output/findings.json");
    assert!(
        expected_output.exists(),
        "Output should be written relative to config dir at {}. stderr: {}",
        expected_output.display(),
        stderr
    );
}
