use super::*;
use pg_migration_lint::Severity;
use pg_migration_lint::parser::SourceSpan;
use std::path::Path;

fn finding(rule_id: RuleId, severity: Severity, file: &str, start: usize, end: usize) -> Finding {
    Finding::new(
        rule_id,
        severity,
        format!("{rule_id} test finding"),
        Path::new(file),
        &SourceSpan::at(start, end),
    )
}

/// Builds an [`InlineEntry`] the way [`super::super::filter::split_findings`]
/// would: with the GitHub-spelled `path` and the hunk-clamped
/// `anchor_line` carried explicitly, not re-derived from the finding.
fn inline_entry(finding: Finding, path: &str, anchor_line: usize) -> InlineEntry {
    InlineEntry {
        finding,
        path: path.to_string(),
        anchor_line,
    }
}

mod pure_body_tests {
    use super::*;

    #[test]
    fn inline_comment_body_is_terse_and_carries_the_marker() {
        let f = finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3);
        let body = inline_comment_body(&f);

        assert!(
            body.contains("error"),
            "must carry a SARIF-level severity badge (Decided §2A)"
        );
        assert!(
            !body.contains("CRITICAL"),
            "must not use SonarQube's severity vocabulary (Decided §2A)"
        );
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

    /// Decided §2A: every per-finding severity badge in the summary
    /// comment must use the same `error`/`warning`/`note` vocabulary as
    /// the severity-count table above it (both keyed off
    /// [`sarif_level`]), never SonarQube's
    /// `CRITICAL`/`MAJOR`/`MINOR`/`INFO`/`BLOCKER`.
    #[test]
    fn summary_body_per_entry_badges_use_sarif_vocabulary_not_sonarqube() {
        let summary = vec![
            SummaryEntry {
                finding: finding(RuleId::Pgm001, Severity::Blocker, "a.sql", 1, 1),
                reason: SummaryReason::OutsideDiff,
            },
            SummaryEntry {
                finding: finding(RuleId::Pgm501, Severity::Major, "b.sql", 2, 2),
                reason: SummaryReason::OutsideDiff,
            },
            SummaryEntry {
                finding: finding(RuleId::Pgm101, Severity::Info, "c.sql", 3, 3),
                reason: SummaryReason::OutsideDiff,
            },
        ];

        let body = summary_comment_body(&summary, SeverityCounts::default(), &[], None);

        assert!(body.contains("**error**"), "Blocker maps to SARIF error");
        assert!(body.contains("**warning**"), "Major maps to SARIF warning");
        assert!(body.contains("**note**"), "Info maps to SARIF note");
        assert!(
            !body.contains("BLOCKER") && !body.contains("MAJOR") && !body.contains("INFO"),
            "must not leak SonarQube's severity vocabulary into the PR comment"
        );
    }

    #[test]
    fn summary_body_reports_zero_severity_counts_correctly() {
        let body = summary_comment_body(&[], SeverityCounts::default(), &[], None);

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

        let body = summary_comment_body(&summary, SeverityCounts::default(), &[], None);

        assert!(body.contains("PGM101"));
        assert!(body.contains("outside this pull request's diff"));
        assert!(body.contains("PGM201"));
        assert!(body.contains("too large"));
    }

