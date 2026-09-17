#![doc = include_str!("docs/pgm108.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::column_type_check;
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses varchar(n) instead of text";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm108.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    column_type_check::check_column_types(
        statements,
        ctx,
        rule,
        |tn| tn.name.eq_ignore_ascii_case("varchar") && !tn.modifiers.is_empty(),
        |col, table, tn| {
            let n = tn
                .modifiers
                .first()
                .map(|m| m.to_string())
                .unwrap_or_default();
            format!(
                "Column '{}' on '{}' uses varchar({}). Prefer text \
                 — varchar(n) has no performance benefit in PostgreSQL \
                 and adds an artificial limit that may require future \
                 schema changes.",
                col,
                table.display_name(),
                n,
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
    fn test_varchar_n_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users")).with_columns(vec![
                ColumnDef::test("name", "varchar")
                    .with_type(TypeName::with_modifiers("varchar", vec![100])),
            ]),
        ))];

        let findings = RuleId::Pgm108.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_bare_varchar_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users"))
                .with_columns(vec![ColumnDef::test("name", "varchar")]),
        ))];

        let findings = RuleId::Pgm108.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_text_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("users"))
                .with_columns(vec![ColumnDef::test("name", "text")]),
        ))];

        let findings = RuleId::Pgm108.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_varchar_n_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("description", "varchar")
                    .with_type(TypeName::with_modifiers("varchar", vec![255])),
            )],
        }))];

        let findings = RuleId::Pgm108.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_alter_column_type_varchar_n_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::AlterColumnType {
                column_name: "name".to_string(),
                new_type: TypeName {
                    name: "varchar".to_string(),
                    modifiers: vec![200],
                },
                old_type: None,
            }],
        }))];

        let findings = RuleId::Pgm108.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
