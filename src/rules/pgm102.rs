#![doc = include_str!("docs/pgm102.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses timestamp or timestamptz with precision 0";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm102.md");

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
        |tn| {
            (tn.name.eq_ignore_ascii_case("timestamp")
                || tn.name.eq_ignore_ascii_case("timestamptz"))
                && tn.modifiers == [0]
        },
        |col, table, tn| {
            format!(
                "Column '{}' on '{}' uses '{}(0)'. Precision 0 causes \
                     rounding, not truncation \u{2014} a value of '23:59:59.9' \
                     rounds to the next day. Use full precision and format on \
                     output instead.",
                col,
                table.display_name(),
                tn.name,
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
    fn test_timestamptz_0_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("created_at", "timestamptz")
                    .with_type(TypeName::with_modifiers("timestamptz", vec![0])),
            ]),
        ))];

        let findings = RuleId::Pgm102.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_timestamp_0_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("ts", "timestamp")
                    .with_type(TypeName::with_modifiers("timestamp", vec![0])),
            ]),
        ))];

        let findings = RuleId::Pgm102.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_timestamptz_3_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events")).with_columns(vec![
                ColumnDef::test("created_at", "timestamptz")
                    .with_type(TypeName::with_modifiers("timestamptz", vec![3])),
            ]),
        ))];

        let findings = RuleId::Pgm102.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_timestamptz_no_modifier_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("created_at", "timestamptz")]),
        ))];

        let findings = RuleId::Pgm102.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
