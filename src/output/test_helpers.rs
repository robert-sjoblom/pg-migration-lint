//! Shared test helpers for output module tests.

use crate::rules::{Finding, RuleId, Severity};
use std::path::PathBuf;

/// Create a standard test finding for output format tests.
pub fn test_finding() -> Finding {
    Finding {
        rule_id: RuleId::Pgm001,
        severity: Severity::Critical,
        message: "CREATE INDEX on existing table 'orders' should use CONCURRENTLY.".to_string(),
        file: PathBuf::from("db/migrations/V042__add_index.sql"),
        start_line: 3,
        end_line: 3,
    }
}
