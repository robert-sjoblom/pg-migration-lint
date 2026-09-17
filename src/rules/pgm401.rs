#![doc = include_str!("docs/pgm401.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Missing IF EXISTS on DROP TABLE / DROP INDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm401.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::DropTable(dt) if !dt.if_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "DROP TABLE '{}': add IF EXISTS for idempotent migrations.",
                        dt.name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            IrNode::DropIndex(di) if !di.if_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "DROP INDEX '{}': add IF EXISTS for idempotent migrations.",
                        di.index_name
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            _ => {}
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn test_drop_table_without_if_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("orders")).with_if_exists(false),
        ))];

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_table_with_if_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropTable(DropTable::test(
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_index_without_if_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_index_with_if_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(DropIndex::test(
            "idx_orders_status",
        )))];

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
