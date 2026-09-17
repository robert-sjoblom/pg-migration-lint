#![doc = include_str!("docs/pgm201.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "DROP TABLE on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm201.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::DropTable(dt) = node {
            Some((
                &dt.name,
                format!(
                    "DROP TABLE '{}' removes an existing table. \
                     This is irreversible and all data will be lost.",
                    dt.name.display_name()
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
    fn test_drop_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm201.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
