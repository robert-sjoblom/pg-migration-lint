//! Liquibase `update-sql` output parser
//!
//! Strategy 2: Shell out to `liquibase update-sql` and heuristically parse
//! the output to extract changeset SQL. The output contains comments that
//! mark changeset boundaries:
//!
//! ```text
//! -- Changeset changelog.xml::id::author
//! CREATE TABLE ...;
//! ```
//!
//! This module parses those markers and extracts the SQL between them.

use crate::input::LoadError;
use crate::input::RawMigrationUnit;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter for unique offline temp directories.
static OFFLINE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Loader that uses `liquibase update-sql` to generate SQL output,
/// then parses changeset boundaries from the SQL comments.
///
/// Always runs Liquibase in offline mode (`offline:postgresql`) so that
/// SQL is generated for **all** changesets without needing a live database.
/// This is required because pg-migration-lint replays the full migration
/// history to build its table catalog.
pub struct UpdateSqlLoader {
    /// Path to the liquibase binary.
    pub binary_path: PathBuf,
    /// Optional path to a liquibase properties file (--defaults-file).
    pub properties_file: Option<PathBuf>,
}

impl UpdateSqlLoader {
    /// Create a new UpdateSqlLoader with the given binary path.
    pub fn new(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            properties_file: None,
        }
    }

    /// Create a new UpdateSqlLoader with a binary path and optional properties file.
    pub fn with_properties(binary_path: PathBuf, properties_file: Option<PathBuf>) -> Self {
        Self {
            binary_path,
            properties_file,
        }
    }

    /// Load migration units from a changelog file by running `liquibase update-sql`.
    ///
    /// Runs in offline mode so all changesets produce SQL regardless of what
    /// has been applied to any real database. An empty `databasechangelog.csv`
    /// is created automatically in a temp directory.
    pub fn load(&self, changelog_path: &Path) -> Result<Vec<RawMigrationUnit>, LoadError> {
        let mut cmd = Command::new(&self.binary_path);
        if let Some(ref props) = self.properties_file {
            cmd.arg("--defaults-file").arg(props);
        }

        // Run in offline mode so Liquibase generates SQL for ALL changesets
        // without connecting to a database. Each invocation gets its own temp
        // directory with a fresh empty CSV so parallel runs don't conflict
        // (Liquibase writes applied changeset IDs back to the CSV).
        let seq = OFFLINE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let offline_dir = std::env::temp_dir().join(format!(
            "pg-migration-lint-offline-{}-{}",
            std::process::id(),
            seq
        ));
        std::fs::create_dir_all(&offline_dir).map_err(|e| LoadError::BridgeError {
            message: format!("Failed to create offline temp dir: {}", e),
        })?;
        let csv_path = offline_dir.join("databasechangelog.csv");
        std::fs::write(&csv_path, "").map_err(|e| LoadError::BridgeError {
            message: format!("Failed to create empty databasechangelog.csv: {}", e),
        })?;
        let offline_url = format!("offline:postgresql?changeLogFile={}", csv_path.display());
        cmd.arg("--url").arg(&offline_url);

        // Liquibase resolves --changelog-file relative to its search path.
        // Set --search-path to the changelog's parent directory and pass
        // just the filename so Liquibase can find it (and any included files).
        let search_path = changelog_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let changelog_name = changelog_path
            .file_name()
            .map(Path::new)
            .unwrap_or(changelog_path);
        cmd.arg("--search-path").arg(search_path);
        let output = cmd
            .arg("update-sql")
            .arg("--changelog-file")
            .arg(changelog_name)
            .output()
            .map_err(|e| LoadError::BridgeError {
                message: format!(
                    "Failed to execute liquibase at '{}': {}",
                    self.binary_path.display(),
                    e
                ),
            })?;

        // Best-effort cleanup of the temporary offline directory.
        let _ = std::fs::remove_dir_all(&offline_dir);

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("duplicate") && stderr.contains("changeset") {
                eprintln!(
                    "warning: liquibase update-sql rejected the changelog due to duplicate \
                     changeset identifiers. This is a known limitation of update-sql; the \
                     bridge JAR handles duplicate <include> directives correctly. \
                     Consider configuring bridge_jar_path for this project."
                );
            }
            return Err(LoadError::BridgeError {
                message: format!(
                    "liquibase update-sql exited with status {}: {}",
                    output.status, stderr
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_updatesql_output(&stdout)
    }
}

/// A changeset parsed from the update-sql output, before conversion to RawMigrationUnit.
#[derive(Debug)]
struct ParsedChangeset {
    id: String,
    source_file: String,
    sql_lines: Vec<String>,
}

/// Parse the output of `liquibase update-sql` into `RawMigrationUnit`s.
///
/// Liquibase `update-sql` output contains changeset markers in the form:
/// ```text
/// -- Changeset changelog.xml::changeset-id::author
/// SQL statements...
/// ```
///
/// This function extracts SQL between consecutive changeset markers.
pub fn parse_updatesql_output(output: &str) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let mut changesets: Vec<ParsedChangeset> = Vec::new();
    let mut current: Option<ParsedChangeset> = None;

    for line in output.lines() {
        if let Some((file, id)) = parse_changeset_marker(line) {
            // Save previous changeset if any
            if let Some(cs) = current.take() {
                changesets.push(cs);
            }
            current = Some(ParsedChangeset {
                id,
                source_file: file,
                sql_lines: Vec::new(),
            });
        } else if let Some(ref mut cs) = current {
            // Skip Liquibase internal comments and blank lines at start
            let trimmed = line.trim();
            if !trimmed.is_empty() && !is_liquibase_internal_comment(trimmed) {
                cs.sql_lines.push(line.to_string());
            }
        }
        // Lines before the first changeset marker are preamble (ignored)
    }

    // Don't forget the last changeset
    if let Some(cs) = current.take() {
        changesets.push(cs);
    }

    let units = changesets
        .into_iter()
        .filter(|cs| !cs.sql_lines.is_empty())
        .map(|cs| {
            let sql = cs.sql_lines.join("\n");
            RawMigrationUnit {
                id: cs.id,
                sql,
                source_file: PathBuf::from(cs.source_file),
                source_line_offset: 1,
                run_in_transaction: true, // update-sql doesn't reliably expose this
                is_down: false,
            }
        })
        .collect();

    Ok(units)
}

/// Try to parse a line as a Liquibase changeset marker.
///
/// Expected format: `-- Changeset <file>::<id>::<author>`
/// Also handles: `-- Changeset <file>::<id>::<author> (with extra info)`
///
/// Returns `Some((file, id))` if the line matches, `None` otherwise.
fn parse_changeset_marker(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim();

    // Match "-- Changeset " prefix (case-insensitive on "Changeset")
    let rest = if let Some(r) = trimmed.strip_prefix("-- Changeset ") {
        r
    } else if let Some(r) = trimmed.strip_prefix("-- changeset ") {
        r
    } else {
        return None;
    };

    // Split on "::" — format is file::id::author
    let parts: Vec<&str> = rest.splitn(3, "::").collect();
    if parts.len() < 2 {
        return None;
    }

    let file = parts[0].trim().to_string();
    let id = parts[1].trim().to_string();

    Some((file, id))
}

/// Check if a line is a Liquibase internal comment that should be skipped.
///
/// These are metadata comments that Liquibase inserts but aren't part of
/// the actual migration SQL.
fn is_liquibase_internal_comment(line: &str) -> bool {
    let trimmed = line.trim();

    // Skip DATABASECHANGELOGLOCK and DATABASECHANGELOG insert statements
    if trimmed.starts_with("INSERT INTO DATABASECHANGELOG") {
        return true;
    }
    if trimmed.starts_with("INSERT INTO public.DATABASECHANGELOG") {
        return true;
    }

    // Skip lock table operations
    if trimmed.contains("DATABASECHANGELOGLOCK") {
        return true;
    }

    // Skip Liquibase preamble comments
    if trimmed.starts_with("-- Lock Database") {
        return true;
    }
    if trimmed.starts_with("-- Release Database Lock") {
        return true;
    }
    if trimmed.starts_with("-- *************") {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_changeset_marker_standard() {
        let line = "-- Changeset changelog.xml::20240315-1::robert";
        let result = parse_changeset_marker(line);
        assert_eq!(
            result,
            Some(("changelog.xml".to_string(), "20240315-1".to_string()))
        );
    }

    #[test]
    fn test_parse_changeset_marker_lowercase() {
        let line = "-- changeset db/changelog.xml::create-table::admin";
        let result = parse_changeset_marker(line);
        assert_eq!(
            result,
            Some(("db/changelog.xml".to_string(), "create-table".to_string()))
        );
    }

    #[test]
    fn test_parse_changeset_marker_not_a_marker() {
        assert!(parse_changeset_marker("-- This is a regular comment").is_none());
        assert!(parse_changeset_marker("SELECT 1;").is_none());
        assert!(parse_changeset_marker("").is_none());
    }

    #[test]
    fn test_parse_changeset_marker_too_few_parts() {
        // Only one part, no "::" separator
        assert!(parse_changeset_marker("-- Changeset nodelimiter").is_none());
    }

    #[test]
    fn test_parse_updatesql_single_changeset() {
        let output = r#"-- *********************************************************************
-- Update Database Script
-- *********************************************************************

-- Lock Database

-- Changeset changelog.xml::1::author
CREATE TABLE users (
    id integer PRIMARY KEY,
    name text NOT NULL
);

INSERT INTO DATABASECHANGELOG (ID, AUTHOR) VALUES ('1', 'author');

-- Release Database Lock
"#;

        let units = parse_updatesql_output(output).expect("Should parse output");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].id, "1");
        assert_eq!(units[0].source_file, PathBuf::from("changelog.xml"));
        assert!(units[0].sql.contains("CREATE TABLE users"));
        // Should not contain DATABASECHANGELOG insert
        assert!(!units[0].sql.contains("DATABASECHANGELOG"));
    }

    #[test]
    fn test_parse_updatesql_multiple_changesets() {
        let output = r#"-- Changeset db/changelog.xml::create-users::alice
CREATE TABLE users (id int PRIMARY KEY);

INSERT INTO DATABASECHANGELOG (ID, AUTHOR) VALUES ('create-users', 'alice');

-- Changeset db/changelog.xml::create-orders::bob
CREATE TABLE orders (id int PRIMARY KEY, user_id int);

INSERT INTO DATABASECHANGELOG (ID, AUTHOR) VALUES ('create-orders', 'bob');
"#;

        let units = parse_updatesql_output(output).expect("Should parse output");
        assert_eq!(units.len(), 2);

        assert_eq!(units[0].id, "create-users");
        assert!(units[0].sql.contains("CREATE TABLE users"));

        assert_eq!(units[1].id, "create-orders");
        assert!(units[1].sql.contains("CREATE TABLE orders"));
    }

    #[test]
    fn test_parse_updatesql_empty_output() {
        let output = "";
        let units = parse_updatesql_output(output).expect("Should handle empty output");
        assert!(units.is_empty());
    }

    #[test]
    fn test_parse_updatesql_only_preamble() {
        let output = r#"-- *********************************************************************
-- Update Database Script
-- *********************************************************************
-- Lock Database
-- Release Database Lock
"#;

        let units = parse_updatesql_output(output).expect("Should handle preamble-only");
        assert!(units.is_empty());
    }

    #[test]
    fn test_is_liquibase_internal_comment() {
        assert!(is_liquibase_internal_comment(
            "INSERT INTO DATABASECHANGELOG (ID) VALUES ('1');"
        ));
        assert!(is_liquibase_internal_comment("-- Lock Database"));
        assert!(is_liquibase_internal_comment("-- Release Database Lock"));
        assert!(is_liquibase_internal_comment(
            "-- *********************************************************************"
        ));
        assert!(is_liquibase_internal_comment(
            "UPDATE DATABASECHANGELOGLOCK SET LOCKED = TRUE;"
        ));

        // Regular SQL should not be filtered
        assert!(!is_liquibase_internal_comment("CREATE TABLE foo (id int);"));
        assert!(!is_liquibase_internal_comment("-- A normal comment"));
    }

    #[test]
    fn test_parse_updatesql_changeset_with_multi_statement() {
        let output = r#"-- Changeset changelog.xml::multi::dev
CREATE TABLE a (id int);
CREATE TABLE b (id int);
ALTER TABLE a ADD COLUMN name text;
"#;

        let units = parse_updatesql_output(output).expect("Should parse multi-statement");
        assert_eq!(units.len(), 1);
        assert!(units[0].sql.contains("CREATE TABLE a"));
        assert!(units[0].sql.contains("CREATE TABLE b"));
        assert!(units[0].sql.contains("ALTER TABLE a"));
    }
}
