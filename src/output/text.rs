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
use std::fmt::Write as FmtWrite;
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
    let file_str = finding.file.to_string_lossy().replace('\\', "/");
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
    /// Emit findings as human-readable text.
    ///
    /// If `use_stdout` is true, writes to stdout. Otherwise writes
    /// `findings.txt` to the given `output_dir`. Creates the directory
    /// if it does not exist.
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError> {
        let text = format_all(findings);

        if self.use_stdout {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(text.as_bytes())?;
            handle.flush()?;
        } else {
            std::fs::create_dir_all(output_dir)?;
            let path = output_dir.join("findings.txt");
            std::fs::write(path, text)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::{Finding, Severity};
    use std::path::PathBuf;

    fn test_finding() -> Finding {
        Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "CREATE INDEX on existing table 'orders' should use CONCURRENTLY."
                .to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 3,
            end_line: 3,
        }
    }

    #[test]
    fn single_finding_correct_format() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = TextReporter { use_stdout: false };
        let findings = vec![test_finding()];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.txt")).expect("read");

        let expected = "CRITICAL PGM001 db/migrations/V042__add_index.sql:3\n  CREATE INDEX on existing table 'orders' should use CONCURRENTLY.\n";
        assert_eq!(content, expected);
    }

    #[test]
    fn multiple_findings_separated_by_blank_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = TextReporter { use_stdout: false };

        let findings = vec![
            Finding {
                rule_id: "PGM001".to_string(),
                severity: Severity::Critical,
                message: "first finding".to_string(),
                file: PathBuf::from("a.sql"),
                start_line: 1,
                end_line: 1,
            },
            Finding {
                rule_id: "PGM003".to_string(),
                severity: Severity::Major,
                message: "second finding".to_string(),
                file: PathBuf::from("b.sql"),
                start_line: 7,
                end_line: 7,
            },
        ];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.txt")).expect("read");

        let expected = "CRITICAL PGM001 a.sql:1\n  first finding\n\nMAJOR PGM003 b.sql:7\n  second finding\n";
        assert_eq!(content, expected);
    }

    #[test]
    fn no_findings_produces_empty_output() {
        let dir = tempfile::tempdir().expect("tempdir");
        let reporter = TextReporter { use_stdout: false };
        let findings: Vec<Finding> = vec![];

        reporter.emit(&findings, dir.path()).expect("emit");

        let content = std::fs::read_to_string(dir.path().join("findings.txt")).expect("read");
        assert!(content.is_empty());
    }

    #[test]
    fn format_finding_uses_forward_slashes() {
        let finding = Finding {
            rule_id: "PGM001".to_string(),
            severity: Severity::Critical,
            message: "test".to_string(),
            file: PathBuf::from("db/migrations/V042__add_index.sql"),
            start_line: 1,
            end_line: 1,
        };

        let formatted = format_finding(&finding);
        assert!(formatted.contains("db/migrations/V042__add_index.sql"));
        assert!(!formatted.contains('\\'));
    }
}