    #[test]
    fn summary_body_says_everything_landed_inline_when_summary_is_empty() {
        let body = summary_comment_body(&[], SeverityCounts::default(), &[], None);
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

        let body = summary_comment_body(&summary, SeverityCounts::default(), &rule_ids, None);

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
        let body = summary_comment_body(&[], SeverityCounts::default(), &rule_ids, None);

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
        let inline = vec![inline_entry(
            finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3),
            "a.sql",
            3,
        )];

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
            inline_entry(
                finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3),
                "a.sql",
                3,
            ),
            inline_entry(
                finding(RuleId::Pgm501, Severity::Major, "b.sql", 8, 8),
                "b.sql",
                8,
            ),
            inline_entry(
                finding(RuleId::Pgm201, Severity::Blocker, "c.sql", 1, 1),
                "c.sql",
                1,
            ),
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

    /// Mounts the two `GET` list endpoints [`post_comments`] always
    /// hits first, each returning `comments`.
    async fn mount_comment_listings(
        mock_server: &MockServer,
        review_comments: serde_json::Value,
        issue_comments: serde_json::Value,
    ) {
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(review_comments))
            .mount(mock_server)
            .await;
        Mock::given(method("GET"))
            .and(path("/repos/o/r/issues/1/comments"))
            .respond_with(ResponseTemplate::new(200).set_body_json(issue_comments))
            .mount(mock_server)
            .await;
    }

    /// A failed inline-review post must not stop the summary comment
    /// from landing. The delete pass has already run by then, so
    /// bailing out would leave the pull request with neither its old
    /// comments nor any new ones -- strictly worse than before the run.
    #[tokio::test]
    async fn inline_review_failure_still_posts_the_summary_comment() {
        let mock_server = MockServer::start().await;
        mount_comment_listings(&mock_server, serde_json::json!([]), serde_json::json!([]))
            .await;

        // The exact failure this guards against: GitHub rejecting the
        // whole batched review.
        Mock::given(method("POST"))
            .and(path("/repos/o/r/pulls/1/reviews"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Unprocessable Entity",
                "documentation_url": "https://docs.github.com/rest",
            })))
            .mount(&mock_server)
            .await;

        Mock::given(method("POST"))
            .and(path("/repos/o/r/issues/1/comments"))
            .respond_with(
                ResponseTemplate::new(201).set_body_json(issue_comment_json(99, "summary")),
            )
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());
        let inline = vec![inline_entry(
            finding(RuleId::Pgm001, Severity::Critical, "a.sql", 3, 3),
            "a.sql",
            3,
        )];

        post_comments(
            &client,
            pr_ref(),
            "deadbeef",
            &inline,
            &[],
            SeverityCounts::default(),
            &[],
        )
        .await
        .expect("a failed inline post must not fail the whole run");

        let requests = mock_server
            .received_requests()
            .await
            .expect("request recording should be enabled");
        let summary_request = requests
            .iter()
            .find(|r| {
                r.method.as_str() == "POST" && r.url.path().ends_with("/issues/1/comments")
            })
            .expect("the summary comment must still have been posted");
        let sent_body: serde_json::Value = summary_request
            .body_json()
            .expect("request body should be JSON");
        let body = sent_body["body"].as_str().unwrap_or_default();
        assert!(
            body.contains("Inline review comments could not be posted"),
            "the summary must say the inline comments are missing: {body}"
        );
    }

    /// A clean run posts no summary comment at all -- otherwise every
    /// pull request in a consumer's repository collects a bot comment
    /// showing a table of zeros.
    #[tokio::test]
    async fn clean_run_posts_no_summary_comment() {
        let mock_server = MockServer::start().await;
        mount_comment_listings(
            &mock_server,
            serde_json::json!([]),
            serde_json::json!([issue_comment_json(7, "an unrelated human comment")]),
        )
        .await;
        // Deliberately no POST/PATCH/DELETE mocks -- any write would
        // 404 and surface as an `Err` below.

        let client = test_client(&mock_server.uri());

        post_comments(
            &client,
            pr_ref(),
            "deadbeef",
            &[],
            &[],
            SeverityCounts::default(),
            &[],
        )
        .await
        .expect("a clean run must not write anything");

        let requests = mock_server
            .received_requests()
            .await
            .expect("request recording should be enabled");
        assert!(
            requests.iter().all(|r| r.method.as_str() == "GET"),
            "a clean run must only read, never write"
        );
    }

    /// ...but a stale summary comment from an earlier run (when there
    /// *were* findings) must be deleted, so a since-fixed pull request
    /// stops displaying an obsolete summary.
    #[tokio::test]
    async fn clean_run_deletes_a_stale_summary_comment() {
        let mock_server = MockServer::start().await;
        mount_comment_listings(
            &mock_server,
            serde_json::json!([]),
            serde_json::json!([issue_comment_json(
                42,
                &format!("old summary\n\n{SUMMARY_MARKER}")
            )]),
        )
        .await;

        Mock::given(method("DELETE"))
            .and(path("/repos/o/r/issues/comments/42"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());

        post_comments(
            &client,
            pr_ref(),
            "deadbeef",
            &[],
            &[],
            SeverityCounts::default(),
            &[],
        )
        .await
        .expect("deleting the stale summary should succeed");
    }

    /// A 403 from any comment-posting call must be recognizable as a
    /// permission problem, so `super::super::run` can replace octocrab's
    /// raw error with an actionable message (most often: this is a fork
    /// pull request, whose GITHUB_TOKEN is read-only).
    #[tokio::test]
    async fn a_403_from_comment_posting_is_classified_as_a_permission_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(403).set_body_json(serde_json::json!({
                "message": "Resource not accessible by integration",
                "documentation_url": "https://docs.github.com/rest",
            })))
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());

        let error = post_comments(
            &client,
            pr_ref(),
            "deadbeef",
            &[],
            &[],
            SeverityCounts::default(),
            &[],
        )
        .await
        .expect_err("a 403 must surface as an error");

        assert!(
            crate::github::is_permission_error(&error),
            "a 403 must be classified as a permission error: {error:#}"
        );
    }

    #[tokio::test]
    async fn a_422_is_not_classified_as_a_permission_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/1/comments"))
            .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
                "message": "Unprocessable Entity",
                "documentation_url": "https://docs.github.com/rest",
            })))
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());

        let error = post_comments(
            &client,
            pr_ref(),
            "deadbeef",
            &[],
            &[],
            SeverityCounts::default(),
            &[],
        )
        .await
        .expect_err("a 422 must surface as an error");

        assert!(!crate::github::is_permission_error(&error));
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
