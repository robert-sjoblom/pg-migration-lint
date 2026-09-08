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

/// A normalizer whose repository root and working directory are the
/// same (non-existent, so canonicalization is a no-op) directory --
/// i.e. the default `working-directory: .` shape, where a relative
/// finding path and a relative GitHub path resolve identically.
fn test_paths() -> PathNormalizer {
    PathNormalizer::new(Path::new("/repo"), Path::new("/repo"))
}

#[test]
fn finding_in_file_with_no_hunk_entry_is_outside_diff() {
    let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 5)];
    let hunks: HashMap<PathBuf, Vec<LineRange>> = HashMap::new();

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert!(inline.is_empty());
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].reason, SummaryReason::OutsideDiff);
}

#[test]
fn finding_in_file_with_empty_hunk_list_is_patch_too_large() {
    let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 5)];
    let hunks = hunks_for("a.sql", vec![]);

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert!(inline.is_empty());
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].reason, SummaryReason::PatchTooLarge);
}

#[test]
fn single_line_finding_inside_a_hunk_is_inline() {
    let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 12)];
    let hunks = hunks_for("a.sql", vec![LineRange { start: 10, end: 16 }]);

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    assert!(summary.is_empty());
    assert_eq!(inline[0].finding.start_line, 12);
}

#[test]
fn single_line_finding_outside_every_hunk_is_outside_diff() {
    let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 50)];
    let hunks = hunks_for("a.sql", vec![LineRange { start: 10, end: 16 }]);

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

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

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

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

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    assert!(summary.is_empty());
}

/// A multi-line finding that fully contains a hunk (hunk nested inside
/// the finding's span) must also overlap.
#[test]
fn multi_line_finding_fully_containing_a_hunk_is_inline() {
    let findings = vec![finding_spanning(RuleId::Pgm501, "d.sql", 1, 100)];
    let hunks = hunks_for("d.sql", vec![LineRange { start: 40, end: 41 }]);

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

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

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

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

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

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

/// The C1 regression, at unit level: a finding whose path carries the
/// `./` prefix `Config::from_file` produces for the default
/// bare-filename config lookup must still match the plain,
/// never-`./`-prefixed key GitHub's Files API uses. Before
/// [`PathNormalizer`], `Path`'s `Eq`/`Hash` judged these two unequal
/// (a leading `./` is a real `Component::CurDir`) and every finding
/// silently fell through to `OutsideDiff`.
#[test]
fn dot_slash_prefixed_finding_path_matches_its_github_hunk_entry() {
    let findings = vec![finding_at(
        RuleId::Pgm001,
        Severity::Critical,
        "./db/migrations/001.sql",
        5,
    )];
    let hunks = hunks_for(
        "db/migrations/001.sql",
        vec![LineRange { start: 1, end: 9 }],
    );

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1, "a './' prefix must not defeat the lookup");
    assert!(summary.is_empty());
    assert_eq!(
        inline[0].path, "db/migrations/001.sql",
        "the posted path must be GitHub's own spelling, not the finding's"
    );
}

/// The absolute-`--config` variant of the same bug: with
/// `--config /abs/pg-migration-lint.toml` (the `${{ github.workspace }}`
/// Actions idiom), `Config::resolve_paths` makes every migration path
/// absolute, and an absolute path can never equal a repo-relative one.
#[test]
fn absolute_finding_path_matches_its_repo_relative_github_hunk_entry() {
    let findings = vec![finding_at(
        RuleId::Pgm001,
        Severity::Critical,
        "/repo/db/migrations/001.sql",
        5,
    )];
    let hunks = hunks_for(
        "db/migrations/001.sql",
        vec![LineRange { start: 1, end: 9 }],
    );

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    assert!(summary.is_empty());
    assert_eq!(inline[0].path, "db/migrations/001.sql");
}

/// The `working-directory` variant: the process runs from a
/// subdirectory of the repository, so a finding's path is relative to
/// that subdirectory while GitHub's path stays relative to the
/// repository root.
#[test]
fn finding_under_a_non_default_working_directory_matches_its_github_hunk_entry() {
    let paths = PathNormalizer::new(Path::new("/repo"), Path::new("/repo/backend"));
    let findings = vec![finding_at(
        RuleId::Pgm001,
        Severity::Critical,
        "./db/001.sql",
        5,
    )];
    let hunks = hunks_for("backend/db/001.sql", vec![LineRange { start: 1, end: 9 }]);

    let (inline, summary) = split_findings(&findings, &hunks, &paths);

    assert_eq!(inline.len(), 1);
    assert!(summary.is_empty());
    assert_eq!(inline[0].path, "backend/db/001.sql");
}

/// A finding in a genuinely different file must still not match, even
/// though both sides now go through the normalizer -- normalization
/// must not make unrelated paths collide.
#[test]
fn normalization_does_not_make_different_files_match() {
    let findings = vec![finding_at(
        RuleId::Pgm001,
        Severity::Critical,
        "./db/migrations/001.sql",
        5,
    )];
    let hunks = hunks_for(
        "db/migrations/002.sql",
        vec![LineRange { start: 1, end: 9 }],
    );

    let (inline, summary) = split_findings(&findings, &hunks, &test_paths());

    assert!(inline.is_empty());
    assert_eq!(summary.len(), 1);
    assert_eq!(summary[0].reason, SummaryReason::OutsideDiff);
}

