//! SARIF 2.1.0 output reporter
//!
//! Generates SARIF (Static Analysis Results Interchange Format) JSON files
//! compatible with GitHub Code Scanning. Upload via `github/codeql-action/upload-sarif@v3`.

use crate::output::{ReportError, Reporter, SarifReporter};
use crate::rules::{Finding, FindingLocation, RuleId, Severity};
use serde::Serialize;
use std::collections::BTreeMap;

/// GitHub Code Scanning rejects a whole SARIF upload when one result has
/// more locations than this, so a larger group is split across several
/// results.
const MAX_LOCATIONS_PER_RESULT: usize = 1000;

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

/// A SARIF location. Only a grouped finding's locations carry a message,
/// each the one the rule gave that statement.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SarifLocation {
    physical_location: SarifPhysicalLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<SarifMessage>,
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

/// Maps a finding severity to a SARIF `level` string.
pub fn sarif_level(severity: &Severity) -> &'static str {
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

/// The SARIF results for one finding: one result whose `locations` hold
/// every location of the finding, primary first, or for a group of more
/// than [`MAX_LOCATIONS_PER_RESULT`] locations, one result per chunk of
/// that many.
///
/// GitHub shows only a result's first location, so the result message of a
/// group ends by naming the lines of the result's other locations.
fn results_for(finding: &Finding) -> Vec<SarifResult> {
    let is_group = !finding.additional_locations.is_empty();
    let uri = path_to_uri(&finding.file);
    let locations: Vec<FindingLocation> = finding.locations().collect();

    locations
        .chunks(MAX_LOCATIONS_PER_RESULT)
        .filter_map(|chunk| {
            let (first, others) = chunk.split_first()?;
            let text = match super::also_at_lines(first.start_line, others) {
                Some(also_at) => format!("{} {also_at}", first.message),
                None => first.message.clone(),
            };
            Some(SarifResult {
                rule_id: finding.rule_id.to_string(),
                level: sarif_level(&finding.severity),
                message: SarifMessage { text },
                locations: chunk
                    .iter()
                    .map(|location| SarifLocation {
                        physical_location: SarifPhysicalLocation {
                            artifact_location: SarifArtifactLocation { uri: uri.clone() },
                            region: SarifRegion {
                                start_line: location.start_line,
                                end_line: location.end_line,
                            },
                        },
                        message: is_group.then(|| SarifMessage {
                            text: location.message.clone(),
                        }),
                    })
                    .collect(),
            })
        })
        .collect()
}

