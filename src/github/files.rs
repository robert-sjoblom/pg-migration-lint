//! Changed-files + diff-hunk computation for the `github-review` subcommand.
//!
//! [`fetch_changed_files_and_hunks`] fetches a pull request's changed files
//! via octocrab's Pull Requests API (paginated with
//! [`octocrab::Octocrab::all_pages`] -- GitHub's default page size is 30, and
//! this repo's own `enterprise/` fixture alone has 31 migration files, so a PR
//! touching all of them would silently lose file #31 without walking every
//! page), then parses each file's `patch` field's `@@ -a,b +c,d @@` hunk
//! headers into inclusive 1-based new-file line ranges for [`super::filter`]'s
//! hunk-overlap check.
use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use octocrab::Octocrab;
use octocrab::models::repos::DiffEntry;

/// An inclusive, 1-based new-file line range that a PR's diff actually
/// touches (or shows as unchanged context within a hunk).
///
/// Both endpoints are line numbers in the file's *new* (post-change)
/// version, matching what a `Finding`'s own line numbers refer to -- see
/// [`super::filter`] for how this is used to decide whether a finding can
/// be posted as an inline PR review comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRange {
    /// First line included in the range.
    pub start: usize,
    /// Last line included in the range (inclusive).
    pub end: usize,
}

/// Fetches every changed file in pull request `pr` of `owner/repo`, and
/// computes each file's inline-eligible new-file line ranges from its diff
/// hunks.
///
/// Returns `(changed_files, hunks)`:
/// - `changed_files` -- every changed file's path, in the order the GitHub
///   API returned them (page order, then within-page order). Renamed files
///   are keyed by their *new* path (`DiffEntry::filename`), never
///   `previous_filename`.
/// - `hunks` -- a map from that same path to its list of inline-eligible
///   [`LineRange`]s. Every changed file has an entry (possibly an empty
///   `Vec`), so callers can distinguish "no entry" (shouldn't happen for a
///   file this function itself returned) from "entry present but empty"
///   (GitHub omitted the `patch` field -- e.g. a very large diff or a
///   binary file -- or every hunk in it was a pure-deletion hunk
///   contributing zero new-file lines; these two cases are indistinguishable
///   from this map alone, which is fine, both mean "nothing in this file is
///   inline-eligible").
///
/// # Errors
///
/// Returns an error if the underlying GitHub API call (including walking
/// every paginated page) fails.
pub async fn fetch_changed_files_and_hunks(
    octocrab: &Octocrab,
    owner: &str,
    repo: &str,
    pr: u64,
) -> anyhow::Result<(Vec<PathBuf>, HashMap<PathBuf, Vec<LineRange>>)> {
    let first_page = octocrab
        .pulls(owner, repo)
        .list_files(pr)
        .await
        .with_context(|| format!("Failed to fetch changed files for {owner}/{repo}#{pr}"))?;

    let entries = octocrab
        .all_pages::<DiffEntry>(first_page)
        .await
        .with_context(|| {
            format!("Failed to walk paginated changed-files response for {owner}/{repo}#{pr}")
        })?;

    Ok(changed_files_and_hunks_from_entries(&entries))
}

/// Pure computation half of [`fetch_changed_files_and_hunks`]: turns
/// already-fetched (and already-paginated) [`DiffEntry`] values into the
/// same `(changed_files, hunks)` shape. Split out from the async fetch so
/// it can be unit-tested directly against real `DiffEntry` values, without
/// any network/mock-HTTP layer involved.
fn changed_files_and_hunks_from_entries(
    entries: &[DiffEntry],
) -> (Vec<PathBuf>, HashMap<PathBuf, Vec<LineRange>>) {
    let mut changed_files = Vec::with_capacity(entries.len());
    let mut hunks = HashMap::with_capacity(entries.len());

    for entry in entries {
        // Always key off `filename` (the *new* path), never
        // `previous_filename` -- a finding's own file path will be the new
        // path too, and that's also what a SARIF `uri` would be.
        let path = PathBuf::from(&entry.filename);
        let ranges = entry
            .patch
            .as_deref()
            .map(hunk_ranges_from_patch)
            .unwrap_or_default();

        changed_files.push(path.clone());
        hunks.insert(path, ranges);
    }

    (changed_files, hunks)
}

