//! PGM012 — `ADD PRIMARY KEY` on existing table without prior UNIQUE constraint
//!
//! Detects `ALTER TABLE ... ADD PRIMARY KEY (col)` on tables that already exist
//! where the target columns don't already have a UNIQUE constraint or unique index.
//! The safe pattern is to first create a unique index CONCURRENTLY, then add the
//! primary key using that index.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str =
    "ADD PRIMARY KEY on existing table without prior UNIQUE constraint";

pub(super) const EXPLAIN: &str = "PGM012 — ADD PRIMARY KEY on existing table without prior UNIQUE constraint\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD PRIMARY KEY (columns) where the table already exists\n\
         and the target columns don't have a pre-existing UNIQUE constraint or\n\
         unique index.\n\
         \n\
         Why it's dangerous:\n\
         Adding a primary key without a pre-existing unique index causes\n\
         PostgreSQL to build a unique index inline (not concurrently). This\n\
         takes an ACCESS EXCLUSIVE lock on the table for the duration of the\n\
         index build. If duplicates exist, the command fails at deploy time.\n\
         \n\
         If the columns already have a unique index or UNIQUE constraint,\n\
         uniqueness is already enforced and the PK addition is logically safe\n\
         (though still takes a brief lock).\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD PRIMARY KEY (id);\n\
         \n\
         Fix (safe pattern — build unique index concurrently first):\n\
           CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);\n\
           ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;";

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
            let AlterTableAction::AddConstraint(TableConstraint::PrimaryKey { columns }) = action
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

            vec![rule.make_finding(
                format!(
                    "ADD PRIMARY KEY on existing table '{table}' without a \
                     prior UNIQUE constraint or unique index on column(s) \
                     [{columns}]. Create a unique index CONCURRENTLY first, \
                     then use ADD PRIMARY KEY USING INDEX.",
                    table = at.name.display_name(),
                    columns = columns.join(", "),
                ),
                ctx.file,
                &stmt.span,
            )]
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

    fn add_pk_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: columns.iter().map(|s| s.to_string()).collect(),
                },
            )],
        }))
    }

    #[test]
    fn test_add_pk_no_unique_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_with_unique_constraint_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .unique("uq_orders_id", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_with_unique_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("nonexistent", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Migration(MigrationRule::Pgm012).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
