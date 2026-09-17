#![doc = include_str!("docs/pgm302.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "UPDATE on existing table in migration";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm302.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Minor;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::UpdateTable(ut) = node {
            Some((
                &ut.table_name,
                format!(
                    "UPDATE on existing table '{}' in a migration. Unbatched updates \
                     hold row locks for the full statement duration. Verify row volume \
                     and consider batched execution.",
                    ut.table_name.display_name()
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
        RuleId::Pgm302
    }

    #[test]
    fn test_update_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/005.sql");

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_update_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_two_updates_same_table_emit_two_findings_with_dedup_key() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/005.sql");

        let stmts = vec![
            located(UpdateTable::test(QualifiedName::unqualified("orders")).into()),
            located(UpdateTable::test(QualifiedName::unqualified("orders")).into()),
        ];

        let findings = rule_id().check(&stmts, &ctx);
        // Rule emits one finding per statement (dedup is pipeline-level)
        assert_eq!(findings.len(), 2);
        // Both carry the same dedup key so the pipeline can collapse them
        assert_eq!(findings[0].dedup_key.as_deref(), Some("orders"));
        assert_eq!(findings[1].dedup_key.as_deref(), Some("orders"));
    }

    #[test]
    fn test_update_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(
            UpdateTable::test(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
