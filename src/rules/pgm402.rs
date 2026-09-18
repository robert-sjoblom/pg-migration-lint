#![doc = include_str!("docs/pgm402.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Missing IF NOT EXISTS on CREATE TABLE / CREATE INDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm402.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) if !ct.if_not_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "CREATE TABLE '{}': add IF NOT EXISTS for idempotent migrations.",
                        ct.name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            IrNode::CreateIndex(ci) if !ci.if_not_exists => {
                let index_name = ci.index_name.as_deref().unwrap_or("<unnamed>");
                findings.push(rule.make_finding(
                    format!(
                        "CREATE INDEX '{}': add IF NOT EXISTS for idempotent migrations.",
                        index_name
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
    fn test_create_table_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::CreateTable(CreateTable::test(
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Pgm402.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_create_table_with_if_not_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm402.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_create_index_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex::test(
            Some("idx_orders_status".to_string()),
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Pgm402.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn create_index_unnamed_without_if_not_exists_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::CreateIndex(CreateIndex::test(
            None,
            QualifiedName::unqualified("orders"),
        )))];

        let findings = RuleId::Pgm402.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_create_index_with_if_not_exists_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm402.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
