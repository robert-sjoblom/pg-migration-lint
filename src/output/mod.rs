//! Output reporters for different formats
//!
//! Supports SARIF 2.1.0, SonarQube Generic Issue Import JSON, and text output.

use crate::rules::{Finding, RuleId, RuleRegistry, Severity};
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("IO error writing report: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Trait for output format reporters.
pub trait Reporter {
    /// Write findings to the given output directory.
    /// The filename is determined by the reporter (e.g., "findings.sarif").
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError>;
}

/// Text reporter also supports writing to stdout (for --format text).
pub struct TextReporter {
    pub use_stdout: bool,
}

impl TextReporter {
    pub fn new(use_stdout: bool) -> Self {
        Self { use_stdout }
    }
}

pub struct SarifReporter;

impl SarifReporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SarifReporter {
    fn default() -> Self {
        Self::new()
    }
}

/// Rule metadata for reporters that need per-rule information (e.g. SonarQube 10.3+).
pub struct RuleInfo {
    /// Rule identifier.
    pub id: RuleId,
    /// Short human-readable name (from `Rule::description()`).
    pub name: String,
    /// Detailed explanation (from `Rule::explain()`).
    pub description: String,
    /// Default severity for this rule.
    pub default_severity: Severity,
}

impl RuleInfo {
    /// Extract rule metadata from all rules in a registry.
    pub fn from_registry(registry: &RuleRegistry) -> Vec<Self> {
        registry
            .iter()
            .map(|r| RuleInfo {
                id: r.id(),
                name: r.description().to_string(),
                description: r.explain().to_string(),
                default_severity: r.default_severity(),
            })
            .collect()
    }
}

pub struct SonarQubeReporter {
    rules: Vec<RuleInfo>,
}

impl SonarQubeReporter {
    /// Create a new SonarQube reporter with rule metadata for the 10.3+ format.
    pub fn new(rules: Vec<RuleInfo>) -> Self {
        Self { rules }
    }
}

/// Normalize a path to use forward slashes for cross-platform output.
pub fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
pub mod test_helpers;

pub mod sarif;
pub mod sonarqube;
pub mod text;
