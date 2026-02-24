//! pg-migration-lint: Static analyzer for PostgreSQL migration files
//!
//! This library provides the core functionality for linting PostgreSQL migrations.
//! It parses SQL and Liquibase changesets, builds a table catalog by replaying
//! migration history, and runs safety rules against changed files.

pub mod catalog;
pub mod config;
#[cfg(feature = "docgen")]
pub mod docgen;
pub mod input;
pub mod normalize;
pub mod output;
pub mod parser;
pub mod pipeline;
pub mod rules;
pub mod suppress;

// Re-export commonly used types
pub use catalog::{Catalog, TableState};
pub use config::Config;
pub use output::RuleInfo;
pub use parser::ir::{IrNode, Located};
pub use pipeline::LintPipeline;
pub use rules::{Finding, Rule, RuleRegistry, Severity};
