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
                        "DROP TABLE '{}' without IF EXISTS will fail if the table does not exist.",
                        dt.name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
            IrNode::DropIndex(di) if !di.if_exists => {
                findings.push(rule.make_finding(
                    format!(
                        "DROP INDEX '{}' without IF EXISTS will fail if the index does not exist.",
                        di.index_name.display_name()
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
    use crate::rules::test_helpers::{lint_ctx, located, normalized};

    #[test]
    fn test_drop_index_qualified_by_user_reports_schema_in_message() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = normalized(vec![located(IrNode::DropIndex(
            DropIndex::test(QualifiedName::qualified("myschema", "idx_orders_status"))
                .with_if_exists(false),
        ))]);

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].message,
            "DROP INDEX 'myschema.idx_orders_status' without IF EXISTS will fail if the index does not exist."
        );
    }

    #[test]
    fn test_drop_index_unqualified_reports_bare_name_after_default_schema_applied() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = normalized(vec![located(IrNode::DropIndex(
            DropIndex::test(QualifiedName::unqualified("idx_orders_status")).with_if_exists(false),
        ))]);

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].message,
            "DROP INDEX 'idx_orders_status' without IF EXISTS will fail if the index does not exist."
        );
    }

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
            DropIndex::test(QualifiedName::unqualified("idx_orders_status")).with_if_exists(false),
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
            QualifiedName::unqualified("idx_orders_status"),
        )))];

        let findings = RuleId::Pgm401.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
