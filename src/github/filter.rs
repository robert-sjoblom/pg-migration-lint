//! Finding filtering for the `github-review` subcommand.
//!
//! [`split_findings`] takes the lint pipeline's `Vec<Finding>` (see
//! [`super::run`]) and [`super::files`]'s per-file diff-hunk ranges, and
//! splits findings into inline-eligible (the finding's line range overlaps a
//! diff hunk for that file, so it can be posted as an inline PR review
//! comment) versus summary-only (everything else, tagged with why -- see
//! [`SummaryReason`]). It also computes the severity-count table and unique
//! rule-id list Task 5's summary comment needs.
//!
//! This is deliberately pure (no I/O, no async) so it's the most
//! straightforwardly testable part of the `github-review` subcommand --
//! correctness here directly determines which findings a PR author sees
//! inline vs. buried in a summary comment.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use pg_migration_lint::{Finding, RuleId};

use super::files::LineRange;

/// A finding that lands inside the PR's diff and can be posted as an inline
/// PR review comment.
#[derive(Debug, Clone)]
pub struct InlineEntry {
    /// The finding to post inline.
    pub finding: Finding,
}

/// A finding that can't be posted as an inline PR review comment, tagged
/// with why -- so the summary comment can explain itself instead of just
/// silently listing findings the PR author might expect to see inline.
#[derive(Debug, Clone)]
pub struct SummaryEntry {
    /// The finding to mention in the summary comment.
    pub finding: Finding,
    /// Why this finding didn't get an inline comment.
    pub reason: SummaryReason,
}

/// Why a finding was routed to the summary comment instead of posted
/// inline.
///
/// This is the two-reason survivor of the deleted bash implementation's
/// three-reason design: its third reason, "the linter reported a tool
/// error, not a finding" (its exit-2 case), doesn't apply here -- a genuine
/// tool error (bad config, I/O failure) is now just an `Err` propagated
/// before any `Finding`s exist (see [`super::run`]), handled as a normal
/// Rust error path rather than a value this enum needs to represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryReason {
    /// This finding's line range doesn't fall inside any part of the PR's
    /// diff for its file. Covers two cases that are indistinguishable
    /// without per-line blame data, and don't need to be distinguished:
    /// the file has no entry in the hunk map at all (in practice this
    /// shouldn't happen for this subcommand, since it only lints
    /// PR-changed files -- handled defensively regardless), and the file
    /// does have hunks, but none of them overlap this finding.
    OutsideDiff,
    /// The file's hunk list is present but empty: GitHub omitted its
    /// `patch` field (typically a very large diff), or every hunk in it
    /// was a pure-deletion hunk contributing zero new-file lines. These two
    /// cases are indistinguishable from the hunk map alone -- confirmed
    /// unavoidable in the deleted bash implementation's review -- so both
    /// get this one reason rather than a false claim of precision.
    PatchTooLarge,
}

/// Splits `findings` into inline-eligible and summary-only buckets using
/// `hunks` (Task 3's per-file diff-hunk ranges, see
/// [`super::files::fetch_changed_files_and_hunks`]).
///
/// For each finding, its file is looked up in `hunks`:
/// - No entry for that file, or an entry whose ranges don't overlap the
///   finding's line span -> [`SummaryReason::OutsideDiff`].
/// - An entry present but empty (`[]`) -> [`SummaryReason::PatchTooLarge`].
/// - An entry with at least one range overlapping the finding's line span
///   -> inline.
///
/// Overlap is checked as an **interval overlap**, not naive
/// `start_line`-only matching: a finding spanning multiple lines
/// (`start_line..=end_line`) is inline-eligible if *any* hunk range
/// overlaps *any part* of that span (`hunk.start <= finding.end_line &&
/// hunk.end >= finding.start_line`). This matters because a multi-line
/// finding whose start is outside a hunk but whose end is inside it must
/// still land inline -- e.g. a PGM503-shaped finding spanning lines 30-39
/// against a hunk covering lines 38-45 overlaps (at lines 38-39) and is
/// inline-eligible, even though `finding.start_line` (30) itself falls
/// outside the hunk.
pub fn split_findings(
    findings: &[Finding],
    hunks: &HashMap<PathBuf, Vec<LineRange>>,
) -> (Vec<InlineEntry>, Vec<SummaryEntry>) {
    let mut inline = Vec::new();
    let mut summary = Vec::new();

    for finding in findings {
        match hunks.get(&finding.file) {
            None => summary.push(SummaryEntry {
                finding: finding.clone(),
                reason: SummaryReason::OutsideDiff,
            }),
            Some(ranges) if ranges.is_empty() => summary.push(SummaryEntry {
                finding: finding.clone(),
                reason: SummaryReason::PatchTooLarge,
            }),
            Some(ranges) => {
                let overlaps_a_hunk = ranges
                    .iter()
                    .any(|hunk| hunk.start <= finding.end_line && hunk.end >= finding.start_line);

                if overlaps_a_hunk {
                    inline.push(InlineEntry {
                        finding: finding.clone(),
                    });
                } else {
                    summary.push(SummaryEntry {
                        finding: finding.clone(),
                        reason: SummaryReason::OutsideDiff,
                    });
                }
            }
        }
    }

    (inline, summary)
}

