//! PGM204 — `TRUNCATE TABLE ... CASCADE` on existing table
//!
//! Detects `TRUNCATE TABLE ... CASCADE` targeting a table that exists in `catalog_before`.
//! CASCADE silently extends the truncation to all tables with foreign key references
//! to the truncated table, and recursively to their dependents.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "TRUNCATE TABLE CASCADE on existing table";

pub(super) const EXPLAIN: &str = "PGM204 — TRUNCATE TABLE CASCADE on existing table\n\
         \n\
         What it detects:\n\
         A TRUNCATE TABLE ... CASCADE statement targeting a table that already\n\
         exists in the database.\n\
         \n\
         Why it matters:\n\
         CASCADE silently extends the truncation to all tables that have foreign\n\
         key references to the truncated table, and recursively to their\n\
         dependents. The developer may not be aware of the full cascade chain,\n\
         leading to unexpected data loss across multiple tables.\n\
         \n\
         A plain TRUNCATE (without CASCADE) would fail if FK dependencies exist,\n\
         which is a safer default. CASCADE bypasses that safety net.\n\
         \n\
         Example:\n\
           TRUNCATE TABLE customers CASCADE;\n\
         \n\
         If the 'orders' table has a FK referencing 'customers', CASCADE will\n\
         silently truncate 'orders' as well.\n\
         \n\
         Recommended approach:\n\
         1. Identify all dependent tables before truncating.\n\
         2. Explicitly truncate each table in the correct order.\n\
         3. Use plain TRUNCATE (without CASCADE) so PostgreSQL will error\n\
            if unexpected dependencies remain.\n\
         \n\
         This rule is MAJOR severity because CASCADE silently destroys\n\
         data in dependent tables the developer may not be aware of.";

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
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

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
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("customers".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(
            TruncateTable::test(QualifiedName::unqualified("customers"))
                .with_cascade(true)
                .into(),
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
