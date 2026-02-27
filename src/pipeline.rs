//! Lint pipeline — extracts the shared replay-and-lint loop body.
//!
//! The [`LintPipeline`] struct encapsulates the single-pass replay strategy:
//! catalog state, table-creation tracking, and the clone → replay → lint → cap
//! sequence that was previously duplicated across `main.rs` and integration tests.

use std::collections::HashSet;

use crate::Catalog;
use crate::catalog::replay;
use crate::input::MigrationUnit;
use crate::parser::ir::IrNode;
use crate::rules::{self, Finding, LintContext, Rule, RuleId};

/// Encapsulates the single-pass replay + lint pipeline.
///
/// Callers feed migration units one at a time via [`replay`] (non-changed)
/// or [`lint`] (changed). The pipeline owns the catalog and the
/// `tables_created_in_change` set, handling the clone-replay-track-cap
/// sequence internally.
pub struct LintPipeline {
    catalog: Catalog,
    tables_created_in_change: HashSet<String>,
}

impl LintPipeline {
    /// Create a new pipeline with an empty catalog.
    pub fn new() -> Self {
        Self {
            catalog: Catalog::new(),
            tables_created_in_change: HashSet::new(),
        }
    }

    /// Replay a unit without linting (for non-changed migration files).
    ///
    /// Applies the unit's statements to the catalog so that subsequent
    /// units see the correct schema state.
    pub fn replay(&mut self, unit: &MigrationUnit) {
        replay::apply(&mut self.catalog, unit);
    }

    /// Replay AND lint a changed unit. Returns raw findings (before suppression).
    ///
    /// Handles: catalog clone, replay, track created tables (with IF NOT EXISTS
    /// guard), build [`LintContext`], run rules, and cap severity for down
    /// migrations.
    pub fn lint(&mut self, unit: &MigrationUnit, rules: &[RuleId]) -> Vec<Finding> {
        // Clone catalog BEFORE applying this unit
        let catalog_before = self.catalog.clone();

        // Apply unit to catalog
        replay::apply(&mut self.catalog, unit);

        // Track tables created in this change (for PGM001/002 "new table" detection).
        // Skip IF NOT EXISTS when the table already existed — that is a no-op,
        // not a genuine creation, and must not mask rules on later statements.
        for stmt in &unit.statements {
            if let IrNode::CreateTable(ct) = &stmt.node {
                let key = ct.name.catalog_key().to_string();
                if !(ct.if_not_exists && catalog_before.has_table(&key)) {
                    self.tables_created_in_change.insert(key);
                }
            }
        }

        // Build lint context
        let ctx = LintContext {
            catalog_before: &catalog_before,
            catalog_after: &self.catalog,
            tables_created_in_change: &self.tables_created_in_change,
            run_in_transaction: unit.run_in_transaction,
            is_down: unit.is_down,
            file: &unit.source_file,
        };

        // Run active rules
        let mut findings: Vec<Finding> = Vec::new();
        for rule in rules {
            findings.extend(rule.check(&unit.statements, &ctx));
        }

        // Cap severity for down migrations (PGM901)
        if unit.is_down {
            rules::cap_for_down_migration(&mut findings);
        }

        findings
    }
}

impl Default for LintPipeline {
    fn default() -> Self {
        Self::new()
    }
}