/// Parses a unified-diff `patch` body into its inline-eligible new-file
/// line ranges.
///
/// Every line matching `^@@ -\d+(,\d+)? \+\d+(,\d+)? @@` contributes one
/// range `{start: newStart, end: newStart + newCount - 1}`, where
/// `newCount` defaults to 1 when its optional `,count` is omitted (a
/// single-line hunk). A hunk whose new-side count is 0 (a pure-deletion
/// hunk -- it only removes lines, contributing no new-file lines at all)
/// is dropped rather than emitting a bogus/inverted range (`start=N,
/// end=N-1`). Lines that don't match this pattern (context lines, `+`/`-`
/// content lines, etc.) are ignored.
fn hunk_ranges_from_patch(patch: &str) -> Vec<LineRange> {
    patch
        .lines()
        .filter_map(parse_new_side_hunk_header)
        .filter(|&(_start, count)| count > 0)
        .map(|(start, count)| LineRange {
            start,
            end: start + count - 1,
        })
        .collect()
}

/// Parses one hunk-header line's new-side `(start, count)`, or `None` if
/// `line` isn't a hunk header (i.e. doesn't start with `@@ -<digits>` and
/// contain a ` @@` terminator for the range section).
///
/// This only extracts the new-file side (`+start[,count]`); the old-file
/// side (`-start[,count]`) is parsed just enough to be skipped over, since
/// nothing downstream needs it.
fn parse_new_side_hunk_header(line: &str) -> Option<(usize, usize)> {
    let rest = line.strip_prefix("@@ -")?;
    let (_old_start, rest) = take_digits(rest)?;
    let (_old_count, rest) = take_optional_comma_count(rest);
    let rest = rest.strip_prefix(" +")?;
    let (new_start, rest) = take_digits(rest)?;
    let (new_count, rest) = take_optional_comma_count(rest);
    rest.strip_prefix(" @@")?;
    Some((new_start, new_count.unwrap_or(1)))
}

/// Consumes a run of one or more ASCII digits from the start of `s`,
/// returning the parsed value and the remainder. Returns `None` if `s`
/// doesn't start with a digit.
fn take_digits(s: &str) -> Option<(usize, &str)> {
    let digit_len = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    if digit_len == 0 {
        return None;
    }
    let (digits, rest) = s.split_at(digit_len);
    digits.parse().ok().map(|value| (value, rest))
}

/// Consumes an optional `,<digits>` from the start of `s`. Returns
/// `(None, s)` unchanged if `s` doesn't start with `,<digits>` (including
/// the malformed case of a `,` not followed by any digit -- the caller's
/// next required literal match will then correctly fail, matching how an
/// optional regex group that fails to match leaves the position
/// unadvanced).
fn take_optional_comma_count(s: &str) -> (Option<usize>, &str) {
    let Some(after_comma) = s.strip_prefix(',') else {
        return (None, s);
    };
    match take_digits(after_comma) {
        Some((value, rest)) => (Some(value), rest),
        None => (None, s),
    }
}

#[cfg(test)]
mod hunk_parsing_tests {
    use super::*;

    #[test]
    fn single_hunk_with_explicit_counts() {
        let ranges = hunk_ranges_from_patch(
            "@@ -10,3 +10,7 @@ SELECT 1;\n context1\n-old line\n+new line\n+new line2\n context2",
        );
        assert_eq!(ranges, vec![LineRange { start: 10, end: 16 }]);
    }

    #[test]
    fn multiple_hunks_in_one_patch() {
        let patch = "@@ -10,3 +10,7 @@ SELECT 1;\n context1\n-old line\n+new line\n+new line2\n context2\n@@ -30,2 +34,2 @@ SELECT 2;\n context3\n context4";
        let ranges = hunk_ranges_from_patch(patch);
        assert_eq!(
            ranges,
            vec![
                LineRange { start: 10, end: 16 },
                LineRange { start: 34, end: 35 },
            ]
        );
    }

    #[test]
    fn missing_count_defaults_to_one() {
        let ranges = hunk_ranges_from_patch("@@ -1 +1 @@\n-old\n+new");
        assert_eq!(ranges, vec![LineRange { start: 1, end: 1 }]);
    }

    #[test]
    fn pure_deletion_hunk_is_dropped() {
        // new-side count of 0: a hunk that only removes lines.
        let ranges = hunk_ranges_from_patch("@@ -5,3 +10,0 @@ SELECT 3;\n-a\n-b\n-c");
        assert_eq!(ranges, Vec::<LineRange>::new());
    }

    #[test]
    fn pure_deletion_hunk_mixed_with_normal_hunk_keeps_only_the_normal_one() {
        // The exact regression case that caused a fix round in the deleted
        // bash implementation: a patch containing both a pure-deletion hunk
        // (dropped) and a normal hunk (kept), not just the file boundary
        // case (no ranges at all vs. some ranges).
        let patch = "@@ -5,3 +10,0 @@ SELECT 3;\n-a\n-b\n-c\n@@ -20,2 +20,4 @@ SELECT 4;\n context\n+new1\n+new2\n context2";
        let ranges = hunk_ranges_from_patch(patch);
        assert_eq!(ranges, vec![LineRange { start: 20, end: 23 }]);
    }

