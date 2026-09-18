#![doc = include_str!("docs/pgm013.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str =
    "SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm013.md");

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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_set_not_null_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
