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
use std::collections::HashSet;
use std::path::PathBuf;

use pg_migration_lint::catalog::replay;
use pg_migration_lint::input::MigrationLoader;
use pg_migration_lint::input::sql::SqlLoader;
use pg_migration_lint::output::{Reporter, SarifReporter, SonarQubeReporter, TextReporter};
use pg_migration_lint::rules::{self, LintContext};
use pg_migration_lint::suppress::parse_suppressions;
use pg_migration_lint::{Catalog, Finding, IrNode, RuleRegistry, Severity};

/// Default config file name used when --config is not explicitly provided.
const DEFAULT_CONFIG_FILE: &str = "pg-migration-lint.toml";

#[derive(Parser, Debug)]
#[command(name = "pg-migration-lint")]
#[command(about = "Static analyzer for PostgreSQL migration files", long_about = None)]
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

    // Load configuration.
    // If --config is explicitly provided and the file doesn't exist, that's a tool error.
    // If using the default path and it doesn't exist, warn and use defaults.
    let config = load_config(&args.config)?;

    // Parse changed files
    let changed_files = parse_changed_files(&args)?;

    // --- Step 1: Load migration files ---
    let loader = SqlLoader::new();
    let history = loader
        .load(&config.migrations.paths)
        .context("Failed to load migrations")?;

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

    let mut all_findings: Vec<Finding> = Vec::new();
    let mut tables_created_in_change: HashSet<String> = HashSet::new();

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
                || changed_files_set
                    .iter()
                    .any(|cf| cf.ends_with(&unit.source_file) || unit.source_file.ends_with(cf))
        };

        if is_changed {
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

            // Run all rules
            let mut unit_findings: Vec<Finding> = Vec::new();
            for rule in registry.iter() {
                let mut findings = rule.check(&unit.statements, &ctx);
                unit_findings.append(&mut findings);
            }

            // Cap severity for down migrations (PGM008)
            if unit.is_down {
                rules::cap_for_down_migration(&mut unit_findings);
            }

            // Parse suppressions from source file and filter findings.
            // Read the raw SQL source for suppression comments.
            let source = std::fs::read_to_string(&unit.source_file).unwrap_or_default();
            let suppressions = parse_suppressions(&source);

            unit_findings.retain(|f| !suppressions.is_suppressed(&f.rule_id, f.start_line));

            all_findings.append(&mut unit_findings);
        } else {
            // Not a changed file -- just replay to build catalog
            replay::apply(&mut catalog, unit);
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
    eprintln!(
        "pg-migration-lint: {} finding(s)",
        all_findings.len()
    );

    let fail_on = Severity::parse(&config.cli.fail_on);
    if let Some(threshold) = fail_on {
        if all_findings.iter().any(|f| f.severity >= threshold) {
            return Ok(true);
        }
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
                anyhow::bail!(
                    "Config file not found: {}",
                    path.display()
                );
            }
            pg_migration_lint::Config::from_file(path)
                .context("Failed to load configuration")
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
        let contents = std::fs::read_to_string(file_path)
            .context("Failed to read changed-files-from file")?;
        for line in contents.lines() {
            let line = line.trim();
            if !line.is_empty() {
                files.push(PathBuf::from(line));
            }
        }
    }

    Ok(files)
}
