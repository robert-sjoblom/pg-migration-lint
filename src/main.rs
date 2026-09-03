//! pg-migration-lint CLI
//!
//! Entry point for the command-line tool.
//!
//! Exit codes:
//! - 0: No findings at or above the configured severity threshold
//! - 1: One or more findings at or above the threshold
//! - 2: Tool error (config error, parse failure, I/O error, etc.)

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

mod github;

use pg_migration_lint::input::MigrationHistory;
use pg_migration_lint::input::liquibase_bridge::load_liquibase;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::normalize;
use pg_migration_lint::output::{
    Reporter, RuleInfo, SarifReporter, SonarQubeReporter, TextReporter,
};
use pg_migration_lint::rules::dedup_findings;
use pg_migration_lint::rules::{Rule, RuleId};
use pg_migration_lint::suppress::parse_suppressions;
use pg_migration_lint::{Config, Finding, LintPipeline, Severity};

/// Default config file name used when --config is not explicitly provided.
const DEFAULT_CONFIG_FILE: &str = "pg-migration-lint.toml";

#[derive(Parser, Debug)]
#[command(name = "pg-migration-lint")]
#[command(about = "Static analyzer for PostgreSQL migration files", long_about = None, version)]
struct Args {
    /// Path to configuration file
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Comma-separated list of changed files to lint
    #[arg(long)]
    changed_files: Option<String>,

    /// Path to file containing changed file paths (one per line)
    #[arg(long)]
    changed_files_from: Option<PathBuf>,

    /// Explain a specific rule (e.g., --explain PGM001)
    #[arg(long)]
    explain: Option<String>,

    /// Override output format (text, sarif, sonarqube)
    #[arg(long)]
    format: Option<String>,

    /// Show configuration reference. Optionally specify a section name.
    #[arg(long, num_args = 0..=1, default_missing_value = "all")]
    explain_config: Option<String>,

    /// Override exit code threshold (critical, major, minor, info, none)
    #[arg(long)]
    fail_on: Option<String>,

    /// Validate configuration and check that paths and tools exist, then exit
    #[arg(long)]
    validate_config: bool,

    /// Subcommand to run instead of the default lint flow. Absent entirely
    /// preserves every existing flat-flag invocation unchanged.
    #[command(subcommand)]
    command: Option<Commands>,
}

/// Subcommands available alongside the default flat-flag lint flow.
#[derive(Subcommand, Debug)]
enum Commands {
    /// Run the GitHub Action PR-review workflow: fetch a pull request's
    /// changed files, lint them, and post findings back as PR comments.
    GithubReview(GithubReviewArgs),
}

/// Arguments for `pg-migration-lint github-review`.
///
/// This subcommand is driven by the `pg-migration-lint` GitHub Action
/// (`action.yml` at the repo root). Every field below can be given
/// explicitly, or falls back to a GitHub Actions-provided environment
/// variable when omitted -- see [`ResolvedGithubReviewArgs::resolve`] for
/// the exact fallback chain.
///
/// The tool resolves `--config` and migration paths relative to CWD, matching
/// the flat CLI mode.
#[derive(clap::Args, Debug)]
struct GithubReviewArgs {
    /// Pull request number. Defaults to `.pull_request.number` read from
    /// the JSON file at `$GITHUB_EVENT_PATH` (the standard GitHub Actions
    /// event payload for a `pull_request`-triggered workflow).
    #[arg(long)]
    pr: Option<u64>,

    /// Repository in `owner/repo` form. Defaults to `$GITHUB_REPOSITORY`.
    #[arg(long)]
    repo: Option<String>,

    /// GitHub token used to call the REST API. Defaults to `$GITHUB_TOKEN`,
    /// then `$GH_TOKEN`.
    #[arg(long)]
    github_token: Option<String>,

    /// Path to configuration file. Identical semantics to the flat CLI
    /// mode's `--config` (see `load_config`): reused, not reimplemented.
    #[arg(short, long)]
    config: Option<PathBuf>,

