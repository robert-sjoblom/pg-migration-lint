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
///
/// Implementors provide [`render`](Reporter::render) (pure string generation) and
/// [`filename`](Reporter::filename). The default [`emit`](Reporter::emit) handles
/// filesystem operations so reporters don't duplicate `create_dir_all` / `fs::write`.
pub trait Reporter {
    /// Render findings into a format-specific string (JSON, text, etc.).
    fn render(&self, findings: &[Finding]) -> Result<String, ReportError>;

    /// The output filename (e.g. `"findings.sarif"`).
    fn filename(&self) -> &str;

    /// Write findings to the given output directory.
    ///
    /// The default implementation calls [`render`](Reporter::render), creates
    /// `output_dir` if needed, and writes `<output_dir>/<filename>`.
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError> {
        let content = self.render(findings)?;
        std::fs::create_dir_all(output_dir)?;
        std::fs::write(output_dir.join(self.filename()), content)?;
        Ok(())
    }
}

/// Render findings via `reporter` and write the result to `output_dir/<filename>`.
///
/// Shared helper so that [`TextReporter`]'s `emit()` override can delegate here
/// for the file-writing path without duplicating the default trait body.
pub(crate) fn emit_to_file(
    reporter: &dyn Reporter,
    findings: &[Finding],
    output_dir: &Path,
) -> Result<(), ReportError> {
    let content = reporter.render(findings)?;
    std::fs::create_dir_all(output_dir)?;
    std::fs::write(output_dir.join(reporter.filename()), content)?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::test_helpers::test_finding;
    use crate::rules::{Finding, MigrationRule, RuleId, Severity};
    use std::path::PathBuf;

    #[test]
    fn emit_creates_file_on_disk() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;
        reporter.emit(&[test_finding()], dir.path()).expect("emit");
        let path = dir.path().join("findings.sarif");
        assert!(path.exists(), "findings.sarif should exist");
        let meta = std::fs::metadata(&path).expect("metadata");
        assert!(meta.len() > 0, "file should be non-empty");
    }

    #[test]
    fn emit_creates_output_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let nested = dir.path().join("a").join("b").join("c");
        let reporter = SarifReporter;
        reporter.emit(&[test_finding()], &nested).expect("emit");
        let path = nested.join("findings.sarif");
        assert!(path.exists(), "findings.sarif should exist in nested dir");
    }

    #[test]
    fn emit_overwrites_existing_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = SarifReporter;

        let first = vec![Finding {
            rule_id: RuleId::Migration(MigrationRule::Pgm001),
            severity: Severity::Critical,
            message: "first".to_string(),
            file: PathBuf::from("a.sql"),
            start_line: 1,
            end_line: 1,
        }];
        reporter.emit(&first, dir.path()).expect("emit first");

        let second = vec![Finding {
            rule_id: RuleId::Migration(MigrationRule::Pgm003),
            severity: Severity::Major,
            message: "second".to_string(),
            file: PathBuf::from("b.sql"),
            start_line: 2,
            end_line: 2,
        }];
        reporter.emit(&second, dir.path()).expect("emit second");

        let content = std::fs::read_to_string(dir.path().join("findings.sarif")).expect("read");
        assert!(content.contains("second"), "second emit should win");
        assert!(
            !content.contains("first"),
            "first emit should be overwritten"
        );
    }

    #[test]
    fn text_emit_stdout_does_not_create_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = TextReporter { use_stdout: true };
        reporter.emit(&[test_finding()], dir.path()).expect("emit");
        let path = dir.path().join("findings.txt");
        assert!(
            !path.exists(),
            "findings.txt should not exist when use_stdout is true"
        );
    }

    #[test]
    fn sarif_filename() {
        assert_eq!(SarifReporter.filename(), "findings.sarif");
    }

    #[test]
    fn sonarqube_filename() {
        let mut registry = crate::rules::RuleRegistry::new();
        registry.register_defaults();
        let reporter = SonarQubeReporter::new(RuleInfo::from_registry(&registry));
        assert_eq!(reporter.filename(), "findings.json");
    }

    #[test]
    fn text_filename() {
        let reporter = TextReporter::new(false);
        assert_eq!(reporter.filename(), "findings.txt");
    }
}
