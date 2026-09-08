//! PR comment posting for the `github-review` subcommand.
//!
//! [`post_comments`] implements:
//! - Inline review comments (one per [`InlineEntry`]) are deleted-then-
//!   reposted, matched by a hidden marker in the comment body
//!   ([`INLINE_MARKER`]) rather than by actor/login, so this works
//!   regardless of which token/identity posted the previous run's
//!   comments. Reposting is a single batched
//!   `POST /repos/{owner}/{repo}/pulls/{pr}/reviews` call, not one POST per
//!   comment.
//! - The summary comment (a PR "conversation"/issue comment, tagged with a
//!   different marker, [`SUMMARY_MARKER`]) is found-then-PATCHed if it
//!   already exists, or POSTed fresh otherwise -- never an "edit the last
//!   comment by the current actor" shortcut, since a human could have
//!   commented on the PR between runs.
//!
//! Two `octocrab` API quirks in the pinned version (see `Cargo.toml`)
//! shaped the implementation below; both are documented inline at their
//! call site rather than only here:
//! - [`octocrab::models::pulls::ReviewComment`] (the type
//!   `PullRequestHandler::reviews().create_review`'s `comments` parameter
//!   wants) is `#[non_exhaustive]` with no public constructor, so it can't
//!   be built from outside the `octocrab` crate. Posting the batched
//!   review below builds the same request body `create_review` would, via
//!   `Octocrab::post`'s lower-level generic escape hatch, instead.
//! - `IssueHandler::update_comment` sends its request via
//!   `Octocrab::post` (i.e. an HTTP `POST`) to
//!   `/repos/{owner}/{repo}/issues/comments/{comment_id}`, a route GitHub
//!   only defines `GET`/`PATCH`/`DELETE` handlers for -- so calling it
//!   would 404 in production. The summary comment's update path sends the
//!   `PATCH` itself against the same route instead.

use anyhow::Context;
use octocrab::Octocrab;
use pg_migration_lint::output::sarif_level;
use pg_migration_lint::{Finding, Rule as _, RuleId};

use super::filter::{InlineEntry, SeverityCounts, SummaryEntry, SummaryReason};
use super::github_path;

#[cfg(test)]
mod tests;

/// Hidden marker embedded in every bot-posted inline PR review comment's
/// body, used to find (and delete, before reposting) this tool's own past
/// comments without relying on actor/login matching.
const INLINE_MARKER: &str = "<!-- pg-migration-lint:inline -->";

/// Hidden marker embedded in the bot-posted PR summary (issue) comment's
/// body, used to find it on later runs so it can be updated in place
/// instead of duplicated. Deliberately distinct from [`INLINE_MARKER`] --
/// the two comment kinds live in different GitHub APIs (review comments vs.
/// issue comments) and are never searched for together.
const SUMMARY_MARKER: &str = "<!-- pg-migration-lint:summary -->";

/// The repository and pull request every helper in this module posts
/// comments against. Grouping these three (otherwise repeated on every
/// call) also keeps [`post_comments`] under clippy's argument-count limit.
#[derive(Debug, Clone, Copy)]
pub struct PullRequestRef<'a> {
    /// Repository owner (user or organization login).
    pub owner: &'a str,
    /// Repository name.
    pub repo: &'a str,
    /// Pull request number.
    pub number: u64,
}

impl std::fmt::Display for PullRequestRef<'_> {
    /// Renders as `owner/repo#number`, matching how this tool's own
    /// diagnostic log lines (see [`super::run`]) already refer to a PR.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}#{}", self.owner, self.repo, self.number)
    }
}

/// Posts (or updates) this run's PR comments: deletes any marker-tagged
/// inline review comments left over from a previous run, posts fresh ones
/// as a single batched review (skipped entirely if `inline` is empty), and
/// creates/updates/deletes the marker-tagged PR summary comment.
///
/// A run with no findings at all deletes the summary comment instead of
/// posting one, so a since-fixed pull request doesn't keep displaying an
/// obsolete summary -- but the stale-inline-comment delete pass above
/// always runs regardless.
///
/// `commit_sha` is the PR's head commit SHA -- the `commit_id` GitHub's
/// "create a review" endpoint anchors each inline comment's `path`+`line`
/// against.
///
/// A failure to post the batched inline review is deliberately **not**
/// fatal: it's folded into the summary comment's body instead, since the
/// delete pass above has already run and bailing out would leave the pull
/// request worse off than before this run.
///
/// # Errors
///
/// Returns an error if listing or deleting existing comments fails, or if
/// creating/updating/deleting the summary comment fails.
pub async fn post_comments(
    octocrab: &Octocrab,
    pr: PullRequestRef<'_>,
    commit_sha: &str,
    inline: &[InlineEntry],
    summary: &[SummaryEntry],
    severity_counts: SeverityCounts,
    rule_ids: &[RuleId],
) -> anyhow::Result<()> {
    delete_marker_review_comments(octocrab, pr).await?;

    if inline.is_empty() && summary.is_empty() {
        return delete_summary_comment(octocrab, pr).await;
    }

    let inline_error = match post_inline_review(octocrab, pr, commit_sha, inline).await {
        Ok(()) => None,
        Err(err) => {
            let rendered = format!("{err:#}");
            eprintln!("Warning: failed to post inline review comments on {pr}: {rendered}");
            Some(rendered)
        }
    };

    let body = summary_comment_body(summary, severity_counts, rule_ids, inline_error.as_deref());
    upsert_summary_comment(octocrab, pr, &body).await?;

    Ok(())
}

