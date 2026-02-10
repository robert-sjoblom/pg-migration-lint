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
    path.to_string_lossy().replace('\\', "/")
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
    use crate::rules::{Finding, Severity};
    use std::path::PathBuf;

    fn test_finding() -> Finding {
        Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "CREATE INDEX on existing table 'orders' should use CONCURRENTLY."
                .to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 3,
            end_line: 3,
        }
    }

    #[test]
    fn single_finding_produces_valid_json() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;
        let findings = vec![test_finding()];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let issues = parsed["issues"].as_array().expect("issues array");
        assert_eq!(issues.len(), 1);

        let issue = &issues[0];
        assert_eq!(issue["engineId"], "pg-migration-lint");
        assert_eq!(issue["ruleId"], "PGM001");
        assert_eq!(issue["severity"], "CRITICAL");
        assert_eq!(issue["type"], "BUG");
        assert_eq!(
            issue["primaryLocation"]["filePath"],
            "db/migrations/V042__add_index.sql"
        );
        assert_eq!(issue["primaryLocation"]["textRange"]["startLine"], 3);
        assert_eq!(issue["primaryLocation"]["textRange"]["endLine"], 3);
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

            let content =
                std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
            let parsed: serde_json::Value =
                serde_json::from_str(&content).expect("parse json");

            assert_eq!(
                parsed["issues"][0]["severity"], expected_str,
                "severity {:?} should map to {}",
                severity, expected_str
            );
        }
    }

    #[test]
    fn multiple_findings_all_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;

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

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let issues = parsed["issues"].as_array().expect("issues array");
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0]["ruleId"], "PGM001");
        assert_eq!(issues[1]["ruleId"], "PGM003");
    }

    #[test]
    fn file_paths_use_forward_slashes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SonarQubeReporter;

        let findings = vec![Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "test".to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 1,
            end_line: 1,
        }];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.json")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let file_path = parsed["issues"][0]["primaryLocation"]["filePath"]
            .as_str()
            .expect("file path string");
        assert!(!file_path.contains('\\'));
        assert!(file_path.contains('/'));
    }
}
