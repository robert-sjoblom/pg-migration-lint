use super::*;

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
        let ranges = hunk_ranges_from_patch("@@ -5,3 +10,0 @@ SELECT 3;\n-a\n-b\n-c");
        assert_eq!(ranges, Vec::<LineRange>::new());
    }

    #[test]
    fn pure_deletion_hunk_mixed_with_normal_hunk_keeps_only_the_normal_one() {
        let pure_deletion_and_normal_patch = "@@ -5,3 +10,0 @@ SELECT 3;\n-a\n-b\n-c\n@@ -20,2 +20,4 @@ SELECT 4;\n context\n+new1\n+new2\n context2";
        let ranges = hunk_ranges_from_patch(pure_deletion_and_normal_patch);
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

mod fetch_tests {
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

    #[tokio::test]
    async fn walks_every_page_and_keeps_page_order() {
        let mock_server = MockServer::start().await;

        // The mock's page-2 URL doesn't need to look like GitHub's real
        // `?page=N` pagination -- octocrab just GETs whatever URL the
        // `Link` header gives it, verbatim.
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
