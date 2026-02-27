//! SQL file loading
//!
//! Reads `.sql` migration files from disk, parses them into IR using the
//! pg_query parser, and returns `MigrationUnit`s ready for catalog replay
//! and linting.

use crate::input::{LoadError, MigrationHistory, MigrationUnit};
use crate::parser::pg_query::parse_sql;
use std::path::{Path, PathBuf};

/// Loader for plain SQL migration files.
///
/// Reads `.sql` files from the configured migration directories, sorted
/// lexicographically by filename. Each file becomes one `MigrationUnit`.
///
/// Down migrations are detected by filename suffix: the stem (minus `.sql`)
/// must end with `.down` or `_down`.
pub struct SqlLoader {
    run_in_transaction: bool,
}

impl SqlLoader {
    /// Create a new `SqlLoader` with the given default `run_in_transaction` value.
    pub fn new(run_in_transaction: bool) -> Self {
        Self { run_in_transaction }
    }

    /// Load migrations from the given paths.
    ///
    /// Each path can be either a directory (in which case all `.sql` files
    /// within it are loaded, sorted lexicographically) or a direct path to
    /// a `.sql` file.
    ///
    /// Files that fail to read or parse are reported as errors. The loader
    /// collects all SQL files across all paths, sorts them, and returns the
    /// complete migration history.
    pub fn load(&self, paths: &[PathBuf]) -> Result<MigrationHistory, LoadError> {
        let mut sql_files: Vec<PathBuf> = Vec::new();

        for path in paths {
            if path.is_dir() {
                let entries = collect_sql_files(path)?;
                sql_files.extend(entries);
            } else if path.is_file() {
                if is_sql_file(path) {
                    sql_files.push(path.clone());
                }
            } else {
                return Err(LoadError::Io {
                    path: path.clone(),
                    source: std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("Path does not exist: {}", path.display()),
                    ),
                });
            }
        }

        // Sort lexicographically by filename to ensure deterministic ordering
        sql_files.sort_by(|a, b| {
            let a_name = a.file_name().unwrap_or_default();
            let b_name = b.file_name().unwrap_or_default();
            a_name.cmp(b_name)
        });

        let mut units = Vec::new();
        for file in &sql_files {
            let unit = self.load_file(file)?;
            units.push(unit);
        }

        Ok(MigrationHistory { units })
    }

    /// Load a single SQL file and parse it into a `MigrationUnit`.
    ///
    /// The file is read entirely into memory, parsed into IR nodes, and
    /// wrapped in a `MigrationUnit` with metadata derived from the filename.
    pub fn load_file(&self, path: &Path) -> Result<MigrationUnit, LoadError> {
        let source = std::fs::read_to_string(path).map_err(|e| LoadError::Io {
            path: path.to_path_buf(),
            source: e,
        })?;

        let statements = parse_sql(&source);

        let filename = path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());

        let is_down = is_down_migration(&filename);

        Ok(MigrationUnit {
            id: filename,
            statements,
            source_file: path.to_path_buf(),
            source_line_offset: 1,
            run_in_transaction: self.run_in_transaction,
            is_down,
        })
    }
}

impl Default for SqlLoader {
    fn default() -> Self {
        Self {
            run_in_transaction: true,
        }
    }
}

/// Collect all `.sql` files from a directory (non-recursive).
fn collect_sql_files(dir: &Path) -> Result<Vec<PathBuf>, LoadError> {
    let entries = std::fs::read_dir(dir).map_err(|e| LoadError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    let mut files: Vec<PathBuf> = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| LoadError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;

        let path = entry.path();
        if path.is_file() && is_sql_file(&path) {
            files.push(path);
        }
    }

    Ok(files)
}

/// Check if a path has a `.sql` extension.
fn is_sql_file(path: &Path) -> bool {
    path.extension()
        .map(|ext| ext.eq_ignore_ascii_case("sql"))
        .unwrap_or(false)
}

