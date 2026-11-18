use std::{collections::HashSet, path::Path};

use crate::Catalog;
use crate::names::{RelationName, ResolvedName};
use crate::parser::ir::{IrNode, Located};
use crate::rules::{TableScope, TransactionWindows};

/// Context available to rules during linting.
pub struct LintContext<'a> {
    /// The catalog state BEFORE the current unit was applied.
    /// Clone taken just before apply(). Most rules read it, directly or
    /// through `is_existing_table` and the shared helpers, to tell a
    /// pre-existing table from one created in this change and to look up the
    /// schema the unit's statements run against.
    pub catalog_before: &'a Catalog,

    /// The catalog state AFTER the current unit was applied.
    /// Used for post-file checks (PGM501, PGM502, PGM503, PGM508).
    pub catalog_after: &'a Catalog,

    /// Set of table names created in the current set of changed files.
    /// Built incrementally during the single-pass replay: when a changed
    /// file contains a CreateTable, add it to this set before linting
    /// subsequent changed files.
    pub tables_created_in_change: &'a HashSet<RelationName>,

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
    pub fn is_existing_table(&self, table_key: &RelationName) -> bool {
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
        ir_partition_of: Option<&ResolvedName>,
        table_key: &RelationName,
    ) -> bool {
        // Primary: from IR (CREATE TABLE ... PARTITION OF parent)
        if let Some(parent_name) = ir_partition_of {
            return match self.catalog_after.get_table(parent_name.relation()) {
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

    /// Resolves the display name of `table_key`'s parent partition table from
    /// `catalog_before`. Returns `None` when `table_key` is not a partition
    /// child (no `parent_table` recorded). Falls back to the parent's bare
    /// name when the parent itself is not tracked in the catalog, so a
    /// default schema never shows up in a message.
    pub fn parent_display_name(&self, table_key: &RelationName) -> Option<String> {
        let parent_key = self
            .catalog_before
            .get_table(table_key)?
            .parent_table
            .as_ref()?;
        Some(
            self.catalog_before
                .get_table(parent_key)
                .map_or_else(|| parent_key.name.clone(), |p| p.display_name.clone()),
        )
    }

    /// Check if a table matches the given scope filter.
    pub fn table_matches_scope(&self, table_key: &RelationName, scope: TableScope) -> bool {
        match scope {
            TableScope::ExcludeCreatedInChange => self.is_existing_table(table_key),
            TableScope::AnyPreExisting => self.catalog_before.has_table(table_key),
        }
    }

    /// The transaction windows of `statements`, the statements of the unit
    /// this context lints, with `run_in_transaction` saying whether the
    /// migration tool runs them in a transaction. See [`TransactionWindows`]
    /// for how statements are windowed.
    ///
    /// Built on each call, in one pass over `statements`, so a rule that does
    /// not ask builds none. Nothing is cached on the context: it does not hold
    /// the statements, and a cache could not tell a different list from the
    /// one it was built from.
    pub fn transaction_windows(&self, statements: &[Located<IrNode>]) -> TransactionWindows {
        TransactionWindows::new(statements, self.run_in_transaction)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;
    use crate::catalog::builder::{CatalogBuilder, rel};
    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::{lint_ctx, normalized};

    #[test]
    fn test_parent_display_name_resolves_tracked_parent() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .display_name("Measurements")
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/test.sql");

        assert_eq!(
            ctx.parent_display_name(&rel("measurements_2024")),
            Some("Measurements".to_string())
        );
    }

    #[test]
    fn test_parent_display_name_falls_back_to_key_when_parent_untracked() {
        let before = CatalogBuilder::new()
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false).partition_of("ghost_parent");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/test.sql");

        assert_eq!(
            ctx.parent_display_name(&rel("measurements_2024")),
            Some("ghost_parent".to_string())
        );
    }

    #[test]
    fn test_parent_display_name_none_for_standalone_table() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/test.sql");

        assert_eq!(ctx.parent_display_name(&rel("measurements")), None);
    }

    #[test]
    fn test_parent_display_name_none_when_table_missing() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/test.sql");

        assert_eq!(ctx.parent_display_name(&rel("nonexistent")), None);
    }

    #[rstest]
    fn transaction_windows_follow_run_in_transaction(
        #[values(true, false)] run_in_transaction: bool,
    ) {
        let catalog = Catalog::new();
        lint_ctx!(ctx, &catalog, &catalog, "migrations/test.sql", txn: run_in_transaction);
        let statements = normalized(parse_sql(
            "DROP TABLE a;
             BEGIN;
             DROP TABLE b;
             COMMIT;",
        ));

        assert_eq!(
            ctx.transaction_windows(&statements),
            TransactionWindows::new(&statements, run_in_transaction)
        );
    }
}
