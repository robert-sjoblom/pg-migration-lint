#![doc = include_str!("docs/pgm505.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "RENAME COLUMN on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm505.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::RenameColumn {
            table,
            old_name,
            new_name,
        } = node
        {
            Some((
                table,
                format!(
                    "Renaming column '{old_name}' to '{new_name}' on existing table \
                     '{table}' will break queries referencing the old column name.",
                    old_name = old_name,
                    new_name = new_name,
                    table = table.display_name(),
                ),
            ))
        } else {
            None
        }
    })
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
    fn test_rename_column_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        })];

        let findings = RuleId::Pgm505.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_rename_column_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("order_status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        })];

        let findings = RuleId::Pgm505.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
