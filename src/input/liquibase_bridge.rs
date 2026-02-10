//! Liquibase Bridge Jar loader
//!
//! Strategy 1: Shell out to `java -jar <bridge_jar_path> --changelog <path>`
//! and parse the JSON output to produce `RawMigrationUnit`s.
//!
//! The bridge jar is a small Java program that embeds Liquibase and produces
//! JSON with exact changeset-to-SQL-to-line mapping.

use crate::config::LiquibaseConfig;
use crate::input::LoadError;
use crate::input::RawMigrationUnit;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Loader that uses the Liquibase bridge JAR to extract migration SQL.
///
/// The bridge jar embeds Liquibase and produces structured JSON output
/// mapping changesets to their SQL statements with line numbers.
pub struct BridgeLoader {
    /// Path to the bridge JAR file.
    pub jar_path: PathBuf,
}

/// A single changeset entry from the bridge JAR JSON output.
#[derive(Debug, Deserialize)]
struct BridgeChangeset {
    changeset_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    author: String,
    sql: String,
    xml_file: String,
    #[serde(default = "default_xml_line")]
    xml_line: usize,
    #[serde(default = "default_run_in_transaction")]
    run_in_transaction: bool,
}

fn default_xml_line() -> usize {
    1
}

fn default_run_in_transaction() -> bool {
    true
}

impl BridgeLoader {
    /// Create a new BridgeLoader with the given JAR path.
    pub fn new(jar_path: PathBuf) -> Self {
        Self { jar_path }
    }

    /// Load migration units from a single changelog file using the bridge JAR.
    ///
    /// Shells out to `java -jar <jar_path> --changelog <changelog_path>` and
    /// parses the resulting JSON array of changeset entries.
    pub fn load(&self, changelog_path: &Path) -> Result<Vec<RawMigrationUnit>, LoadError> {
        if !self.jar_path.exists() {
            return Err(LoadError::BridgeError {
                message: format!("Bridge JAR not found at: {}", self.jar_path.display()),
            });
        }

        let output = Command::new("java")
            .arg("-jar")
            .arg(&self.jar_path)
            .arg("--changelog")
            .arg(changelog_path)
            .output()
            .map_err(|e| LoadError::BridgeError {
                message: format!("Failed to execute java: {}", e),
            })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(LoadError::BridgeError {
                message: format!(
                    "Bridge JAR exited with status {}: {}",
                    output.status, stderr
                ),
            });
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_bridge_json(&stdout)
    }
}

/// Parse the JSON output from the bridge JAR into `RawMigrationUnit`s.
///
/// The JSON is expected to be an array of changeset objects, each containing
/// the changeset ID, SQL text, source file, line number, and transaction mode.
pub fn parse_bridge_json(json_str: &str) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let changesets: Vec<BridgeChangeset> =
        serde_json::from_str(json_str).map_err(|e| LoadError::BridgeError {
            message: format!("Failed to parse bridge JSON: {}", e),
        })?;

    let units = changesets
        .into_iter()
        .map(|cs| RawMigrationUnit {
            id: cs.changeset_id,
            sql: cs.sql,
            source_file: PathBuf::from(cs.xml_file),
            source_line_offset: cs.xml_line,
            run_in_transaction: cs.run_in_transaction,
            is_down: false,
        })
        .collect();

    Ok(units)
}

/// Load Liquibase migrations using the configured strategy.
///
/// Strategy selection:
/// - `"bridge"`: Use bridge JAR only, fail if unavailable.
/// - `"update-sql"`: Use `liquibase update-sql` only.
/// - `"xml-fallback"`: Use the lightweight XML parser only.
/// - `"auto"` (default): Try bridge -> update-sql -> xml-fallback in order.
///
/// The `paths` parameter should contain paths to changelog files.
pub fn load_liquibase(
    config: &LiquibaseConfig,
    paths: &[PathBuf],
) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let strategy = config.strategy.as_str();

    match strategy {
        "bridge" => load_with_bridge(config, paths),
        "update-sql" => load_with_updatesql(config, paths),
        "xml-fallback" => load_with_xml_fallback(paths),
        "auto" => load_auto(config, paths),
        other => Err(LoadError::Config {
            message: format!("Unknown liquibase strategy: '{}'", other),
        }),
    }
}

/// Try bridge -> update-sql -> xml-fallback in order.
fn load_auto(
    config: &LiquibaseConfig,
    paths: &[PathBuf],
) -> Result<Vec<RawMigrationUnit>, LoadError> {
    // Try bridge first
    if config.bridge_jar_path.is_some() {
        match load_with_bridge(config, paths) {
            Ok(units) => return Ok(units),
            Err(_) => { /* fall through to next strategy */ }
        }
    }

    // Try update-sql
    if config.binary_path.is_some() {
        match load_with_updatesql(config, paths) {
            Ok(units) => return Ok(units),
            Err(_) => { /* fall through to XML fallback */ }
        }
    }

    // Fall back to XML
    load_with_xml_fallback(paths)
}

/// Load using the bridge JAR strategy.
fn load_with_bridge(
    config: &LiquibaseConfig,
    paths: &[PathBuf],
) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let jar_path = config
        .bridge_jar_path
        .as_ref()
        .ok_or_else(|| LoadError::Config {
            message: "bridge_jar_path is required for 'bridge' strategy".to_string(),
        })?;

    let loader = BridgeLoader::new(jar_path.clone());
    let mut all_units = Vec::new();

    for path in paths {
        let units = loader.load(path)?;
        all_units.extend(units);
    }

    Ok(all_units)
}

