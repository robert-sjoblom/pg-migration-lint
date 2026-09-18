#![doc = include_str!("docs/pgm008.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD COLUMN NOT NULL without DEFAULT on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm008.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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
            if let AlterTableAction::AddColumn(col) = action
                && !col.nullable
                && col.default_expr.is_none()
            {
                vec![rule.make_finding(
                    format!(
                        "Adding NOT NULL column '{col}' to existing table '{table}' \
                         without a DEFAULT will fail if the table has any rows. \
                         Add a DEFAULT value, or add the column as nullable and backfill.",
                        col = col.name,
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
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn test_not_null_no_default_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("status", "text").with_nullable(false),
            )],
        }))];

        let findings = RuleId::Pgm008.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_not_null_with_default_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("status", "text")
                    .with_nullable(false)
                    .with_default(DefaultExpr::Literal("pending".to_string())),
            )],
        }))];

        let findings = RuleId::Pgm008.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_nullable_no_default_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "notes", "text",
            ))],
        }))];

        let findings = RuleId::Pgm008.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_table_not_null_no_default_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("status", "text").with_nullable(false),
            )],
        }))];

        let findings = RuleId::Pgm008.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
