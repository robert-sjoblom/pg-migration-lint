//! pg-migration-lint CLI
//!
//! Entry point for the command-line tool.

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "pg-migration-lint")]
#[command(about = "Static analyzer for PostgreSQL migration files", long_about = None)]
struct Args {
    /// Path to configuration file
    #[arg(short, long, default_value = "pg-migration-lint.toml")]
    config: PathBuf,

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

fn main() -> Result<()> {
    let args = Args::parse();

    // Handle --explain early exit
    if let Some(rule_id) = args.explain {
        return explain_rule(&rule_id);
    }

    // Load configuration
    let config = if args.config.exists() {
        pg_migration_lint::Config::from_file(&args.config)
            .context("Failed to load configuration")?
    } else {
        eprintln!(
            "Warning: Config file {} not found, using defaults",
            args.config.display()
        );
        pg_migration_lint::Config::default()
    };

    // Parse changed files
    let changed_files = parse_changed_files(&args)?;

    // TODO: Phase 2 - implement the full pipeline:
    // 1. Load migration files
    // 2. Parse to IR
    // 3. Single-pass replay and lint
    // 4. Apply suppressions
    // 5. Emit reports
    // 6. Exit with appropriate code

    println!("Config loaded: {:?}", config);
    println!("Changed files: {:?}", changed_files);

    Ok(())
}

fn explain_rule(rule_id: &str) -> Result<()> {
    use pg_migration_lint::RuleRegistry;

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
