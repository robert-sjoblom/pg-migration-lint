//! Orchestration for the `pg-migration-lint github-review` subcommand.
//!
//! This is the async entry point invoked from `main.rs` inside a dedicated
//! `tokio` runtime; the rest of the binary stays synchronous.
//! - [`files`]: changed-files + diff-hunk computation via octocrab's Pull
//!   Requests API.
//! - [`filter`]: filtering findings into inline-eligible vs. summary-only,
//!   based on [`files`]'s diff hunks.
//! - [`comments`]: posting/updating PR review comments and the summary
//!   comment via octocrab.
//!
//! [`run`] resolves [`crate::GithubReviewArgs`] (applying environment-variable
//! fallbacks), builds an `Octocrab` client, fetches the PR's changed files +
//! diff hunks, lints those files in-process via the same
//! [`crate::lint_history`] pipeline the flat CLI mode uses, splits the
//! resulting findings into inline-eligible vs. summary-only via
//! [`filter::split_findings`], and posts/updates the PR's comments via
//! [`comments::post_comments`].

pub mod comments;
pub mod files;
pub mod filter;

use anyhow::Context;
use pg_migration_lint::output::{Reporter, SarifReporter};

use crate::GithubReviewArgs;

/// [`GithubReviewArgs`] after applying every environment-variable fallback.
///
/// Unlike [`GithubReviewArgs`] (whose fields are `Option` because clap
/// can't apply GitHub Actions' env-var conventions on its own), every
/// field here is required: [`ResolvedGithubReviewArgs::resolve`] fails
/// with a descriptive error if a value is missing from both the CLI and
/// its fallback source.
#[derive(Clone)]
pub struct ResolvedGithubReviewArgs {
    /// Pull request number to review.
    pub pr: u64,
    /// Repository in `owner/repo` form.
    pub repo: String,
    /// GitHub token used to call the REST API, via
    /// `Octocrab::builder().personal_token(...)` in [`run`]. Deliberately
    /// excluded from the `Debug` impl below (it's a secret, never
    /// something to print).
    pub github_token: String,
    /// Path to configuration file, passed through unchanged (see
    /// `main.rs`'s `load_config` for resolution semantics).
    pub config: Option<std::path::PathBuf>,
    /// Exit code severity threshold, passed through unchanged.
    pub fail_on: Option<String>,
}

impl std::fmt::Debug for ResolvedGithubReviewArgs {
    /// Redacts `github_token` -- this struct gets printed as a debug
    /// summary (stderr, and eventually Action logs), and a GitHub token is
    /// a secret that must never land in a log line.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedGithubReviewArgs")
            .field("pr", &self.pr)
            .field("repo", &self.repo)
            .field("github_token", &"<redacted>")
            .field("config", &self.config)
            .field("fail_on", &self.fail_on)
            .finish()
    }
}

impl ResolvedGithubReviewArgs {
    /// Resolves `args` against the process environment, applying the same
    /// fallbacks the (now-removed) `compute-changed-files.sh` used:
    /// - `--pr` falls back to `.pull_request.number` read from the JSON
    ///   file at `$GITHUB_EVENT_PATH`.
    /// - `--repo` falls back to `$GITHUB_REPOSITORY`.
    /// - `--github-token` falls back to `$GITHUB_TOKEN`, then `$GH_TOKEN`.
    ///
    /// # Errors
    ///
    /// Returns an error if any of the above has neither an explicit CLI
    /// value nor a usable fallback (including: `$GITHUB_EVENT_PATH` set
    /// but unreadable or not valid JSON, or lacking
    /// `.pull_request.number`).
    pub fn resolve(args: &GithubReviewArgs) -> anyhow::Result<Self> {
        let pr = resolve_pr(args.pr, std::env::var("GITHUB_EVENT_PATH").ok())?;
        let repo = resolve_repo(args.repo.clone(), std::env::var("GITHUB_REPOSITORY").ok())?;
        let github_token = resolve_token(
            args.github_token.clone(),
            std::env::var("GITHUB_TOKEN").ok(),
            std::env::var("GH_TOKEN").ok(),
        )?;

        Ok(Self {
            pr,
            repo,
            github_token,
            config: args.config.clone(),
            fail_on: args.fail_on.clone(),
        })
    }
}

