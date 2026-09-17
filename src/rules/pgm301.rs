#![doc = include_str!("docs/pgm301.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, existing_table_check};

pub(super) const DESCRIPTION: &str = "INSERT INTO existing table in migration";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm301.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Info;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    existing_table_check::check_existing_table(statements, ctx, rule, |node| {
        if let IrNode::InsertInto(ii) = node {
            Some((
                &ii.table_name,
                format!(
                    "INSERT INTO existing table '{}' in a migration. \
                     Ensure this is intentional seed data and that row volume is bounded.",
                    ii.table_name.display_name()
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
        RuleId::Pgm301
    }

    #[test]
    fn test_insert_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("config", |t| {
                t.column("key", "text", false)
                    .column("value", "text", true)
                    .pk(&["key"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/005.sql");

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_insert_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("config", |t| {
                t.column("key", "text", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["config"]);

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_insert_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(
            InsertInto::test(QualifiedName::unqualified("config")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
