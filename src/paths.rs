//! Path-normalization helper shared by every changed-file/path-matching
//! comparison in the binary: `main.rs`'s `lint_history` (matching
//! `unit.source_file` against a `--changed-files`/`--changed-files-from`
//! list, or the `github-review` subcommand's PR changed-files list) and
//! `github::filter::PathNormalizer` (routing findings against GitHub's
//! `hunks` map).
//!
//! All of these compare two spellings of what should be the same path, and
//! one side is frequently `./`-prefixed: the default bare-filename config
//! lookup (`Config::resolve_paths` falling back to `Path::new(".")` as
//! `config_dir`) produces `unit.source_file`s like `./migrations/foo.sql`.
//! `Path`'s `Eq`/`Hash`/`ends_with` all treat a leading `./` as a real,
//! significant [`Component::CurDir`] rather than a no-op, so two otherwise-
//! identical paths that differ only by a leading `./` never compare equal,
//! and never satisfy `ends_with` in either direction, without stripping it
//! first.

use std::path::{Component, Path, PathBuf};

/// Drops a leading `./` from `path` (a real [`Component::CurDir`], which
/// `Path`'s `Eq`/`Hash`/`ends_with` treat as significant) -- [`Path::components`]
/// already normalizes away interior `.` components, so this only ever
/// matters for a leading one.
pub fn without_curdir_components(path: &Path) -> PathBuf {
    let mut cleaned = PathBuf::new();
    for component in path.components() {
        if component != Component::CurDir {
            cleaned.push(component);
        }
    }

    if cleaned.as_os_str().is_empty() {
        PathBuf::from(".")
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_a_leading_dot_slash() {
        assert_eq!(
            without_curdir_components(Path::new("./db/001.sql")),
            PathBuf::from("db/001.sql")
        );
    }

    #[test]
    fn leaves_a_plain_relative_path_alone() {
        assert_eq!(
            without_curdir_components(Path::new("db/001.sql")),
            PathBuf::from("db/001.sql")
        );
    }

    #[test]
    fn leaves_an_absolute_path_absolute() {
        assert_eq!(
            without_curdir_components(Path::new("/repo/db/001.sql")),
            PathBuf::from("/repo/db/001.sql")
        );
    }

    #[test]
    fn of_a_bare_dot_stays_a_dot() {
        assert_eq!(
            without_curdir_components(Path::new(".")),
            PathBuf::from(".")
        );
    }

    #[test]
    fn of_a_bare_filename_with_no_prefix_is_unchanged() {
        assert_eq!(
            without_curdir_components(Path::new("foo.sql")),
            PathBuf::from("foo.sql")
        );
    }
}
