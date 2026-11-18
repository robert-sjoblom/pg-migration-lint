//! Shared test helpers for output module tests.

use crate::parser::SourceSpan;
use crate::rules::{Finding, FindingLocation, RuleId, Severity};
use std::path::Path;

/// Create a standard test finding for output format tests.
pub fn test_finding() -> Finding {
    Finding::new(
        RuleId::Pgm001,
        Severity::Critical,
        "CREATE INDEX on existing table 'orders' should use CONCURRENTLY.".to_string(),
        Path::new("db/migrations/V042__add_index.sql"),
        &SourceSpan::at(3, 3),
    )
}

/// A PGM401 group of `statement_count` `DROP TABLE` statements on
/// consecutive lines of one file, starting at line 1.
pub fn grouped_test_finding(statement_count: usize) -> Finding {
    let lines: Vec<usize> = (1..=statement_count).collect();
    grouped_test_finding_at(&lines)
}

/// A PGM401 group with one finding per entry of `lines`, the first being the
/// primary location. A line listed twice holds two findings.
pub fn grouped_test_finding_at(lines: &[usize]) -> Finding {
    let (primary_line, other_lines) = lines.split_first().unwrap_or((&1, &[]));
    let mut group = Finding::new(
        RuleId::Pgm401,
        Severity::Minor,
        drop_table_message(*primary_line),
        Path::new("db/migrations/V043__drop_legacy_reports.sql"),
        &SourceSpan::at(*primary_line, *primary_line),
    );
    group.additional_locations = other_lines
        .iter()
        .map(|&line| FindingLocation {
            start_line: line,
            end_line: line,
            message: drop_table_message(line),
        })
        .collect();
    group
}

fn drop_table_message(line: usize) -> String {
    format!(
        "DROP TABLE 'legacy_report_{line:02}' without IF EXISTS will fail if the table does not exist."
    )
}
