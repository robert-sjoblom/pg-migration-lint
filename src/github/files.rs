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

#[cfg(test)]
mod tests;

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
///   API returned them. Renamed files are keyed by their *new* path
///   (`DiffEntry::filename`), never `previous_filename`.
/// - `hunks` -- a map from that same path to its list of inline-eligible
///   [`LineRange`]s. Every changed file has an entry, possibly empty
///   (GitHub omitted `patch`, or every hunk was pure-deletion).
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
        // Key off `filename` (the new path), never `previous_filename`.
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