/// Resolves the PR number: `explicit` if given, else reads
/// `.pull_request.number` from the JSON file at `event_path`.
fn resolve_pr(explicit: Option<u64>, event_path: Option<String>) -> anyhow::Result<u64> {
    if let Some(pr) = explicit {
        return Ok(pr);
    }

    let event_path = event_path.context(
        "--pr not given and $GITHUB_EVENT_PATH is not set; cannot determine the PR number",
    )?;
    let contents = std::fs::read_to_string(&event_path)
        .with_context(|| format!("Failed to read GITHUB_EVENT_PATH file '{event_path}'"))?;
    pr_number_from_event_json(&contents)
}

/// Parses `.pull_request.number` out of a GitHub Actions event payload.
///
/// This is the same fallback the (now-removed) `compute-changed-files.sh`
/// used, ported from `jq -r '.pull_request.number // empty'`.
fn pr_number_from_event_json(contents: &str) -> anyhow::Result<u64> {
    let value: serde_json::Value =
        serde_json::from_str(contents).context("Failed to parse GITHUB_EVENT_PATH file as JSON")?;

    value
        .get("pull_request")
        .and_then(|pull_request| pull_request.get("number"))
        .and_then(serde_json::Value::as_u64)
        .context(
            "No .pull_request.number in GITHUB_EVENT_PATH file; \
              github-review only supports pull_request-triggered events",
        )
}

/// Resolves the `owner/repo` string: `explicit` if given, else `env_repo`.
fn resolve_repo(explicit: Option<String>, env_repo: Option<String>) -> anyhow::Result<String> {
    explicit
        .or(env_repo)
        .context("--repo not given and $GITHUB_REPOSITORY is not set")
}

/// Resolves the GitHub token: `explicit` if given, else `github_token_env`,
/// else `gh_token_env`.
fn resolve_token(
    explicit: Option<String>,
    github_token_env: Option<String>,
    gh_token_env: Option<String>,
) -> anyhow::Result<String> {
    explicit
        .or(github_token_env)
        .or(gh_token_env)
        .context("--github-token not given and neither $GITHUB_TOKEN nor $GH_TOKEN is set")
}

