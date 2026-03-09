//! Human-readable text output reporter
//!
//! Produces plain text output suitable for terminal display during local development.
//! Format follows spec section 7.3:
//! ```text
//! CRITICAL PGM001 db/migrations/V042__add_order_index.sql:3
//!   CREATE INDEX on existing table 'orders' should use CONCURRENTLY.
//! ```

use crate::output::{ReportError, Reporter, TextReporter};
use crate::rules::Finding;
use std::fmt::Write as _;
use std::io::Write;
use std::path::Path;

/// Format a single finding as a text block.
///
/// Returns a string of the form:
/// ```text
/// SEVERITY RULE_ID file:line
///   message
/// ```
fn format_finding(finding: &Finding) -> String {
    let file_str = super::normalize_path(&finding.file);
    let mut buf = String::new();
    // Using write! on String is infallible, but we handle the result properly.
    let _ = write!(
        buf,
        "{} {} {}:{}\n  {}\n",
        finding.severity, finding.rule_id, file_str, finding.start_line, finding.message
    );
    buf
}

/// Format all findings into a single text string.
///
/// Each finding is separated by a blank line for readability.
fn format_all(findings: &[Finding]) -> String {
    let mut output = String::new();
    for (i, finding) in findings.iter().enumerate() {
        output.push_str(&format_finding(finding));
        if i < findings.len() - 1 {
            output.push('\n');
        }
    }
    output
}

impl Reporter for TextReporter {
    fn render(&self, findings: &[Finding]) -> Result<String, ReportError> {
        Ok(format_all(findings))
    }

    fn filename(&self) -> &str {
        "findings.txt"
    }

    /// Emit findings as human-readable text.
    ///
    /// If `use_stdout` is true, writes to stdout. Otherwise writes
    /// `findings.txt` to the given `output_dir`. Creates the directory
    /// if it does not exist.
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError> {
        if self.use_stdout {
            let text = self.render(findings)?;
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(text.as_bytes())?;
            handle.flush()?;
            Ok(())
        } else {
            super::emit_to_file(self, findings, output_dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::test_helpers::test_finding;
    use crate::parser::SourceSpan;
    use crate::rules::{Finding, RuleId, Severity};
    use std::path::Path;

    #[test]
    fn single_finding_correct_format() {
        let reporter = TextReporter { use_stdout: false };
        let findings = vec![test_finding()];
        let content = reporter.render(&findings).expect("render");
        insta::assert_snapshot!(content);
    }

    #[test]
    fn multiple_findings_separated_by_blank_line() {
        let reporter = TextReporter { use_stdout: false };

        let findings = vec![
            Finding::new(
                RuleId::Pgm001,
                Severity::Critical,
                "first finding".to_string(),
                Path::new("a.sql"),
                &SourceSpan::at(1, 1),
            ),
            Finding::new(
                RuleId::Pgm501,
                Severity::Major,
                "second finding".to_string(),
                Path::new("b.sql"),
                &SourceSpan::at(7, 7),
            ),
        ];

        let content = reporter.render(&findings).expect("render");
        insta::assert_snapshot!(content);
    }

    #[test]
    fn no_findings_produces_empty_output() {
        let reporter = TextReporter { use_stdout: false };
        let findings: Vec<Finding> = vec![];
        let content = reporter.render(&findings).expect("render");
        assert!(content.is_empty());
    }

    #[test]
    fn format_finding_uses_forward_slashes() {
        let finding = Finding::new(
            RuleId::Pgm001,
            Severity::Critical,
            "test".to_string(),
            Path::new("db/migrations/V042__add_index.sql"),
            &SourceSpan::at(1, 1),
        );

        let formatted = format_finding(&finding);
        assert!(formatted.contains("db/migrations/V042__add_index.sql"));
        assert!(!formatted.contains('\\'));
    }
}
