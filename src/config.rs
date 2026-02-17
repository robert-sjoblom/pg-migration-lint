//! Configuration file parsing
//!
//! Reads pg-migration-lint.toml configuration files.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
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

    #[serde(default)]
    pub rules: RulesConfig,
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

    /// Default schema for unqualified table names (default: "public").
    /// Used to normalize unqualified references so that `orders` and
    /// `public.orders` resolve to the same catalog entry.
    #[serde(default = "default_schema")]
    pub default_schema: String,

    /// Default `run_in_transaction` for plain SQL files.
    /// When `None`, defaults to `true` (backward compatible).
    /// Set to `false` for golang-migrate repos where files run outside transactions.
    #[serde(default)]
    pub run_in_transaction: Option<bool>,
}

impl Default for MigrationsConfig {
    fn default() -> Self {
        Self {
            paths: vec![PathBuf::from("db/migrations")],
            strategy: default_strategy(),
            include: default_include(),
            exclude: vec![],
            default_schema: default_schema(),
            run_in_transaction: None,
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

    /// Strategy: "auto", "bridge", "update-sql"
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

    /// Optional prefix to strip from finding file paths before emitting reports.
    /// Useful when running from a project root but SonarQube expects module-relative paths.
    /// Example: `strip_prefix = "impl/"` turns `impl/src/main/...` into `src/main/...`.
    #[serde(default)]
    pub strip_prefix: Option<String>,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            formats: default_formats(),
            dir: default_output_dir(),
            strip_prefix: None,
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

/// Configuration for rule selection.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RulesConfig {
    /// Rule IDs to disable globally (e.g., `["PGM007", "PGM101"]`).
    /// Findings from disabled rules are not emitted.
    /// Invalid rule IDs cause a config-load error (exit 2).
    #[serde(default)]
    pub disabled: Vec<crate::rules::RuleId>,
}

fn default_schema() -> String {
    "public".to_string()
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

/// Valid section names for `--explain-config`.
const VALID_SECTIONS: &[&str] = &["migrations", "liquibase", "output", "cli", "rules"];

const SECTION_MIGRATIONS: &str = "\
[migrations]

  Note: relative paths in the config file are resolved relative to the
  config file's directory, not the current working directory. This allows
  running the tool from any directory with --config pointing at the file.

  paths = [\"db/migrations\"]
    Paths to migration directories or changelog files.
    Type: list of paths
    Default: [\"db/migrations\"]

  strategy = \"filename_lexicographic\"
    Migration ordering strategy.
    Type: string
    Values: \"filename_lexicographic\", \"liquibase\"
    Default: \"filename_lexicographic\"

  include = [\"*.sql\", \"*.xml\"]
    Glob patterns for files to include.
    Type: list of strings
    Default: [\"*.sql\", \"*.xml\"]

  exclude = []
    Glob patterns for files to exclude.
    Type: list of strings
    Default: []

  default_schema = \"public\"
    Schema applied to unqualified table names so that `orders` and
    `public.orders` resolve to the same catalog entry.
    Type: string
    Default: \"public\"

  run_in_transaction = true
    Whether plain SQL files run inside a transaction by default.
    Set to false for golang-migrate repos where files run outside transactions.
    Type: boolean (optional)
    Default: true (when absent)
";

const SECTION_LIQUIBASE: &str = "\
[liquibase]

  bridge_jar_path = \"tools/liquibase-bridge.jar\"
    Path to the liquibase-bridge.jar for structured SQL extraction.
    Type: path (optional)
    Default: \"tools/liquibase-bridge.jar\"

  binary_path = \"liquibase\"
    Path to the liquibase binary for update-sql fallback.
    Type: path (optional)
    Default: \"liquibase\"

  properties_file
    Path to liquibase properties file (passed as --defaults-file).
    Type: path (optional)
    Default: none

  strategy = \"auto\"
    Liquibase sub-strategy: which method to use for SQL extraction.
    Type: string
    Values: \"auto\", \"bridge\", \"update-sql\"
    Default: \"auto\" (tries bridge -> update-sql)
";

const SECTION_OUTPUT: &str = "\
[output]

  formats = [\"sarif\"]
    Output report formats to generate.
    Type: list of strings
    Values: \"sarif\", \"sonarqube\", \"text\"
    Default: [\"sarif\"]

  dir = \"build/reports/migration-lint\"
    Directory where report files are written.
    Type: path
    Default: \"build/reports/migration-lint\"

  strip_prefix = \"impl/\"
    Optional prefix to strip from finding file paths before emitting reports.
    Useful when running from a project root but the report consumer (e.g.
    SonarQube) expects module-relative paths.
    Type: string (optional)
    Default: none
";

const SECTION_CLI: &str = "\
[cli]

  fail_on = \"critical\"
    Exit non-zero if any finding meets or exceeds this severity.
    Set to \"none\" to always exit 0.
    Type: string
    Values: \"blocker\", \"critical\", \"major\", \"minor\", \"info\", \"none\"
    Default: \"critical\"
";

const SECTION_RULES: &str = "\
[rules]

  disabled = []
    Rule IDs to disable globally. Findings from disabled rules are not emitted.
    Example: [\"PGM007\", \"PGM101\"]
    Type: list of strings
    Default: []
";

/// Print configuration reference for a specific section, or all sections.
///
/// Pass `"all"` to print everything, or a section name like `"migrations"`.
/// Returns an error for unknown section names.
pub fn explain_config(section: &str) -> Result<(), ConfigError> {
    let sections: &[(&str, &str)] = &[
        ("migrations", SECTION_MIGRATIONS),
        ("liquibase", SECTION_LIQUIBASE),
        ("output", SECTION_OUTPUT),
        ("cli", SECTION_CLI),
        ("rules", SECTION_RULES),
    ];

    if section == "all" {
        for (i, (_, text)) in sections.iter().enumerate() {
            if i > 0 {
                println!();
            }
            print!("{text}");
        }
    } else if let Some((_, text)) = sections.iter().find(|(name, _)| *name == section) {
        print!("{text}");
    } else {
        return Err(ConfigError::Validation(format!(
            "unknown config section '{}'. Valid sections: {}",
            section,
            VALID_SECTIONS.join(", ")
        )));
    }

    Ok(())
}

impl Config {
    /// Load configuration from a file.
    ///
    /// Relative paths in the config are resolved relative to the config file's
    /// parent directory, so running from any CWD works as long as `--config`
    /// points at the right file.
    pub fn from_file(path: &PathBuf) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path)?;
        let mut config: Config = toml::from_str(&contents)?;
        config.validate()?;

        // Resolve relative paths against the config file's directory.
        // Note: Path::parent() on a bare filename returns Some(""), not None,
        // so we also check for empty to fall back to ".".
        let config_dir = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        config.resolve_paths(config_dir);

        Ok(config)
    }

    /// Prepend `config_dir` to every relative `PathBuf` field so that config
    /// paths resolve correctly regardless of the current working directory.
    ///
    /// Absolute paths are left unchanged.
    pub fn resolve_paths(&mut self, config_dir: &Path) {
        // migrations.paths
        for p in &mut self.migrations.paths {
            if p.is_relative() {
                *p = config_dir.join(&*p);
            }
        }

        // liquibase.bridge_jar_path
        if let Some(ref mut p) = self.liquibase.bridge_jar_path
            && p.is_relative()
        {
            *p = config_dir.join(&*p);
        }

        // liquibase.binary_path — skip if it looks like a bare command name
        // (no path separators), since those are resolved via $PATH.
        if let Some(ref mut p) = self.liquibase.binary_path
            && p.is_relative()
            && p.components().count() > 1
        {
            *p = config_dir.join(&*p);
        }

        // liquibase.properties_file
        if let Some(ref mut p) = self.liquibase.properties_file
            && p.is_relative()
        {
            *p = config_dir.join(&*p);
        }

        // output.dir
        if self.output.dir.is_relative() {
            self.output.dir = config_dir.join(&self.output.dir);
        }
    }

    /// Validate configuration values.
    fn validate(&self) -> Result<(), ConfigError> {
        let fail_on = &self.cli.fail_on;
        if !fail_on.eq_ignore_ascii_case("none") && crate::rules::Severity::parse(fail_on).is_none()
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

    #[test]
    fn test_rules_disabled_deserialization() {
        let toml = "[rules]\ndisabled = [\"PGM007\", \"PGM101\"]";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(
            config.rules.disabled,
            vec![
                "PGM007".parse::<crate::rules::RuleId>().unwrap(),
                "PGM101".parse::<crate::rules::RuleId>().unwrap(),
            ]
        );
    }

    #[test]
    fn test_rules_section_defaults_to_empty() {
        let config = Config::default();
        assert!(config.rules.disabled.is_empty());
    }

    #[test]
    fn test_no_rules_section_uses_defaults() {
        let toml = "[cli]\nfail_on = \"critical\"";
        let config = parse_and_validate(toml).unwrap();
        assert!(config.rules.disabled.is_empty());
    }

    #[test]
    fn test_run_in_transaction_defaults_to_none() {
        let config = Config::default();
        assert_eq!(config.migrations.run_in_transaction, None);
    }

    #[test]
    fn test_run_in_transaction_parses_true() {
        let toml = "[migrations]\nrun_in_transaction = true";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(config.migrations.run_in_transaction, Some(true));
    }

    #[test]
    fn test_run_in_transaction_parses_false() {
        let toml = "[migrations]\nrun_in_transaction = false";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(config.migrations.run_in_transaction, Some(false));
    }

    #[test]
    fn test_run_in_transaction_absent_is_none() {
        let toml = "[migrations]\nstrategy = \"filename_lexicographic\"";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(config.migrations.run_in_transaction, None);
    }

    // --- resolve_paths tests ---

    #[test]
    fn test_resolve_paths_prepends_config_dir() {
        let mut config = Config::default();
        config.migrations.paths = vec![PathBuf::from("db/migrations")];
        config.liquibase.bridge_jar_path = Some(PathBuf::from("tools/bridge.jar"));
        config.liquibase.properties_file = Some(PathBuf::from("liquibase.properties"));
        config.output.dir = PathBuf::from("build/reports");

        config.resolve_paths(Path::new("/project/impl"));

        assert_eq!(
            config.migrations.paths,
            vec![PathBuf::from("/project/impl/db/migrations")]
        );
        assert_eq!(
            config.liquibase.bridge_jar_path,
            Some(PathBuf::from("/project/impl/tools/bridge.jar"))
        );
        assert_eq!(
            config.liquibase.properties_file,
            Some(PathBuf::from("/project/impl/liquibase.properties"))
        );
        assert_eq!(
            config.output.dir,
            PathBuf::from("/project/impl/build/reports")
        );
    }

    #[test]
    fn test_resolve_paths_leaves_absolute_paths_unchanged() {
        let mut config = Config::default();
        config.migrations.paths = vec![PathBuf::from("/abs/db/migrations")];
        config.liquibase.bridge_jar_path = Some(PathBuf::from("/abs/tools/bridge.jar"));
        config.output.dir = PathBuf::from("/abs/build/reports");

        config.resolve_paths(Path::new("/project/impl"));

        assert_eq!(
            config.migrations.paths,
            vec![PathBuf::from("/abs/db/migrations")]
        );
        assert_eq!(
            config.liquibase.bridge_jar_path,
            Some(PathBuf::from("/abs/tools/bridge.jar"))
        );
        assert_eq!(config.output.dir, PathBuf::from("/abs/build/reports"));
    }

    #[test]
    fn test_resolve_paths_bare_binary_name_unchanged() {
        let mut config = Config::default();
        // A bare command name like "liquibase" should not be prefixed
        config.liquibase.binary_path = Some(PathBuf::from("liquibase"));

        config.resolve_paths(Path::new("/project"));

        assert_eq!(
            config.liquibase.binary_path,
            Some(PathBuf::from("liquibase"))
        );
    }

    #[test]
    fn test_resolve_paths_relative_binary_path_resolved() {
        let mut config = Config::default();
        // A relative path with directory components should be resolved
        config.liquibase.binary_path = Some(PathBuf::from("bin/liquibase"));

        config.resolve_paths(Path::new("/project"));

        assert_eq!(
            config.liquibase.binary_path,
            Some(PathBuf::from("/project/bin/liquibase"))
        );
    }

    #[test]
    fn test_resolve_paths_dot_config_dir_is_identity() {
        let mut config = Config::default();
        config.migrations.paths = vec![PathBuf::from("db/migrations")];
        config.output.dir = PathBuf::from("build/reports");

        config.resolve_paths(Path::new("."));

        assert_eq!(
            config.migrations.paths,
            vec![PathBuf::from("./db/migrations")]
        );
        assert_eq!(config.output.dir, PathBuf::from("./build/reports"));
    }

    // --- strip_prefix tests ---

    #[test]
    fn test_strip_prefix_deserialization() {
        let toml = "[output]\nstrip_prefix = \"impl/\"";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(config.output.strip_prefix, Some("impl/".to_string()));
    }

    #[test]
    fn test_strip_prefix_defaults_to_none() {
        let config = Config::default();
        assert_eq!(config.output.strip_prefix, None);
    }

    #[test]
    fn test_strip_prefix_absent_is_none() {
        let toml = "[output]\nformats = [\"sarif\"]";
        let config = parse_and_validate(toml).unwrap();
        assert_eq!(config.output.strip_prefix, None);
    }

    // --- config defaults tests ---

    /// Assert that every field in the config has its expected default value.
    /// Used by multiple tests to avoid duplicating the list of defaults.
    fn assert_defaults_sane(config: &Config) {
        // migrations
        assert_eq!(
            config.migrations.paths,
            vec![PathBuf::from("db/migrations")],
            "migrations.paths"
        );
        assert_eq!(
            config.migrations.strategy, "filename_lexicographic",
            "migrations.strategy"
        );
        assert_eq!(
            config.migrations.include,
            vec!["*.sql".to_string(), "*.xml".to_string()],
            "migrations.include"
        );
        assert!(
            config.migrations.exclude.is_empty(),
            "migrations.exclude should be empty"
        );
        assert_eq!(
            config.migrations.default_schema, "public",
            "migrations.default_schema"
        );
        assert_eq!(
            config.migrations.run_in_transaction, None,
            "migrations.run_in_transaction"
        );

        // liquibase
        assert_eq!(
            config.liquibase.bridge_jar_path,
            Some(PathBuf::from("tools/liquibase-bridge.jar")),
            "liquibase.bridge_jar_path"
        );
        assert_eq!(
            config.liquibase.binary_path,
            Some(PathBuf::from("liquibase")),
            "liquibase.binary_path"
        );
        assert_eq!(
            config.liquibase.properties_file, None,
            "liquibase.properties_file"
        );
        assert_eq!(config.liquibase.strategy, "auto", "liquibase.strategy");

        // output
        assert_eq!(
            config.output.formats,
            vec!["sarif".to_string()],
            "output.formats"
        );
        assert_eq!(
            config.output.dir,
            PathBuf::from("build/reports/migration-lint"),
            "output.dir"
        );
        assert_eq!(config.output.strip_prefix, None, "output.strip_prefix");

        // cli
        assert_eq!(config.cli.fail_on, "critical", "cli.fail_on");

        // rules
        assert!(
            config.rules.disabled.is_empty(),
            "rules.disabled should be empty"
        );
    }

    #[test]
    fn test_config_default_values() {
        assert_defaults_sane(&Config::default());
    }

    #[test]
    fn test_empty_toml_matches_default() {
        let config: Config = toml::from_str("").unwrap();
        assert_defaults_sane(&config);
    }

    #[test]
    fn test_default_config_round_trips() {
        let original = Config::default();
        let toml_str = toml::to_string(&original).unwrap();
        let round_tripped: Config = toml::from_str(&toml_str).unwrap();
        assert_defaults_sane(&round_tripped);
    }
}
