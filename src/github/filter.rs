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

#[cfg(test)]
mod tests;

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
/// Deliberately has no "tool error" variant: a genuine tool error (bad
/// config, I/O failure) is just an `Err` propagated before any `Finding`s
/// exist (see [`super::run`]), handled as a normal Rust error path rather
/// than a value this enum needs to represent.
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
    /// cases are indistinguishable from the hunk map alone, so both get
    /// this one reason rather than a false claim of precision.
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
