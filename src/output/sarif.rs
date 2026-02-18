//! SARIF 2.1.0 output reporter
//!
//! Generates SARIF (Static Analysis Results Interchange Format) JSON files
//! compatible with GitHub Code Scanning. Upload via `github/codeql-action/upload-sarif@v3`.

use crate::output::{ReportError, Reporter, SarifReporter};
use crate::rules::{Finding, RuleId, Severity};
use serde::Serialize;
use std::collections::BTreeMap;

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
fn path_to_uri(path: &std::path::Path) -> String {
    super::normalize_path(path)
}

/// Collect unique rules from findings, keyed by rule_id.
///
/// Returns a map from rule_id to the highest severity seen for that rule,
/// preserving deterministic ordering via BTreeMap. Uses the first finding's
/// message as the rule's short description.
fn collect_rule_descriptors(findings: &[Finding]) -> Vec<SarifRuleDescriptor> {
    let mut rule_map: BTreeMap<RuleId, (&Severity, &str)> = BTreeMap::new();

    for f in findings {
        rule_map
            .entry(f.rule_id)
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
            id: id.to_string(),
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
    /// Render findings as a SARIF 2.1.0 JSON string.
    fn render(&self, findings: &[Finding]) -> Result<String, ReportError> {
        let rules = collect_rule_descriptors(findings);

        let results: Vec<SarifResult> = findings
            .iter()
            .map(|f| SarifResult {
                rule_id: f.rule_id.to_string(),
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

        serde_json::to_string_pretty(&log).map_err(|e| ReportError::Serialization(e.to_string()))
    }

    /// The output filename for SARIF reports.
    fn filename(&self) -> &str {
        "findings.sarif"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::test_helpers::test_finding;
    use crate::rules::{Finding, SchemaDesignRule, Severity, UnsafeDdlRule};
    use std::path::PathBuf;

    /// Helper: render findings via SarifReporter and parse the resulting JSON.
    fn emit_and_parse(findings: &[Finding]) -> serde_json::Value {
        let reporter = SarifReporter;
        let json = reporter.render(findings).expect("render");
        serde_json::from_str(&json).expect("parse json")
    }

    #[test]
    fn single_finding_produces_valid_sarif() {
        let parsed = emit_and_parse(&[test_finding()]);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn no_findings_produces_empty_results() {
        let findings: Vec<Finding> = vec![];
        let parsed = emit_and_parse(&findings);

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
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Blocker,
                message: "blocker finding".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm002),
                severity: Severity::Critical,
                message: "critical finding".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "major finding".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 3,
                end_line: 3,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm502),
                severity: Severity::Minor,
                message: "minor finding".to_string(),
                file: PathBuf::from("d.sql"),
                start_line: 4,
                end_line: 4,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm503),
                severity: Severity::Info,
                message: "info finding".to_string(),
                file: PathBuf::from("e.sql"),
                start_line: 5,
                end_line: 5,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn file_paths_use_forward_slashes() {
        let findings = vec![Finding {
            rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
            severity: Severity::Critical,
            message: "test".to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 1,
            end_line: 1,
        }];

        let parsed = emit_and_parse(&findings);

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
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "first".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "second".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "third".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 3,
                end_line: 3,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn multi_file_findings_reference_correct_paths() {
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "index issue in file A".to_string(),
                file: PathBuf::from("db/migrations/V001__create_tables.sql"),
                start_line: 5,
                end_line: 5,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "missing FK index in file B".to_string(),
                file: PathBuf::from("db/migrations/V002__add_fk.sql"),
                start_line: 10,
                end_line: 12,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm502),
                severity: Severity::Major,
                message: "no primary key in file C".to_string(),
                file: PathBuf::from("db/changelog/003_audit.sql"),
                start_line: 1,
                end_line: 1,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn rule_metadata_has_correct_fields() {
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "critical finding".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "major finding".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm503),
                severity: Severity::Info,
                message: "info finding".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 3,
                end_line: 3,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn line_numbers_are_correct() {
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "single line".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 7,
                end_line: 7,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "multi line".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 15,
                end_line: 20,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm502),
                severity: Severity::Major,
                message: "line 1".to_string(),
                file: PathBuf::from("c.sql"),
                start_line: 1,
                end_line: 1,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn round_trip_sarif_all_fields_verified() {
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "CREATE INDEX on 'orders' should use CONCURRENTLY.".to_string(),
                file: PathBuf::from("db/migrations/V042__add_index.sql"),
                start_line: 3,
                end_line: 3,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "FK on 'orders.customer_id' has no covering index.".to_string(),
                file: PathBuf::from("db/migrations/V043__add_fk.sql"),
                start_line: 10,
                end_line: 12,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm503),
                severity: Severity::Info,
                message: "Table 'events' has UNIQUE NOT NULL but no PRIMARY KEY.".to_string(),
                file: PathBuf::from("db/migrations/V042__add_index.sql"),
                start_line: 20,
                end_line: 20,
            },
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }
}
