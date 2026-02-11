//! Configuration file parsing
//!
//! Reads pg-migration-lint.toml configuration files.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("IO error reading config file: {0}")]
    Io(#[from] std::io::Error),

    #[error("TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),

    #[error("Invalid configuration: {0}")]
    Validation(String),
}

/// Main configuration structure
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    #[serde(default)]
    pub migrations: MigrationsConfig,

    #[serde(default)]
    pub liquibase: LiquibaseConfig,

    #[serde(default)]
    pub output: OutputConfig,

    #[serde(default)]
    pub cli: CliConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MigrationsConfig {
    /// Paths to migration directories or changelog files
    #[serde(default)]
    pub paths: Vec<PathBuf>,

    /// Migration ordering strategy
    #[serde(default = "default_strategy")]
    pub strategy: String,

    /// File patterns to include
    #[serde(default = "default_include")]
    pub include: Vec<String>,

    /// File patterns to exclude
    #[serde(default)]
    pub exclude: Vec<String>,
}

impl Default for MigrationsConfig {
    fn default() -> Self {
        Self {
            paths: vec![PathBuf::from("db/migrations")],
            strategy: default_strategy(),
            include: default_include(),
            exclude: vec![],
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LiquibaseConfig {
    /// Path to liquibase-bridge.jar
    pub bridge_jar_path: Option<PathBuf>,

    /// Path to liquibase binary
    pub binary_path: Option<PathBuf>,

    /// Path to liquibase properties file (passed as --defaults-file to liquibase CLI)
    pub properties_file: Option<PathBuf>,

    /// Strategy: "auto", "bridge", "update-sql", "xml-fallback"
    #[serde(default = "default_liquibase_strategy")]
    pub strategy: String,
}

impl Default for LiquibaseConfig {
    fn default() -> Self {
        Self {
            bridge_jar_path: Some(PathBuf::from("tools/liquibase-bridge.jar")),
            binary_path: Some(PathBuf::from("liquibase")),
            properties_file: None,
            strategy: default_liquibase_strategy(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OutputConfig {
    /// Output formats: "sarif", "sonarqube", "text"
    #[serde(default = "default_formats")]
    pub formats: Vec<String>,

    /// Output directory for report files
    #[serde(default = "default_output_dir")]
    pub dir: PathBuf,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            formats: default_formats(),
            dir: default_output_dir(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CliConfig {
    /// Exit non-zero if findings meet or exceed this severity
    #[serde(default = "default_fail_on")]
    pub fail_on: String,
}

impl Default for CliConfig {
    fn default() -> Self {
        Self {
            fail_on: default_fail_on(),
        }
    }
}

fn default_strategy() -> String {
    "filename_lexicographic".to_string()
}

fn default_include() -> Vec<String> {
    vec!["*.sql".to_string(), "*.xml".to_string()]
}

fn default_liquibase_strategy() -> String {
    "auto".to_string()
}

fn default_formats() -> Vec<String> {
    vec!["sarif".to_string()]
}

fn default_output_dir() -> PathBuf {
    PathBuf::from("build/reports/migration-lint")
}

fn default_fail_on() -> String {
    "critical".to_string()
}

impl Config {
    /// Load configuration from a file
    pub fn from_file(path: &PathBuf) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&contents)?;
        config.validate()?;
        Ok(config)
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<(), ConfigError> {
        let fail_on = &self.cli.fail_on;
        if !fail_on.eq_ignore_ascii_case("none")
            && crate::rules::Severity::parse(fail_on).is_none()
        {
            return Err(ConfigError::Validation(format!(
                "invalid fail_on value '{}'. Valid values: blocker, critical, major, minor, info, none",
                fail_on
            )));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse TOML into Config and run validation.
    fn parse_and_validate(toml_str: &str) -> Result<Config, ConfigError> {
        let config: Config = toml::from_str(toml_str)?;
        config.validate()?;
        Ok(config)
    }

    #[test]
    fn test_valid_fail_on_values() {
        for value in &["blocker", "critical", "major", "minor", "info", "none"] {
            let toml = format!("[cli]\nfail_on = \"{}\"", value);
            assert!(
                parse_and_validate(&toml).is_ok(),
                "fail_on = '{}' should be valid",
                value
            );
        }
    }

    #[test]
    fn test_invalid_fail_on_rejected() {
        let toml = "[cli]\nfail_on = \"garbage\"";
        let err = parse_and_validate(toml).unwrap_err();
        assert!(
            err.to_string().contains("invalid fail_on"),
            "Expected validation error, got: {}",
            err
        );
    }

    #[test]
    fn test_default_fail_on_is_valid() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }
}
