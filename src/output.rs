//! Output reporters for different formats
//!
//! Supports SARIF 2.1.0, SonarQube Generic Issue Import JSON, and text output.

use crate::rules::{Finding, FindingLocation, Rule, RuleId, Severity};
use std::collections::BTreeSet;
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

/// Rule metadata for the SonarQube 10.3+ reporter.
pub struct RuleInfo {
    /// Rule identifier.
    pub id: RuleId,
    /// Short human-readable name (from `Rule::description()`).
    pub name: String,
    /// Default severity for this rule.
    pub default_severity: Severity,
}

impl RuleInfo {
    /// Build rule metadata for all non-meta rules.
    pub fn all() -> Vec<Self> {
        RuleId::lint_rules()
            .map(|r| RuleInfo {
                id: r,
                name: r.description().to_string(),
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
pub(crate) fn normalize_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// The sentence that names where a grouped finding's other findings are,
/// such as `Also at lines 5, 7, 9.`, or `None` when there are none (an
/// ungrouped finding).
///
/// `anchor_line` is the start line of the location the finding is shown
/// at, and `others` are its other locations. Each start line is named once
/// and `anchor_line` is left out, since a rule that fires per column reports
/// several findings on one statement. When there are more findings than
/// named lines, the sentence counts them: `Also at lines 15, 22 (4 more
/// findings).`, or `Also 2 more findings at this line.` when they all share
/// `anchor_line`.
///
/// Reporters whose viewer shows a single location per finding (text, the
/// SARIF result message GitHub displays, PR comments) append it to the
/// finding's message.
pub fn also_at_lines(anchor_line: usize, others: &[FindingLocation]) -> Option<String> {
    if others.is_empty() {
        return None;
    }
    let other_lines: Vec<String> = others
        .iter()
        .map(|location| location.start_line)
        .filter(|&line| line != anchor_line)
        .collect::<BTreeSet<usize>>()
        .into_iter()
        .map(|line| line.to_string())
        .collect();
    let more_findings = match others.len() {
        1 => "1 more finding".to_string(),
        count => format!("{count} more findings"),
    };

    Some(match other_lines.as_slice() {
        [] => format!("Also {more_findings} at this line."),
        [only] if others.len() == 1 => format!("Also at line {only}."),
        [only] => format!("Also at line {only} ({more_findings})."),
        several if others.len() == several.len() => {
            format!("Also at lines {}.", several.join(", "))
        }
        several => format!("Also at lines {} ({more_findings}).", several.join(", ")),
    })
}

#[cfg(test)]
pub mod test_helpers;

pub(crate) mod sarif;
pub(crate) mod sonarqube;
pub(crate) mod text;

// Re-exported so external consumers (the `pg-migration-lint` binary crate's
// `github-review` subcommand) can reuse the exact severity-to-SARIF-level
// mapping without reaching into the otherwise-`pub(crate)` `sarif` module.
pub use sarif::sarif_level;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::test_helpers::test_finding;
    use crate::parser::SourceSpan;
    use crate::rules::{Finding, FindingLocation, RuleId, Severity};
    use std::path::Path;

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

        let first = vec![Finding::new(
            RuleId::Pgm001,
            Severity::Critical,
            "first".to_string(),
            Path::new("a.sql"),
            &SourceSpan::at(1, 1),
        )];
        reporter.emit(&first, dir.path()).expect("emit first");

        let second = vec![Finding::new(
            RuleId::Pgm501,
            Severity::Major,
            "second".to_string(),
            Path::new("b.sql"),
            &SourceSpan::at(2, 2),
        )];
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
        let reporter = SonarQubeReporter::new(RuleInfo::all());
        assert_eq!(reporter.filename(), "findings.json");
    }

    #[test]
    fn text_filename() {
        let reporter = TextReporter::new(false);
        assert_eq!(reporter.filename(), "findings.txt");
    }

    fn locations_at(lines: &[usize]) -> Vec<FindingLocation> {
        lines
            .iter()
            .map(|&line| FindingLocation {
                start_line: line,
                end_line: line + 2,
                message: format!("statement at line {line}"),
            })
            .collect()
    }

    #[test]
    fn also_at_lines_is_none_without_other_locations() {
        assert_eq!(also_at_lines(1, &[]), None);
    }

    #[test]
    fn also_at_lines_names_a_single_line() {
        assert_eq!(
            also_at_lines(1, &locations_at(&[7])),
            Some("Also at line 7.".to_string())
        );
    }

    #[test]
    fn also_at_lines_names_every_start_line_in_order() {
        assert_eq!(
            also_at_lines(1, &locations_at(&[4, 9, 12])),
            Some("Also at lines 4, 9, 12.".to_string())
        );
    }

    #[test]
    fn also_at_lines_names_each_line_once_and_counts_the_findings() {
        assert_eq!(
            also_at_lines(3, &locations_at(&[15, 22, 29, 29, 29])),
            Some("Also at lines 15, 22, 29 (5 more findings).".to_string())
        );
    }

    #[test]
    fn also_at_lines_leaves_out_the_anchor_line() {
        assert_eq!(
            also_at_lines(3, &locations_at(&[3, 15])),
            Some("Also at line 15 (2 more findings).".to_string())
        );
    }

    #[test]
    fn also_at_lines_counts_findings_that_share_the_anchor_line() {
        assert_eq!(
            also_at_lines(3, &locations_at(&[3, 3])),
            Some("Also 2 more findings at this line.".to_string())
        );
    }

    #[test]
    fn also_at_lines_counts_one_finding_that_shares_the_anchor_line() {
        assert_eq!(
            also_at_lines(3, &locations_at(&[3])),
            Some("Also 1 more finding at this line.".to_string())
        );
    }

    #[test]
    fn also_at_lines_names_lines_before_the_anchor_in_order() {
        assert_eq!(
            also_at_lines(12, &locations_at(&[14, 2])),
            Some("Also at lines 2, 14.".to_string())
        );
    }

    #[test]
    fn emit_to_file_writes_text_report() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = TextReporter::new(false);
        emit_to_file(&reporter, &[test_finding()], dir.path()).expect("emit_to_file");
        let path = dir.path().join("findings.txt");
        assert!(path.exists(), "findings.txt should exist");
        let content = std::fs::read_to_string(&path).expect("read");
        assert!(content.contains("PGM001"), "content should contain rule id");
    }
}
