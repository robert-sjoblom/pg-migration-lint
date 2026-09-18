#![doc = include_str!("docs/pgm104.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses the money type";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm104.md");

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
        |tn| tn.name.eq_ignore_ascii_case("money"),
        |col, table, _tn| {
            format!(
                "Column '{}' on '{}' uses the 'money' type. The money type \
                     depends on the lc_monetary locale setting, making it \
                     unreliable across environments. Use numeric(p,s) instead.",
                col,
                table.display_name(),
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
    fn test_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![ColumnDef::test("total", "money").with_nullable(false)]),
        ))];

        let findings = RuleId::Pgm104.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_numeric_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("total", "numeric")
                    .with_nullable(false)
                    .with_type(TypeName::with_modifiers("numeric", vec![12, 2])),
            ]),
        ))];

        let findings = RuleId::Pgm104.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_money_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "discount", "money",
            ))],
        }))];

        let findings = RuleId::Pgm104.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
