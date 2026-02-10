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
/// preserving deterministic ordering via BTreeMap. Uses the first finding's
/// message as the rule's short description.
fn collect_rule_descriptors(findings: &[Finding]) -> Vec<SarifRuleDescriptor> {
    let mut rule_map: BTreeMap<String, (&Severity, &str)> = BTreeMap::new();

    for f in findings {
        rule_map
            .entry(f.rule_id.clone())
            .and_modify(|(existing_sev, _)| {
                if f.severity > **existing_sev {
                    *existing_sev = &f.severity;
                }
            })
            .or_insert((&f.severity, &f.message));
    }

    rule_map
        .into_iter()
        .map(|(id, (severity, message))| SarifRuleDescriptor {
            id,
            short_description: SarifMessage {
                text: message.to_string(),
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
                        version: env!("CARGO_PKG_VERSION"),
                        information_uri: "https://github.com/robert-sjoblom/pg-migration-lint",
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
            message: "CREATE INDEX on existing table 'orders' should use CONCURRENTLY.".to_string(),
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
        assert_eq!(
            parsed["runs"][0]["tool"]["driver"]["name"],
            "pg-migration-lint"
        );
        assert_eq!(parsed["runs"][0]["results"][0]["ruleId"], "PGM001");
        assert_eq!(parsed["runs"][0]["results"][0]["level"], "error");
        assert_eq!(
            parsed["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]["startLine"],
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
        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");
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
        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");

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

        let uri =
            parsed["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"]
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

    #[test]
    fn multi_file_findings_reference_correct_paths() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

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

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");
        assert_eq!(results.len(), 3);

        // Verify each result references the correct file path
        let uri_0 = results[0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("uri 0");
        assert_eq!(uri_0, "db/migrations/V001__create_tables.sql");

        let uri_1 = results[1]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("uri 1");
        assert_eq!(uri_1, "db/migrations/V002__add_fk.sql");

        let uri_2 = results[2]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .as_str()
            .expect("uri 2");
        assert_eq!(uri_2, "db/changelog/003_audit.sql");

        // Verify messages are preserved per result
        assert_eq!(results[0]["message"]["text"], "index issue in file A");
        assert_eq!(results[1]["message"]["text"], "missing FK index in file B");
        assert_eq!(results[2]["message"]["text"], "no primary key in file C");
    }

    #[test]
    fn rule_metadata_has_correct_fields() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "critical finding".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "major finding".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: "PGM005".to_string(),
                severity: Severity::Info,
                message: "info finding".to_string(),
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
        assert_eq!(rules.len(), 3);

        // PGM001: Critical -> error
        assert_eq!(rules[0]["id"], "PGM001");
        assert!(rules[0]["shortDescription"]["text"].is_string());
        assert_eq!(rules[0]["shortDescription"]["text"], "critical finding");
        assert_eq!(rules[0]["defaultConfiguration"]["level"], "error");

        // PGM003: Major -> warning
        assert_eq!(rules[1]["id"], "PGM003");
        assert!(rules[1]["shortDescription"]["text"].is_string());
        assert_eq!(rules[1]["defaultConfiguration"]["level"], "warning");

        // PGM005: Info -> note
        assert_eq!(rules[2]["id"], "PGM005");
        assert!(rules[2]["shortDescription"]["text"].is_string());
        assert_eq!(rules[2]["defaultConfiguration"]["level"], "note");
    }

    #[test]
    fn line_numbers_are_correct() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "single line".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 7,
                end_line: 7,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "multi line".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 15,
                end_line: 20,
            },
            Finding {
                rule_id: "PGM004".to_string(),
                severity: Severity::Major,
                message: "line 1".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 1,
                end_line: 1,
            },
        ];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");

        // First finding: line 7
        let region_0 = &results[0]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region_0["startLine"], 7);
        assert_eq!(region_0["endLine"], 7);

        // Second finding: lines 15-20
        let region_1 = &results[1]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region_1["startLine"], 15);
        assert_eq!(region_1["endLine"], 20);

        // Third finding: line 1
        let region_2 = &results[2]["locations"][0]["physicalLocation"]["region"];
        assert_eq!(region_2["startLine"], 1);
        assert_eq!(region_2["endLine"], 1);
    }

    #[test]
    fn round_trip_sarif_all_fields_verified() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

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

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");

        // Top-level SARIF structure
        assert_eq!(
            parsed["$schema"],
            "https://raw.githubusercontent.com/oasis-tcs/sarif-spec/main/sarif-2.1/schema/sarif-schema-2.1.0.json"
        );
        assert_eq!(parsed["version"], "2.1.0");

        let runs = parsed["runs"].as_array().expect("runs array");
        assert_eq!(runs.len(), 1);

        // Tool metadata
        let driver = &runs[0]["tool"]["driver"];
        assert_eq!(driver["name"], "pg-migration-lint");
        assert_eq!(driver["version"], env!("CARGO_PKG_VERSION"));
        assert!(driver["informationUri"].is_string());

        // Rules array: 3 distinct rules
        let rules = driver["rules"].as_array().expect("rules array");
        assert_eq!(rules.len(), 3);

        let rule_ids: Vec<&str> = rules.iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert!(rule_ids.contains(&"PGM001"));
        assert!(rule_ids.contains(&"PGM003"));
        assert!(rule_ids.contains(&"PGM005"));

        // Each rule has required fields
        for rule in rules {
            assert!(rule["id"].is_string(), "rule must have id");
            assert!(
                rule["shortDescription"]["text"].is_string(),
                "rule must have shortDescription.text"
            );
            assert!(
                rule["defaultConfiguration"]["level"].is_string(),
                "rule must have defaultConfiguration.level"
            );
        }

        // Results array: 3 findings
        let results = runs[0]["results"].as_array().expect("results array");
        assert_eq!(results.len(), 3);

        // Verify result 0: PGM001, error, file A, line 3
        assert_eq!(results[0]["ruleId"], "PGM001");
        assert_eq!(results[0]["level"], "error");
        assert_eq!(
            results[0]["message"]["text"],
            "CREATE INDEX on 'orders' should use CONCURRENTLY."
        );
        let loc0 = &results[0]["locations"][0]["physicalLocation"];
        assert_eq!(
            loc0["artifactLocation"]["uri"],
            "db/migrations/V042__add_index.sql"
        );
        assert_eq!(loc0["region"]["startLine"], 3);
        assert_eq!(loc0["region"]["endLine"], 3);

        // Verify result 1: PGM003, warning, file B, lines 10-12
        assert_eq!(results[1]["ruleId"], "PGM003");
        assert_eq!(results[1]["level"], "warning");
        assert_eq!(
            results[1]["message"]["text"],
            "FK on 'orders.customer_id' has no covering index."
        );
        let loc1 = &results[1]["locations"][0]["physicalLocation"];
        assert_eq!(
            loc1["artifactLocation"]["uri"],
            "db/migrations/V043__add_fk.sql"
        );
        assert_eq!(loc1["region"]["startLine"], 10);
        assert_eq!(loc1["region"]["endLine"], 12);

        // Verify result 2: PGM005, note, file A again, line 20
        assert_eq!(results[2]["ruleId"], "PGM005");
        assert_eq!(results[2]["level"], "note");
        assert_eq!(
            results[2]["message"]["text"],
            "Table 'events' has UNIQUE NOT NULL but no PRIMARY KEY."
        );
        let loc2 = &results[2]["locations"][0]["physicalLocation"];
        assert_eq!(
            loc2["artifactLocation"]["uri"],
            "db/migrations/V042__add_index.sql"
        );
        assert_eq!(loc2["region"]["startLine"], 20);
        assert_eq!(loc2["region"]["endLine"], 20);
    }
}
