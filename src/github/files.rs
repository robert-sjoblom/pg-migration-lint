//! Changed-files + diff-hunk computation for the `github-review` subcommand.
//!
//! Reserved for a later task: fetch a pull request's changed files via
//! octocrab's `PullRequestHandler::list_files()` (paginated with
//! `all_pages()` -- pagination is load-bearing, not optional; GitHub's
//! default page size is 30 and this repo's own `enterprise/` fixture alone
//! has 31 migration files), then parse each file's `patch` field's
//! `@@ -a,b +c,d @@` headers into inclusive 1-based new-file line ranges
//! for hunk-overlap filtering (see [`super::filter`]).
//!
//! Empty as of this task -- no real logic yet.