impl Reporter for SarifReporter {
    /// Render findings as a SARIF 2.1.0 JSON string.
    fn render(&self, findings: &[Finding]) -> Result<String, ReportError> {
        let rules = collect_rule_descriptors(findings);

        let results: Vec<SarifResult> = findings.iter().flat_map(results_for).collect();

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
    use crate::output::test_helpers::{
        grouped_test_finding, grouped_test_finding_at, test_finding,
    };
    use crate::parser::SourceSpan;
    use crate::rules::{Finding, Severity};
    use std::path::Path;

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
    fn grouped_finding_is_one_result_with_every_location() {
        let parsed = emit_and_parse(&[grouped_test_finding(5)]);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn grouped_result_message_names_the_other_lines() {
        let parsed = emit_and_parse(&[grouped_test_finding(4)]);

        assert_eq!(
            parsed["runs"][0]["results"][0]["message"]["text"],
            "DROP TABLE 'legacy_report_01' without IF EXISTS will fail if the table does not exist. \
             Also at lines 2, 3, 4."
        );
    }

    #[test]
    fn grouped_result_message_names_each_other_line_once() {
        let parsed = emit_and_parse(&[grouped_test_finding_at(&[3, 3, 15, 15])]);

        let results = &parsed["runs"][0]["results"];
        assert_eq!(results[0]["locations"].as_array().map(Vec::len), Some(4));
        assert!(
            results[0]["message"]["text"]
                .as_str()
                .expect("text")
                .ends_with(" Also at line 15 (3 more findings).")
        );
    }

    #[test]
    fn ungrouped_location_has_no_message() {
        let parsed = emit_and_parse(&[test_finding()]);

        let location = &parsed["runs"][0]["results"][0]["locations"][0];
        assert!(location.get("message").is_none());
    }

    fn start_line(location: &serde_json::Value) -> u64 {
        location["physicalLocation"]["region"]["startLine"]
            .as_u64()
            .expect("startLine")
    }

    #[test]
    fn group_beyond_github_location_limit_splits_into_results_of_at_most_1000_locations() {
        let parsed = emit_and_parse(&[grouped_test_finding(2500)]);

        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");
        let location_counts: Vec<usize> = results
            .iter()
            .map(|r| r["locations"].as_array().expect("locations").len())
            .collect();
        assert_eq!(location_counts, vec![1000, 1000, 500]);
        let first_lines: Vec<u64> = results
            .iter()
            .map(|r| start_line(&r["locations"][0]))
            .collect();
        assert_eq!(first_lines, vec![1, 1001, 2001]);

        let last_result_message = results[2]["message"]["text"].as_str().expect("text");
        assert!(last_result_message.starts_with("DROP TABLE 'legacy_report_2001'"));
        assert!(last_result_message.contains("Also at lines 2002, 2003,"));
        assert!(last_result_message.ends_with(", 2500."));
        assert!(!last_result_message.contains(" 2000,"));
    }

    #[test]
    fn group_at_github_location_limit_stays_one_result() {
        let parsed = emit_and_parse(&[grouped_test_finding(1000)]);

        let results = parsed["runs"][0]["results"]
            .as_array()
            .expect("results array");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["locations"].as_array().map(Vec::len), Some(1000));
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
            Finding::new(
                RuleId::Pgm001,
                Severity::Blocker,
                "blocker finding".to_string(),
                Path::new("a.sql"),
                &SourceSpan::at(1, 1),
            ),
            Finding::new(
                RuleId::Pgm002,
                Severity::Critical,
                "critical finding".to_string(),
                Path::new("b.sql"),
                &SourceSpan::at(2, 2),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "major finding".to_string(),
                Path::new("c.sql"),
                &SourceSpan::at(3, 3),
            ),
            Finding::new(
                RuleId::Pgm502,
                Severity::Minor,
                "minor finding".to_string(),
                Path::new("d.sql"),
                &SourceSpan::at(4, 4),
            ),
            Finding::new(
                RuleId::Pgm503,
                Severity::Info,
                "info finding".to_string(),
                Path::new("e.sql"),
                &SourceSpan::at(5, 5),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn file_paths_use_forward_slashes() {
        let findings = vec![Finding::new(
            RuleId::Pgm001,
            Severity::Critical,
            "test".to_string(),
            Path::new("db/migrations/V042__add_index.sql"),
            &SourceSpan::at(1, 1),
        )];

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
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "first".to_string(),
                Path::new("a.sql"),
                &SourceSpan::at(1, 1),
            ),
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "second".to_string(),
                Path::new("b.sql"),
                &SourceSpan::at(2, 2),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "third".to_string(),
                Path::new("c.sql"),
                &SourceSpan::at(3, 3),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn multi_file_findings_reference_correct_paths() {
        let findings = vec![
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "index issue in file A".to_string(),
                Path::new("db/migrations/V001__create_tables.sql"),
                &SourceSpan::at(5, 5),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "missing FK index in file B".to_string(),
                Path::new("db/migrations/V002__add_fk.sql"),
                &SourceSpan::at(10, 12),
            ),
            Finding::new(
                RuleId::Pgm502,
                Severity::Major,
                "no primary key in file C".to_string(),
                Path::new("db/changelog/003_audit.sql"),
                &SourceSpan::at(1, 1),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn rule_metadata_has_correct_fields() {
        let findings = vec![
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "critical finding".to_string(),
                Path::new("a.sql"),
                &SourceSpan::at(1, 1),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "major finding".to_string(),
                Path::new("b.sql"),
                &SourceSpan::at(2, 2),
            ),
            Finding::new(
                RuleId::Pgm503,
                Severity::Info,
                "info finding".to_string(),
                Path::new("c.sql"),
                &SourceSpan::at(3, 3),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn line_numbers_are_correct() {
        let findings = vec![
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "single line".to_string(),
                Path::new("a.sql"),
                &SourceSpan::at(7, 7),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "multi line".to_string(),
                Path::new("b.sql"),
                &SourceSpan::at(15, 20),
            ),
            Finding::new(
                RuleId::Pgm502,
                Severity::Major,
                "line 1".to_string(),
                Path::new("c.sql"),
                &SourceSpan::at(1, 1),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }

    #[test]
    fn round_trip_sarif_all_fields_verified() {
        let findings = vec![
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "CREATE INDEX on 'orders' should use CONCURRENTLY.".to_string(),
                Path::new("db/migrations/V042__add_index.sql"),
                &SourceSpan::at(3, 3),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "FK on 'orders.customer_id' has no covering index.".to_string(),
                Path::new("db/migrations/V043__add_fk.sql"),
                &SourceSpan::at(10, 12),
            ),
            Finding::new(
                RuleId::Pgm503,
                Severity::Info,
                "Table 'events' has UNIQUE NOT NULL but no PRIMARY KEY.".to_string(),
                Path::new("db/migrations/V042__add_index.sql"),
                &SourceSpan::at(20, 20),
            ),
        ];

        let parsed = emit_and_parse(&findings);

        insta::assert_json_snapshot!(parsed, {
            ".runs[0].tool.driver.version" => "[version]",
        });
    }
}
