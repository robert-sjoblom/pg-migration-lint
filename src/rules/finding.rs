use std::collections::HashSet;
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
    /// Optional key for per-unit deduplication.
    ///
    /// When set, at most one finding per `(rule_id, dedup_key)` is kept
    /// after suppression filtering. Used by `existing_table_check` to
    /// collapse multiple DML findings on the same table into one.
    #[serde(skip)]
    pub dedup_key: Option<String>,
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
            dedup_key: None,
        }
    }

    /// Set the dedup key, consuming and returning self.
    pub fn with_dedup_key(mut self, key: String) -> Self {
        self.dedup_key = Some(key);
        self
    }
}

/// Remove duplicate findings that share the same `(rule_id, dedup_key)`.
///
/// Keeps the first occurrence. Findings without a `dedup_key` are always kept.
/// Call **after** suppression filtering so that a suppressed first-occurrence
/// does not shadow unsuppressed later occurrences.
pub fn dedup_findings(findings: &mut Vec<Finding>) {
    let mut seen: HashSet<(RuleId, String)> = HashSet::new();
    findings.retain(|f| match &f.dedup_key {
        Some(key) => seen.insert((f.rule_id, key.clone())),
        None => true,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Rule;

    fn make_finding(rule_id: RuleId, dedup_key: Option<&str>, line: usize) -> Finding {
        let mut f = Finding::new(
            rule_id,
            rule_id.default_severity(),
            "test".to_string(),
            Path::new("test.sql"),
            &SourceSpan::at(line, line),
        );
        if let Some(key) = dedup_key {
            f = f.with_dedup_key(key.to_string());
        }
        f
    }

    #[test]
    fn dedup_same_rule_same_table_keeps_first() {
        let mut findings = vec![
            make_finding(RuleId::Pgm302, Some("public.orders"), 2),
            make_finding(RuleId::Pgm302, Some("public.orders"), 5),
        ];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].start_line, 2);
    }

    #[test]
    fn dedup_same_rule_different_tables_keeps_both() {
        let mut findings = vec![
            make_finding(RuleId::Pgm302, Some("public.orders"), 2),
            make_finding(RuleId::Pgm302, Some("public.products"), 5),
        ];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn dedup_different_rules_same_table_keeps_both() {
        let mut findings = vec![
            make_finding(RuleId::Pgm301, Some("public.orders"), 2),
            make_finding(RuleId::Pgm302, Some("public.orders"), 5),
        ];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn dedup_no_key_always_kept() {
        let mut findings = vec![
            make_finding(RuleId::Pgm001, None, 1),
            make_finding(RuleId::Pgm001, None, 3),
        ];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 2);
    }

    #[test]
    fn dedup_after_suppression_promotes_second() {
        // Simulate: first finding was removed by suppression, second survives dedup
        let mut findings = vec![make_finding(RuleId::Pgm302, Some("public.orders"), 5)];
        dedup_findings(&mut findings);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].start_line, 5);
    }
}
