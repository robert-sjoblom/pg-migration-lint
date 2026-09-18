#![doc = include_str!("docs/pgm101.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses timestamp without time zone";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm101.md");

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
        |tn| tn.name.eq_ignore_ascii_case("timestamp"),
        |col, table, _tn| {
            format!(
                "Column '{}' on '{}' uses 'timestamp without time zone'. \
                     Use 'timestamptz' (timestamp with time zone) instead to \
                     store unambiguous points in time.",
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
    fn test_create_table_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("created_at", "timestamp")]),
        ))];

        let findings = RuleId::Pgm101.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_timestamptz_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("events"))
                .with_columns(vec![ColumnDef::test("created_at", "timestamptz")]),
        ))];

        let findings = RuleId::Pgm101.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AddColumn(ColumnDef::test(
                "updated_at",
                "timestamp",
            ))],
        }))];

        let findings = RuleId::Pgm101.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_alter_column_type_timestamp_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "created_at".to_string(),
                new_type: TypeName::simple("timestamp"),
                old_type: None,
            }],
        }))];

        let findings = RuleId::Pgm101.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
