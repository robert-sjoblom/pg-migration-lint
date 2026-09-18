#![doc = include_str!("docs/pgm022.md")]

use crate::parser::ir::{IrNode, Located, ReindexTarget};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Missing CONCURRENTLY on REINDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm022.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::Reindex(ref r) = stmt.node {
            if r.concurrent {
                continue;
            }

            let target_display = match &r.target {
                ReindexTarget::Relation(name) => name.display_name().to_string(),
                ReindexTarget::Named(name) => name.clone(),
            };

            findings.push(rule.make_finding(
                format!(
                    "REINDEX {} '{target_display}' should use CONCURRENTLY to avoid \
                     holding an ACCESS EXCLUSIVE lock. \
                     Use REINDEX {} CONCURRENTLY '{target_display}' (PostgreSQL 12+).",
                    r.kind, r.kind,
                ),
                ctx.file,
                &stmt.span,
            ));
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
    fn reindex_table_without_concurrently_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::unqualified("orders")).into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn reindex_concurrently_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::unqualified("orders"))
                .with_concurrent()
                .into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn all_reindex_kinds_fire_without_concurrently() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let cases: Vec<(&str, IrNode)> = vec![
            (
                "INDEX",
                Reindex::test_index(QualifiedName::unqualified("idx_foo")).into(),
            ),
            ("SCHEMA", Reindex::test_schema("public").into()),
            ("DATABASE", Reindex::test_database("mydb").into()),
        ];

        for (kind_label, node) in cases {
            let stmts = vec![located(node)];
            let findings = RuleId::Pgm022.check(&stmts, &ctx);
            assert_eq!(
                findings.len(),
                1,
                "REINDEX {kind_label} should produce exactly one finding",
            );
            assert!(
                findings[0]
                    .message
                    .contains(&format!("REINDEX {kind_label}")),
                "message should mention REINDEX {kind_label}, got: {}",
                findings[0].message,
            );
        }
    }

    #[test]
    fn reindex_schema_qualified_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Reindex::test_table(QualifiedName::qualified("myschema", "orders")).into(),
        )];

        let findings = RuleId::Pgm022.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("myschema.orders"),
            "message should include schema-qualified name, got: {}",
            findings[0].message,
        );
    }
}
