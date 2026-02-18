//! SonarQube Generic Issue Import JSON reporter (10.3+ format)
//!
//! Generates JSON files in the SonarQube 10.3+ Generic Issue Import format
//! with a top-level `rules` array containing clean-code attributes and impacts.
//! See: <https://docs.sonarsource.com/sonarqube-server/10.3/analyzing-source-code/importing-external-issues/generic-issue-import-format/>

use crate::output::{ReportError, Reporter, SonarQubeReporter};
use crate::rules::{Finding, RuleId};
use serde::Serialize;
use std::collections::HashSet;

/// SonarQube-specific metadata for a rule.
struct SonarQubeRuleMeta {
    clean_code_attribute: &'static str,
    issue_type: &'static str,
    software_quality: &'static str,
    impact_severity: &'static str,
}

/// Look up SonarQube-specific metadata for a rule ID.
///
/// This match is exhaustive — adding a new `RuleId` variant without handling it
/// here is a compile error.
fn sonarqube_meta(rule_id: RuleId) -> SonarQubeRuleMeta {
    use crate::rules::{
        DestructiveRule::*, IdempotencyRule::*, MetaRule, SchemaDesignRule::*,
        TypeAntiPatternRule::*, UnsafeDdlRule::*,
    };
    match rule_id {
        // Safety-critical: causes lock contention, table rewrites, or data issues
        RuleId::UnsafeDdl(
            Pgm001 | Pgm002 | Pgm003 | Pgm007 | Pgm008 | Pgm013 | Pgm014 | Pgm015 | Pgm016 | Pgm017,
        ) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "BUG",
            software_quality: "RELIABILITY",
            impact_severity: "HIGH",
        },
        // Volatile default: potentially dangerous but severity is Minor (PG 11+ mitigates)
        RuleId::UnsafeDdl(Pgm006) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "BUG",
            software_quality: "RELIABILITY",
            impact_severity: "MEDIUM",
        },
        // Silent constraint drops: risk data integrity (duplicates, orphaned rows)
        RuleId::UnsafeDdl(Pgm010 | Pgm011 | Pgm012) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "BUG",
            software_quality: "RELIABILITY",
            impact_severity: "MEDIUM",
        },
        // Schema quality / side-effect warnings (UnsafeDdl: DROP COLUMN)
        RuleId::UnsafeDdl(Pgm009) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
        // Performance: missing FK index
        RuleId::SchemaDesign(Pgm501) => SonarQubeRuleMeta {
            clean_code_attribute: "EFFICIENT",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
        // Schema quality: table without PK, rename operations
        RuleId::SchemaDesign(Pgm502 | Pgm504 | Pgm505) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
        // UNIQUE NOT NULL instead of PK
        RuleId::SchemaDesign(Pgm503) => SonarQubeRuleMeta {
            clean_code_attribute: "CONVENTIONAL",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "LOW",
        },
        // Destructive: DROP TABLE
        RuleId::Destructive(Pgm201) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
        // Idempotency: missing IF EXISTS / IF NOT EXISTS
        RuleId::Idempotency(Pgm401 | Pgm402) => SonarQubeRuleMeta {
            clean_code_attribute: "COMPLETE",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
        // Type anti-pattern rules (PGM101-106)
        RuleId::TypeAntiPattern(Pgm101 | Pgm102 | Pgm103 | Pgm104 | Pgm105 | Pgm106) => {
            SonarQubeRuleMeta {
                clean_code_attribute: "CONVENTIONAL",
                issue_type: "CODE_SMELL",
                software_quality: "MAINTAINABILITY",
                impact_severity: "LOW",
            }
        }
        // Meta-behavior (PGM901) — should not appear in findings, but handle gracefully
        RuleId::Meta(MetaRule::Pgm901) => SonarQubeRuleMeta {
            clean_code_attribute: "CONVENTIONAL",
            issue_type: "CODE_SMELL",
            software_quality: "MAINTAINABILITY",
            impact_severity: "MEDIUM",
        },
    }
}