    #[test]
    fn non_hunk_lines_are_ignored() {
        let ranges = hunk_ranges_from_patch("just some text\nno hunk headers here");
        assert_eq!(ranges, Vec::<LineRange>::new());
    }

    #[test]
    fn empty_patch_has_no_ranges() {
        assert_eq!(hunk_ranges_from_patch(""), Vec::<LineRange>::new());
    }
}

#[cfg(test)]
mod entry_computation_tests {
    use super::*;

    /// Builds a `DiffEntry` by deserializing a small canned JSON object.
    /// `DiffEntry` is `#[non_exhaustive]` (defined in the `octocrab` crate),
    /// so it can't be constructed with a struct literal from here -- this
    /// mirrors how real Files-API responses arrive anyway.
    fn diff_entry(json: serde_json::Value) -> DiffEntry {
        serde_json::from_value(json).expect("test fixture should deserialize as a DiffEntry")
    }

    fn entries_fixture() -> Vec<DiffEntry> {
        vec![
            diff_entry(serde_json::json!({
                "filename": "migrations/003-add-column.sql",
                "status": "modified",
                "additions": 4,
                "deletions": 1,
                "changes": 5,
                "contents_url": "https://api.github.com/repos/o/r/contents/migrations/003-add-column.sql",
                "patch": "@@ -10,3 +10,7 @@ SELECT 1;\n context1\n-old line\n+new line\n+new line2\n context2\n@@ -30,2 +34,2 @@ SELECT 2;\n context3\n context4"
            })),
            diff_entry(serde_json::json!({
                "filename": "migrations/999-huge.sql",
                "status": "modified",
                "additions": 0,
                "deletions": 0,
                "changes": 0,
                "contents_url": "https://api.github.com/repos/o/r/contents/migrations/999-huge.sql"
            })),
            diff_entry(serde_json::json!({
                "filename": "migrations/500-mixed-deletion.sql",
                "status": "modified",
                "additions": 2,
                "deletions": 3,
                "changes": 5,
                "contents_url": "https://api.github.com/repos/o/r/contents/migrations/500-mixed-deletion.sql",
                "patch": "@@ -5,3 +10,0 @@ SELECT 3;\n-a\n-b\n-c\n@@ -20,2 +20,4 @@ SELECT 4;\n context\n+new1\n+new2\n context2"
            })),
            diff_entry(serde_json::json!({
                "filename": "migrations/010-rename.sql",
                "previous_filename": "migrations/010-old-name.sql",
                "status": "renamed",
                "additions": 0,
                "deletions": 0,
                "changes": 0,
                "contents_url": "https://api.github.com/repos/o/r/contents/migrations/010-rename.sql",
                "patch": "@@ -1 +1 @@\n-old\n+new"
            })),
        ]
    }

    #[test]
    fn changed_files_lists_every_entry_by_filename_in_order() {
        let (changed_files, _hunks) = changed_files_and_hunks_from_entries(&entries_fixture());
        assert_eq!(
            changed_files,
            vec![
                PathBuf::from("migrations/003-add-column.sql"),
                PathBuf::from("migrations/999-huge.sql"),
                PathBuf::from("migrations/500-mixed-deletion.sql"),
                PathBuf::from("migrations/010-rename.sql"),
            ]
        );
    }

    #[test]
    fn multi_hunk_file_gets_both_ranges() {
        let (_changed_files, hunks) = changed_files_and_hunks_from_entries(&entries_fixture());
        assert_eq!(
            hunks[&PathBuf::from("migrations/003-add-column.sql")],
            vec![
                LineRange { start: 10, end: 16 },
                LineRange { start: 34, end: 35 },
            ]
        );
    }

    #[test]
    fn no_patch_field_gives_empty_range_list_but_keeps_the_key() {
        let (_changed_files, hunks) = changed_files_and_hunks_from_entries(&entries_fixture());
        let key = PathBuf::from("migrations/999-huge.sql");
        assert!(hunks.contains_key(&key));
        assert_eq!(hunks[&key], Vec::<LineRange>::new());
    }

    #[test]
    fn pure_deletion_hunk_mixed_with_normal_hunk_keeps_file_key_and_the_normal_range() {
        let (_changed_files, hunks) = changed_files_and_hunks_from_entries(&entries_fixture());
        let key = PathBuf::from("migrations/500-mixed-deletion.sql");
        assert!(
            hunks.contains_key(&key),
            "mixed-deletion file must keep its key, not be treated like the no-patch case"
        );
        assert_eq!(hunks[&key], vec![LineRange { start: 20, end: 23 }]);
    }