/// Runs the `github-review` workflow for a single pull request: fetches
/// its changed files, lints them, and posts findings back as PR comments.
///
/// Returns the process exit code the caller should use (0 = no findings at
/// or above threshold, 1 = findings at or above threshold, 2 = tool error),
/// mirroring the flat CLI mode's exit code contract.
///
/// # Errors
///
/// Returns an error if argument resolution fails (see
/// [`ResolvedGithubReviewArgs::resolve`]), the `repo` string isn't in
/// `owner/repo` form, the `Octocrab` client fails to build, the changed
/// files/diff-hunk fetch (see [`files::fetch_changed_files_and_hunks`])
/// fails, fetching the PR itself (for its head commit SHA) fails, config
/// loading fails (`--config` given-and-missing is a hard error -- see
/// `crate::load_config`), migration loading fails, writing the SARIF report
/// fails, posting/updating PR comments fails (see
/// [`comments::post_comments`]), `--fail-on`/`config.cli.fail_on` names
/// an unknown severity, or writing this action's `exit-code`/`findings-count`
/// outputs to `$GITHUB_OUTPUT` fails (see [`write_outputs`]).
pub async fn run(args: &GithubReviewArgs) -> anyhow::Result<i32> {
    let resolved = ResolvedGithubReviewArgs::resolve(args)?;
    let (owner, repo) = split_owner_repo(&resolved.repo)?;

    let octocrab = octocrab::Octocrab::builder()
        .personal_token(resolved.github_token.clone())
        .build()
        .context("Failed to build the GitHub API client")?;

    let (changed_files, hunks) =
        files::fetch_changed_files_and_hunks(&octocrab, owner, repo, resolved.pr).await?;

    // The PR's head commit SHA -- needed as `create_review`'s `commit_id`
    // when posting inline comments (see `comments::post_comments`).
    // Resolved directly from octocrab's own PR-get response rather than
    // threading another CLI/env value through, since this is the more
    // reliable source (always matches whatever `changed_files`/`hunks`
    // above were just computed against) and Task 3's own fetch doesn't
    // carry it.
    let pull_request = octocrab
        .pulls(owner, repo)
        .get(resolved.pr)
        .await
        .with_context(|| format!("Failed to fetch PR #{} for {owner}/{repo}", resolved.pr))?;
    let commit_sha = pull_request.head.sha.clone();

    // Run the same lint pipeline the flat CLI mode uses (`crate::load_config`,
    // `crate::load_migrations`, `crate::lint_history`), restricted to the
    // PR's changed files instead of a `--changed-files`/`--changed-files-from`
    // CLI value. `resolved.config` already carries the flat mode's exact
    // `--config` semantics: given-and-missing is a hard error, omitted falls
    // back to `./pg-migration-lint.toml` then `Config::default()`.
    let config = crate::load_config(&resolved.config)?;
    let mut history = crate::load_migrations(&config)?;
    let mut all_findings = crate::lint_history(&mut history, &config, Some(&changed_files));

    // Route findings against Task 3's `hunks` map BEFORE applying any
    // configured `output.strip_prefix` -- `hunks`' keys are GitHub's own
    // repo-root-relative `filename`s, never stripped, and
    // `comments::post_comments` below needs this same raw path to post PR
    // review comments against the right file. Stripping first (as the flat
    // CLI mode's report-writing step does) would make every finding's
    // `file` fail to match its own hunk entry, silently routing everything
    // to `SummaryReason::OutsideDiff` regardless of the PR's actual diff --
    // see `crate::strip_output_prefix`.
    //
    // Both sides of that lookup are normalized through
    // `filter::PathNormalizer` first: a finding's path is whatever
    // `Config::resolve_paths` produced (possibly `./`-prefixed, possibly
    // absolute, possibly relative to a non-root `working-directory`),
    // while `hunks`' keys are always GitHub's plain repo-root-relative
    // `filename`s -- see `PathNormalizer`'s doc comment.
    let paths = filter::PathNormalizer::from_env();
    let (inline, summary) = filter::split_findings(&all_findings, &hunks, &paths);
    let severity_counts = filter::count_by_severity(&all_findings);
    let rule_ids = filter::unique_rule_ids(&all_findings);

    // Optional (not required for the PR-comment feature, but low-cost and
    // independently valuable): also write the SARIF report, at the same
    // `config.output.dir` the flat CLI mode's own SARIF output already
    // resolves to (relative paths in the config are resolved against the
    // `--config` file's directory by `Config::from_file` itself, so no
    // separate resolution logic is needed here). This lets consumers wire
    // this subcommand's output into GitHub Code Scanning via
    // `github/codeql-action/upload-sarif`, independent of whatever
    // `config.output.formats` says for the flat CLI mode. `strip_prefix` is
    // applied now, only for this report-display purpose, now that routing
    // above is already decided against the raw paths.
    crate::strip_output_prefix(&mut all_findings, &config);
    SarifReporter::new()
        .emit(&all_findings, &config.output.dir)
        .context("Failed to write SARIF report")?;

    eprintln!(
        "github-review: PR #{} ({owner}/{repo}) -- {} changed file(s), {} finding(s) \
          ({} inline-eligible, {} summary-only); severities: {} error, {} warning, {} note; \
          rules: [{}]",
        resolved.pr,
        changed_files.len(),
        all_findings.len(),
        inline.len(),
        summary.len(),
        severity_counts.error,
        severity_counts.warning,
        severity_counts.note,
        rule_ids
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", "),
    );

    log_finding_routing(&inline, &summary);

    comments::post_comments(
        &octocrab,
        comments::PullRequestRef {
            owner,
            repo,
            number: resolved.pr,
        },
        &commit_sha,
        &inline,
        &summary,
        severity_counts,
        &rule_ids,
    )
    .await
    .map_err(|err| {
        if is_permission_error(&err) {
            err.context(PERMISSION_ERROR_HINT)
        } else {
            err.context("Failed to post PR comments")
        }
    })?;
    if all_findings.is_empty() {
        // `post_comments` deliberately posts nothing on a clean run (it
        // only clears out any stale comments a previous run left), so
        // claiming "comments posted" here would be a lie.
        eprintln!(
            "github-review: PR #{} is clean -- no comments posted",
            resolved.pr
        );
    } else {
        eprintln!("github-review: PR #{} comments posted", resolved.pr);
    }

    let fail_on_str = resolved.fail_on.as_deref().unwrap_or(&config.cli.fail_on);
    let exit_code = if crate::exceeds_fail_on_threshold(&all_findings, fail_on_str)? {
        1
    } else {
        0
    };

    write_outputs(exit_code, all_findings.len())?;

    Ok(exit_code)
}