/// Load using the `liquibase update-sql` strategy.
fn load_with_updatesql(
    config: &LiquibaseConfig,
    paths: &[PathBuf],
) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let binary_path = config
        .binary_path
        .as_ref()
        .ok_or_else(|| LoadError::Config {
            message: "binary_path is required for 'update-sql' strategy".to_string(),
        })?;

    let loader = super::liquibase_updatesql::UpdateSqlLoader::new(binary_path.clone());
    let mut all_units = Vec::new();

    for path in paths {
        let units = loader.load(path)?;
        all_units.extend(units);
    }

    Ok(all_units)
}

/// Load using the lightweight XML fallback parser.
fn load_with_xml_fallback(paths: &[PathBuf]) -> Result<Vec<RawMigrationUnit>, LoadError> {
    let loader = super::liquibase_xml::XmlFallbackLoader;
    let mut all_units = Vec::new();

    for path in paths {
        let units = loader.load(path)?;
        all_units.extend(units);
    }

    Ok(all_units)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_valid_bridge_json() {
        let json = r#"[
            {
                "changeset_id": "20240315-1",
                "author": "robert",
                "sql": "CREATE TABLE orders (id integer PRIMARY KEY, status text);",
                "xml_file": "db/changelog/20240315-create-orders.xml",
                "xml_line": 5,
                "run_in_transaction": true
            },
            {
                "changeset_id": "20240316-1",
                "author": "robert",
                "sql": "ALTER TABLE orders ADD COLUMN total numeric(10,2);",
                "xml_file": "db/changelog/20240316-alter-orders.xml",
                "xml_line": 3,
                "run_in_transaction": false
            }
        ]"#;

        let units = parse_bridge_json(json).expect("Should parse valid JSON");
        assert_eq!(units.len(), 2);

        assert_eq!(units[0].id, "20240315-1");
        assert_eq!(
            units[0].sql,
            "CREATE TABLE orders (id integer PRIMARY KEY, status text);"
        );
        assert_eq!(
            units[0].source_file,
            PathBuf::from("db/changelog/20240315-create-orders.xml")
        );
        assert_eq!(units[0].source_line_offset, 5);
        assert!(units[0].run_in_transaction);
        assert!(!units[0].is_down);

        assert_eq!(units[1].id, "20240316-1");
        assert!(!units[1].run_in_transaction);
    }

    #[test]
    fn test_parse_malformed_json() {
        let json = r#"{ this is not valid JSON }"#;
        let result = parse_bridge_json(json);
        assert!(result.is_err());
        match result {
            Err(LoadError::BridgeError { message }) => {
                assert!(
                    message.contains("Failed to parse bridge JSON"),
                    "Expected bridge parse error, got: {}",
                    message
                );
            }
            other => panic!("Expected BridgeError, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_empty_array() {
        let json = "[]";
        let units = parse_bridge_json(json).expect("Should parse empty array");
        assert!(units.is_empty());
    }

    #[test]
    fn test_parse_json_with_defaults() {
        // Missing optional fields should use defaults
        let json = r#"[
            {
                "changeset_id": "1",
                "sql": "SELECT 1;",
                "xml_file": "test.xml"
            }
        ]"#;

        let units = parse_bridge_json(json).expect("Should parse with defaults");
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].source_line_offset, 1); // default xml_line
        assert!(units[0].run_in_transaction); // default true
    }

    #[test]
    fn test_load_liquibase_unknown_strategy() {
        let config = LiquibaseConfig {
            bridge_jar_path: None,
            binary_path: None,
            strategy: "invalid-strategy".to_string(),
        };

        let result = load_liquibase(&config, &[]);
        assert!(result.is_err());
        match result {
            Err(LoadError::Config { message }) => {
                assert!(
                    message.contains("Unknown liquibase strategy"),
                    "Expected config error, got: {}",
                    message
                );
            }
            other => panic!("Expected Config error, got: {:?}", other),
        }
    }

    #[test]
    fn test_bridge_loader_missing_jar() {
        let loader = BridgeLoader::new(PathBuf::from("/nonexistent/path/bridge.jar"));
        let result = loader.load(Path::new("changelog.xml"));
        assert!(result.is_err());
        match result {
            Err(LoadError::BridgeError { message }) => {
                assert!(
                    message.contains("Bridge JAR not found"),
                    "Expected missing JAR error, got: {}",
                    message
                );
            }
            other => panic!("Expected BridgeError, got: {:?}", other),
        }
    }

    #[test]
    fn test_parse_json_multiple_sql_statements() {
        let json = r#"[
            {
                "changeset_id": "multi-1",
                "author": "test",
                "sql": "CREATE TABLE a (id int);\nCREATE TABLE b (id int);",
                "xml_file": "multi.xml",
                "xml_line": 1,
                "run_in_transaction": true
            }
        ]"#;

        let units = parse_bridge_json(json).expect("Should parse multi-statement SQL");
        assert_eq!(units.len(), 1);
        assert!(units[0].sql.contains("CREATE TABLE a"));
        assert!(units[0].sql.contains("CREATE TABLE b"));
    }
}
