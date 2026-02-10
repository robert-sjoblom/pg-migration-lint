//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

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

use crate::catalog::Catalog;
use crate::parser::ir::{IrNode, Located};
use std::collections::HashSet;
use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
        match self {
            Severity::Info => write!(f, "INFO"),
            Severity::Minor => write!(f, "MINOR"),
            Severity::Major => write!(f, "MAJOR"),
            Severity::Critical => write!(f, "CRITICAL"),
            Severity::Blocker => write!(f, "BLOCKER"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub rule_id: String,
    pub severity: Severity,
    pub message: String,
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
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
    pub file: &'a PathBuf,
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
    /// Returns findings with severity set to `default_severity()`.
    /// The caller handles down-migration severity capping and suppression filtering.
    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding>;
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

        // We should have 10 rules (PGM001-PGM007, PGM009-PGM011; PGM008 is not a rule)
        assert_eq!(registry.rules.len(), 10);
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
        assert_eq!(Severity::parse("critical"), Some(Severity::Critical));
        assert_eq!(Severity::parse("CRITICAL"), Some(Severity::Critical));
        assert_eq!(Severity::parse("info"), Some(Severity::Info));
        assert_eq!(Severity::parse("garbage"), None);
    }
}
