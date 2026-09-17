#![doc = include_str!("docs/pgm015.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD CHECK on existing table without NOT VALID";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm015.md");

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
            if let AlterTableAction::AddConstraint(TableConstraint::Check {
                not_valid: false,
                ..
            }) = action
            {
                vec![rule.make_finding(
                    format!(
                        "Adding CHECK constraint on existing table '{table}' without \
                         NOT VALID will scan the entire table while holding a SHARE \
                         ROW EXCLUSIVE lock, blocking concurrent writes. Use ADD \
                         CONSTRAINT ... NOT VALID, then VALIDATE CONSTRAINT in a \
                         separate statement.",
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
    fn test_check_without_not_valid_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::Pgm015.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_check_with_not_valid_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: true,
            })],
        }))];

        let findings = RuleId::Pgm015.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::Pgm015.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = RuleId::Pgm015.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
