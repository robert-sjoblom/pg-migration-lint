//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

use crate::parser::ir::{IrNode, Located, SourceSpan};
pub use crate::rules::finding::Finding;
pub use crate::rules::lint_context::LintContext;
pub use crate::rules::rule_id::RuleId;
pub use crate::rules::severity::Severity;

mod alter_table_check;
mod column_type_check;
mod drop_column_check;
mod existing_table_check;
mod finding;
mod lint_context;
mod rule_id;
mod severity;
#[cfg(test)]
mod test_helpers;

// 0xx — Unsafe DDL
mod pgm001;
mod pgm002;
mod pgm003;
mod pgm004;
mod pgm005;
mod pgm006;
mod pgm007;
mod pgm008;
mod pgm009;
mod pgm010;
mod pgm011;
mod pgm012;
mod pgm013;
mod pgm014;
mod pgm015;
mod pgm016;
mod pgm017;
mod pgm018;
mod pgm019;
mod pgm020;

// 1xx — Type anti-patterns
mod pgm101;
mod pgm102;
mod pgm103;
mod pgm104;
mod pgm105;
mod pgm106;

// 2xx — Destructive operations
mod pgm201;
mod pgm202;
mod pgm203;
mod pgm204;
mod pgm205;

// 3xx — DML in migrations
mod pgm301;
mod pgm302;
mod pgm303;

// 4xx — Idempotency guards
mod pgm401;
mod pgm402;
mod pgm403;

// 5xx — Schema design & informational
mod pgm501;
mod pgm502;
mod pgm503;
mod pgm504;
mod pgm505;
mod pgm506;

/// Trait that every rule implements.
pub trait Rule: Send + Sync {
    /// Stable rule identifier.
    fn id(&self) -> RuleId;

    /// Default severity for this rule.
    fn default_severity(&self) -> Severity;

    /// Human-readable short description.
    fn description(&self) -> &'static str;

    /// Detailed explanation for --explain. Includes failure mode, example, fix.
    fn explain(&self) -> &'static str;

    /// Run the rule against a single migration unit.
    ///
    /// `statements` are the IR nodes for the unit being linted.
    /// `ctx` provides catalog state and changed-file context.
    ///
    /// Returns findings, typically with severity from `default_severity()`.
    /// Some rules (e.g. PGM006, PGM007) may use per-finding severity.
    /// The caller handles down-migration severity capping and suppression filtering.
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;

    /// Convenience method to construct a Finding with this rule's ID and default severity.
    fn make_finding(&self, message: String, file: &std::path::Path, span: &SourceSpan) -> Finding {
        Finding::new(self.id(), self.default_severity(), message, file, span)
    }
}

/// Controls which tables a rule considers "existing".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableScope {
    /// Table must exist in catalog_before AND not appear in tables_created_in_change.
    /// For locking/performance rules where brand-new tables are exempt.
    ExcludeCreatedInChange,
    /// Table must exist in catalog_before only.
    /// For side-effect/integrity rules where the warning matters even if the
    /// table was created earlier in the same set of changed files.
    AnyPreExisting,
}

