#![doc = include_str!("docs/pgm019.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD EXCLUDE constraint on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm019.md");

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
            if matches!(
                action,
                AlterTableAction::AddConstraint(TableConstraint::Exclude { .. })
            ) {
                vec![rule.make_finding(
                    format!(
                        "Adding EXCLUDE constraint on existing table '{}' acquires \
                         ACCESS EXCLUSIVE lock and scans all rows. There is no online \
                         alternative \u{2014} consider scheduling this during a maintenance \
                         window.",
                        at.name.display_name(),
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

    /// Helper to build an ALTER TABLE ... ADD CONSTRAINT ... EXCLUDE statement.
    fn add_exclude_stmt(table: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Exclude {
                name: Some("excl_orders".to_string()),
                elements: vec![],
            })],
        }))
    }

    #[test]
    fn test_fires_on_existing_table() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_on_new_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_table_not_in_catalog() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_exclude_stmt("orders")];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_exclude_inside_create_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        // EXCLUDE inside a CreateTable, not an AlterTable
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![
                    ColumnDef::test("id", "bigint").with_nullable(false),
                    ColumnDef::test("order_range", "tsrange"),
                ])
                .with_constraints(vec![TableConstraint::Exclude {
                    name: Some("excl_orders".to_string()),
                    elements: vec![],
                }]),
        ))];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fires_with_schema_qualified_name() {
        let before = CatalogBuilder::new()
            .table("myschema.orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::qualified("myschema", "orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Exclude {
                name: Some("excl_orders".to_string()),
                elements: vec![],
            })],
        }))];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "message should include schema-qualified name, got: {}",
            findings[0].message,
        );
    }

    #[test]
    fn test_fires_in_multi_action_alter_table() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("order_range", "tsrange", true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        // ALTER TABLE with multiple actions: ADD COLUMN + ADD EXCLUDE
        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![
                AlterTableAction::AddColumn(ColumnDef::test("extra", "text")),
                AlterTableAction::AddConstraint(TableConstraint::Exclude {
                    name: Some("excl_orders".to_string()),
                    elements: vec![],
                }),
            ],
        }))];

        let findings = RuleId::Pgm019.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id.as_str(), "PGM019");
    }
}