/// Deletes every existing review comment on `pr` whose body contains
/// [`INLINE_MARKER`] -- run before [`post_inline_review`] so a previous
/// run's stale findings never linger alongside this run's fresh ones.
///
/// Filters by marker, not by the comment's author/login: this must keep
/// working regardless of which token/identity posted the previous run's
/// comments (e.g. a bot app token swapped for a PAT, or vice versa).
async fn delete_marker_review_comments(
    octocrab: &Octocrab,
    pr: PullRequestRef<'_>,
) -> anyhow::Result<()> {
    let first_page = octocrab
        .pulls(pr.owner, pr.repo)
        .list_comments(Some(pr.number))
        .send()
        .await
        .with_context(|| format!("Failed to list review comments on {pr}"))?;

    let comments: Vec<octocrab::models::pulls::Comment> = octocrab
        .all_pages(first_page)
        .await
        .with_context(|| format!("Failed to walk paginated review comments on {pr}"))?;

    for comment in comments {
        if !comment.body.contains(INLINE_MARKER) {
            continue;
        }

        octocrab
            .pulls(pr.owner, pr.repo)
            .comment(comment.id)
            .delete()
            .await
            .with_context(|| {
                format!(
                    "Failed to delete stale inline review comment {} on {pr}",
                    comment.id
                )
            })?;
    }

    Ok(())
}

/// Posts every `inline` finding as a single batched PR review (one
/// `POST /repos/{owner}/{repo}/pulls/{pr}/reviews` call carrying every
/// comment), or does nothing at all if `inline` is empty -- an empty-
/// `comments` review would still show up as a no-op "commented" review on
/// the PR, which is just noise.
async fn post_inline_review(
    octocrab: &Octocrab,
    pr: PullRequestRef<'_>,
    commit_sha: &str,
    inline: &[InlineEntry],
) -> anyhow::Result<()> {
    if inline.is_empty() {
        return Ok(());
    }

    // `path` and `line` both come from the entry, not `entry.finding` --
    // see `InlineEntry`'s field docs for why.
    let comments: Vec<serde_json::Value> = inline
        .iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.path,
                "line": entry.anchor_line,
                "side": "RIGHT",
                "body": inline_comment_body(&entry.finding),
            })
        })
        .collect();

    let request_body = serde_json::json!({
        "commit_id": commit_sha,
        "body": format!(
            "pg-migration-lint found {} issue(s) anchored to this pull request's diff.",
            inline.len()
        ),
        "event": "COMMENT",
        "comments": comments,
    });

    // Deliberately NOT octocrab's typed `reviews().create_review(...)` --
    // see this module's doc comment for why its `comments: Vec<ReviewComment>`
    // parameter can't be constructed from here. This sends the identical
    // request body via `Octocrab::post`'s generic escape hatch instead,
    // still as one call for every inline comment.
    let route = format!(
        "/repos/{}/{}/pulls/{}/reviews",
        pr.owner, pr.repo, pr.number
    );
    let _: serde_json::Value = octocrab
        .post(route, Some(&request_body))
        .await
        .with_context(|| format!("Failed to create the batched inline review on {pr}"))?;

    Ok(())
}

/// Builds one inline review comment's body: a severity badge, the
/// finding's message, its rule id, and the trailing [`INLINE_MARKER`] this
/// tool searches for on the next run. Deliberately terse -- no `--explain`
/// text, that belongs in the summary comment instead, once per unique
/// rule.
///
/// The severity badge is [`sarif_level`], not `finding.severity`'s own
/// `Display` (SonarQube's `CRITICAL`/`MAJOR`/... vocabulary) -- PR comments
/// must show the same `error`/`warning`/`note` badges
/// [`super::filter::count_by_severity`]'s aggregate table uses, so the two
/// never disagree on what counts as an "error".
fn inline_comment_body(finding: &Finding) -> String {
    format!(
        "**{}** {} ({})\n\n{INLINE_MARKER}",
        sarif_level(&finding.severity),
        finding.message,
        finding.rule_id
    )
}

/// Creates-or-updates the PR's summary comment: finds the existing one by
/// [`SUMMARY_MARKER`] (never by "the last comment this actor posted" --
/// that shortcut breaks if a human comments on the PR between runs) and
/// PATCHes it if found, or POSTs a fresh issue comment otherwise.
async fn upsert_summary_comment(
    octocrab: &Octocrab,
    pr: PullRequestRef<'_>,
    body: &str,
) -> anyhow::Result<()> {
    match find_summary_comment_id(octocrab, pr).await? {
        Some(comment_id) => {
            // Deliberately NOT octocrab's `IssueHandler::update_comment` --
            // see this module's doc comment for why it sends the wrong HTTP
            // method. This sends the `PATCH` GitHub's API actually expects,
            // against the same route `update_comment` computes.
            let route = format!(
                "/repos/{}/{}/issues/comments/{comment_id}",
                pr.owner, pr.repo
            );
            let _: serde_json::Value = octocrab
                .patch(route, Some(&serde_json::json!({ "body": body })))
                .await
                .with_context(|| format!("Failed to update the summary comment on {pr}"))?;
        }
        None => {
            octocrab
                .issues(pr.owner, pr.repo)
                .create_comment(pr.number, body)
                .await
                .with_context(|| format!("Failed to create the summary comment on {pr}"))?;
        }
    }

    Ok(())
}

