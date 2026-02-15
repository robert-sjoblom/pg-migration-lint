//! pg-migration-lint CLI
//!
//! Entry point for the command-line tool.
//!
//! Exit codes:
//! - 0: No findings at or above the configured severity threshold
//! - 1: One or more findings at or above the threshold
//! - 2: Tool error (config error, parse failure, I/O error, etc.)

use anyhow::{Context, Result};
use clap::Parser;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use pg_migration_lint::catalog::replay;
use pg_migration_lint::input::liquibase_bridge::load_liquibase;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::input::{MigrationHistory, MigrationLoader};
use pg_migration_lint::normalize;
use pg_migration_lint::output::{Reporter, SarifReporter, SonarQubeReporter, TextReporter};
use pg_migration_lint::rules::{self, LintContext};
use pg_migration_lint::suppress::parse_suppressions;
use pg_migration_lint::{Catalog, Config, Finding, IrNode, RuleRegistry, Severity};

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
}

fn main() {
    let args = Args::parse();

    match run(args) {
        Ok(has_findings_above_threshold) => {
            if has_findings_above_threshold {
                std::process::exit(1);
            }
            // exit 0 is implicit
        }
        Err(err) => {
            eprintln!("Error: {:#}", err);
            std::process::exit(2);
        }
    }
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

    // Parse changed files
    let changed_files = parse_changed_files(&args)?;

    // --- Step 1: Load migration files ---
    let mut history = load_migrations(&config)?;

    // --- Step 1b: Normalize schemas ---
    // Assign the configured default schema to every unqualified QualifiedName
    // so that catalog keys are always schema-qualified.
    normalize::normalize_schemas(&mut history.units, &config.migrations.default_schema);

    // --- Step 2: Build changed files set for O(1) lookup ---
    // Convert the Vec<PathBuf> into a HashSet<PathBuf>.
    // Canonicalize paths where possible for reliable matching.
    let changed_files_set: HashSet<PathBuf> = changed_files
        .iter()
        .map(|p| std::fs::canonicalize(p).unwrap_or_else(|_| p.clone()))
        .collect();

    let lint_all = changed_files_set.is_empty();

    // --- Step 3: Single-pass replay and lint ---
    let mut catalog = Catalog::new();
    let mut registry = RuleRegistry::new();
    registry.register_defaults();

    // Build active rules list, filtering out any disabled via config.
    let disabled: HashSet<&str> = config.rules.disabled.iter().map(|s| s.as_str()).collect();
    for rule_id in &disabled {
        if registry.get(rule_id).is_none() {
            eprintln!(
                "WARNING: unknown rule '{}' in [rules].disabled, ignoring",
                rule_id
            );
        }
    }
    let active_rules: Vec<&dyn rules::Rule> = registry
        .iter()
        .filter(|r| !disabled.contains(r.id()))
        .collect();

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();
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

            // Clone catalog BEFORE applying this unit
            let catalog_before = catalog.clone();

            // Apply unit to catalog
            replay::apply(&mut catalog, unit);

            // Track tables created in this change (for PGM001/002 "new table" detection)
            for stmt in &unit.statements {
                if let IrNode::CreateTable(ct) = &stmt.node {
                    tables_created_in_change.insert(ct.name.catalog_key().to_string());
                }
            }

            // Build lint context
            let ctx = LintContext {
                catalog_before: &catalog_before,
                catalog_after: &catalog,
                tables_created_in_change: &tables_created_in_change,
                run_in_transaction: unit.run_in_transaction,
                is_down: unit.is_down,
                file: &unit.source_file,
            };

            // Run active rules (disabled rules already filtered out)
            let mut unit_findings: Vec<Finding> = Vec::new();
            for rule in &active_rules {
                let mut findings = rule.check(&unit.statements, &ctx);
                unit_findings.append(&mut findings);
            }

            // Cap severity for down migrations (PGM008)
            if unit.is_down {
                rules::cap_for_down_migration(&mut unit_findings);
            }

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
                if registry.get(id).is_none() {
                    eprintln!(
                        "WARNING: unknown rule '{}' in suppression comment in {}",
                        id,
                        unit.source_file.display()
                    );
                }
            }

            unit_findings.retain(|f| !suppressions.is_suppressed(&f.rule_id, f.start_line));

            all_findings.append(&mut unit_findings);
        } else {
            // Not a changed file -- just replay to build catalog
            replay::apply(&mut catalog, unit);
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

    // --- Step 4: Emit reports ---
    let formats: Vec<String> = if let Some(ref fmt) = args.format {
        vec![fmt.clone()]
    } else {
        config.output.formats.clone()
    };

    for format in &formats {
        let reporter: Box<dyn Reporter> = match format.as_str() {
            "text" => Box::new(TextReporter::new(true)),
            "sarif" => Box::new(SarifReporter::new()),
            "sonarqube" => Box::new(SonarQubeReporter::new()),
            other => {
                eprintln!("Warning: Unknown output format '{}', skipping", other);
                continue;
            }
        };

        reporter
            .emit(&all_findings, &config.output.dir)
            .context(format!("Failed to write {} report", format))?;
    }

    // --- Step 5: Summary and exit code ---
    eprintln!("pg-migration-lint: {} finding(s)", all_findings.len());

    let fail_on_str = args.fail_on.as_deref().unwrap_or(&config.cli.fail_on);
    let fail_on = if fail_on_str.eq_ignore_ascii_case("none") {
        None
    } else {
        match Severity::parse(fail_on_str) {
            Some(s) => Some(s),
            None => anyhow::bail!(
                "Unknown severity '{}' for --fail-on. Valid values: blocker, critical, major, minor, info, none",
                fail_on_str
            ),
        }
    };
    if let Some(threshold) = fail_on
        && all_findings.iter().any(|f| f.severity >= threshold)
    {
        return Ok(true);
    }

    Ok(false)
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
    let mut registry = RuleRegistry::new();
    registry.register_defaults();

    if let Some(rule) = registry.get(rule_id) {
        println!("Rule: {}", rule.id());
        println!("Severity: {}", rule.default_severity());
        println!("Description: {}", rule.description());
        println!();
        println!("{}", rule.explain());
    } else {
        anyhow::bail!("Unknown rule: {}", rule_id);
    }

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
                "pg-migration-lint: unknown strategy '{}', falling back to filename_lexicographic",
                other
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
