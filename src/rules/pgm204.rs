#![doc = include_str!("docs/pgm204.md")]

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "TRUNCATE TABLE CASCADE on existing table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm204.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::TruncateTable(ref tt) = stmt.node {
            if !tt.cascade {
                continue;
            }

            let table_key = tt.name.catalog_key();

            if !ctx.is_existing_table(table_key) {
                continue;
            }

            // Find FK dependencies: tables whose FKs reference the truncated table
            let mut dependents: Vec<String> = Vec::new();
            for table in ctx.catalog_before.tables() {
                // Skip the table being truncated itself
                if table.name == table_key {
                    continue;
                }
                for constraint in &table.constraints {
                    if let ConstraintState::ForeignKey { ref_table, .. } = constraint
                        && ref_table == table_key
                    {
                        dependents.push(table.display_name.clone());
                        break; // One mention per table is enough
                    }
                }
            }

            let message = if dependents.is_empty() {
                format!(
                    "TRUNCATE TABLE '{}' CASCADE silently extends to all tables with \
                     foreign key references to '{}', and recursively to their dependents. \
                     Verify the full cascade chain is intentionally truncated.",
                    tt.name.display_name(),
                    tt.name.display_name()
                )
            } else {
                dependents.sort();
                format!(
                    "TRUNCATE TABLE '{}' CASCADE silently extends to all tables with \
                     foreign key references to '{}', and recursively to their dependents. \
                     Known FK dependencies from: {}.",
                    tt.name.display_name(),
                    tt.name.display_name(),
                    dependents.join(", ")
                )
            };

            findings.push(rule.make_finding(message, ctx.file, &stmt.span));
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

    fn rule_id() -> RuleId {
        RuleId::Pgm204
    }

    #[test]
    fn test_truncate_cascade_existing_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_truncate_cascade_with_fk_deps_lists_them() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", true)
                    .pk(&["id"])
                    .fk("fk_orders_customer", &["customer_id"], "customers", &["id"]);
            })
            .table("addresses", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", true)
                    .pk(&["id"])
                    .fk(
                        "fk_addresses_customer",
                        &["customer_id"],
                        "customers",
                        &["id"],
                    );
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_truncate_without_cascade_no_finding() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers")).into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_truncate_cascade_new_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["customers"]);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_truncate_cascade_nonexistent_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/010.sql");

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
