//! PGM021 — `ADD UNIQUE` on existing table without `USING INDEX`
//!
//! Detects `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE` on existing tables
//! that doesn't use `USING INDEX` to reference a pre-built unique index.
//! Even if a matching unique index already exists, PostgreSQL will build a
//! **new** index under ACCESS EXCLUSIVE lock unless `USING INDEX` is explicit.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD UNIQUE on existing table without USING INDEX";

pub(super) const EXPLAIN: &str = "PGM021 — ADD UNIQUE on existing table without USING INDEX\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... UNIQUE on an existing table that\n\
         does not use USING INDEX, or where the referenced index does not\n\
         exist or is not UNIQUE.\n\
         \n\
         Why it's dangerous:\n\
         Without USING INDEX, PostgreSQL always builds a new unique index\n\
         inline under an ACCESS EXCLUSIVE lock, even if a matching unique\n\
         index already exists. For large tables this causes extended downtime.\n\
         NOT VALID does NOT apply to UNIQUE constraints.\n\
         \n\
         When USING INDEX is specified, PostgreSQL validates that the\n\
         referenced index exists and is unique, then promotes it to a\n\
         constraint without rebuilding.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE (email);\n\
         \n\
         Fix (safe pattern — build unique index concurrently first):\n\
           CREATE UNIQUE INDEX CONCURRENTLY idx_orders_email ON orders (email);\n\
           ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_orders_email;";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::ExcludeCreatedInChange,
        |at, action, stmt, ctx| {
            let AlterTableAction::AddConstraint(TableConstraint::Unique {
                columns,
                using_index,
                ..
            }) = action
            else {
                return vec![];
            };

            let table_key = at.name.catalog_key();
            // Table must exist in catalog_before (i.e. pre-existing).
            if ctx.catalog_before.get_table(table_key).is_none() {
                return vec![];
            }

            let message = match using_index {
                Some(idx_name) => {
                    let idx = ctx
                        .catalog_before
                        .get_index(idx_name)
                        .or_else(|| ctx.catalog_after.get_index(idx_name));
                    match idx {
                        None => format!(
                            "ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': \
                             referenced index does not exist.",
                            table = at.name.display_name(),
                        ),
                        Some(idx) if !idx.unique => format!(
                            "ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': \
                             referenced index is not UNIQUE.",
                            table = at.name.display_name(),
                        ),
                        Some(_) => return vec![], // safe
                    }
                }
                None => format!(
                    "ADD UNIQUE on existing table '{table}' without USING INDEX \
                     on column(s) [{columns}]. Create a unique index CONCURRENTLY \
                     first, then use ADD CONSTRAINT ... UNIQUE USING INDEX.",
                    table = at.name.display_name(),
                    columns = columns.join(", "),
                ),
            };

            vec![rule.make_finding(message, ctx.file, &stmt.span)]
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{MigrationRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn add_unique_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                name: Some(format!("uq_{}", columns.join("_"))),
                columns: columns.iter().map(|s| s.to_string()).collect(),
                using_index: None,
            })],
        }))
    }

    fn add_unique_using_index_stmt(table: &str, idx_name: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                name: Some(format!("uq_{}", idx_name)),
                columns: vec![], // empty with USING INDEX
                using_index: Some(idx_name.to_string()),
            })],
        }))
    }

    #[test]
    fn test_add_unique_no_existing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_index_still_fires() {
        // Even with a pre-existing unique index, without USING INDEX PostgreSQL
        // builds a NEW index under ACCESS EXCLUSIVE lock.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_unique_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("nonexistent", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_constraint_still_fires() {
        // Even with a pre-existing UNIQUE constraint, without USING INDEX
        // PostgreSQL builds a NEW index.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .unique("uq_orders_email", &["email"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_unique_using_index_with_backing_unique_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_unique_using_index_non_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_using_index_no_backing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_nonexistent")];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_using_index_created_in_same_migration_no_finding() {
        // Index exists in catalog_after but not in catalog_before
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], true);
            })
            .build();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Migration(MigrationRule::Pgm021).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