/// Detect down migration by checking that the filename (minus `.sql` extension)
/// ends with `.down` or `_down`.
///
/// Matches: `000001_create_users.down.sql`, `V001__drop_table_down.sql`
/// Does not match: `downtown_orders.sql`, `V001_shutdown.sql`
fn is_down_migration(filename: &str) -> bool {
    let stem = filename
        .strip_suffix(".sql")
        .or_else(|| filename.strip_suffix(".SQL"))
        .unwrap_or(filename);
    stem.ends_with(".down") || stem.ends_with("_down")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_is_sql_file() {
        assert!(is_sql_file(Path::new("V001__create_table.sql")));
        assert!(is_sql_file(Path::new("V001__create_table.SQL")));
        assert!(is_sql_file(Path::new("/path/to/migration.sql")));
        assert!(!is_sql_file(Path::new("changelog.xml")));
        assert!(!is_sql_file(Path::new("readme.md")));
        assert!(!is_sql_file(Path::new("noext")));
    }

    #[test]
    fn test_load_file_basic() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users.sql");
        fs::write(
            &file_path,
            "CREATE TABLE users (id integer PRIMARY KEY, name text NOT NULL);",
        )
        .expect("Failed to write test file");

        let unit = SqlLoader::default()
            .load_file(&file_path)
            .expect("Failed to load file");
        assert_eq!(unit.id, "V001__create_users.sql");
        assert_eq!(unit.source_file, file_path);
        assert_eq!(unit.source_line_offset, 1);
        assert!(unit.run_in_transaction);
        assert!(!unit.is_down);
        assert!(!unit.statements.is_empty());
    }

    #[test]
    fn test_is_down_migration_dot_suffix() {
        assert!(is_down_migration("V001__create_users.down.sql"));
        assert!(is_down_migration("000001_create_users.down.sql"));
    }

    #[test]
    fn test_is_down_migration_underscore_suffix() {
        assert!(is_down_migration("V001__create_users_down.sql"));
        assert!(is_down_migration("000001_create_users_down.sql"));
    }

    #[test]
    fn test_is_down_migration_case_insensitive_extension() {
        assert!(is_down_migration("V001__drop.down.SQL"));
        assert!(is_down_migration("V001__drop_down.SQL"));
    }

    #[test]
    fn test_is_not_down_migration_regular_files() {
        assert!(!is_down_migration("V001__create_users.sql"));
        assert!(!is_down_migration("000001_create_users.sql"));
    }

    #[test]
    fn test_is_not_down_false_positive_contains_down() {
        // "down" appears in the name but is not a suffix — must NOT match
        assert!(!is_down_migration("downtown_orders.sql"));
        assert!(!is_down_migration("V001_shutdown.sql"));
        assert!(!is_down_migration("markdown_notes.sql"));
        assert!(!is_down_migration("V002__breakdown_tables.sql"));
        assert!(!is_down_migration("V003__download_cache.sql"));
    }

    #[test]
    fn test_load_file_down_migration_dot() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users.down.sql");
        fs::write(&file_path, "DROP TABLE users;").expect("Failed to write test file");

        let unit = SqlLoader::default()
            .load_file(&file_path)
            .expect("Failed to load file");
        assert!(unit.is_down);
    }

    #[test]
    fn test_load_file_down_migration_underscore() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users_down.sql");
        fs::write(&file_path, "DROP TABLE users;").expect("Failed to write test file");

        let unit = SqlLoader::default()
            .load_file(&file_path)
            .expect("Failed to load file");
        assert!(unit.is_down);
    }

    #[test]
    fn test_load_file_not_down_migration() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users.sql");
        fs::write(&file_path, "CREATE TABLE users (id int);").expect("Failed to write test file");

        let unit = SqlLoader::default()
            .load_file(&file_path)
            .expect("Failed to load file");
        assert!(!unit.is_down);
    }

    #[test]
    fn test_load_file_nonexistent() {
        let result = SqlLoader::default().load_file(Path::new("/nonexistent/path/migration.sql"));
        assert!(result.is_err());
        match result {
            Err(LoadError::Io { path, .. }) => {
                assert_eq!(path, PathBuf::from("/nonexistent/path/migration.sql"));
            }
            other => panic!("Expected LoadError::Io, got: {:?}", other),
        }
    }

    #[test]
    fn test_load_file_multi_statement() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__multi.sql");
        fs::write(
            &file_path,
            "CREATE TABLE a (id int);\nCREATE TABLE b (id int);\nCREATE INDEX idx ON a (id);",
        )
        .expect("Failed to write test file");

        let unit = SqlLoader::default()
            .load_file(&file_path)
            .expect("Failed to load file");
        assert_eq!(unit.statements.len(), 3);
    }

    #[test]
    fn test_loader_directory() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");

        // Create files in non-alphabetical order
        fs::write(
            dir.path().join("V002__add_index.sql"),
            "CREATE INDEX idx ON users (name);",
        )
        .expect("write");
        fs::write(
            dir.path().join("V001__create_table.sql"),
            "CREATE TABLE users (id int, name text);",
        )
        .expect("write");
        fs::write(
            dir.path().join("V003__alter.sql"),
            "ALTER TABLE users ADD COLUMN email text;",
        )
        .expect("write");

        // Also create a non-SQL file that should be ignored
        fs::write(dir.path().join("README.md"), "# Migrations").expect("write");

        let loader = SqlLoader::default();
        let history = loader
            .load(&[dir.path().to_path_buf()])
            .expect("Failed to load migrations");

        assert_eq!(history.units.len(), 3);

        // Should be sorted lexicographically
        assert_eq!(history.units[0].id, "V001__create_table.sql");
        assert_eq!(history.units[1].id, "V002__add_index.sql");
        assert_eq!(history.units[2].id, "V003__alter.sql");
    }

    #[test]
    fn test_loader_single_file_path() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("migration.sql");
        fs::write(&file_path, "CREATE TABLE t (id int);").expect("write");

        let loader = SqlLoader::default();
        let history = loader.load(&[file_path]).expect("Failed to load migration");
        assert_eq!(history.units.len(), 1);
    }

    #[test]
    fn test_loader_nonexistent_path() {
        let loader = SqlLoader::default();
        let result = loader.load(&[PathBuf::from("/nonexistent/path")]);
        assert!(result.is_err());
    }

    #[test]
    fn test_loader_empty_paths() {
        let loader = SqlLoader::default();
        let history = loader.load(&[]).expect("Empty paths should succeed");
        assert!(history.units.is_empty());
    }

    #[test]
    fn test_loader_multiple_directories() {
        let dir1 = tempfile::tempdir().expect("Failed to create temp dir");
        let dir2 = tempfile::tempdir().expect("Failed to create temp dir");

        fs::write(
            dir1.path().join("V001__first.sql"),
            "CREATE TABLE a (id int);",
        )
        .expect("write");
        fs::write(
            dir2.path().join("V002__second.sql"),
            "CREATE TABLE b (id int);",
        )
        .expect("write");

        let loader = SqlLoader::default();
        let history = loader
            .load(&[dir1.path().to_path_buf(), dir2.path().to_path_buf()])
            .expect("Failed to load migrations");

        assert_eq!(history.units.len(), 2);
    }

    #[test]
    fn test_loader_new_false_produces_no_transaction() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users.sql");
        fs::write(&file_path, "CREATE TABLE users (id integer PRIMARY KEY);")
            .expect("Failed to write test file");

        let loader = SqlLoader::new(false);
        let unit = loader.load_file(&file_path).expect("Failed to load file");
        assert!(!unit.run_in_transaction);
    }

    #[test]
    fn test_loader_default_produces_transaction_true() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        let file_path = dir.path().join("V001__create_users.sql");
        fs::write(&file_path, "CREATE TABLE users (id integer PRIMARY KEY);")
            .expect("Failed to write test file");

        let loader = SqlLoader::default();
        let unit = loader.load_file(&file_path).expect("Failed to load file");
        assert!(unit.run_in_transaction);
    }

    #[test]
    fn test_collect_sql_files_ignores_non_sql() {
        let dir = tempfile::tempdir().expect("Failed to create temp dir");
        fs::write(dir.path().join("migration.sql"), "SELECT 1;").expect("write");
        fs::write(dir.path().join("changelog.xml"), "<xml/>").expect("write");
        fs::write(dir.path().join("notes.txt"), "notes").expect("write");

        let files = collect_sql_files(dir.path()).expect("collect failed");
        assert_eq!(files.len(), 1);
        assert!(files[0].to_string_lossy().contains("migration.sql"));
    }
}
