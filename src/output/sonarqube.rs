//! SonarQube Generic Issue Import JSON reporter
//!
//! Generates JSON files in the SonarQube Generic Issue Import format.
//! See: <https://docs.sonarqube.org/latest/analysis/generic-issue/>

use crate::output::{ReportError, Reporter, SonarQubeReporter};
use crate::rules::Finding;
use serde::Serialize;
use std::path::Path;

/// Top-level SonarQube report envelope.
#[derive(Serialize)]
struct SonarQubeReport {
    issues: Vec<SonarQubeIssue>,
}

/// A single SonarQube issue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeIssue {
    engine_id: &'static str,
    rule_id: String,
    severity: String,
    #[serde(rename = "type")]
    issue_type: &'static str,
    primary_location: SonarQubePrimaryLocation,
}

/// Primary location for a SonarQube issue.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubePrimaryLocation {
    message: String,
    file_path: String,
    text_range: SonarQubeTextRange,
}

/// Text range (line range) for a SonarQube issue location.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeTextRange {
    start_line: usize,
    end_line: usize,
}

/// Convert a file path to a string with forward slashes.
fn path_to_forward_slashes(path: &Path) -> String {
    super::normalize_path(path)
}

impl Reporter for SonarQubeReporter {
    /// Emit findings as a SonarQube Generic Issue Import JSON file.
    ///
    /// Writes `findings.json` to the given `output_dir`. Creates the directory
    /// if it does not exist.
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError> {
        std::fs::create_dir_all(output_dir)?;

        let issues: Vec<SonarQubeIssue> = findings
            .iter()
            .map(|f| SonarQubeIssue {
                engine_id: "pg-migration-lint",
                rule_id: f.rule_id.clone(),
                severity: f.severity.sonarqube_str().to_string(),
                issue_type: "BUG",
                primary_location: SonarQubePrimaryLocation {
                    message: f.message.clone(),
                    file_path: path_to_forward_slashes(&f.file),
                    text_range: SonarQubeTextRange {
                        start_line: f.start_line,
                        end_line: f.end_line,
                    },
                },
            })
            .collect();

        let report = SonarQubeReport { issues };

        let json = serde_json::to_string_pretty(&report)
            .map_err(|e| ReportError::Serialization(e.to_string()))?;

        let path = output_dir.join("findings.json");
        std::fs::write(path, json)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::test_helpers::test_finding;
    use crate::rules::{Finding, Severity};
    use std::path::PathBuf;

    /// Helper: emit findings via the reporter and return the parsed JSON.
    fn emit_and_parse(findings: &[Finding]) -> serde_json::Value {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;
        reporter.emit(findings, dir.path()).expect("emit");
        let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
        serde_json::from_str(&content).expect("parse json")
    }

    #[test]
    fn single_finding_produces_valid_json() {
        let findings = vec![test_finding()];
        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn severity_mapping_is_correct() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;

        let severities = vec![
            (Severity::Info, "INFO"),
            (Severity::Minor, "MINOR"),
            (Severity::Major, "MAJOR"),
            (Severity::Critical, "CRITICAL"),
            (Severity::Blocker, "BLOCKER"),
        ];

        for (severity, expected_str) in severities {
            let findings = vec![Finding {
                rule_id: "PGM001".to_string(),
                severity,
                message: "test".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            }];

            reporter.emit(&findings, dir.path()).expect("emit");

            let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
            let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

            assert_eq!(
                parsed["issues"][0]["severity"], expected_str,
                "severity {:?} should map to {}",
                severity, expected_str
            );
        }
    }

    #[test]
    fn multiple_findings_all_present() {
        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "first".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "second".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 5,
                end_line: 5,
            },
        ];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn file_paths_use_forward_slashes() {
        let findings = vec![Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "test".to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 1,
            end_line: 1,
        }];

        let parsed = emit_and_parse(&findings);

        let file_path = parsed["issues"][0]["primaryLocation"]["filePath"]
            .as_str()
            .expect("file path string");
        assert!(!file_path.contains('\\'));
        assert!(file_path.contains('/'));
    }

    #[test]
    fn multi_file_findings_have_correct_file_paths() {
        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "index issue in file A".to_string(),
                file: PathBuf::from("db/migrations/V001__create_tables.sql"),
                start_line: 5,
                end_line: 5,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "missing FK index in file B".to_string(),
                file: PathBuf::from("db/migrations/V002__add_fk.sql"),
                start_line: 10,
                end_line: 12,
            },
            Finding {
                rule_id: "PGM004".to_string(),
                severity: Severity::Major,
                message: "no primary key in file C".to_string(),
                file: PathBuf::from("db/changelog/003_audit.sql"),
                start_line: 1,
                end_line: 1,
            },
        ];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn message_content_is_preserved() {
        let msg = "CREATE INDEX on existing table 'orders' should use CONCURRENTLY. This is a long message with special characters: <>, &, \"quotes\".";
        let findings = vec![Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: msg.to_string(),
            file: PathBuf::from("a.sql"),
            start_line: 1,
            end_line: 1,
        }];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn type_field_is_bug_for_all_severities() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;

        // The current implementation uses "BUG" for all severities.
        // Verify this is consistent across all severity levels.
        let severities = vec![
            Severity::Blocker,
            Severity::Critical,
            Severity::Major,
            Severity::Minor,
            Severity::Info,
        ];

        for severity in severities {
            let findings = vec![Finding {
                rule_id: "PGM001".to_string(),
                severity,
                message: "test".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            }];

            reporter.emit(&findings, dir.path()).expect("emit");

            let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
            let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

            assert_eq!(
                parsed["issues"][0]["type"], "BUG",
                "type field should be BUG for severity {:?}",
                severity
            );
        }
    }

    #[test]
    fn round_trip_sonarqube_all_fields_verified() {
        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "CREATE INDEX on 'orders' should use CONCURRENTLY.".to_string(),
                file: PathBuf::from("db/migrations/V042__add_index.sql"),
                start_line: 3,
                end_line: 3,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "FK on 'orders.customer_id' has no covering index.".to_string(),
                file: PathBuf::from("db/migrations/V043__add_fk.sql"),
                start_line: 10,
                end_line: 12,
            },
            Finding {
                rule_id: "PGM005".to_string(),
                severity: Severity::Info,
                message: "Table 'events' has UNIQUE NOT NULL but no PRIMARY KEY.".to_string(),
                file: PathBuf::from("db/migrations/V042__add_index.sql"),
                start_line: 20,
                end_line: 20,
            },
        ];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn no_findings_produces_empty_issues() {
        let findings: Vec<Finding> = vec![];
        let parsed = emit_and_parse(&findings);

        let issues = parsed["issues"].as_array().expect("issues array");
        assert!(issues.is_empty());
    }

    #[test]
    fn engine_id_is_consistent() {
        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "first".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "second".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
        ];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn line_numbers_are_correct() {
        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "single line".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 42,
                end_line: 42,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "multi line".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 100,
                end_line: 105,
            },
        ];

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }
}
