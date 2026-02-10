//! Migration loading from different input formats
//!
//! Supports SQL files and Liquibase XML changesets.

use crate::parser::ir::{IrNode, Located};
use std::path::PathBuf;
use thiserror::Error;

/// A single migration unit: one changeset (Liquibase) or one file (go-migrate).
#[derive(Debug, Clone)]
pub struct MigrationUnit {
    /// Identifier for ordering/logging. Changeset ID or filename.
    pub id: String,

    /// The SQL statements as IR nodes with source locations.
    pub statements: Vec<Located<IrNode>>,

    /// The source file to report findings against.
    pub source_file: PathBuf,

    /// Line offset in the source file where this unit starts.
    /// For raw SQL files this is 1. For Liquibase XML, it's the <changeSet> line.
    pub source_line_offset: usize,

    /// Whether this unit executes inside a transaction.
    /// Liquibase: derived from runInTransaction attribute.
    /// go-migrate: true by default unless explicitly disabled.
    pub run_in_transaction: bool,

    /// Is this a down/rollback migration?
    pub is_down: bool,
}

/// An ordered sequence of migration units representing the full history.
#[derive(Debug)]
pub struct MigrationHistory {
    pub units: Vec<MigrationUnit>,
}

/// Trait for migration loaders. Each input format implements this.
pub trait MigrationLoader {
    /// Load migrations from the given paths, in the configured order.
    fn load(&self, paths: &[PathBuf]) -> Result<MigrationHistory, LoadError>;
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("IO error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("Parse error in {path}: {message}")]
    Parse { path: PathBuf, message: String },

    #[error("Liquibase bridge failed: {message}")]
    BridgeError { message: String },

    #[error("Configuration error: {message}")]
    Config { message: String },
}