    #[test]
    fn renamed_file_is_keyed_by_new_filename_not_previous_filename() {
        let (changed_files, hunks) = changed_files_and_hunks_from_entries(&entries_fixture());
        let new_key = PathBuf::from("migrations/010-rename.sql");
        let old_key = PathBuf::from("migrations/010-old-name.sql");

        assert!(changed_files.contains(&new_key));
        assert!(!changed_files.contains(&old_key));

        assert_eq!(hunks[&new_key], vec![LineRange { start: 1, end: 1 }]);
        assert!(!hunks.contains_key(&old_key));
    }

    #[test]
    fn no_entries_yields_empty_results() {
        let (changed_files, hunks) = changed_files_and_hunks_from_entries(&[]);
        assert!(changed_files.is_empty());
        assert!(hunks.is_empty());
    }
}

#[cfg(test)]
mod fetch_tests {
    //! Exercises the actual async fetch + pagination path against a mock
    //! HTTP server (`wiremock`, the same crate `octocrab`'s own test suite
    //! uses -- see `Octocrab::builder().base_uri(...)`). This is what
    //! proves `all_pages` is genuinely walked, not just the first page.

    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Builds an `Octocrab` client pointed at a mock server instead of the
    /// real GitHub API.
    fn test_client(base_uri: &str) -> Octocrab {
        Octocrab::builder()
            .base_uri(base_uri)
            .expect("mock server URI should be a valid base_uri")
            .build()
            .expect("failed to build a test Octocrab client")
    }

    #[tokio::test]
    async fn walks_every_page_and_keeps_page_order() {
        let mock_server = MockServer::start().await;

        // Page 1: one file with a patch, and a `Link` header pointing at a
        // second page. Real GitHub pagination page URLs carry a `?page=N`
        // query string; this test uses an arbitrary distinct path instead
        // purely so the two mocks below can be matched unambiguously by
        // path alone -- octocrab just GETs whatever URL the `Link` header
        // gives it, verbatim.
        let page1_body = serde_json::json!([
            {
                "filename": "a.sql",
                "status": "modified",
                "additions": 1,
                "deletions": 0,
                "changes": 1,
                "contents_url": "https://example.com/a",
                "patch": "@@ -1 +1,2 @@\n context\n+new line"
            }
        ]);
        let next_link = format!(
            "<{}/repos/o/r/pulls/1/files/page2>; rel=\"next\"",
            mock_server.uri()
        );

        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/1/files"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(&page1_body)
                    .append_header("Link", next_link.as_str()),
            )
            .mount(&mock_server)
            .await;

        // Page 2: a second file, no `patch` field, and no further `Link`
        // header -- pagination stops here.
        let page2_body = serde_json::json!([
            {
                "filename": "b.sql",
                "status": "added",
                "additions": 0,
                "deletions": 0,
                "changes": 0,
                "contents_url": "https://example.com/b"
            }
        ]);

        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/1/files/page2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&page2_body))
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());
        let (changed_files, hunks) = fetch_changed_files_and_hunks(&client, "o", "r", 1)
            .await
            .expect("fetch should succeed against the mock server");

        // Both pages' files are present, in page order -- this is the part
        // that would silently fail (missing "b.sql") if `all_pages` weren't
        // actually walking the `Link: rel="next"` header.
        assert_eq!(
            changed_files,
            vec![PathBuf::from("a.sql"), PathBuf::from("b.sql")]
        );
        assert_eq!(
            hunks[&PathBuf::from("a.sql")],
            vec![LineRange { start: 1, end: 2 }]
        );
        assert_eq!(hunks[&PathBuf::from("b.sql")], Vec::<LineRange>::new());
    }

    #[tokio::test]
    async fn single_page_response_needs_no_further_requests() {
        let mock_server = MockServer::start().await;

        let body = serde_json::json!([
            {
                "filename": "only.sql",
                "status": "modified",
                "additions": 1,
                "deletions": 1,
                "changes": 2,
                "contents_url": "https://example.com/only",
                "patch": "@@ -1 +1 @@\n-old\n+new"
            }
        ]);

        // No `Link` header at all -- `all_pages` must not attempt a second
        // request (there's no second mock mounted to answer one).
        Mock::given(method("GET"))
            .and(path("/repos/o/r/pulls/2/files"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let client = test_client(&mock_server.uri());
        let (changed_files, hunks) = fetch_changed_files_and_hunks(&client, "o", "r", 2)
            .await
            .expect("fetch should succeed against the mock server");

        assert_eq!(changed_files, vec![PathBuf::from("only.sql")]);
        assert_eq!(
            hunks[&PathBuf::from("only.sql")],
            vec![LineRange { start: 1, end: 1 }]
        );
    }
}
