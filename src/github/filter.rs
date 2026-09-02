//! Finding filtering for the `github-review` subcommand.
//!
//! Reserved for a later task: given the lint pipeline's `Vec<Finding>` and
//! [`super::files`]'s per-file diff-hunk ranges, split findings into
//! inline-eligible (the finding's line falls inside a diff hunk for that
//! file) versus summary-only (everything else -- outside the diff, or in a
//! file GitHub omitted a `patch` for because the diff was too large).
//!
//! Empty as of this task -- no real logic yet.