/// Cap all finding severities to INFO for down/rollback migrations (PGM901).
///
/// Down migrations are informational only. This function mutates the
/// findings in place, setting every severity to `Severity::Info`.
pub fn cap_for_down_migration(findings: &mut [Finding]) {
    for f in findings {
        f.severity = Severity::Info;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use strum::IntoEnumIterator;

    use crate::rules::finding::Finding;
    use crate::rules::rule_id::RuleId;
    use crate::rules::severity::Severity;

    use super::*;

    #[test]
    fn test_cap_for_down_migration() {
        let mut findings = vec![
            Finding {
                rule_id: RuleId::Pgm001,
                severity: Severity::Critical,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: RuleId::Pgm502,
                severity: Severity::Major,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 2,
                end_line: 2,
            },
        ];

        cap_for_down_migration(&mut findings);

        assert_eq!(findings[0].severity, Severity::Info);
        assert_eq!(findings[1].severity, Severity::Info);
    }

    #[test]
    fn test_severity_ordering() {
        assert!(Severity::Info < Severity::Minor);
        assert!(Severity::Minor < Severity::Major);
        assert!(Severity::Major < Severity::Critical);
        assert!(Severity::Critical < Severity::Blocker);
    }

    #[test]
    fn test_all_rules_have_valid_description() {
        for id in RuleId::lint_rules() {
            let desc = id.description();
            assert!(desc.len() > 10, "{id} description too short: {desc:?}");
        }
    }

    #[test]
    fn test_all_rules_have_valid_explain() {
        for id in RuleId::lint_rules() {
            let explain = id.explain();
            assert!(
                explain.len() > 20,
                "{id} explain text too short: {explain:?}"
            );
        }
    }

    #[test]
    fn test_explain_output_snapshots() {
        for id in RuleId::lint_rules() {
            let output = format!(
                "Rule: {}\nSeverity: {}\nDescription: {}\n\n{}",
                id,
                id.default_severity(),
                id.description(),
                id.explain()
            );
            insta::assert_snapshot!(format!("explain_{}", id), output);
        }
    }

    #[test]
    fn test_severity_parse() {
        assert_eq!(Severity::parse("blocker"), Some(Severity::Blocker));
        assert_eq!(Severity::parse("critical"), Some(Severity::Critical));
        assert_eq!(Severity::parse("major"), Some(Severity::Major));
        assert_eq!(Severity::parse("minor"), Some(Severity::Minor));
        assert_eq!(Severity::parse("info"), Some(Severity::Info));
        // Case-insensitive
        assert_eq!(Severity::parse("CRITICAL"), Some(Severity::Critical));
        assert_eq!(Severity::parse("Blocker"), Some(Severity::Blocker));
        // Invalid
        assert_eq!(Severity::parse("garbage"), None);
        assert_eq!(Severity::parse("none"), None);
    }

    // -----------------------------------------------------------------------
    // RuleId enum tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_rule_id_display_round_trip() {
        // Every variant should survive Display → FromStr round-trip
        for id in RuleId::iter() {
            let s = id.to_string();
            let parsed: RuleId = s.parse().unwrap_or_else(|_| panic!("failed to parse {s}"));
            assert_eq!(id, parsed, "round-trip failed for {s}");
            assert_eq!(id.as_str(), s.as_str());
        }
        assert_eq!(RuleId::iter().count(), 44);
    }

    #[test]
    fn test_rule_id_from_str_unknown() {
        assert!("PGM000".parse::<RuleId>().is_err());
        assert!("PGM999".parse::<RuleId>().is_err());
        assert!("garbage".parse::<RuleId>().is_err());
        assert!("pgm001".parse::<RuleId>().is_err()); // case-sensitive
    }

    #[test]
    fn test_rule_id_ordering() {
        // Variants are ordered by declaration order
        assert!(RuleId::Pgm017 < RuleId::Pgm101);
        assert!(RuleId::Pgm106 < RuleId::Pgm201);
        assert!(RuleId::Pgm201 < RuleId::Pgm301);
        assert!(RuleId::Pgm303 < RuleId::Pgm401);
        assert!(RuleId::Pgm402 < RuleId::Pgm501);
        assert!(RuleId::Pgm506 < RuleId::Pgm901);
        // Within a family
        assert!(RuleId::Pgm001 < RuleId::Pgm017);
    }

    #[test]
    fn test_rule_id_serialize_json() {
        let id = RuleId::Pgm003;
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "\"PGM003\"");
    }

    #[test]
    fn test_parse_rule_id_error_display() {
        let err = "BOGUS".parse::<RuleId>().unwrap_err();
        assert_eq!(err.to_string(), "Matching variant not found");
    }

    #[test]
    fn meta_rule_pgm901_description_is_non_empty() {
        let rule_id = RuleId::Pgm901;
        let desc = rule_id.description();
        assert!(!desc.is_empty(), "PGM901 description should not be empty");
        assert!(
            desc.contains("Meta"),
            "PGM901 description should mention Meta"
        );
    }

    #[test]
    fn meta_rule_pgm901_explain_is_non_empty() {
        let rule_id = RuleId::Pgm901;
        let explain = rule_id.explain();
        assert!(!explain.is_empty(), "PGM901 explain should not be empty");
        assert!(
            explain.contains("INFO"),
            "PGM901 explain should mention INFO"
        );
    }
}
