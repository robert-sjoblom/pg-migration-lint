#![doc = include_str!("docs/pgm403.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "CREATE TABLE IF NOT EXISTS for already-existing table is a misleading no-op";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm403.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateTable(ct) = &stmt.node
            && ct.if_not_exists
        {
            let key = ct.name.catalog_key();
            if ctx.catalog_before.has_table(key) {
                findings.push(rule.make_finding(
                    format!(
                        "CREATE TABLE IF NOT EXISTS '{}' is a no-op \u{2014} the table already \
                             exists in the migration history. The definition in this statement is \
                             silently ignored by PostgreSQL. If the column definitions differ from \
                             the actual table state, this migration is misleading.",
                        ct.name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
        }
    }

    findings
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
    fn fires_when_table_exists_in_catalog_before() {
        let before = CatalogBuilder::new()
            .table("public.customers", |t| {
                t.column("id", "bigint", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::qualified("public", "customers"))
                .with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn no_finding_when_table_does_not_exist() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_if_not_exists(true),
        ))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn no_finding_without_if_not_exists() {
        let before = CatalogBuilder::new()
            .table("public.customers", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        // CREATE TABLE without IF NOT EXISTS — PGM403 should not fire
        // (this would be caught by PGM402 instead)
        let stmts = vec![located(IrNode::CreateTable(CreateTable::test(
            QualifiedName::qualified("public", "customers"),
        )))];

        let findings = RuleId::Pgm403.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