// --- Serde structures for the 10.3+ format ---

/// Top-level SonarQube report envelope.
#[derive(Serialize)]
struct SonarQubeReport {
    rules: Vec<SonarQubeRule>,
    issues: Vec<SonarQubeIssue>,
}

/// A rule definition in the top-level `rules` array.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeRule {
    id: String,
    name: String,
    description: String,
    engine_id: &'static str,
    clean_code_attribute: &'static str,
    #[serde(rename = "type")]
    issue_type: &'static str,
    severity: String,
    impacts: Vec<SonarQubeImpact>,
}

/// An impact entry within a rule definition.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeImpact {
    software_quality: &'static str,
    severity: &'static str,
}

/// A slim issue entry (10.3+ format — metadata lives on the rule).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SonarQubeIssue {
    rule_id: String,
    effort_minutes: u32,
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

/// Effort estimate in minutes based on rule category.
///
/// Exhaustive — adding a new `RuleId` variant without handling it here is a compile error.
fn effort_minutes(rule_id: RuleId) -> u32 {
    use crate::rules::{
        DestructiveRule::*, IdempotencyRule::*, MetaRule, SchemaDesignRule::*,
        TypeAntiPatternRule::*, UnsafeDdlRule::*,
    };
    match rule_id {
        // Concurrently fixes are usually quick
        RuleId::UnsafeDdl(Pgm001 | Pgm002 | Pgm003) => 5,
        // Index/constraint additions
        RuleId::UnsafeDdl(Pgm016 | Pgm017) | RuleId::SchemaDesign(Pgm501) => 15,
        // Table rewrites / schema changes need more thought
        RuleId::UnsafeDdl(Pgm006 | Pgm007 | Pgm008 | Pgm013 | Pgm014 | Pgm015) => 30,
        // Schema quality / side-effect warnings
        RuleId::UnsafeDdl(Pgm009 | Pgm010 | Pgm011 | Pgm012) => 10,
        RuleId::SchemaDesign(Pgm502 | Pgm503 | Pgm504 | Pgm505) => 10,
        RuleId::Destructive(Pgm201) => 10,
        RuleId::Idempotency(Pgm401 | Pgm402) => 10,
        // Type anti-pattern rules
        RuleId::TypeAntiPattern(Pgm101 | Pgm102 | Pgm103 | Pgm104 | Pgm105 | Pgm106) => 10,
        // Meta-behavior
        RuleId::Meta(MetaRule::Pgm901) => 10,
    }
}

impl Reporter for SonarQubeReporter {
    /// Render findings as a SonarQube 10.3+ Generic Issue Import JSON string.
    ///
    /// Only rules that produced at least one finding are included in the
    /// `rules` array.
    fn render(&self, findings: &[Finding]) -> Result<String, ReportError> {
        // Collect the set of rule IDs that appear in findings
        let fired_rules: HashSet<RuleId> = findings.iter().map(|f| f.rule_id).collect();

        // Build rules array from stored RuleInfo, filtered to only rules that fired
        let rules: Vec<SonarQubeRule> = self
            .rules
            .iter()
            .filter(|r| fired_rules.contains(&r.id))
            .map(|r| {
                let meta = sonarqube_meta(r.id);
                SonarQubeRule {
                    id: r.id.to_string(),
                    name: r.name.clone(),
                    description: r.description.clone(),
                    engine_id: "pg-migration-lint",
                    clean_code_attribute: meta.clean_code_attribute,
                    issue_type: meta.issue_type,
                    severity: r.default_severity.sonarqube_str().to_string(),
                    impacts: vec![SonarQubeImpact {
                        software_quality: meta.software_quality,
                        severity: meta.impact_severity,
                    }],
                }
            })
            .collect();

        // Build slim issues array
        let issues: Vec<SonarQubeIssue> = findings
            .iter()
            .map(|f| SonarQubeIssue {
                rule_id: f.rule_id.to_string(),
                effort_minutes: effort_minutes(f.rule_id),
                primary_location: SonarQubePrimaryLocation {
                    message: f.message.clone(),
                    file_path: super::normalize_path(&f.file),
                    text_range: SonarQubeTextRange {
                        start_line: f.start_line,
                        end_line: f.end_line,
                    },
                },
            })
            .collect();

        let report = SonarQubeReport { rules, issues };

        serde_json::to_string_pretty(&report).map_err(|e| ReportError::Serialization(e.to_string()))
    }

