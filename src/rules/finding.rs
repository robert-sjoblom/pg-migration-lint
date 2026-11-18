use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::{RuleId, Severity, names::RelationName, parser::SourceSpan};

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub rule_id: RuleId,
    pub severity: Severity,
    pub message: String,
    #[serde(serialize_with = "serialize_path_forward_slash")]
    pub file: PathBuf,
    pub start_line: usize,
    pub end_line: usize,
    /// The other statements in `file` this finding stands for, ordered by
    /// line, each with the message the rule gave it.
    ///
    /// Empty unless [`group_repeated_findings`] merged several findings into
    /// this one, in which case `start_line`, `end_line` and `message` are
    /// the group's primary location.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub additional_locations: Vec<FindingLocation>,
    /// Optional key for per-unit deduplication.
    ///
    /// When set, at most one finding per `(rule_id, dedup_key)` is kept
    /// after suppression filtering. Used by `existing_table_check` to
    /// collapse findings on the same table into one, and by PGM024 to
    /// collapse findings under the same partitioned parent.
    #[serde(skip)]
    pub dedup_key: Option<RelationName>,
}

/// One statement a [`Finding`] reports, in the finding's own file: its line
/// span and the message the rule gave for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FindingLocation {
    pub start_line: usize,
    pub end_line: usize,
    pub message: String,
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
            additional_locations: Vec::new(),
            dedup_key: None,
        }
    }

    /// Set the dedup key, consuming and returning self.
    pub fn with_dedup_key(mut self, key: RelationName) -> Self {
        self.dedup_key = Some(key);
        self
    }

    /// The primary location (`start_line`, `end_line`, `message`).
    pub fn primary_location(&self) -> FindingLocation {
        FindingLocation {
            start_line: self.start_line,
            end_line: self.end_line,
            message: self.message.clone(),
        }
    }

    /// Every location this finding reports: the primary location, then
    /// [`additional_locations`](Self::additional_locations).
    pub fn locations(&self) -> impl Iterator<Item = FindingLocation> + '_ {
        std::iter::once(self.primary_location()).chain(self.additional_locations.iter().cloned())
    }

    fn absorb(&mut self, member: Finding) {
        self.additional_locations.extend(member.locations());
    }

    fn put_lowest_line_first(&mut self) {
        let mut by_line: Vec<FindingLocation> = self.locations().collect();
        by_line.sort_by_key(|location| (location.start_line, location.end_line));
        let mut by_line = by_line.into_iter();
        if let Some(primary) = by_line.next() {
            self.start_line = primary.start_line;
            self.end_line = primary.end_line;
            self.message = primary.message;
        }
        self.additional_locations = by_line.collect();
    }
}

/// Remove duplicate findings that share the same `(rule_id, dedup_key)`.
///
/// Keeps the first occurrence. Findings without a `dedup_key` are always kept.
/// Call **after** suppression filtering so that a suppressed first-occurrence
/// does not shadow unsuppressed later occurrences.
pub fn dedup_findings(findings: &mut Vec<Finding>) {
    let mut seen: HashSet<(RuleId, RelationName)> = HashSet::new();
    findings.retain(|f| match &f.dedup_key {
        Some(key) => seen.insert((f.rule_id, key.clone())),
        None => true,
    });
}

/// The findings [`group_repeated_findings`] may merge: one rule, at one
/// severity, in one file.
type GroupKey = (PathBuf, RuleId, Severity);

fn group_key(finding: &Finding) -> GroupKey {
    (finding.file.clone(), finding.rule_id, finding.severity)
}

