//! SARIF 2.1.0 output reporter
//!
//! Generates SARIF (Static Analysis Results Interchange Format) JSON files
//! compatible with GitHub Code Scanning. Upload via `github/codeql-action/upload-sarif@v3`.

use crate::output::{ReportError, Reporter, SarifReporter};
use crate::rules::{Finding, Severity};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// Top-level SARIF envelope.
#[derive(Serialize)]
struct SarifLog {
    #[serde(rename = "$schema")]
    schema: &'static str,
    version: &'static str,
    runs: Vec<SarifRun>,
}

/// A single SARIF run.
#[derive(Serialize)]
struct SarifRun {
    tool: SarifTool,
    results: Vec<SarifResult>,
}

/// SARIF tool descriptor.
#[derive(Serialize)]
struct SarifTool {
    driver: SarifDriver,
}

/// SARIF driver (tool metadata + rule definitions).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifDriver {
    name: &'static str,
    version: &'static str,
    information_uri: &'static str,
    rules: Vec<SarifRuleDescriptor>,
}

/// A SARIF rule descriptor appearing in the tool driver.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRuleDescriptor {
    id: String,
    short_description: SarifMessage,
    default_configuration: SarifDefaultConfiguration,
}

/// SARIF default configuration for a rule (severity level).
#[derive(Serialize)]
struct SarifDefaultConfiguration {
    level: &'static str,
}

/// A SARIF result (one finding).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifResult {
    rule_id: String,
    level: &'static str,
    message: SarifMessage,
    locations: Vec<SarifLocation>,
}

/// SARIF message wrapper.
#[derive(Serialize)]
struct SarifMessage {
    text: String,
}

/// A SARIF location.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLocation {
    physical_location: SarifPhysicalLocation,
}

/// SARIF physical location (file + region).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifPhysicalLocation {
    artifact_location: SarifArtifactLocation,
    region: SarifRegion,
}

/// SARIF artifact location (relative file path).
#[derive(Serialize)]
struct SarifArtifactLocation {
    uri: String,
}

/// SARIF region (line range).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifRegion {
    start_line: usize,
    end_line: usize,
}

/// Map a finding severity to a SARIF level string.
fn sarif_level(severity: &Severity) -> &'static str {
    match severity {
        Severity::Blocker | Severity::Critical => "error",
        Severity::Major => "warning",
        Severity::Minor | Severity::Info => "note",
    }
}