/// Writes this action's public `exit-code` and `findings-count` outputs to
/// the file at `$GITHUB_OUTPUT`, in the `key=value` line format the GitHub
/// Actions runner expects.
///
/// Resolves the path from the process environment and delegates to
/// [`write_outputs_to`], which takes the path as a plain parameter so tests
/// can point it at a tempfile instead of mutating `$GITHUB_OUTPUT` itself
/// (the same explicit-parameter-over-env-read split [`resolve_pr`],
/// [`resolve_repo`], and [`resolve_token`] already use).
///
/// # Errors
///
/// Returns an error if `$GITHUB_OUTPUT` is not set. A real Actions run
/// always sets it; this is a hard error (matching the same-required
/// convention the now-removed `parse-and-filter.sh` used for the same
/// variable) rather than a silent no-op, since silently dropping the
/// action's declared outputs would be a confusing footgun.
fn write_outputs(exit_code: i32, findings_count: usize) -> anyhow::Result<()> {
    let path = std::env::var("GITHUB_OUTPUT")
        .context("GITHUB_OUTPUT is required (normally set by the GitHub Actions runner)")?;
    write_outputs_to(std::path::Path::new(&path), exit_code, findings_count)
}

/// Appends `exit-code` and `findings-count` lines to the `$GITHUB_OUTPUT`
/// file at `path`, in the `key=value` format documented at
/// <https://docs.github.com/en/actions/using-workflows/workflow-commands-for-github-actions#setting-an-output-parameter>.
///
/// Opens `path` in append mode (creating it if missing) rather than
/// truncating -- `$GITHUB_OUTPUT` is a single file shared across every step
/// in a job, and other steps' own output lines (this action's
/// `download-binary` step writes none today, but the convention holds
/// generally) must never be clobbered.
///
/// # Errors
///
/// Returns an error if `path` can't be opened for appending or written to.
fn write_outputs_to(
    path: &std::path::Path,
    exit_code: i32,
    findings_count: usize,
) -> anyhow::Result<()> {
    use std::io::Write as _;

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("Failed to open GITHUB_OUTPUT file '{}'", path.display()))?;

    write!(
        file,
        "exit-code={exit_code}\nfindings-count={findings_count}\n"
    )
    .with_context(|| format!("Failed to write to GITHUB_OUTPUT file '{}'", path.display()))?;

    Ok(())
}

/// Logs each finding's routing decision to stderr: one line per finding,
/// tagged `inline` or `summary(<reason>)`.
///
/// This is deliberately just a diagnostic Action-log line, independent of
/// [`comments::post_comments`] turning the same `inline`/`summary` slices
/// into actual PR review/summary comments.
fn log_finding_routing(inline: &[filter::InlineEntry], summary: &[filter::SummaryEntry]) {
    for entry in inline {
        let f = &entry.finding;
        eprintln!(
            "  [inline] {} {}:{}-{}",
            f.rule_id,
            f.file.display(),
            f.start_line,
            f.end_line,
        );
    }
    for entry in summary {
        let f = &entry.finding;
        eprintln!(
            "  [summary:{:?}] {} {}:{}-{}",
            entry.reason,
            f.rule_id,
            f.file.display(),
            f.start_line,
            f.end_line,
        );
    }
}

