use std::{collections::HashSet, path::Path};

use crate::{Catalog, rules::TableScope};

/// Context available to rules during linting.
pub struct LintContext<'a> {
    /// The catalog state BEFORE the current unit was applied.
    /// Clone taken just before apply(). Used by PGM001/002 to check
    /// if a table is pre-existing.
    pub catalog_before: &'a Catalog,

    /// The catalog state AFTER the current unit was applied.
    /// Used for post-file checks (PGM501, PGM502, PGM503).
    pub catalog_after: &'a Catalog,

    /// Set of table names created in the current set of changed files.
    /// Built incrementally during the single-pass replay: when a changed
    /// file contains a CreateTable, add it to this set before linting
    /// subsequent changed files.
    pub tables_created_in_change: &'a HashSet<String>,

    /// Whether this migration unit runs in a transaction.
    pub run_in_transaction: bool,

    /// Whether this is a down/rollback migration.
    pub is_down: bool,

    /// The source file being linted.
    pub file: &'a Path,
}

impl<'a> LintContext<'a> {
    /// Check if a table existed before this change and was not created in the
    /// current set of changed files.
    pub fn is_existing_table(&self, table_key: &str) -> bool {
        self.catalog_before.has_table(table_key)
            && !self.tables_created_in_change.contains(table_key)
    }

    /// Check whether a partition child should be exempt from PK-related rules
    /// (PGM502, PGM503) because its parent has a PK or the parent is not in
    /// the catalog.
    ///
    /// Checks two sources:
    /// 1. The IR's `partition_of` field (`CREATE TABLE child PARTITION OF parent`).
    /// 2. The catalog's `parent_table` field (set by `ALTER TABLE parent ATTACH PARTITION child`).
    ///
    /// Returns `true` (suppress) when:
    /// - Parent has a PK in `catalog_after` — PK is inherited by the child.
    /// - Parent is not in `catalog_after` — trust that production parents have a PK
    ///   (common in incremental CI where only new migrations are analyzed).
    ///
    /// Returns `false` (fire normally) when:
    /// - The table is not a partition child.
    /// - The parent exists but lacks a PK.
    pub fn partition_child_inherits_pk(
        &self,
        ir_partition_of: Option<&crate::parser::ir::QualifiedName>,
        table_key: &str,
    ) -> bool {
        // Primary: from IR (CREATE TABLE ... PARTITION OF parent)
        if let Some(parent_name) = ir_partition_of {
            let parent_key = parent_name.catalog_key();
            return match self.catalog_after.get_table(parent_key) {
                Some(parent) if parent.has_primary_key => true,
                Some(_) => false,
                None => true,
            };
        }

        // Fallback: from catalog (ALTER TABLE parent ATTACH PARTITION child)
        if let Some(table) = self.catalog_after.get_table(table_key)
            && let Some(ref parent_key) = table.parent_table
        {
            return match self.catalog_after.get_table(parent_key) {
                Some(parent) if parent.has_primary_key => true,
                Some(_) => false,
                None => true,
            };
        }

        false
    }

    /// Check if a table matches the given scope filter.
    pub fn table_matches_scope(&self, table_key: &str, scope: TableScope) -> bool {
        match scope {
            TableScope::ExcludeCreatedInChange => self.is_existing_table(table_key),
            TableScope::AnyPreExisting => self.catalog_before.has_table(table_key),
        }
    }
}
