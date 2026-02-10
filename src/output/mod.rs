//! Output reporters for different formats
//!
//! Supports SARIF 2.1.0, SonarQube Generic Issue Import JSON, and text output.

use crate::rules::Finding;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("IO error writing report: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),
}

/// Trait for output format reporters.
pub trait Reporter {
    /// Write findings to the given output directory.
    /// The filename is determined by the reporter (e.g., "findings.sarif").
    fn emit(&self, findings: &[Finding], output_dir: &Path) -> Result<(), ReportError>;
}

/// Text reporter also supports writing to stdout (for --format text).
pub struct TextReporter {
    pub use_stdout: bool,
}

impl TextReporter {
    pub fn new(use_stdout: bool) -> Self {
        Self { use_stdout }
    }
}

pub struct SarifReporter;

impl SarifReporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SarifReporter {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SonarQubeReporter;

impl SonarQubeReporter {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SonarQubeReporter {
    fn default() -> Self {
        Self::new()
    }
}

pub mod sarif;
pub mod sonarqube;
pub mod text;
