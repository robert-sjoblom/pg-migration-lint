//! PGM021 — `ADD UNIQUE` on existing table without `USING INDEX`
//!
//! Detects `ALTER TABLE ... ADD CONSTRAINT ... UNIQUE (columns)` on tables that
//! already exist where the target columns don't already have a covering unique
//! index. The safe pattern is to first create a unique index CONCURRENTLY, then
//! add the constraint using that index.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

/// Rule that flags adding a UNIQUE constraint to an existing table without a
/// pre-existing unique index on the constraint columns.
pub struct Pgm021;

impl Rule for Pgm021 {
    fn id(&self) -> &'static str {
        "PGM021"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "ADD UNIQUE on existing table without USING INDEX"
    }

    fn explain(&self) -> &'static str {
        "PGM021 — ADD UNIQUE on existing table without USING INDEX\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... UNIQUE (columns) where the table\n\
         already exists and the target columns don't have a pre-existing unique\n\
         index.\n\
         \n\
         Why it's dangerous:\n\
         Adding a UNIQUE constraint inline builds a unique index under an ACCESS\n\
         EXCLUSIVE lock, blocking all reads and writes for the duration. For\n\
         large tables this can cause extended downtime. Unlike CHECK and FOREIGN\n\
         KEY constraints, NOT VALID does NOT apply to UNIQUE constraints, so\n\
         there is no NOT VALID escape hatch.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE (email);\n\
         \n\
         Fix (safe pattern — build unique index concurrently first):\n\
           CREATE UNIQUE INDEX CONCURRENTLY idx_orders_email ON orders (email);\n\
           ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_orders_email;"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        alter_table_check::check_alter_actions(
            statements,
            ctx,
            TableScope::ExcludeCreatedInChange,
            |at, action, stmt, ctx| {
                let AlterTableAction::AddConstraint(TableConstraint::Unique { columns, .. }) =
                    action
                else {
                    return vec![];
                };

                let table_key = at.name.catalog_key();
                let Some(table) = ctx.catalog_before.get_table(table_key) else {
                    return vec![];
                };

                if table.has_unique_covering(columns) {
                    return vec![];
                }

                vec![self.make_finding(
                    format!(
                        "ADD UNIQUE on existing table '{table}' without a \
                     pre-existing unique index on column(s) [{columns}]. \
                     Create a unique index CONCURRENTLY first, then use \
                     ADD CONSTRAINT ... UNIQUE USING INDEX.",
                        table = at.name.display_name(),
                        columns = columns.join(", "),
                    ),
                    ctx.file,
                    &stmt.span,
                )]
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn add_unique_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                name: Some(format!("uq_{}", columns.join("_"))),
                columns: columns.iter().map(|s| s.to_string()).collect(),
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

        let findings = Pgm021.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_index_no_finding() {
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

        let findings = Pgm021.check(&stmts, &ctx);
        assert!(findings.is_empty());
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

        let findings = Pgm021.check(&stmts, &ctx);
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

        let findings = Pgm021.check(&stmts, &ctx);
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

        let findings = Pgm021.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_constraint_no_finding() {
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

        let findings = Pgm021.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