    /// Override exit code threshold (critical, major, minor, info, none).
    /// Identical semantics to the flat CLI mode's `--fail-on`.
    #[arg(long)]
    fail_on: Option<String>,
}

fn main() {
    let args = Args::parse();

    if let Some(Commands::GithubReview(gh_args)) = &args.command {
        let exit_code = match run_github_review(gh_args) {
            Ok(code) => code,
            Err(err) => {
                eprintln!("Error: {err:#}",);
                2
            }
        };
        std::process::exit(exit_code);
    }

    match run(args) {
        Ok(has_findings_above_threshold) => {
            if has_findings_above_threshold {
                std::process::exit(1);
            }
            // exit 0 is implicit
        }
        Err(err) => {
            eprintln!("Error: {err:#}",);
            std::process::exit(2);
        }
    }
}

/// Runs the `github-review` subcommand by handing `args` off to
/// [`github::run`] inside a dedicated `tokio` runtime -- the only place in
/// this binary an async runtime is constructed; everything else stays
/// synchronous.
///
/// Returns the process exit code (0/1/2, matching the flat CLI mode's
/// contract) on success.
fn run_github_review(args: &GithubReviewArgs) -> Result<i32> {
    let runtime = tokio::runtime::Runtime::new().context("Failed to start async runtime")?;
    runtime.block_on(github::run(args))
}

/// Run the main lint pipeline.
///
/// Returns `Ok(true)` if findings at or above the severity threshold were found,
/// `Ok(false)` if no findings met the threshold, or `Err` on tool errors.
fn run(args: Args) -> Result<bool> {
    // Handle --explain early exit
    if let Some(rule_id) = args.explain {
        explain_rule(&rule_id)?;
        return Ok(false);
    }

    // Handle --explain-config early exit
    if let Some(ref section) = args.explain_config {
        pg_migration_lint::config::explain_config(section)?;
        return Ok(false);
    }

    // Load configuration.
    // If --config is explicitly provided and the file doesn't exist, that's a tool error.
    // If using the default path and it doesn't exist, warn and use defaults.
    let config = load_config(&args.config)?;

    // Handle --validate-config early exit
    if args.validate_config {
        return print_config_validation(&config);
    }

    // Parse changed files
    let changed_files = parse_changed_files(&args)?;

    // --- Step 1: Load migration files ---
    let mut history = load_migrations(&config)?;

    // Selective mode: if the user passed --changed-files or --changed-files-from,
    // we only lint the files they named even if the resulting set is empty.
    // An empty set in selective mode means "lint nothing, but still write reports"
    // so that CI consumers (e.g. SonarQube) always find the expected report file.
    let selective_mode = args.changed_files.is_some() || args.changed_files_from.is_some();
    let changed_files_arg: Option<&[PathBuf]> = if selective_mode {
        Some(&changed_files)
    } else {
        None
    };

    let all_findings = lint_history(&mut history, &config, changed_files_arg);

    let formats: Vec<String> = if let Some(ref fmt) = args.format {
        vec![fmt.clone()]
    } else {
        config.output.formats.clone()
    };

    for format in &formats {
        let reporter: Box<dyn Reporter> = match format.as_str() {
            "text" => Box::new(TextReporter::new(true)),
            "sarif" => Box::new(SarifReporter::new()),
            "sonarqube" => Box::new(SonarQubeReporter::new(RuleInfo::all())),
            other => {
                eprintln!("Warning: Unknown output format '{other}', skipping",);
                continue;
            }
        };

        reporter
            .emit(&all_findings, &config.output.dir)
            .context(format!("Failed to write {format} report",))?;
    }

    eprintln!("pg-migration-lint: {} finding(s)", all_findings.len());

    let fail_on_str = args.fail_on.as_deref().unwrap_or(&config.cli.fail_on);
    exceeds_fail_on_threshold(&all_findings, fail_on_str)
}