/// Convert a file path to a SARIF-compatible URI with forward slashes.
fn path_to_uri(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Collect unique rules from findings, keyed by rule_id.
///
/// Returns a map from rule_id to the highest severity seen for that rule,
/// preserving deterministic ordering via BTreeMap.
fn collect_rule_descriptors(findings: &[Finding]) -> Vec<SarifRuleDescriptor> {
    let mut rule_map: BTreeMap<String, &Severity> = BTreeMap::new();

    for f in findings {
        rule_map
            .entry(f.rule_id.clone())
            .and_modify(|existing| {
                if f.severity > **existing {
                    *existing = &f.severity;
                }
            })
            .or_insert(&f.severity);
    }

    rule_map
        .into_iter()
        .map(|(id, severity)| SarifRuleDescriptor {
            id: id.clone(),
            short_description: SarifMessage {
                text: id,
            },
            default_configuration: SarifDefaultConfiguration {
                level: sarif_level(severity),
            },
        })
        .collect()
}

impl Reporter for SarifReporter {
    /// Emit findings as a SARIF 2.1.0 JSON file.
    ///
    /// Writes `findings.sarif` to the given `output_dir`. Creates the directory
    /// if it does not exist.
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError> {
        std::fs::create_dir_all(output_dir)?;

        let rules = collect_rule_descriptors(findings);

        let results: Vec<SarifResult> = findings
            .iter()
            .map(|f| SarifResult {
                rule_id: f.rule_id.clone(),
                level: sarif_level(&f.severity),
                message: SarifMessage {
                    text: f.message.clone(),
                },
                locations: vec![SarifLocation {
                    physical_location: SarifPhysicalLocation {
                        artifact_location: SarifArtifactLocation {
                            uri: path_to_uri(&f.file),
                        },
                        region: SarifRegion {
                            start_line: f.start_line,
                            end_line: f.end_line,
                        },
                    },
                }],
            })
            .collect();

        let log = SarifLog {
            schema: "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json",
            version: "2.1.0",
            runs: vec![SarifRun {
                tool: SarifTool {
                    driver: SarifDriver {
                        name: "pg-migration-lint",
                        version: "0.1.0",
                        information_uri: "https://github.com/yourusername/pg-migration-lint",
                        rules,
                    },
                },
                results,
            }],
        };

        let json = serde_json::to_string_pretty(&log)
            .map_err(|e| ReportError::Serialization(e.to_string()))?;

        let path = output_dir.join("findings.sarif");
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
    fn single_finding_produces_valid_sarif() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;
        let findings = vec![test_finding()];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        assert_eq!(parsed["version"], "2.1.0");
        assert_eq!(parsed["runs"][0]["tool"]["driver"]["name"], "pg-migration-lint");
        assert_eq!(parsed["runs"][0]["results"][0]["ruleId"], "PGM001");
        assert_eq!(parsed["runs"][0]["results"][0]["level"], "error");
        assert_eq!(
            parsed["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]
                ["startLine"],
            3
        );
        assert_eq!(
            parsed["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "db/migrations/V042__add_index.sql"
        );
    }

    #[test]
    fn no_findings_produces_empty_results() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;
        let findings: Vec<Finding> = vec![];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        assert_eq!(parsed["version"], "2.1.0");
        let results = parsed["runs"][0]["results"].as_array().expect("results array");
        assert!(results.is_empty());
        let rules = parsed["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .expect("rules array");
        assert!(rules.is_empty());
    }

    #[test]
    fn severity_mapping_produces_correct_levels() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Blocker,
                message: "blocker finding".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM002".to_string(),
                severity: Severity::Critical,
                message: "critical finding".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "major finding".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 3,
                end_line: 3,
            },
            Finding {
                rule_id: "PGM004".to_string(),
                severity: Severity::Minor,
                message: "minor finding".to_string(),
                file: PathBuf::from("d.sql"),
                start_line: 4,
                end_line: 4,
            },
            Finding {
                rule_id: "PGM005".to_string(),
                severity: Severity::Info,
                message: "info finding".to_string(),
                file: PathBuf::from("e.sql"),
                start_line: 5,
                end_line: 5,
            },
        ];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        let results = parsed["runs"][0]["results"].as_array().expect("results array");

        assert_eq!(results[0]["level"], "error"); // Blocker
        assert_eq!(results[1]["level"], "error"); // Critical
        assert_eq!(results[2]["level"], "warning"); // Major
        assert_eq!(results[3]["level"], "note"); // Minor
        assert_eq!(results[4]["level"], "note"); // Info
    }

    #[test]
    fn file_paths_use_forward_slashes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

        let findings = vec![Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "test".to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 1,
            end_line: 1,
        }];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let uri = parsed["runs"][0]["results"][0]["locations"][0]["physicalLocation"]
            ["artifactLocation"]["uri"]
            .as_str()
            .expect("uri string");
        assert!(!uri.contains('\\'));
        assert!(uri.contains('/'));
    }

    #[test]
    fn unique_rules_appear_in_driver_rules() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

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
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "second".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "third".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 3,
                end_line: 3,
            },
        ];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let rules = parsed["runs"][0]["tool"]["driver"]["rules"]
            .as_array()
            .expect("rules array");
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0]["id"], "PGM001");
        assert_eq!(rules[1]["id"], "PGM003");
    }
}