/// Renders `path` the way GitHub's own API always does: forward slashes,
/// even if some future caller on Windows ever built a
/// [`pg_migration_lint::Finding`] from a backslash-separated
/// [`std::path::PathBuf`]. Mirrors `pg_migration_lint::output`'s own
/// `normalize_path`, which is `pub(crate)` to the lib crate and so not
/// reachable from here (this binary-crate module lives in `main.rs`'s
/// `mod github;`, a separate crate from the `pg_migration_lint` lib).
///
/// Shared by [`filter`] (which renders the path an inline comment is posted
/// against) and [`comments`] (which renders paths in the summary comment's
/// body) so the two can't disagree about how a path is spelled.
pub fn github_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Reports whether `error`'s cause chain contains a GitHub API error with
/// HTTP status 403 -- the shape a comment-posting call takes when the
/// token it was given lacks write access to the pull request.
///
/// The overwhelmingly common cause is a fork pull request: a
/// `pull_request`-triggered workflow run from a fork gets a read-only
/// `GITHUB_TOKEN` no matter what the workflow's `permissions:` block asks
/// for. [`run`] uses this to replace a raw octocrab error dump with an
/// actionable message.
fn is_permission_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<octocrab::Error>(),
            Some(octocrab::Error::GitHub { source, .. }) if source.status_code.as_u16() == 403
        )
    })
}

/// The actionable message [`run`] attaches to a 403 from comment posting,
/// in place of octocrab's raw error text.
const PERMISSION_ERROR_HINT: &str = "Posting PR comments failed with a permission error (HTTP 403). \
      Check that the workflow grants 'permissions: pull-requests: write'. \
      If this is a pull request from a fork, note that a pull_request-triggered \
      run always receives a read-only GITHUB_TOKEN regardless of that block -- \
      see GitHub's documentation on pull_request_target for the supported way \
      to comment on fork pull requests";

