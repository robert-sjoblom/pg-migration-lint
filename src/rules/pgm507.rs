//! PGM507 — `DROP NOT NULL` on existing table
//!
//! Detects `ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL` on tables that
//! already exist. Dropping NOT NULL silently allows NULLs where application
//! code may assume non-NULL values.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DROP NOT NULL on existing table allows NULL values";

pub(super) const EXPLAIN: &str = "PGM507 — DROP NOT NULL on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ALTER COLUMN ... DROP NOT NULL on a table that already\n\
         exists in the database (not created in the same set of changed files).\n\
         \n\
         Why it matters:\n\
         Dropping NOT NULL silently allows NULL values in a column where the\n\
         application may assume non-NULL. This is especially dangerous when the\n\
         column feeds into aggregations (COUNT vs COUNT(*), SUM with NULLs),\n\
         joins (NULL != NULL), or application logic that doesn't check for NULL.\n\
         \n\
         Example (risky):\n\
           ALTER TABLE orders ALTER COLUMN status DROP NOT NULL;\n\
         \n\
         Recommended approach:\n\
         1. Verify that all application code paths handle NULLs in this column.\n\
         2. Update aggregations and joins that assume non-NULL.\n\
         3. Consider adding a CHECK constraint or application-level validation\n\
            if only certain rows should allow NULL.";

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
            if let AlterTableAction::DropNotNull { column_name } = action {
                vec![rule.make_finding(
                    format!(
                        "DROP NOT NULL on column '{col}' of existing table '{table}' \
                         allows NULL values where the application may assume non-NULL. \
                         Verify that all code paths handle NULLs.",
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_drop_not_null_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm507.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_not_null_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm507.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_not_null_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm507.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