/// Finding counts bucketed by the SARIF severity level
/// ([`pg_migration_lint::output::sarif_level`]) each finding's `Severity`
/// maps to -- the same three buckets (`error`/`warning`/`note`) the SARIF
/// report itself uses, so the PR summary comment's severity table never
/// disagrees with `findings.sarif`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SeverityCounts {
    /// Findings at `Blocker` or `Critical` severity.
    pub error: usize,
    /// Findings at `Major` severity.
    pub warning: usize,
    /// Findings at `Minor` or `Info` severity.
    pub note: usize,
}

/// Computes the severity-count table for the PR summary comment, across
/// every finding (inline and summary alike).
pub fn count_by_severity(findings: &[Finding]) -> SeverityCounts {
    let mut counts = SeverityCounts::default();

    for finding in findings {
        match pg_migration_lint::output::sarif_level(&finding.severity) {
            "error" => counts.error += 1,
            "warning" => counts.warning += 1,
            _ => counts.note += 1,
        }
    }

    counts
}

/// Returns the unique [`RuleId`]s that fired across `findings` (inline and
/// summary alike), sorted by declaration order (via `RuleId`'s derived
/// `Ord`). Used for Task 5's `--explain` lookup, now a direct function call
/// instead of a subprocess.
pub fn unique_rule_ids(findings: &[Finding]) -> Vec<RuleId> {
    findings
        .iter()
        .map(|f| f.rule_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_migration_lint::Rule as _;
    use pg_migration_lint::Severity;
    use pg_migration_lint::parser::SourceSpan;
    use std::path::Path;

    fn finding_at(rule_id: RuleId, severity: Severity, file: &str, line: usize) -> Finding {
        Finding::new(
            rule_id,
            severity,
            format!("{rule_id} test finding"),
            Path::new(file),
            &SourceSpan::at(line, line),
        )
    }

    fn finding_spanning(rule_id: RuleId, file: &str, start: usize, end: usize) -> Finding {
        Finding::new(
            rule_id,
            rule_id.default_severity(),
            format!("{rule_id} test finding"),
            Path::new(file),
            &SourceSpan::at(start, end),
        )
    }

    fn hunks_for(file: &str, ranges: Vec<LineRange>) -> HashMap<PathBuf, Vec<LineRange>> {
        let mut hunks = HashMap::new();
        hunks.insert(PathBuf::from(file), ranges);
        hunks
    }

    #[test]
    fn finding_in_file_with_no_hunk_entry_is_outside_diff() {
        let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 5)];
        let hunks: HashMap<PathBuf, Vec<LineRange>> = HashMap::new();

        let (inline, summary) = split_findings(&findings, &hunks);

        assert!(inline.is_empty());
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].reason, SummaryReason::OutsideDiff);
    }

    #[test]
    fn finding_in_file_with_empty_hunk_list_is_patch_too_large() {
        let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 5)];
        let hunks = hunks_for("a.sql", vec![]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert!(inline.is_empty());
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].reason, SummaryReason::PatchTooLarge);
    }

    #[test]
    fn single_line_finding_inside_a_hunk_is_inline() {
        let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 12)];
        let hunks = hunks_for("a.sql", vec![LineRange { start: 10, end: 16 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert_eq!(inline.len(), 1);
        assert!(summary.is_empty());
        assert_eq!(inline[0].finding.start_line, 12);
    }

    #[test]
    fn single_line_finding_outside_every_hunk_is_outside_diff() {
        let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 50)];
        let hunks = hunks_for("a.sql", vec![LineRange { start: 10, end: 16 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert!(inline.is_empty());
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].reason, SummaryReason::OutsideDiff);
    }

    /// The multi-line interval-overlap case the brief calls out explicitly:
    /// a PGM503-shaped finding spanning lines 30-39, checked against a hunk
    /// covering lines 38-45. Naive `start_line`-only matching would miss
    /// this (30 isn't in 38..=45), but the finding's *end* (39) falls
    /// inside the hunk, so it must still be inline-eligible.
    #[test]
    fn multi_line_finding_whose_end_overlaps_a_hunk_is_inline() {
        let findings = vec![finding_spanning(RuleId::Pgm503, "b.sql", 30, 39)];
        let hunks = hunks_for("b.sql", vec![LineRange { start: 38, end: 45 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert_eq!(inline.len(), 1, "end-of-span overlap must count as inline");
        assert!(summary.is_empty());
    }

    /// Mirror case: the finding's *start* overlaps a hunk but its end
    /// extends past it -- also must be inline, for the same
    /// interval-overlap reason.
    #[test]
    fn multi_line_finding_whose_start_overlaps_a_hunk_is_inline() {
        let findings = vec![finding_spanning(RuleId::Pgm501, "c.sql", 10, 20)];
        let hunks = hunks_for("c.sql", vec![LineRange { start: 5, end: 12 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert_eq!(inline.len(), 1);
        assert!(summary.is_empty());
    }

    /// A multi-line finding that fully contains a hunk (hunk nested inside
    /// the finding's span) must also overlap.
    #[test]
    fn multi_line_finding_fully_containing_a_hunk_is_inline() {
        let findings = vec![finding_spanning(RuleId::Pgm501, "d.sql", 1, 100)];
        let hunks = hunks_for("d.sql", vec![LineRange { start: 40, end: 41 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert_eq!(inline.len(), 1);
        assert!(summary.is_empty());
    }

    /// A multi-line finding entirely between two hunks (touching neither)
    /// must not be inline, even though the file has non-empty hunks
    /// elsewhere.
    #[test]
    fn multi_line_finding_between_two_hunks_is_outside_diff() {
        let findings = vec![finding_spanning(RuleId::Pgm501, "e.sql", 20, 25)];
        let hunks = hunks_for(
            "e.sql",
            vec![
                LineRange { start: 1, end: 10 },
                LineRange { start: 30, end: 40 },
            ],
        );

        let (inline, summary) = split_findings(&findings, &hunks);

        assert!(inline.is_empty());
        assert_eq!(summary.len(), 1);
        assert_eq!(summary[0].reason, SummaryReason::OutsideDiff);
    }

    /// The named regression scenario: a changelog file (shaped like
    /// `tests/fixtures/repos/liquibase-multi-schema/changelog/003-same-name-ops.xml`)
    /// carries several pre-existing findings the PR didn't touch, plus one
    /// new finding on a line the PR's diff actually added. Only the new
    /// in-range finding should be inline; the rest must land in the
    /// summary as outside-diff, not silently vanish or wrongly go inline.
    #[test]
    fn named_regression_pre_existing_findings_summary_new_finding_inline() {
        let file = "changelog/003-same-name-ops.xml";
        let findings = vec![
            finding_at(RuleId::Pgm101, RuleId::Pgm101.default_severity(), file, 12),
            finding_at(RuleId::Pgm108, RuleId::Pgm108.default_severity(), file, 19),
            finding_at(RuleId::Pgm402, RuleId::Pgm402.default_severity(), file, 26),
            finding_at(RuleId::Pgm402, RuleId::Pgm402.default_severity(), file, 33),
        ];
        // The PR's diff only touches lines 30-35 (the new changeset that
        // introduces the line-33 finding).
        let hunks = hunks_for(file, vec![LineRange { start: 30, end: 35 }]);

        let (inline, summary) = split_findings(&findings, &hunks);

        assert_eq!(inline.len(), 1, "only the line-33 finding is in the diff");
        assert_eq!(inline[0].finding.start_line, 33);
        assert_eq!(inline[0].finding.rule_id, RuleId::Pgm402);

        assert_eq!(summary.len(), 3, "the three pre-existing findings");
        let summary_lines: BTreeSet<usize> = summary.iter().map(|s| s.finding.start_line).collect();
        assert_eq!(
            summary_lines,
            BTreeSet::from([12, 19, 26]),
            "pre-existing findings must all be routed to the summary"
        );
        assert!(
            summary
                .iter()
                .all(|s| s.reason == SummaryReason::OutsideDiff),
            "pre-existing findings are outside the diff, not patch-too-large"
        );
    }

    #[test]
    fn severity_counts_match_sarif_level_mapping() {
        let findings = vec![
            finding_at(RuleId::Pgm001, Severity::Blocker, "a.sql", 1),
            finding_at(RuleId::Pgm002, Severity::Critical, "a.sql", 2),
            finding_at(RuleId::Pgm501, Severity::Major, "a.sql", 3),
            finding_at(RuleId::Pgm502, Severity::Minor, "a.sql", 4),
            finding_at(RuleId::Pgm503, Severity::Info, "a.sql", 5),
        ];

        let counts = count_by_severity(&findings);

        assert_eq!(
            counts,
            SeverityCounts {
                error: 2,   // Blocker + Critical
                warning: 1, // Major
                note: 2,    // Minor + Info
            }
        );
    }

    #[test]
    fn severity_counts_of_no_findings_are_all_zero() {
        assert_eq!(count_by_severity(&[]), SeverityCounts::default());
    }

    #[test]
    fn unique_rule_ids_dedups_and_sorts() {
        let findings = vec![
            finding_at(RuleId::Pgm501, Severity::Major, "a.sql", 1),
            finding_at(RuleId::Pgm001, Severity::Critical, "b.sql", 2),
            finding_at(RuleId::Pgm501, Severity::Major, "c.sql", 3),
        ];

        let rule_ids = unique_rule_ids(&findings);

        assert_eq!(rule_ids, vec![RuleId::Pgm001, RuleId::Pgm501]);
    }

    #[test]
    fn unique_rule_ids_of_no_findings_is_empty() {
        assert!(unique_rule_ids(&[]).is_empty());
    }
}
