use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{RuleId, Severity, parser::SourceSpan};

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule_id: RuleId,
    pub severity: Severity,
    pub message: String,
    #[serde(serialize_with = "serialize_path_forward_slash")]
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
}

#[allow(clippy::ptr_arg)] // serde serialize_with requires &PathBuf, not &Path
fn serialize_path_forward_slash<S: serde::Serializer>(
    path: &std::path::PathBuf,
    s: S,
) -> Result<S::Ok, S::Error> {
    s.serialize_str(&path.to_string_lossy().replace('\\', "/"))
}

impl Finding {
    /// Create a finding from a rule, lint context, source span, and message.
    pub fn new(
        rule_id: RuleId,
        severity: Severity,
        message: String,
        file: &Path,
        span: &SourceSpan,
    ) -> Self {
        Self {
            rule_id,
            severity,
            message,
            file: file.to_path_buf(),
            start_line: span.start_line,
            end_line: span.end_line,
        }
    }
}