/// Merge the findings one rule reports at one severity in one file into a
/// single finding, once there are at least `threshold` of them.
///
/// The merged finding takes the place of the group's first finding. Its
/// primary location is the member with the lowest line, and the other
/// members become its [`Finding::additional_locations`], ordered by line,
/// each keeping its own message. Severity is never merged: findings at
/// different severities (a rule that reports some statements as INFO, or
/// a down migration capped by PGM901) count and group separately. Findings
/// whose group stays below `threshold` are returned unchanged and in order.
///
/// A `threshold` of 0 turns grouping off; 1 makes one finding per rule,
/// severity and file.
///
/// Call **after** suppression filtering and [`dedup_findings`], so a
/// suppressed statement is never a member and does not count toward
/// `threshold`.
pub fn group_repeated_findings(findings: Vec<Finding>, threshold: usize) -> Vec<Finding> {
    if threshold == 0 {
        return findings;
    }

    let mut member_counts: HashMap<GroupKey, usize> = HashMap::new();
    for finding in &findings {
        *member_counts.entry(group_key(finding)).or_default() += 1;
    }

    let mut grouped: Vec<Finding> = Vec::with_capacity(findings.len());
    let mut group_positions: HashMap<GroupKey, usize> = HashMap::new();
    for finding in findings {
        let key = group_key(&finding);
        let reaches_threshold = member_counts
            .get(&key)
            .is_some_and(|&count| count >= threshold);
        if !reaches_threshold {
            grouped.push(finding);
            continue;
        }
        match group_positions.entry(key) {
            Entry::Occupied(position) => {
                if let Some(group) = grouped.get_mut(*position.get()) {
                    group.absorb(finding);
                }
            }
            Entry::Vacant(position) => {
                position.insert(grouped.len());
                grouped.push(finding);
            }
        }
    }

    for position in group_positions.into_values() {
        if let Some(group) = grouped.get_mut(position) {
            group.put_lowest_line_first();
        }
    }

    grouped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{catalog::builder::rel, rules::Rule};

    fn make_finding(rule_id: RuleId, dedup_key: Option<&str>, line: usize) -> Finding {
        let mut f = Finding::new(
            rule_id,
            rule_id.default_severity(),
            "test".to_string(),
            Path::new("test.sql"),
            &SourceSpan::at(line, line),
        );
        if let Some(key) = dedup_key {
            f = f.with_dedup_key(rel(key));
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

    fn finding_in(file: &str, rule_id: RuleId, severity: Severity, line: usize) -> Finding {
        Finding::new(
            rule_id,
            severity,
            format!("{rule_id} at line {line}"),
            Path::new(file),
            &SourceSpan::at(line, line),
        )
    }

    fn drops_without_if_exists(file: &str, lines: &[usize]) -> Vec<Finding> {
        lines
            .iter()
            .map(|&line| finding_in(file, RuleId::Pgm401, Severity::Minor, line))
            .collect()
    }

    fn location(line: usize, message: &str) -> FindingLocation {
        FindingLocation {
            start_line: line,
            end_line: line,
            message: message.to_string(),
        }
    }

    type FindingSummary = (String, RuleId, Severity, usize, Vec<usize>);

    fn summarize(findings: &[Finding]) -> Vec<FindingSummary> {
        findings
            .iter()
            .map(|f| {
                (
                    f.file.to_string_lossy().into_owned(),
                    f.rule_id,
                    f.severity,
                    f.start_line,
                    f.additional_locations
                        .iter()
                        .map(|l| l.start_line)
                        .collect(),
                )
            })
            .collect()
    }

    #[test]
    fn group_below_threshold_keeps_every_finding() {
        let findings = drops_without_if_exists("a.sql", &[1, 2, 3, 4]);

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![
                ("a.sql".into(), RuleId::Pgm401, Severity::Minor, 1, vec![]),
                ("a.sql".into(), RuleId::Pgm401, Severity::Minor, 2, vec![]),
                ("a.sql".into(), RuleId::Pgm401, Severity::Minor, 3, vec![]),
                ("a.sql".into(), RuleId::Pgm401, Severity::Minor, 4, vec![]),
            ]
        );
    }

    #[test]
    fn group_at_threshold_merges_into_one_finding() {
        let findings = drops_without_if_exists("a.sql", &[1, 2, 3, 4, 5]);

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(grouped.len(), 1);
        let group = &grouped[0];
        assert_eq!((group.start_line, group.end_line), (1, 1));
        assert_eq!(group.message, "PGM401 at line 1");
        assert_eq!(
            group.additional_locations,
            vec![
                location(2, "PGM401 at line 2"),
                location(3, "PGM401 at line 3"),
                location(4, "PGM401 at line 4"),
                location(5, "PGM401 at line 5"),
            ]
        );
    }

    #[test]
    fn group_above_threshold_lists_every_other_statement() {
        let lines: Vec<usize> = (1..=20).collect();
        let findings = drops_without_if_exists("a.sql", &lines);

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![(
                "a.sql".into(),
                RuleId::Pgm401,
                Severity::Minor,
                1,
                (2..=20).collect()
            )]
        );
    }

    #[test]
    fn group_keeps_the_rule_severity() {
        let findings = drops_without_if_exists("a.sql", &[1, 2, 3, 4, 5]);

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(grouped[0].severity, Severity::Minor);
        assert_eq!(grouped[0].rule_id, RuleId::Pgm401);
    }

    #[test]
    fn group_counts_per_file() {
        let mut findings = drops_without_if_exists("a.sql", &[1, 2, 3]);
        findings.extend(drops_without_if_exists("b.sql", &[1, 2, 3]));

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(grouped.len(), 6);
        assert!(grouped.iter().all(|f| f.additional_locations.is_empty()));
    }

    #[test]
    fn group_makes_one_finding_per_file() {
        let mut findings = drops_without_if_exists("a.sql", &[1, 2, 3, 4, 5]);
        findings.extend(drops_without_if_exists("b.sql", &[7, 8, 9, 10, 11]));

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![
                (
                    "a.sql".into(),
                    RuleId::Pgm401,
                    Severity::Minor,
                    1,
                    vec![2, 3, 4, 5]
                ),
                (
                    "b.sql".into(),
                    RuleId::Pgm401,
                    Severity::Minor,
                    7,
                    vec![8, 9, 10, 11]
                ),
            ]
        );
    }

    #[test]
    fn group_counts_per_rule_and_leaves_other_rules_in_place() {
        let findings = vec![
            finding_in("a.sql", RuleId::Pgm401, Severity::Minor, 1),
            finding_in("a.sql", RuleId::Pgm201, Severity::Minor, 1),
            finding_in("a.sql", RuleId::Pgm401, Severity::Minor, 2),
            finding_in("a.sql", RuleId::Pgm401, Severity::Minor, 3),
            finding_in("a.sql", RuleId::Pgm201, Severity::Minor, 3),
            finding_in("a.sql", RuleId::Pgm401, Severity::Minor, 4),
            finding_in("a.sql", RuleId::Pgm401, Severity::Minor, 5),
            finding_in("a.sql", RuleId::Pgm001, Severity::Critical, 6),
        ];

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![
                (
                    "a.sql".into(),
                    RuleId::Pgm401,
                    Severity::Minor,
                    1,
                    vec![2, 3, 4, 5]
                ),
                ("a.sql".into(), RuleId::Pgm201, Severity::Minor, 1, vec![]),
                ("a.sql".into(), RuleId::Pgm201, Severity::Minor, 3, vec![]),
                (
                    "a.sql".into(),
                    RuleId::Pgm001,
                    Severity::Critical,
                    6,
                    vec![]
                ),
            ]
        );
    }

    #[test]
    fn group_never_merges_findings_of_different_severities() {
        let findings: Vec<Finding> = (1..=10)
            .map(|line| {
                let severity = if line % 2 == 0 {
                    Severity::Info
                } else {
                    Severity::Minor
                };
                finding_in("a.sql", RuleId::Pgm006, severity, line)
            })
            .collect();

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![
                (
                    "a.sql".into(),
                    RuleId::Pgm006,
                    Severity::Minor,
                    1,
                    vec![3, 5, 7, 9]
                ),
                (
                    "a.sql".into(),
                    RuleId::Pgm006,
                    Severity::Info,
                    2,
                    vec![4, 6, 8, 10]
                ),
            ]
        );
    }

    #[test]
    fn group_counts_each_severity_against_the_threshold_separately() {
        let findings: Vec<Finding> = (1..=6)
            .map(|line| {
                let severity = if line <= 3 {
                    Severity::Minor
                } else {
                    Severity::Info
                };
                finding_in("a.sql", RuleId::Pgm006, severity, line)
            })
            .collect();

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(grouped.len(), 6);
    }

    #[test]
    fn group_primary_location_is_the_lowest_line() {
        let findings = drops_without_if_exists("a.sql", &[9, 3, 7, 1, 5]);

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(grouped.len(), 1);
        assert_eq!(grouped[0].start_line, 1);
        assert_eq!(grouped[0].message, "PGM401 at line 1");
        assert_eq!(
            grouped[0].additional_locations,
            vec![
                location(3, "PGM401 at line 3"),
                location(5, "PGM401 at line 5"),
                location(7, "PGM401 at line 7"),
                location(9, "PGM401 at line 9"),
            ]
        );
    }

    #[test]
    fn group_keeps_each_member_line_span() {
        let findings: Vec<Finding> = [(1, 3), (4, 4), (5, 9), (10, 10), (11, 12)]
            .iter()
            .map(|&(start, end)| {
                Finding::new(
                    RuleId::Pgm401,
                    Severity::Minor,
                    "m".to_string(),
                    Path::new("a.sql"),
                    &SourceSpan::at(start, end),
                )
            })
            .collect();

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!((grouped[0].start_line, grouped[0].end_line), (1, 3));
        let spans: Vec<(usize, usize)> = grouped[0]
            .additional_locations
            .iter()
            .map(|l| (l.start_line, l.end_line))
            .collect();
        assert_eq!(spans, vec![(4, 4), (5, 9), (10, 10), (11, 12)]);
    }

    #[test]
    fn group_keeps_findings_that_share_a_span_as_separate_locations() {
        let findings = vec![
            finding_in("a.sql", RuleId::Pgm101, Severity::Minor, 3),
            finding_in("a.sql", RuleId::Pgm101, Severity::Minor, 3),
            finding_in("a.sql", RuleId::Pgm101, Severity::Minor, 15),
            finding_in("a.sql", RuleId::Pgm101, Severity::Minor, 15),
            finding_in("a.sql", RuleId::Pgm101, Severity::Minor, 22),
        ];

        let grouped = group_repeated_findings(findings, 5);

        assert_eq!(
            summarize(&grouped),
            vec![(
                "a.sql".into(),
                RuleId::Pgm101,
                Severity::Minor,
                3,
                vec![3, 15, 15, 22]
            )]
        );
    }

    #[test]
    fn group_threshold_zero_disables_grouping() {
        let lines: Vec<usize> = (1..=20).collect();
        let findings = drops_without_if_exists("a.sql", &lines);

        let grouped = group_repeated_findings(findings, 0);

        assert_eq!(grouped.len(), 20);
        assert!(grouped.iter().all(|f| f.additional_locations.is_empty()));
    }

    #[test]
    fn group_threshold_one_makes_one_finding_per_rule_per_file() {
        let mut findings = drops_without_if_exists("a.sql", &[1, 2]);
        findings.push(finding_in("a.sql", RuleId::Pgm201, Severity::Minor, 1));

        let grouped = group_repeated_findings(findings, 1);

        assert_eq!(
            summarize(&grouped),
            vec![
                ("a.sql".into(), RuleId::Pgm401, Severity::Minor, 1, vec![2]),
                ("a.sql".into(), RuleId::Pgm201, Severity::Minor, 1, vec![]),
            ]
        );
    }

    #[test]
    fn group_of_no_findings_is_empty() {
        assert!(group_repeated_findings(Vec::new(), 5).is_empty());
    }

    fn lint_drops_with_suppressions(source: &str) -> Vec<Finding> {
        let suppressions = crate::suppress::parse_suppressions(source);
        let statement_lines: Vec<usize> = source
            .lines()
            .enumerate()
            .filter(|(_, line)| line.starts_with("DROP TABLE"))
            .map(|(index, _)| index + 1)
            .collect();
        let mut findings = drops_without_if_exists("a.sql", &statement_lines);
        findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));
        dedup_findings(&mut findings);
        group_repeated_findings(findings, 5)
    }

    #[test]
    fn suppressed_statement_is_not_a_group_member() {
        let source = "\
DROP TABLE t1;
DROP TABLE t2;
-- pgm-lint:suppress PGM401
DROP TABLE t3;
DROP TABLE t4;
DROP TABLE t5;
DROP TABLE t6;
DROP TABLE t7;
";

        let findings = lint_drops_with_suppressions(source);

        assert_eq!(
            summarize(&findings),
            vec![(
                "a.sql".into(),
                RuleId::Pgm401,
                Severity::Minor,
                1,
                vec![2, 5, 6, 7, 8]
            )]
        );
    }

    #[test]
    fn suppressed_statement_does_not_count_toward_the_threshold() {
        let source = "\
DROP TABLE t1;
DROP TABLE t2;
-- pgm-lint:suppress PGM401
DROP TABLE t3;
DROP TABLE t4;
DROP TABLE t5;
";

        let findings = lint_drops_with_suppressions(source);

        assert_eq!(findings.len(), 4);
        assert!(findings.iter().all(|f| f.additional_locations.is_empty()));
    }

    #[test]
    fn file_level_suppression_removes_the_whole_group() {
        let source = "\
-- pgm-lint:suppress-file PGM401
DROP TABLE t1;
DROP TABLE t2;
DROP TABLE t3;
DROP TABLE t4;
DROP TABLE t5;
";

        assert!(lint_drops_with_suppressions(source).is_empty());
    }

    #[test]
    fn locations_lists_the_primary_location_first() {
        let lines: Vec<usize> = vec![4, 8, 15, 16, 23];
        let grouped = group_repeated_findings(drops_without_if_exists("a.sql", &lines), 5);

        let locations: Vec<FindingLocation> = grouped[0].locations().collect();

        assert_eq!(
            locations,
            lines
                .iter()
                .map(|&line| location(line, &format!("PGM401 at line {line}")))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn ungrouped_finding_serializes_without_additional_locations() {
        let value = serde_json::to_value(make_finding(RuleId::Pgm001, None, 1)).expect("json");

        assert!(value.get("additional_locations").is_none());
    }
}
