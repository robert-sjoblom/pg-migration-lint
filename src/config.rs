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
#[derive(Debug, Clone, Deserialize, Serialize)]
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
        Ok(config)
    }

    /// Create a default configuration
    pub fn default_config() -> Self {
        Self {
            migrations: MigrationsConfig::default(),
            liquibase: LiquibaseConfig::default(),
            output: OutputConfig::default(),
            cli: CliConfig::default(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::default_config()
    }
}