/// Lint `history`'s units, exactly matching the flat CLI mode's default flow:
/// normalizes schemas, replays every unit into the catalog, lints only units in
/// `changed_files` (or every unit when `changed_files` is `None`) applies
/// suppression comments and per-unit dedup, warns about single-file changelogs
/// that look suspiciously large, and strips `config.output.strip_prefix` from the
/// result.
///
/// Shared between the flat CLI mode (above) and the `github-review`
/// subcommand ([`github::run`]) so the two lint flows never drift: the only
/// difference between them is which files count as "changed" -- a CLI flag
/// here, the PR's changed-files list there.
fn lint_history(
    history: &mut MigrationHistory,
    config: &Config,
    changed_files: Option<&[PathBuf]>,
) -> Vec<Finding> {
    // Assign the configured default schema to every unqualified QualifiedName
    // so that catalog keys are always schema-qualified.
    normalize::normalize_schemas(&mut history.units, &config.migrations.default_schema);

    // Build changed files set for O(1) lookup. Canonicalize paths where
    // possible for reliable matching.
    let changed_files_set: HashSet<PathBuf> = changed_files
        .unwrap_or(&[])
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();

    let lint_all = changed_files.is_none();

    let mut pipeline = LintPipeline::new();

    // Build active rules list, filtering out any disabled via config.
    let disabled: HashSet<RuleId> = config.rules.disabled.iter().copied().collect();
    let active_rules: Vec<RuleId> = RuleId::lint_rules()
        .filter(|r| !disabled.contains(r))
        .collect();

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut changed_units_per_file: HashMap<PathBuf, usize> = HashMap::new();

    for unit in &history.units {
        // Determine if this unit is in the changed set.
        // Try canonicalized comparison first, then fall back to direct and ends_with matching.
        let is_changed = if lint_all {
            true
        } else {
            let canonical = std::fs::canonicalize(&unit.source_file)
                .unwrap_or_else(|_| unit.source_file.clone());
            changed_files_set.contains(&canonical)
                || changed_files_set.contains(&unit.source_file)
                || changed_files_set.iter().any(|cf| {
                    // Only allow suffix matching when the shorter path includes a directory
                    // component, to prevent bare filenames from matching across directories.
                    (cf.ends_with(&unit.source_file) && unit.source_file.components().count() > 1)
                        || (unit.source_file.ends_with(cf) && cf.components().count() > 1)
                })
        };

        if is_changed {
            *changed_units_per_file
                .entry(unit.source_file.clone())
                .or_insert(0) += 1;

            let mut unit_findings = pipeline.lint(unit, &active_rules);

            // Parse suppressions from source file and filter findings.
            // Read the raw SQL source for suppression comments.
            let source = match std::fs::read_to_string(&unit.source_file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!(
                        "Warning: could not read '{}' for suppression comments: {}",
                        unit.source_file.display(),
                        e
                    );
                    String::new()
                }
            };
            let suppressions = parse_suppressions(&source);

            for id in suppressions.rule_ids() {
                if id.is_meta() {
                    eprintln!(
                        "WARNING: meta rule '{}' in suppression comment in {} (meta rules cannot be suppressed)",
                        id,
                        unit.source_file.display()
                    );
                }
            }

            unit_findings.retain(|f| !suppressions.is_suppressed(f.rule_id, f.start_line));
            dedup_findings(&mut unit_findings);

            all_findings.append(&mut unit_findings);
        } else {
            // Not a changed file -- just replay to build catalog
            pipeline.replay(unit);
        }
    }

    // Warn when a single file contributes many changesets (likely a single-file changelog)
    const MULTI_CHANGESET_THRESHOLD: usize = 20;
    if !lint_all {
        for (file, count) in &changed_units_per_file {
            if *count >= MULTI_CHANGESET_THRESHOLD {
                eprintln!(
                    "Warning: {} changesets from '{}' matched as changed. \
                     If this is a single-file changelog, findings may include \
                     historical changesets. Consider using <include> with one \
                     changeset per file for accurate changed-file detection.",
                    count,
                    file.display()
                );
            }
        }
    }

    // Strip path prefix (if configured)
    if let Some(ref prefix) = config.output.strip_prefix {
        for finding in &mut all_findings {
            if let Ok(stripped) = finding.file.strip_prefix(prefix) {
                finding.file = stripped.to_path_buf();
            }
        }
    }

    all_findings
}

