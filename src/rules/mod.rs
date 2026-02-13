//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

#[cfg(test)]
pub mod test_helpers;

pub mod alter_table_check;
pub mod column_type_check;
pub mod pgm001;
pub mod pgm002;
pub mod pgm003;
pub mod pgm004;
pub mod pgm005;
pub mod pgm006;
pub mod pgm007;
pub mod pgm009;
pub mod pgm010;
pub mod pgm011;
pub mod pgm012;
pub mod pgm013;
pub mod pgm014;
pub mod pgm015;
pub mod pgm016;
pub mod pgm017;
pub mod pgm018;
pub mod pgm019;
pub mod pgm020;
pub mod pgm101;
pub mod pgm102;
pub mod pgm103;
pub mod pgm104;
pub mod pgm105;
pub mod pgm108;

use crate::catalog::Catalog;
use crate::parser::ir::{IrNode, Located, SourceSpan};
use serde::Serialize;
use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Severity {
    Info,
    Minor,
    Major,
    Critical,
    Blocker,
}

impl Severity {
    /// Parse from config string. Case-insensitive.
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "info" => Some(Self::Info),
            "minor" => Some(Self::Minor),
            "major" => Some(Self::Major),
            "critical" => Some(Self::Critical),
            "blocker" => Some(Self::Blocker),
            _ => None,
        }
    }

    /// SonarQube severity string.
    pub fn sonarqube_str(&self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Minor => "MINOR",
            Severity::Major => "MAJOR",
            Severity::Critical => "CRITICAL",
            Severity::Blocker => "BLOCKER",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.sonarqube_str())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    #[serde(serialize_with = "serialize_path_forward_slash")]
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
}

#[allow(clippy::ptr_arg)] // serde serialize_with requires &PathBuf, not &Path
fn serialize_path_forward_slash<S: serde::Serializer>(
    path: &std::path::PathBuf,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy().replace('\\', "/"))
}

impl Finding {
    /// Create a finding from a rule, lint context, source span, and message.
    pub fn new(
        rule_id: &str,
        severity: Severity,
        message: String,
        file: &Path,
        span: &SourceSpan,
    ) -> Self {
        Self {
            rule_id: rule_id.to_string(),
            severity,
            message,
            file: file.to_path_buf(),
            start_line: span.start_line,
            end_line: span.end_line,
        }
    }
}

/// Context available to rules during linting.
pub struct LintContext<'a> {
    /// The catalog state BEFORE the current unit was applied.
    /// Clone taken just before apply(). Used by PGM001/002 to check
    /// if a table is pre-existing.
    pub catalog_before: &'a Catalog,

    /// The catalog state AFTER the current unit was applied.
    /// Used for post-file checks (PGM003, PGM004, PGM005).
    pub catalog_after: &'a Catalog,

    /// Set of table names created in the current set of changed files.
    /// Built incrementally during the single-pass replay: when a changed
    /// file contains a CreateTable, add it to this set before linting
    /// subsequent changed files.
    pub tables_created_in_change: &'a HashSet<String>,

    /// Whether this migration unit runs in a transaction.
    pub run_in_transaction: bool,

    /// Whether this is a down/rollback migration.
    pub is_down: bool,

    /// The source file being linted.
    pub file: &'a Path,
}

impl<'a> LintContext<'a> {
    /// Check if a table existed before this change and was not created in the
    /// current set of changed files.
    pub fn is_existing_table(&self, table_key: &str) -> bool {
        self.catalog_before.has_table(table_key)
            && !self.tables_created_in_change.contains(table_key)
    }
}

/// Trait that every rule implements.
pub trait Rule: Send + Sync {
    /// Stable rule identifier: "PGM001", "PGM002", etc.
    fn id(&self) -> &'static str;

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
    /// Some rules (e.g. PGM007, PGM009) may use per-finding severity.
    /// The caller handles down-migration severity capping and suppression filtering.
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;

    /// Convenience method to construct a Finding with this rule's ID and default severity.
    fn make_finding(&self, message: String, file: &std::path::Path, span: &SourceSpan) -> Finding {
        Finding::new(self.id(), self.default_severity(), message, file, span)
    }
}

/// Cap all finding severities to INFO for down/rollback migrations (PGM008).
///
/// Down migrations are informational only. This function mutates the
/// findings in place, setting every severity to `Severity::Info`.
pub fn cap_for_down_migration(findings: &mut [Finding]) {
    for f in findings {
        f.severity = Severity::Info;
    }
}

/// Registry of all rules.
pub struct RuleRegistry {
    rules: Vec<Box<dyn Rule>>,
}

impl RuleRegistry {
    /// Create a new empty rule registry.
    pub fn new() -> Self {
        Self { rules: vec![] }
    }

    /// Register all built-in rules.
    pub fn register_defaults(&mut self) {
        self.register(Box::new(pgm001::Pgm001));
        self.register(Box::new(pgm002::Pgm002));
        self.register(Box::new(pgm003::Pgm003));
        self.register(Box::new(pgm004::Pgm004));
        self.register(Box::new(pgm005::Pgm005));
        self.register(Box::new(pgm006::Pgm006));
        self.register(Box::new(pgm007::Pgm007));
        self.register(Box::new(pgm009::Pgm009));
        self.register(Box::new(pgm010::Pgm010));
        self.register(Box::new(pgm011::Pgm011));
        self.register(Box::new(pgm012::Pgm012));
        self.register(Box::new(pgm013::Pgm013));
        self.register(Box::new(pgm014::Pgm014));
        self.register(Box::new(pgm015::Pgm015));
        self.register(Box::new(pgm016::Pgm016));
        self.register(Box::new(pgm017::Pgm017));
        self.register(Box::new(pgm018::Pgm018));
        self.register(Box::new(pgm019::Pgm019));
        self.register(Box::new(pgm020::Pgm020));
        self.register(Box::new(pgm101::Pgm101));
        self.register(Box::new(pgm102::Pgm102));
        self.register(Box::new(pgm103::Pgm103));
        self.register(Box::new(pgm104::Pgm104));
        self.register(Box::new(pgm105::Pgm105));
        self.register(Box::new(pgm108::Pgm108));
    }

    /// Register a single rule.
    pub fn register(&mut self, rule: Box<dyn Rule>) {
        self.rules.push(rule);
    }

    /// Get a rule by ID (for --explain).
    pub fn get(&self, id: &str) -> Option<&dyn Rule> {
        self.rules.iter().find(|r| r.id() == id).map(|b| &**b)
    }

    /// Iterate all rules.
    pub fn iter(&self) -> impl Iterator<Item = &dyn Rule> {
        self.rules.iter().map(|b| &**b)
    }
}

impl Default for RuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_register_defaults() {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        // We should have 25 rules (PGM001-PGM007, PGM009-PGM020, PGM101-PGM105, PGM108; PGM008 is not a rule)
        assert_eq!(registry.rules.len(), 25);
    }

    #[test]
    fn test_registry_get_by_id() {
        let mut registry = RuleRegistry::new();
        registry.register_defaults();

        assert!(registry.get("PGM001").is_some());
        assert!(registry.get("PGM005").is_some());
        assert!(registry.get("PGM011").is_some());
        assert!(registry.get("PGM008").is_none()); // Not a separate rule
        assert!(registry.get("PGM999").is_none());
    }

    #[test]
    fn test_cap_for_down_migration() {
        let mut findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "test".to_string(),
                file: PathBuf::from("test.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM004".to_string(),
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
}