    fn filename(&self) -> &str {
        "findings.json"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::RuleInfo;
    use crate::output::test_helpers::test_finding;
    use crate::rules::{Finding, RuleRegistry, SchemaDesignRule, Severity, UnsafeDdlRule};
    use std::path::PathBuf;

    /// Helper: render findings via the reporter and return the parsed JSON.
    fn emit_and_parse(findings: &[Finding]) -> serde_json::Value {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();
        let reporter = SonarQubeReporter::new(RuleInfo::from_registry(&registry));
        let json = reporter.render(findings).expect("render");
        serde_json::from_str(&json).expect("parse json")
    }

    #[test]
    fn single_finding_produces_valid_json() {
        let findings = vec![test_finding()];
        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn multiple_findings_all_present() {
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
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
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
            rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
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
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn message_content_is_preserved() {
        let msg = "CREATE INDEX on existing table 'orders' should use CONCURRENTLY. This is a long message with special characters: <>, &, \"quotes\".";
        let findings = vec![Finding {
            rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
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
    fn round_trip_sonarqube_all_fields_verified() {
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
        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn no_findings_produces_empty_issues_and_rules() {
        let findings: Vec<Finding> = vec![];
        let parsed = emit_and_parse(&findings);

        let issues = parsed["issues"].as_array().expect("issues array");
        assert!(issues.is_empty());
        let rules = parsed["rules"].as_array().expect("rules array");
        assert!(rules.is_empty());
    }

    #[test]
    fn rules_array_only_contains_fired_rules() {
        // Only PGM001 fires — other rules should NOT appear in rules
        let findings = vec![test_finding()]; // PGM001
        let parsed = emit_and_parse(&findings);

        let rules = parsed["rules"].as_array().expect("rules array");
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0]["id"], "PGM001");
    }

    #[test]
    fn engine_id_is_on_rules_not_issues() {
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
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
                severity: Severity::Major,
                message: "second".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 2,
                end_line: 2,
            },
        ];

        let parsed = emit_and_parse(&findings);

        // engineId should be on rules, not on issues
        for rule in parsed["rules"].as_array().expect("rules") {
            assert_eq!(rule["engineId"], "pg-migration-lint");
        }
        for issue in parsed["issues"].as_array().expect("issues") {
            assert!(issue.get("engineId").is_none());
        }

        insta::assert_json_snapshot!(parsed);
    }

    #[test]
    fn line_numbers_are_correct() {
        let findings = vec![
            Finding {
                rule_id: RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001),
                severity: Severity::Critical,
                message: "single line".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 42,
                end_line: 42,
            },
            Finding {
                rule_id: RuleId::SchemaDesign(SchemaDesignRule::Pgm501),
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

    #[test]
    fn all_rules_metadata_snapshot() {
        // One finding per registered rule so the snapshot covers every rule's
        // SonarQube metadata (cleanCodeAttribute, type, impacts, severity).
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        let findings: Vec<Finding> = registry
            .iter()
            .enumerate()
            .map(|(i, rule)| Finding {
                rule_id: rule.id(),
                severity: rule.default_severity(),
                message: format!("{}: {}", rule.id(), rule.description()),
                file: PathBuf::from("test.sql"),
                start_line: i + 1,
                end_line: i + 1,
            })
            .collect();

        let parsed = emit_and_parse(&findings);
        insta::assert_json_snapshot!(parsed);
    }
}
