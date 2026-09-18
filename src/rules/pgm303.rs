#![doc = include_str!("docs/pgm303.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "DELETE FROM existing table in migration";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm303.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::DeleteFrom(df) = node {
            Some((
                &df.table_name,
                format!(
                    "DELETE FROM existing table '{}' in a migration. Unbatched deletes \
                     hold row locks and generate significant WAL. Verify row volume \
                     and consider batched execution.",
                    df.table_name.display_name()
                ),
            ))
        } else {
            None
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    fn rule_id() -> RuleId {
        RuleId::Pgm303
    }

    #[test]
    fn test_delete_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("audit_log", |t| {
                t.column("id", "bigint", false)
                    .column("created_at", "timestamptz", false)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/005.sql");

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_delete_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("audit_log", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["audit_log"]);

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_delete_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(
            DeleteFrom::test(QualifiedName::unqualified("audit_log")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