/// The C2 regression, in the exact shape
/// `multi_line_finding_whose_start_overlaps_a_hunk_is_inline` already
/// encoded: the finding spans 10-20, the hunk covers 5-12, so the two
/// overlap (at 10-12) and the finding is inline-eligible -- but its
/// `end_line` (20) is outside every hunk. Posting there would make
/// GitHub reject the whole batched review with a 422; the carried
/// anchor must be inside the hunk instead.
#[test]
fn anchor_line_of_a_finding_ending_past_its_hunk_is_inside_the_hunk() {
    let findings = vec![finding_spanning(RuleId::Pgm501, "c.sql", 10, 20)];
    let hunk = LineRange { start: 5, end: 12 };
    let hunks = hunks_for("c.sql", vec![hunk]);

    let (inline, _) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    let anchor = inline[0].anchor_line;
    assert!(
        anchor >= hunk.start && anchor <= hunk.end,
        "anchor {anchor} must fall inside the matched hunk {hunk:?}"
    );
    assert_eq!(anchor, 10, "the finding's own start line is postable here");
    assert_ne!(
        anchor, inline[0].finding.end_line,
        "the anchor must not be end_line, which is outside the hunk"
    );
}

/// Mirror case: the finding *starts* before the hunk and ends inside
/// it, so its `start_line` is the unpostable end. The anchor clamps up
/// to the hunk's first line.
#[test]
fn anchor_line_of_a_finding_starting_before_its_hunk_is_inside_the_hunk() {
    let findings = vec![finding_spanning(RuleId::Pgm503, "b.sql", 30, 39)];
    let hunk = LineRange { start: 38, end: 45 };
    let hunks = hunks_for("b.sql", vec![hunk]);

    let (inline, _) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0].anchor_line, 38);
}

/// A finding that fully contains a small hunk: neither endpoint is
/// inside it, so the anchor has to clamp from both sides.
#[test]
fn anchor_line_of_a_finding_containing_its_hunk_is_inside_the_hunk() {
    let findings = vec![finding_spanning(RuleId::Pgm501, "d.sql", 1, 100)];
    let hunks = hunks_for("d.sql", vec![LineRange { start: 40, end: 41 }]);

    let (inline, _) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0].anchor_line, 40);
}

/// A single-line finding inside a hunk anchors at its own line -- the
/// clamp must be a no-op for the common case.
#[test]
fn anchor_line_of_a_single_line_finding_is_its_own_line() {
    let findings = vec![finding_at(RuleId::Pgm001, Severity::Critical, "a.sql", 12)];
    let hunks = hunks_for("a.sql", vec![LineRange { start: 10, end: 16 }]);

    let (inline, _) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline[0].anchor_line, 12);
}

/// With several hunks in a file, the first overlapping one wins -- the
/// same hunk the eligibility check itself stops at -- and the anchor
/// lands in *that* hunk.
#[test]
fn anchor_line_uses_the_first_overlapping_hunk() {
    let findings = vec![finding_spanning(RuleId::Pgm501, "e.sql", 1, 100)];
    let hunks = hunks_for(
        "e.sql",
        vec![
            LineRange { start: 20, end: 25 },
            LineRange { start: 60, end: 70 },
        ],
    );

    let (inline, _) = split_findings(&findings, &hunks, &test_paths());

    assert_eq!(inline[0].anchor_line, 20);
}

#[test]
fn without_curdir_components_strips_a_leading_dot_slash() {
    assert_eq!(
        without_curdir_components(Path::new("./db/001.sql")),
        PathBuf::from("db/001.sql")
    );
}

#[test]
fn without_curdir_components_leaves_a_plain_relative_path_alone() {
    assert_eq!(
        without_curdir_components(Path::new("db/001.sql")),
        PathBuf::from("db/001.sql")
    );
}

#[test]
fn without_curdir_components_leaves_an_absolute_path_absolute() {
    assert_eq!(
        without_curdir_components(Path::new("/repo/db/001.sql")),
        PathBuf::from("/repo/db/001.sql")
    );
}

#[test]
fn without_curdir_components_of_a_bare_dot_stays_a_dot() {
    assert_eq!(
        without_curdir_components(Path::new(".")),
        PathBuf::from(".")
    );
}

#[test]
fn from_env_values_uses_github_workspace_as_the_repo_root() {
    let paths = PathNormalizer::from_env_values(
        Some(PathBuf::from("/repo/backend")),
        Some("/repo".to_string()),
    );

    assert_eq!(
        paths.normalize_finding_path(Path::new("./db/001.sql")),
        PathBuf::from("/repo/backend/db/001.sql")
    );
    assert_eq!(
        paths.normalize_github_path(Path::new("backend/db/001.sql")),
        PathBuf::from("/repo/backend/db/001.sql")
    );
}

#[test]
fn from_env_values_falls_back_to_the_working_directory_as_the_repo_root() {
    let paths = PathNormalizer::from_env_values(Some(PathBuf::from("/repo")), None);

    assert_eq!(
        paths.normalize_finding_path(Path::new("./db/001.sql")),
        paths.normalize_github_path(Path::new("db/001.sql")),
        "outside Actions both bases are the same directory"
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