/// Splits an `owner/repo` string (as used by [`ResolvedGithubReviewArgs::repo`])
/// into its two halves.
///
/// # Errors
///
/// Returns an error if `repo` doesn't contain exactly one `/`, or either
/// half would be empty.
fn split_owner_repo(repo: &str) -> anyhow::Result<(&str, &str)> {
    match repo.split_once('/') {
        Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
            Ok((owner, name))
        }
        _ => anyhow::bail!("Expected repo in 'owner/repo' form, got '{repo}'"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_pr_prefers_explicit_value_over_event_path() {
        let result = resolve_pr(Some(42), None);
        assert_eq!(result.unwrap(), 42);
    }

    #[test]
    fn resolve_pr_falls_back_to_event_path_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), r#"{"pull_request": {"number": 7}}"#).unwrap();

        let result = resolve_pr(None, Some(file.path().to_string_lossy().into_owned()));

        assert_eq!(result.unwrap(), 7);
    }

    #[test]
    fn resolve_pr_errors_when_event_path_env_is_unset() {
        let result = resolve_pr(None, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("GITHUB_EVENT_PATH")
        );
    }

    #[test]
    fn resolve_pr_errors_when_event_path_file_is_missing() {
        let result = resolve_pr(None, Some("/nonexistent/event.json".to_string()));
        assert!(result.is_err());
    }

    #[test]
    fn pr_number_from_event_json_extracts_nested_number() {
        let result = pr_number_from_event_json(r#"{"pull_request": {"number": 123}}"#);
        assert_eq!(result.unwrap(), 123);
    }

    #[test]
    fn pr_number_from_event_json_errors_when_field_missing() {
        let result = pr_number_from_event_json(r#"{"action": "opened"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn pr_number_from_event_json_errors_on_invalid_json() {
        let result = pr_number_from_event_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn resolve_repo_prefers_explicit_value_over_env() {
        let result = resolve_repo(
            Some("explicit/repo".to_string()),
            Some("env/repo".to_string()),
        );
        assert_eq!(result.unwrap(), "explicit/repo");
    }

    #[test]
    fn resolve_repo_falls_back_to_env() {
        let result = resolve_repo(None, Some("env/repo".to_string()));
        assert_eq!(result.unwrap(), "env/repo");
    }

    #[test]
    fn resolve_repo_errors_when_neither_source_is_set() {
        let result = resolve_repo(None, None);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("GITHUB_REPOSITORY")
        );
    }

    #[test]
    fn resolve_token_prefers_explicit_value() {
        let result = resolve_token(
            Some("explicit-token".to_string()),
            Some("github-token".to_string()),
            Some("gh-token".to_string()),
        );
        assert_eq!(result.unwrap(), "explicit-token");
    }

    #[test]
    fn resolve_token_falls_back_to_github_token_env_before_gh_token() {
        let result = resolve_token(
            None,
            Some("github-token".to_string()),
            Some("gh-token".to_string()),
        );
        assert_eq!(result.unwrap(), "github-token");
    }

    #[test]
    fn resolve_token_falls_back_to_gh_token_when_github_token_unset() {
        let result = resolve_token(None, None, Some("gh-token".to_string()));
        assert_eq!(result.unwrap(), "gh-token");
    }

    #[test]
    fn resolve_token_errors_when_no_source_is_set() {
        let result = resolve_token(None, None, None);
        assert!(result.is_err());
    }

    #[test]
    fn resolved_debug_output_redacts_github_token() {
        let resolved = ResolvedGithubReviewArgs {
            pr: 1,
            repo: "owner/repo".to_string(),
            github_token: "super-secret-token".to_string(),
            config: None,
            fail_on: None,
        };

        let debug_output = format!("{resolved:?}");

        assert!(!debug_output.contains("super-secret-token"));
        assert!(debug_output.contains("<redacted>"));
    }

    #[test]
    fn split_owner_repo_splits_on_the_slash() {
        let result = split_owner_repo("octocat/hello-world");
        assert_eq!(result.unwrap(), ("octocat", "hello-world"));
    }

    #[test]
    fn split_owner_repo_errors_when_there_is_no_slash() {
        let result = split_owner_repo("no-slash-here");
        assert!(result.is_err());
    }

    #[test]
    fn split_owner_repo_errors_on_empty_owner() {
        let result = split_owner_repo("/repo");
        assert!(result.is_err());
    }

    #[test]
    fn split_owner_repo_errors_on_empty_repo_name() {
        let result = split_owner_repo("owner/");
        assert!(result.is_err());
    }

    #[test]
    fn split_owner_repo_errors_when_there_is_more_than_one_slash() {
        let result = split_owner_repo("owner/repo/extra");
        assert!(result.is_err());
    }

    #[test]
    fn write_outputs_to_appends_exit_code_and_findings_count_lines() {
        let file = tempfile::NamedTempFile::new().unwrap();

        write_outputs_to(file.path(), 1, 42).unwrap();

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(contents, "exit-code=1\nfindings-count=42\n");
    }

    #[test]
    fn write_outputs_to_appends_rather_than_truncates_existing_content() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "some-other-output=already-here\n").unwrap();

        write_outputs_to(file.path(), 0, 0).unwrap();

        let contents = std::fs::read_to_string(file.path()).unwrap();
        assert_eq!(
            contents,
            "some-other-output=already-here\nexit-code=0\nfindings-count=0\n"
        );
    }

    #[test]
    fn write_outputs_to_errors_when_path_is_unwritable() {
        let result = write_outputs_to(
            std::path::Path::new("/nonexistent-directory/github-output"),
            0,
            0,
        );
        assert!(result.is_err());
    }

    #[test]
    fn write_outputs_errors_when_github_output_env_is_unset() {
        // SAFETY: no other test in this process reads or writes
        // `GITHUB_OUTPUT`, so removing it here can't race with another
        // test's expectations. Cargo test binaries run each test in its
        // own thread within one process, so mutating process-global env
        // state is only safe when no other test touches the same key --
        // this is that key's only test.
        unsafe {
            std::env::remove_var("GITHUB_OUTPUT");
        }

        let result = write_outputs(0, 0);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("GITHUB_OUTPUT"));
    }
}
