//! PR comment posting for the `github-review` subcommand.
//!
//! [`post_comments`] re-implements the exact design the (now-removed)
//! `post-comments.sh` used, just via `octocrab` instead of `gh api`+`jq`:
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
//!   wants) is `#[non_exhaustive]` with 20+ fields shaped for *responses*
//!   (`id`, `node_id`, `diff_hunk`, `commit_id`, `created_at`, `_links`,
//!   ...), not requests, and has no public constructor -- so it can't
//!   actually be constructed from outside the `octocrab` crate. Posting the
//!   batched review below builds the exact same request body
//!   `create_review` would, via `Octocrab::post`'s lower-level generic
//!   escape hatch, instead.
//! - `IssueHandler::update_comment` sends its request via
//!   `Octocrab::post` (i.e. an HTTP `POST`) to
//!   `/repos/{owner}/{repo}/issues/comments/{comment_id}`, a route GitHub
//!   only defines `GET`/`PATCH`/`DELETE` handlers for -- so calling it
//!   would 404 in production. The summary comment's update path sends the
//!   `PATCH` itself against the same route instead.

use anyhow::Context;
use octocrab::Octocrab;
use pg_migration_lint::{Finding, Rule as _, RuleId};

use super::filter::{InlineEntry, SeverityCounts, SummaryEntry, SummaryReason};

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
/// creates-or-updates the marker-tagged PR summary comment.
///
/// `commit_sha` is the PR's head commit SHA -- the `commit_id` GitHub's
/// "create a review" endpoint anchors each inline comment's `path`+`line`
/// against.
///
/// # Errors
///
/// Returns an error if any of the underlying GitHub API calls fail
/// (listing, deleting, or creating/updating comments; creating the batched
/// review).
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
    post_inline_review(octocrab, pr, commit_sha, inline).await?;

    let body = summary_comment_body(summary, severity_counts, rule_ids);
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

    let comments: Vec<serde_json::Value> = inline
        .iter()
        .map(|entry| {
            let finding = &entry.finding;
            serde_json::json!({
                "path": github_path(&finding.file),
                "line": finding.end_line,
                "side": "RIGHT",
                "body": inline_comment_body(finding),
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

/// Builds one inline review comment's body: terse by design (Decided
/// §2B) -- a severity badge, the finding's message, and its rule id, plus
/// the trailing [`INLINE_MARKER`] this tool searches for on the next run.
/// Deliberately not rich (no `--explain` text): that belongs in the summary
/// comment instead, once per unique rule, not repeated on every inline
/// comment that rule produces.
fn inline_comment_body(finding: &Finding) -> String {
    format!(
        "**{}** {} ({})\n\n{INLINE_MARKER}",
        finding.severity, finding.message, finding.rule_id
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
/// [`SummaryReason`] so a human reading the PR understands why each one
/// isn't inline, then one `<details>` block per *unique* triggered rule
/// (Decided §2B) -- `rule_ids` is Task 4's already-deduped list, so
/// `.explain()` is called exactly once per rule here regardless of how many
/// findings that rule produced.
fn summary_comment_body(
    summary: &[SummaryEntry],
    severity_counts: SeverityCounts,
    rule_ids: &[RuleId],
) -> String {
    use std::fmt::Write as _;

    let mut body = String::new();
    body.push_str("## pg-migration-lint\n\n");
    body.push_str("| Severity | Count |\n|---|---|\n");
    let _ = writeln!(body, "| Error | {} |", severity_counts.error);
    let _ = writeln!(body, "| Warning | {} |", severity_counts.warning);
    let _ = writeln!(body, "| Note | {} |", severity_counts.note);
    body.push('\n');

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
                finding.severity,
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

/// Renders `path` the way GitHub's own API always does: forward slashes,
/// even if some future caller on Windows ever built a [`Finding`] from a
/// backslash-separated [`std::path::PathBuf`]. Mirrors
/// `pg_migration_lint::output`'s own `normalize_path`, which is
/// `pub(crate)` to the lib crate and so not reachable from here (this
/// binary-crate module lives in `main.rs`'s `mod github;`, a separate
/// crate from the `pg_migration_lint` lib).
fn github_path(path: &std::path::Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use pg_migration_lint::Severity;
    use pg_migration_lint::parser::SourceSpan;
    use std::path::Path;

    fn finding(
        rule_id: RuleId,
        severity: Severity,
        file: &str,
        start: usize,
        end: usize,
    ) -> Finding {
        Finding::new(
            rule_id,
            severity,
            format!("{rule_id} test finding"),
            Path::new(file),
            &SourceSpan::at(start, end),
        )
    }

    mod pure_body_tests {
        use super::*;

        #[test]
        fn inline_comment_body_is_terse_and_carries_the_marker() {
            let f = finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3);
            let body = inline_comment_body(&f);

            assert!(body.contains("CRITICAL"), "must carry a severity badge");
            assert!(
                body.contains("PGM001 test finding"),
                "must carry the message"
            );
            assert!(body.contains("PGM001"), "must carry the rule id");
            assert!(
                body.ends_with(INLINE_MARKER),
                "must end with the inline marker so a later run can find it"
            );
            assert!(
                !body.contains("<details>"),
                "inline bodies are terse -- no rich --explain text (Decided §2B)"
            );
        }

        #[test]
        fn summary_body_reports_zero_severity_counts_correctly() {
            let body = summary_comment_body(&[], SeverityCounts::default(), &[]);

            assert!(body.contains("| Error | 0 |"));
            assert!(body.contains("| Warning | 0 |"));
            assert!(body.contains("| Note | 0 |"));
            assert!(body.ends_with(&format!("{SUMMARY_MARKER}\n")));
        }

        #[test]
        fn summary_body_lists_every_entry_with_its_reason() {
            let summary = vec![
                SummaryEntry {
                    finding: finding(RuleId::Pgm101, Severity::Minor, "a.sql", 5, 5),
                    reason: SummaryReason::OutsideDiff,
                },
                SummaryEntry {
                    finding: finding(RuleId::Pgm201, Severity::Blocker, "b.sql", 9, 9),
                    reason: SummaryReason::PatchTooLarge,
                },
            ];

            let body = summary_comment_body(&summary, SeverityCounts::default(), &[]);

            assert!(body.contains("PGM101"));
            assert!(body.contains("outside this pull request's diff"));
            assert!(body.contains("PGM201"));
            assert!(body.contains("too large"));
        }

        #[test]
        fn summary_body_says_everything_landed_inline_when_summary_is_empty() {
            let body = summary_comment_body(&[], SeverityCounts::default(), &[]);
            assert!(body.contains("landed inline"));
        }

        /// The exact scenario the brief calls out: a rule with multiple
        /// findings must still only get one `<details>` block, since
        /// `.explain()` is driven off the already-deduped `rule_ids` slice
        /// (Task 4's `unique_rule_ids`), not off `summary`'s findings
        /// directly. This is "verify the counting logic directly" per the
        /// brief -- `.explain()` itself is a pure static-str return with no
        /// side effect to spy on, so the assertion is on the rendered
        /// output's shape instead.
        #[test]
        fn summary_body_has_one_details_block_per_unique_rule_even_with_duplicate_findings() {
            let summary = vec![
                SummaryEntry {
                    finding: finding(RuleId::Pgm501, Severity::Major, "a.sql", 1, 1),
                    reason: SummaryReason::OutsideDiff,
                },
                SummaryEntry {
                    finding: finding(RuleId::Pgm501, Severity::Major, "b.sql", 2, 2),
                    reason: SummaryReason::OutsideDiff,
                },
                SummaryEntry {
                    finding: finding(RuleId::Pgm501, Severity::Major, "c.sql", 3, 3),
                    reason: SummaryReason::OutsideDiff,
                },
            ];
            // Task 4's `unique_rule_ids` already dedups -- one entry even
            // though PGM501 fired three times above.
            let rule_ids = [RuleId::Pgm501];

            let body = summary_comment_body(&summary, SeverityCounts::default(), &rule_ids);

            assert_eq!(
                body.matches("<details>").count(),
                1,
                "one <details> block per unique rule, not per finding"
            );
            assert!(body.contains(RuleId::Pgm501.explain()));
        }

        #[test]
        fn summary_body_has_one_details_block_per_distinct_rule() {
            let rule_ids = [RuleId::Pgm001, RuleId::Pgm501];
            let body = summary_comment_body(&[], SeverityCounts::default(), &rule_ids);

            assert_eq!(body.matches("<details>").count(), 2);
            assert!(body.contains(RuleId::Pgm001.explain()));
            assert!(body.contains(RuleId::Pgm501.explain()));
        }

        #[test]
        fn github_path_normalizes_backslashes() {
            assert_eq!(
                github_path(Path::new(r"migrations\001-init.sql")),
                "migrations/001-init.sql"
            );
        }
    }

    /// Exercises the real async paths against a mock HTTP server
    /// (`wiremock`, matching Task 3's `files.rs` precedent). See
    /// `MockServer::received_requests` for the delete-before-repost
    /// ordering assertion below.
    mod http_tests {
        use super::*;
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        fn test_client(base_uri: &str) -> Octocrab {
            Octocrab::builder()
                .base_uri(base_uri)
                .expect("mock server URI should be a valid base_uri")
                .build()
                .expect("failed to build a test Octocrab client")
        }

        fn pr_ref() -> PullRequestRef<'static> {
            PullRequestRef {
                owner: "o",
                repo: "r",
                number: 1,
            }
        }

        fn review_comment_json(id: u64, body: &str) -> serde_json::Value {
            serde_json::json!({
                "url": "https://example.com/c",
                "pull_request_review_id": null,
                "id": id,
                "node_id": "n",
                "diff_hunk": "",
                "path": "a.sql",
                "position": null,
                "original_position": null,
                "commit_id": "c",
                "original_commit_id": "c",
                "user": null,
                "body": body,
                "created_at": "2024-01-01T00:00:00Z",
                "updated_at": "2024-01-01T00:00:00Z",
                "html_url": "https://example.com/c",
                "pull_request_url": "https://example.com/pr",
                "_links": {},
            })
        }

        fn issue_comment_json(id: u64, body: &str) -> serde_json::Value {
            serde_json::json!({
                "id": id,
                "node_id": "n",
                "url": "https://example.com/c",
                "html_url": "https://example.com/c",
                "body": body,
                "user": {
                    "login": "someone",
                    "id": 1,
                    "node_id": "n",
                    "avatar_url": "https://example.com/a",
                    "gravatar_id": "",
                    "url": "https://example.com/u",
                    "html_url": "https://example.com/u",
                    "followers_url": "https://example.com/u/followers",
                    "following_url": "https://example.com/u/following",
                    "gists_url": "https://example.com/u/gists",
                    "starred_url": "https://example.com/u/starred",
                    "subscriptions_url": "https://example.com/u/subscriptions",
                    "organizations_url": "https://example.com/u/orgs",
                    "repos_url": "https://example.com/u/repos",
                    "events_url": "https://example.com/u/events",
                    "received_events_url": "https://example.com/u/received_events",
                    "type": "User",
                    "site_admin": false,
                    "name": null,
                    "patch_url": null,
                },
                "created_at": "2024-01-01T00:00:00Z",
            })
        }

        #[tokio::test]
        async fn stale_marker_comments_are_deleted_before_the_new_review_is_posted() {
            let mock_server = MockServer::start().await;

            // One stale marker-tagged comment (must be deleted) and one
            // unrelated human comment (must be left alone).
            Mock::given(method("GET"))
                .and(path("/repos/o/r/pulls/1/comments"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    review_comment_json(10, &format!("old finding\n\n{INLINE_MARKER}")),
                    review_comment_json(11, "just a human comment"),
                ])))
                .mount(&mock_server)
                .await;

            Mock::given(method("DELETE"))
                .and(path("/repos/o/r/pulls/comments/10"))
                .respond_with(ResponseTemplate::new(204))
                .expect(1)
                .mount(&mock_server)
                .await;
            // No mock for deleting comment 11 -- if the code tried, it would
            // 404 and the test would fail below.

            Mock::given(method("POST"))
                .and(path("/repos/o/r/pulls/1/reviews"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                .expect(1)
                .mount(&mock_server)
                .await;

            let client = test_client(&mock_server.uri());
            let inline = vec![InlineEntry {
                finding: finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3),
            }];

            delete_marker_review_comments(&client, pr_ref())
                .await
                .expect("delete should succeed");
            post_inline_review(&client, pr_ref(), "deadbeef", &inline)
                .await
                .expect("post should succeed");

            let requests = mock_server
                .received_requests()
                .await
                .expect("request recording should be enabled");
            let delete_index = requests
                .iter()
                .position(|r| r.method.as_str() == "DELETE")
                .expect("a DELETE request must have been sent");
            let post_review_index = requests
                .iter()
                .position(|r| r.method.as_str() == "POST" && r.url.path().ends_with("/reviews"))
                .expect("a POST .../reviews request must have been sent");

            assert!(
                delete_index < post_review_index,
                "the stale comment must be deleted before the new review is posted"
            );
        }

        #[tokio::test]
        async fn empty_inline_never_calls_create_review() {
            let mock_server = MockServer::start().await;
            // Deliberately no mock mounted for POST .../reviews -- any
            // request to it would get wiremock's default 404, surfacing as
            // an `Err` below.

            let client = test_client(&mock_server.uri());

            post_inline_review(&client, pr_ref(), "deadbeef", &[])
                .await
                .expect("posting zero inline comments must be a no-op, not an HTTP call");

            let requests = mock_server.received_requests().await;
            assert!(
                requests.is_none_or(|r| r.is_empty()),
                "no request should have been sent when inline is empty"
            );
        }

        #[tokio::test]
        async fn multiple_inline_comments_are_sent_in_a_single_batched_review_call() {
            let mock_server = MockServer::start().await;

            Mock::given(method("POST"))
                .and(path("/repos/o/r/pulls/1/reviews"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
                .expect(1) // exactly one call, however many comments it carries
                .mount(&mock_server)
                .await;

            let client = test_client(&mock_server.uri());
            let inline = vec![
                InlineEntry {
                    finding: finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3),
                },
                InlineEntry {
                    finding: finding(RuleId::Pgm501, Severity::Major, "b.sql", 8, 8),
                },
                InlineEntry {
                    finding: finding(RuleId::Pgm201, Severity::Blocker, "c.sql", 1, 1),
                },
            ];

            post_inline_review(&client, pr_ref(), "deadbeef", &inline)
                .await
                .expect("batched post should succeed");

            let requests = mock_server
                .received_requests()
                .await
                .expect("request recording should be enabled");
            let review_request = requests
                .iter()
                .find(|r| r.url.path().ends_with("/reviews"))
                .expect("a request to .../reviews must have been sent");
            let sent_body: serde_json::Value = review_request
                .body_json()
                .expect("request body should be JSON");
            assert_eq!(
                sent_body["comments"]
                    .as_array()
                    .expect("comments array")
                    .len(),
                3,
                "all three inline comments must ride in the one batched call"
            );
        }

        #[tokio::test]
        async fn summary_comment_is_patched_when_a_marker_comment_already_exists() {
            let mock_server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/repos/o/r/issues/1/comments"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    issue_comment_json(42, &format!("old summary\n\n{SUMMARY_MARKER}")),
                ])))
                .mount(&mock_server)
                .await;

            Mock::given(method("PATCH"))
                .and(path("/repos/o/r/issues/comments/42"))
                .respond_with(
                    ResponseTemplate::new(200).set_body_json(issue_comment_json(42, "new summary")),
                )
                .expect(1)
                .mount(&mock_server)
                .await;
            // Deliberately no mock for POST .../issues/1/comments -- if the
            // code tried to create instead of update, it would 404.

            let client = test_client(&mock_server.uri());
            upsert_summary_comment(&client, pr_ref(), "new summary")
                .await
                .expect("upsert should PATCH the existing comment");
        }

        #[tokio::test]
        async fn summary_comment_is_created_when_no_marker_comment_exists() {
            let mock_server = MockServer::start().await;

            Mock::given(method("GET"))
                .and(path("/repos/o/r/issues/1/comments"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    issue_comment_json(7, "an unrelated human comment"),
                ])))
                .mount(&mock_server)
                .await;

            Mock::given(method("POST"))
                .and(path("/repos/o/r/issues/1/comments"))
                .respond_with(
                    ResponseTemplate::new(201).set_body_json(issue_comment_json(99, "new summary")),
                )
                .expect(1)
                .mount(&mock_server)
                .await;
            // Deliberately no mock for PATCH .../issues/comments/* -- if the
            // code tried to update instead of create, it would 404.

            let client = test_client(&mock_server.uri());
            upsert_summary_comment(&client, pr_ref(), "new summary")
                .await
                .expect("upsert should POST a fresh comment");
        }

        #[tokio::test]
        async fn find_summary_comment_id_walks_every_page() {
            let mock_server = MockServer::start().await;

            let next_link = format!(
                "<{}/repos/o/r/issues/1/comments/page2>; rel=\"next\"",
                mock_server.uri()
            );
            Mock::given(method("GET"))
                .and(path("/repos/o/r/issues/1/comments"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .set_body_json(serde_json::json!([issue_comment_json(1, "no marker here")]))
                        .append_header("Link", next_link.as_str()),
                )
                .mount(&mock_server)
                .await;

            Mock::given(method("GET"))
                .and(path("/repos/o/r/issues/1/comments/page2"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                    issue_comment_json(2, &format!("summary here\n\n{SUMMARY_MARKER}")),
                ])))
                .mount(&mock_server)
                .await;

            let client = test_client(&mock_server.uri());
            let found = find_summary_comment_id(&client, pr_ref())
                .await
                .expect("lookup should succeed");

            assert_eq!(
                found,
                Some(octocrab::models::CommentId(2)),
                "the marker comment on page 2 must be found, proving pagination is walked"
            );
        }
    }
}
