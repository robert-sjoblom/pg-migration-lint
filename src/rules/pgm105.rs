#![doc = include_str!("docs/pgm105.md")]

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Column uses serial/bigserial instead of identity column";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm105.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                for col in &ct.columns {
                    if col.is_serial {
                        findings.push(rule.make_finding(
                                format!(
                                    "Column '{}' on '{}' uses a sequence default \
                                     (serial/bigserial). Prefer GENERATED {{ ALWAYS | BY DEFAULT }} \
                                     AS IDENTITY for new tables (PostgreSQL 10+). Identity columns \
                                     have better ownership semantics and are the SQL standard \
                                     approach.",
                                    col.name, ct.name.display_name(),
                                ),
                                ctx.file,
                                &stmt.span,
                            ));
                    }
                }
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    if let AlterTableAction::AddColumn(col) = action
                        && col.is_serial
                    {
                        findings.push(rule.make_finding(
                                    format!(
                                        "Column '{}' on '{}' uses a sequence default \
                                         (serial/bigserial). Prefer GENERATED {{ ALWAYS | BY DEFAULT }} \
                                         AS IDENTITY for new tables (PostgreSQL 10+). Identity columns \
                                         have better ownership semantics and are the SQL standard \
                                         approach.",
                                        col.name, at.name.display_name(),
                                    ),
                                    ctx.file,
                                    &stmt.span,
                                ));
                    }
                }
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
    fn test_serial_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("id", "int4")
                    .with_nullable(false)
                    .with_inline_pk()
                    .with_serial(),
            ]),
        ))];

        let findings = RuleId::Pgm105.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_bigserial_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("id", "int8")
                    .with_nullable(false)
                    .with_inline_pk()
                    .with_serial(),
            ]),
        ))];

        let findings = RuleId::Pgm105.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_identity_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        // An int4 column without the is_serial flag — e.g. GENERATED ALWAYS AS IDENTITY
        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("id", "int4")
                    .with_nullable(false)
                    .with_inline_pk(),
            ]),
        ))];

        let findings = RuleId::Pgm105.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_column_serial_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddColumn(
                ColumnDef::test("seq_id", "int4").with_serial(),
            )],
        }))];

        let findings = RuleId::Pgm105.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