/// Parses `fail_on_str` (`"none"` or a [`Severity`] name) and reports
/// whether any finding in `findings` meets or exceeds it.
///
/// Shared between the flat CLI mode's `--fail-on`/`config.cli.fail_on` exit
/// code (`Ok(true)`/`Ok(false)`, see `run`) and the `github-review`
/// subcommand's identical `--fail-on` handling ([`github::run`], which maps
/// the `bool` to a `0`/`1` process exit code), so the two exit-code
/// semantics never drift.
///
/// # Errors
///
/// Returns an error if `fail_on_str` is neither `"none"` nor a valid
/// [`Severity`] name.
fn exceeds_fail_on_threshold(findings: &[Finding], fail_on_str: &str) -> Result<bool> {
    if fail_on_str.eq_ignore_ascii_case("none") {
        return Ok(false);
    }
    let threshold = Severity::parse(fail_on_str).ok_or_else(|| {
        anyhow::anyhow!(
            "Unknown severity '{fail_on_str}' for --fail-on. Valid values: blocker, critical, major, minor, info, none"
        )
    })?;
    Ok(findings.iter().any(|f| f.severity >= threshold))
}

/// Load configuration from file.
///
/// If `config_path` is `Some`, the user explicitly passed `--config` and the file
/// must exist (error if not found). If `None`, the default config path is used;
/// a missing default config file is not an error (falls back to defaults with a warning).
fn load_config(config_path: &Option<PathBuf>) -> Result<pg_migration_lint::Config> {
    match config_path {
        Some(path) => {
            // User explicitly provided --config; file must exist.
            if !path.exists() {
                anyhow::bail!("Config file not found: {}", path.display());
            }
            pg_migration_lint::Config::from_file(path).context("Failed to load configuration")
        }
        None => {
            // Using default config path; missing file is OK.
            let default_path = PathBuf::from(DEFAULT_CONFIG_FILE);
            if default_path.exists() {
                pg_migration_lint::Config::from_file(&default_path)
                    .context("Failed to load configuration")
            } else {
                eprintln!(
                    "Warning: Config file {} not found, using defaults",
                    default_path.display()
                );
                Ok(pg_migration_lint::Config::default())
            }
        }
    }
}

fn explain_rule(rule_id: &str) -> Result<()> {
    let parsed: RuleId = rule_id
        .parse()
        .map_err(|_| anyhow::anyhow!("Unknown rule: {}", rule_id))?;

    println!("Rule: {}", parsed);
    println!("Severity: {}", parsed.default_severity());
    println!("Description: {}", parsed.description());
    println!();
    println!("{}", parsed.explain());

    Ok(())
}

fn parse_changed_files(args: &Args) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if let Some(ref file_list) = args.changed_files {
        for path_str in file_list.split(',') {
            let path_str = path_str.trim();
            if !path_str.is_empty() {
                files.push(PathBuf::from(path_str));
            }
        }
    }

    if let Some(ref file_path) = args.changed_files_from {
        let contents =
            std::fs::read_to_string(file_path).context("Failed to read changed-files-from file")?;
        for line in contents.lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push(PathBuf::from(line));
            }
        }
    }

    Ok(files)
}

