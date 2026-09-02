//! PR comment posting for the `github-review` subcommand.
//!
//! Reserved for a later task: delete previously-posted marker-tagged
//! review comments and post fresh ones as a single batched
//! `create_review(...)` call (octocrab >=0.47.0), and find-then-PATCH-or-POST
//! the marker-tagged summary issue comment, via octocrab's
//! `PullRequestHandler` and `IssueHandler`.
//!
//! Empty as of this task -- no real logic yet.
