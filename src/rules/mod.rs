//! Rule engine and lint context
//!
//! Each rule implements the `Rule` trait and checks for specific migration safety issues.
//! Rules receive IR nodes and catalog state, returning findings with severity levels.

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

/// Registry of all rules.
pub struct RuleRegistry {
    rules: Vec<Box<dyn Rule>>,
}

impl RuleRegistry {
    pub fn new() -> Self {
        Self { rules: vec![] }
    }

    /// Register all built-in rules.
    pub fn register_defaults(&mut self) {
        // Will be populated in Phase 1D by the Rules Agent
        // self.register(Box::new(pgm001::MissingConcurrentCreateIndex));
        // self.register(Box::new(pgm002::MissingConcurrentDropIndex));
        // ... etc
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
