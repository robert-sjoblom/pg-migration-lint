#![doc = include_str!("docs/pgm018.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "CLUSTER on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm018.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::Cluster(ref c) = stmt.node {
            let table_key = c.table.catalog_key();

            if ctx.is_existing_table(table_key) {
                let message = match &c.index {
                    Some(idx) => format!(
                        "CLUSTER on table '{}' USING '{}' rewrites the entire table \
                         under ACCESS EXCLUSIVE lock for the full duration. \
                         All reads and writes are blocked. \
                         This is rarely appropriate in an online migration.",
                        c.table.display_name(),
                        idx,
                    ),
                    None => format!(
                        "CLUSTER on table '{}' rewrites the entire table \
                         under ACCESS EXCLUSIVE lock for the full duration. \
                         All reads and writes are blocked. \
                         This is rarely appropriate in an online migration.",
                        c.table.display_name(),
                    ),
                };

                findings.push(rule.make_finding(message, ctx.file, &stmt.span));
            }
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    #[test]
    fn cluster_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .index("idx_customers_email", &["email"], false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers"))
                .with_index("idx_customers_email")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn cluster_without_index_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers")).into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn cluster_table_created_in_same_change_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["customers"]);

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("customers"))
                .with_index("idx_customers_email")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn cluster_nonexistent_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            Cluster::test(QualifiedName::unqualified("nonexistent"))
                .with_index("idx_foo")
                .into(),
        )];

        let findings = RuleId::Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
