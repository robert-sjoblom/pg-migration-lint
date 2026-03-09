//! Shared test helpers for output module tests.

use crate::parser::SourceSpan;
use crate::rules::{Finding, RuleId, Severity};
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