/// Load migration files using the strategy configured in `config.migrations.strategy`.
///
/// - `"filename_lexicographic"` (default): Load `.sql` files sorted by filename.
/// - `"liquibase"`: Use the Liquibase two-tier fallback (bridge JAR -> update-sql).
///
/// For the Liquibase strategy, the sub-strategy is controlled by `config.liquibase.strategy`
/// (`"auto"`, `"bridge"`, `"update-sql"`).
fn load_migrations(config: &Config) -> Result<MigrationHistory> {
    match config.migrations.strategy.as_str() {
        "liquibase" => {
            eprintln!(
                "pg-migration-lint: using liquibase strategy (sub-strategy: {})",
                config.liquibase.strategy
            );
            let raw_units = load_liquibase(&config.liquibase, &config.migrations.paths)
                .context("Failed to load Liquibase migrations")?;

            let units = raw_units
                .into_iter()
                .map(|r| r.into_migration_unit())
                .collect();

            Ok(MigrationHistory { units })
        }
        "filename_lexicographic" => {
            eprintln!("pg-migration-lint: using filename_lexicographic strategy");
            let run_in_tx = config.migrations.run_in_transaction.unwrap_or(true);
            let loader = SqlLoader::new(run_in_tx);
            let history = loader
                .load(&config.migrations.paths)
                .context("Failed to load migrations")?;
            Ok(history)
        }
        other => {
            eprintln!(
                "pg-migration-lint: unknown strategy '{other}', falling back to filename_lexicographic",
            );
            let run_in_tx = config.migrations.run_in_transaction.unwrap_or(true);
            let loader = SqlLoader::new(run_in_tx);
            let history = loader
                .load(&config.migrations.paths)
                .context("Failed to load migrations")?;
            Ok(history)
        }
    }
}

fn print_config_validation(config: &Config) -> Result<bool> {
    use std::process::Command;

    println!("pg-migration-lint: configuration validation");
    println!("  strategy: {}", config.migrations.strategy);

    if config.migrations.strategy != "liquibase" {
        println!(
            "\n  No Liquibase checks needed for strategy \"{}\".",
            config.migrations.strategy
        );
        return Ok(false);
    }

    let lb = &config.liquibase;
    println!("  liquibase sub-strategy: {}", lb.strategy);
    let mut errors = Vec::new();

    // Bridge JAR
    match lb.bridge_jar_path {
        Some(ref path) if path.exists() => {
            println!("  bridge JAR: found ({})", path.display());
        }
        Some(ref path) => {
            let msg = format!("bridge JAR not found: {}", path.display());
            println!("  bridge JAR: NOT found ({})", path.display());
            if lb.strategy == "bridge" {
                errors.push(msg);
            }
        }
        None if lb.strategy == "bridge" => {
            let msg = "bridge_jar_path not configured but strategy is \"bridge\"".to_string();
            println!("  bridge JAR: not configured");
            errors.push(msg);
        }
        None => {
            println!("  bridge JAR: not configured");
        }
    }

    // Liquibase binary
    match lb.binary_path {
        Some(ref path) => {
            let reachable = if path.components().count() == 1 {
                Command::new(path)
                    .arg("--version")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .is_ok()
            } else {
                path.exists()
            };

            if reachable {
                println!("  liquibase binary: reachable ({})", path.display());
            } else {
                let msg = format!("liquibase binary not reachable: {}", path.display());
                println!("  liquibase binary: NOT reachable ({})", path.display());
                if lb.strategy == "update-sql" {
                    errors.push(msg);
                }
            }
        }
        None if lb.strategy == "update-sql" => {
            let msg = "binary_path not configured but strategy is \"update-sql\"".to_string();
            println!("  liquibase binary: not configured");
            errors.push(msg);
        }
        None => {
            println!("  liquibase binary: not configured");
        }
    }

    // Properties file
    if let Some(ref path) = lb.properties_file {
        if path.exists() {
            println!("  properties file: found ({})", path.display());
        } else {
            let msg = format!("properties file not found: {}", path.display());
            println!("  properties file: NOT found ({})", path.display());
            errors.push(msg);
        }
    }

    println!();
    if errors.is_empty() {
        println!("Validation passed.");
        return Ok(false);
    }

    for err in &errors {
        eprintln!("Error: {err}");
    }
    anyhow::bail!(
        "configuration validation failed with {} error(s)",
        errors.len()
    );
}
