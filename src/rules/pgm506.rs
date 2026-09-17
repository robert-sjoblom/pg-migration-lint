#![doc = include_str!("docs/pgm506.md")]

use crate::parser::ir::{IrNode, Located, TablePersistence};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "CREATE UNLOGGED TABLE";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm506.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateTable(ref ct) = stmt.node
            && ct.persistence == TablePersistence::Unlogged
        {
            findings.push(rule.make_finding(
                format!(
                    "CREATE UNLOGGED TABLE '{}'. Unlogged tables are truncated on \
                     crash recovery and are not replicated to standbys.",
                    ct.name.display_name()
                ),
                ctx.file,
                &stmt.span,
            ));
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

    fn rule_id() -> RuleId {
        RuleId::Pgm506
    }

    #[test]
    fn test_unlogged_table_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("scratch"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .with_persistence(TablePersistence::Unlogged)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_permanent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_temporary_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(
            CreateTable::test(QualifiedName::unqualified("tmp_data"))
                .with_columns(vec![ColumnDef::test("id", "integer")])
                .with_persistence(TablePersistence::Temporary)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
