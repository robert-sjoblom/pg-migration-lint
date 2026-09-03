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
use std::path::{Component, Path, PathBuf};

use pg_migration_lint::{Finding, RuleId};

use super::files::LineRange;

/// A finding that lands inside the PR's diff and can be posted as an inline
/// PR review comment.
#[derive(Debug, Clone)]
pub struct InlineEntry {
    /// The finding to post inline.
    pub finding: Finding,
    /// The path to post the comment against: GitHub's own repo-root-relative
    /// `filename` for the hunk this finding matched, carried verbatim from
    /// the `hunks` map's key rather than re-derived from `finding.file`.
    ///
    /// `finding.file` can be `./`-prefixed or absolute (see
    /// [`PathNormalizer`]); GitHub's "create a review" endpoint only accepts
    /// a comment `path` that exactly matches a file in the PR's diff, so
    /// posting anything but the API's own spelling of the path would 422.
    pub path: String,
    /// The line to anchor the inline comment at, clamped into the diff hunk
    /// this finding matched.
    ///
    /// Inline-eligibility is decided by *interval overlap* (any part of the
    /// finding's `start_line..=end_line` span touching any part of a hunk),
    /// but GitHub anchors a review comment at a single line, and rejects the
    /// entire batched review with a 422 if that line falls outside the diff.
    /// A finding spanning lines 10-20 against a hunk covering 5-12 is
    /// legitimately inline-eligible, yet neither its `start_line` (10, fine
    /// here) nor its `end_line` (20, outside every hunk) is guaranteed to be
    /// postable -- so the anchor is computed as
    /// `start_line.max(hunk.start).min(hunk.end)`, which is always inside
    /// the matched hunk.
    pub anchor_line: usize,
}

/// Maps the two differently-shaped path vocabularies this subcommand has to
/// reconcile onto one comparable form.
///
/// A [`Finding`]'s `file` comes from `unit.source_file`, i.e. from
/// `config.migrations.paths` after `Config::resolve_paths` has joined each
/// entry onto the config file's own directory. That makes it relative to the
/// process's working directory, and it can additionally be:
/// - `./`-prefixed -- `Config::from_file` falls back to `Path::new(".")`
///   whenever the config path's `parent()` is empty, which is exactly what
///   the default bare `pg-migration-lint.toml` lookup (and any bare
///   `config-path` input) produces, so `db/migrations` becomes
///   `./db/migrations`;
/// - absolute -- `--config /abs/path/pg-migration-lint.toml`, the common
///   `${{ github.workspace }}/...` GitHub Actions idiom.
///
/// The `hunks` map's keys, by contrast, are GitHub's own `filename`s: always
/// repo-root-relative, never `./`-prefixed, never absolute.
///
/// `Path`'s `Eq`/`Hash` treat a leading `./` as a real [`Component::CurDir`],
/// so `./db/001.sql != db/001.sql`, and an absolute path can never equal a
/// repo-relative one. Both sides of [`split_findings`]'s lookup therefore go
/// through this type, which resolves each to one absolute path: finding
/// paths against the working directory, GitHub paths against the repository
/// root. Resolving (rather than stripping to a common relative form) is what
/// also makes a non-default `working-directory` work -- there, the process's
/// CWD is a subdirectory of the repository root, and only re-anchoring both
/// sides at their own base lines the two up.
#[derive(Debug, Clone)]
pub struct PathNormalizer {
    /// Repository root, i.e. the directory GitHub's API paths are relative
    /// to.
    repo_root: PathBuf,
    /// Directory a [`Finding`]'s relative path is resolved against: the
    /// process's own working directory.
    working_dir: PathBuf,
}

impl PathNormalizer {
    /// Builds a normalizer from the process environment: the working
    /// directory from [`std::env::current_dir`], and the repository root
    /// from `$GITHUB_WORKSPACE` (set by the GitHub Actions runner), falling
    /// back to the working directory when that variable is absent -- which
    /// is the correct answer outside Actions, and also whenever the action
    /// runs with its default `working-directory: .`.
    ///
    /// Both are canonicalized when possible so a symlinked workspace can't
    /// make the two bases disagree about the same real directory; a
    /// canonicalization failure just leaves the path as given.
    pub fn from_env() -> Self {
        Self::from_env_values(
            std::env::current_dir().ok(),
            std::env::var("GITHUB_WORKSPACE").ok(),
        )
    }

    /// [`Self::from_env`]'s logic with the two environment reads passed in
    /// as plain parameters, so tests can exercise both branches without
    /// mutating process-global state -- the same split
    /// [`super::resolve_pr`], [`super::resolve_repo`], and
    /// [`super::resolve_token`] already use.
    fn from_env_values(current_dir: Option<PathBuf>, github_workspace: Option<String>) -> Self {
        let working_dir = current_dir.unwrap_or_else(|| PathBuf::from("."));
        let repo_root = github_workspace
            .map(PathBuf::from)
            .unwrap_or_else(|| working_dir.clone());

        Self::new(&repo_root, &working_dir)
    }

