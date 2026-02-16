//! PGM016 — `SET NOT NULL` on existing table requires ACCESS EXCLUSIVE lock
//!
//! Detects `ALTER TABLE ... ALTER COLUMN ... SET NOT NULL` on tables that
//! already exist. This requires scanning the entire table and acquiring an
//! ACCESS EXCLUSIVE lock. The safe pattern is to add a CHECK constraint
//! with NOT VALID, validate it, then set NOT NULL.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str =
    "SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock";

pub(super) const EXPLAIN: &str = "PGM016 — SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ALTER COLUMN ... SET NOT NULL on a table that already\n\
         exists in the database (not created in the same set of changed files).\n\
         \n\
         Why it's dangerous:\n\
         SET NOT NULL acquires an ACCESS EXCLUSIVE lock on the table, blocking\n\
         all concurrent reads and writes. PostgreSQL must also perform a full\n\
         table scan to verify that no existing rows contain NULL in the column.\n\
         On large tables this can cause significant downtime.\n\
         \n\
         Safe alternative (PostgreSQL 12+):\n\
         1. Add a CHECK constraint with NOT VALID:\n\
            ALTER TABLE orders ADD CONSTRAINT orders_status_nn\n\
              CHECK (status IS NOT NULL) NOT VALID;\n\
         2. Validate the constraint (only takes a SHARE UPDATE EXCLUSIVE lock):\n\
            ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;\n\
         3. Set NOT NULL (instant since PG 12 sees the validated CHECK):\n\
            ALTER TABLE orders ALTER COLUMN status SET NOT NULL;\n\
         4. Optionally drop the now-redundant CHECK constraint:\n\
            ALTER TABLE orders DROP CONSTRAINT orders_status_nn;\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ALTER COLUMN status SET NOT NULL;\n\
         \n\
         Fix (safe three-step pattern):\n\
           ALTER TABLE orders ADD CONSTRAINT orders_status_nn\n\
             CHECK (status IS NOT NULL) NOT VALID;\n\
           ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;\n\
           ALTER TABLE orders ALTER COLUMN status SET NOT NULL;";

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
            if let AlterTableAction::SetNotNull { column_name } = action {
                vec![rule.make_finding(
                    format!(
                        "SET NOT NULL on column '{col}' of existing table '{table}' \
                         requires an ACCESS EXCLUSIVE lock and full table scan. \
                         Use a CHECK constraint with NOT VALID, validate it, \
                         then set NOT NULL.",
                        col = column_name,
                        table = at.name.display_name(),
                    ),
                    ctx.file,
                    &stmt.span,
                )]
            } else {
                vec![]
            }
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

    #[test]
    fn test_set_not_null_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm016).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_set_not_null_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm016).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_set_not_null_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm016).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
