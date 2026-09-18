#![doc = include_str!("docs/pgm103.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses char(n) type";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm103.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    column_type_check::check_column_types(
        statements,
        ctx,
        rule,
        |tn| tn.name.eq_ignore_ascii_case("bpchar"),
        |col, table, tn| {
            let display = if let Some(&n) = tn.modifiers.first() {
                format!("char({})", n)
            } else {
                "char".to_string()
            };
            format!(
                "Column '{}' on '{}' uses '{}'. The char(n) type pads with \
                     spaces, wastes storage, and is no faster than text or varchar \
                     in PostgreSQL. Use text or varchar instead.",
                col,
                table.display_name(),
                display,
            )
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn test_char_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("countries"))
                .with_columns(vec![ColumnDef::test("code", "bpchar").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm103.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_char_n_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("countries")).with_columns(vec![
                ColumnDef::test("code", "bpchar")
                    .with_nullable(false)
                    .with_type(TypeName::with_modifiers("bpchar", vec![2])),
            ]),
        ))];

        let findings = RuleId::Pgm103.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_text_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("countries"))
                .with_columns(vec![ColumnDef::test("code", "text").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm103.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_varchar_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("countries")).with_columns(vec![
                ColumnDef::test("code", "varchar")
                    .with_nullable(false)
                    .with_type(TypeName::with_modifiers("varchar", vec![2])),
            ]),
        ))];

        let findings = RuleId::Pgm103.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