    /// Builds a normalizer with explicit bases, canonicalizing each when
    /// possible. Tests construct this directly rather than mutating
    /// process-global environment state.
    pub fn new(repo_root: &Path, working_dir: &Path) -> Self {
        Self {
            repo_root: canonicalize_or_keep(repo_root),
            working_dir: canonicalize_or_keep(working_dir),
        }
    }

    /// Normalizes a [`Finding`]'s own `file` path: strips any `./` prefix
    /// and resolves a relative path against the working directory.
    pub fn normalize_finding_path(&self, path: &Path) -> PathBuf {
        self.resolve_against(path, &self.working_dir)
    }

    /// Normalizes a path as GitHub's API reports it (a `hunks` key, always
    /// repo-root-relative) onto the same absolute form
    /// [`Self::normalize_finding_path`] produces.
    pub fn normalize_github_path(&self, path: &Path) -> PathBuf {
        self.resolve_against(path, &self.repo_root)
    }

    /// Shared core of the two `normalize_*` methods: drops `./`
    /// components, resolves a relative path against `base`, and
    /// canonicalizes the result when the file exists.
    ///
    /// Canonicalizing the *result* (not just the bases) matters because a
    /// finding's path can already be absolute -- spelled however the
    /// `--config` value that produced it was spelled -- while the GitHub
    /// side is always built from `base`. Both sides name the same file on
    /// disk, so resolving both to its real path is what makes them
    /// comparable. A path that can't be canonicalized (a file deleted by
    /// the pull request, say) keeps its resolved form, which still matches
    /// as long as both sides share a base.
    fn resolve_against(&self, path: &Path, base: &Path) -> PathBuf {
        let cleaned = without_curdir_components(path);
        let resolved = if cleaned.is_absolute() {
            cleaned
        } else {
            base.join(cleaned)
        };

        canonicalize_or_keep(&resolved)
    }
}

/// Returns `path`'s canonical form, or `path` unchanged if it can't be
/// canonicalized (it doesn't exist yet, or the process can't stat it).
fn canonicalize_or_keep(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Drops every [`Component::CurDir`] from `path`, turning `./db/001.sql`
/// into `db/001.sql` while leaving everything else (including a leading
/// `/`) untouched.
///
/// [`Path::components`] already normalizes away *interior* `.` components,
/// so in practice this only ever removes a leading one. It also covers the
/// `.\`-prefixed spelling for free on Windows, where the standard library
/// treats `\` as a separator and so yields the same [`Component::CurDir`];
/// on Unix `.\foo` is a single, genuinely-named component and is
/// deliberately left alone.
fn without_curdir_components(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            cleaned.push(component);
        }
    }

    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
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
/// For each finding, its file is looked up in `hunks` -- both sides of that
/// lookup normalized through `paths` first, since the two come from
/// different path vocabularies that `Path`'s `Eq`/`Hash` would otherwise
/// (silently, for every finding) judge unequal; see [`PathNormalizer`]:
/// - No entry for that file, or an entry whose ranges don't overlap the
///   finding's line span -> [`SummaryReason::OutsideDiff`].
/// - An entry present but empty (`[]`) -> [`SummaryReason::PatchTooLarge`].
/// - An entry with at least one range overlapping the finding's line span
///   -> inline, carrying that hunk's own GitHub path and an anchor line
///   clamped into it (see [`InlineEntry`]).
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
/// outside the hunk. When several hunks overlap the same finding, the first
/// one in the file's hunk list wins -- the same hunk the eligibility check
/// itself stops at.
pub fn split_findings(
    findings: &[Finding],
    hunks: &HashMap<PathBuf, Vec<LineRange>>,
    paths: &PathNormalizer,
) -> (Vec<InlineEntry>, Vec<SummaryEntry>) {
    // Key every hunk entry by its normalized path once, keeping the original
    // GitHub key alongside it: that key is what an inline comment has to be
    // posted against, and it's the one spelling of the path GitHub is
    // guaranteed to accept.
    let by_normalized_path: HashMap<PathBuf, (&PathBuf, &Vec<LineRange>)> = hunks
        .iter()
        .map(|(github_path, ranges)| {
            (
                paths.normalize_github_path(github_path),
                (github_path, ranges),
            )
        })
        .collect();

    let mut inline = Vec::new();
    let mut summary = Vec::new();

    for finding in findings {
        let normalized = paths.normalize_finding_path(&finding.file);

        match by_normalized_path.get(&normalized) {
            None => summary.push(SummaryEntry {
                finding: finding.clone(),
                reason: SummaryReason::OutsideDiff,
            }),
            Some((_, ranges)) if ranges.is_empty() => summary.push(SummaryEntry {
                finding: finding.clone(),
                reason: SummaryReason::PatchTooLarge,
            }),
            Some((github_path, ranges)) => {
                let matched_hunk = ranges
                    .iter()
                    .find(|hunk| hunk.start <= finding.end_line && hunk.end >= finding.start_line);

                match matched_hunk {
                    Some(hunk) => inline.push(InlineEntry {
                        finding: finding.clone(),
                        path: super::github_path(github_path),
                        anchor_line: finding.start_line.max(hunk.start).min(hunk.end),
                    }),
                    None => summary.push(SummaryEntry {
                        finding: finding.clone(),
                        reason: SummaryReason::OutsideDiff,
                    }),
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
}