/// Deletes `pr`'s summary comment if one exists, and does nothing if it
/// doesn't.
///
/// Called on a clean (zero-finding) run: this run has nothing to say, but a
/// previous run's summary comment may still be sitting on the pull request
/// claiming otherwise.
async fn delete_summary_comment(octocrab: &Octocrab, pr: PullRequestRef<'_>) -> anyhow::Result<()> {
    let Some(comment_id) = find_summary_comment_id(octocrab, pr).await? else {
        return Ok(());
    };

    octocrab
        .issues(pr.owner, pr.repo)
        .delete_comment(comment_id)
        .await
        .with_context(|| format!("Failed to delete the stale summary comment on {pr}"))?;

    Ok(())
}

/// Finds the id of `pr`'s existing summary comment (the one whose body
/// contains [`SUMMARY_MARKER`]), if any, by walking every issue comment on
/// the PR.
async fn find_summary_comment_id(
    octocrab: &Octocrab,
    pr: PullRequestRef<'_>,
) -> anyhow::Result<Option<octocrab::models::CommentId>> {
    let first_page = octocrab
        .issues(pr.owner, pr.repo)
        .list_comments(pr.number)
        .send()
        .await
        .with_context(|| format!("Failed to list issue comments on {pr}"))?;

    let comments: Vec<octocrab::models::issues::Comment> = octocrab
        .all_pages(first_page)
        .await
        .with_context(|| format!("Failed to walk paginated issue comments on {pr}"))?;

    Ok(comments
        .into_iter()
        .find(|comment| {
            comment
                .body
                .as_deref()
                .is_some_and(|body| body.contains(SUMMARY_MARKER))
        })
        .map(|comment| comment.id))
}

/// Builds the summary comment's body: a severity-count table, then (if
/// non-empty) a list of every `summary`-routed finding with its
/// [`SummaryReason`], then one `<details>` block per *unique* triggered
/// rule (`rule_ids` is already deduped, so `.explain()` runs once per rule
/// regardless of how many findings it produced).
///
/// `inline_error`, when present, is the rendered error from a failed
/// batched-inline-review post (see [`post_comments`]): surfaced in the
/// body so a reader learns the findings that should have been inline are
/// missing, rather than silently seeing fewer comments than the severity
/// table implies.
fn summary_comment_body(
    summary: &[SummaryEntry],
    severity_counts: SeverityCounts,
    rule_ids: &[RuleId],
    inline_error: Option<&str>,
) -> String {
    use std::fmt::Write as _;

    let mut body = String::new();
    body.push_str("## pg-migration-lint\n\n");
    body.push_str("| Severity | Count |\n|---|---|\n");
    let _ = writeln!(body, "| Error | {} |", severity_counts.error);
    let _ = writeln!(body, "| Warning | {} |", severity_counts.warning);
    let _ = writeln!(body, "| Note | {} |", severity_counts.note);
    body.push('\n');

    if let Some(error) = inline_error {
        let _ = writeln!(
            body,
            "> [!WARNING]\n\
             > Inline review comments could not be posted, so findings inside this \
             pull request's diff are missing from the files view: `{error}`\n",
        );
    }

    if summary.is_empty() {
        body.push_str(
            "Every finding landed inline on a line this pull request's diff touches.\n\n",
        );
    } else {
        body.push_str(
            "### Findings not shown inline\n\n\
             These findings couldn't be anchored to a line in this pull request's diff:\n\n",
        );
        for entry in summary {
            let finding = &entry.finding;
            let reason = match entry.reason {
                SummaryReason::OutsideDiff => "outside this pull request's diff",
                SummaryReason::PatchTooLarge => {
                    "file's diff is too large for GitHub to report line ranges"
                }
            };
            let _ = writeln!(
                body,
                "- **{}** `{}` {}:{}-{} -- {} ({reason})",
                sarif_level(&finding.severity),
                finding.rule_id,
                github_path(&finding.file),
                finding.start_line,
                finding.end_line,
                finding.message,
            );
        }
        body.push('\n');
    }

    if !rule_ids.is_empty() {
        body.push_str("### Rule explanations\n\n");
        for rule_id in rule_ids {
            let _ = writeln!(
                body,
                "<details>\n<summary>{rule_id} -- {}</summary>\n\n{}\n\n</details>\n",
                rule_id.description(),
                rule_id.explain(),
            );
        }
    }

    body.push_str(SUMMARY_MARKER);
    body.push('\n');

    body
}
