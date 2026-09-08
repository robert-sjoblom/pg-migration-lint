//! Finding filtering for the `github-review` subcommand.
//!
//! [`split_findings`] takes the lint pipeline's `Vec<Finding>` and
//! [`super::files`]'s per-file diff-hunk ranges, and splits findings into
//! inline-eligible (posted as an inline PR review comment) versus
//! summary-only (tagged with why -- see [`SummaryReason`]). Also computes
//! the severity-count table and unique rule-id list the summary comment
//! needs.
//!
//! Deliberately pure (no I/O, no async): correctness here directly
//! determines which findings a PR author sees inline vs. buried in a
//! summary comment.

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
    /// spelling, carried verbatim from the `hunks` key rather than
    /// re-derived from `finding.file` (which can be `./`-prefixed or
    /// absolute, see [`PathNormalizer`] -- GitHub's review endpoint 422s on
    /// anything but its own spelling).
    pub path: String,
    /// The line to anchor the inline comment at, clamped into the matched
    /// diff hunk. See [`split_findings`] for why neither `start_line` nor
    /// `end_line` alone is guaranteed to be postable.
    pub anchor_line: usize,
}

/// Reconciles the two differently-shaped path vocabularies
/// [`split_findings`] has to compare.
///
/// A [`Finding`]'s `file` (from `config.migrations.paths` after
/// `Config::resolve_paths`) is relative to the process's working
/// directory, and can be `./`-prefixed (the default bare
/// `pg-migration-lint.toml` lookup produces this) or absolute (an
/// explicit `--config /abs/path`). The `hunks` map's keys, GitHub's own
/// `filename`s, are always repo-root-relative, never `./`-prefixed or
/// absolute. `Path`'s `Eq`/`Hash` treat a leading `./` as a real
/// [`Component::CurDir`], so these two forms of the same path compare
/// unequal without help.
///
/// Both sides are resolved to one absolute path: finding paths against the
/// working directory, GitHub paths against the repository root. Resolving
/// (rather than stripping to a common relative form) is what also makes a
/// non-default `working-directory` work, since the process's CWD is then a
/// subdirectory of the repository root.
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
    /// mutating process-global state.
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
    /// canonicalizes the result -- needed since a finding's path can
    /// already be absolute while the GitHub side is always built from
    /// `base`, and canonicalizing both is what makes them comparable. A
    /// path that can't be canonicalized (deleted by the pull request, say)
    /// keeps its resolved form, which still matches as long as both sides
    /// share a base.
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

/// Drops a leading `./` from `path` (a real [`Component::CurDir`], which
/// `Path`'s `Eq`/`Hash` treat as significant) -- [`Path::components`]
/// already normalizes away interior `.` components, so this only ever
/// matters for a leading one.
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
/// `hunks` (see [`super::files::fetch_changed_files_and_hunks`]).
///
/// Both sides of the `hunks` lookup are normalized through `paths` first
/// (see [`PathNormalizer`]) before matching by file. A finding is
/// inline-eligible if any hunk range overlaps any part of its
/// `start_line..=end_line` span (interval overlap, not naive
/// `start_line`-only matching -- a finding spanning 30-39 against a hunk
/// covering 38-45 must still count as inline). When several hunks overlap,
/// the first one in the file's hunk list wins.
pub fn split_findings(
    findings: &[Finding],
    hunks: &HashMap<PathBuf, Vec<LineRange>>,
    paths: &PathNormalizer,
) -> (Vec<InlineEntry>, Vec<SummaryEntry>) {
    // Keep the original GitHub-spelled key alongside the normalized one --
    // that's the spelling a comment must be posted against.
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
/// `Ord`).
pub fn unique_rule_ids(findings: &[Finding]) -> Vec<RuleId> {
    findings
        .iter()
        .map(|f| f.rule_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
