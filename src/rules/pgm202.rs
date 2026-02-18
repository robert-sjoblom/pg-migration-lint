//! PGM202 — `DROP TABLE ... CASCADE` on existing table
//!
//! Detects `DROP TABLE ... CASCADE` targeting a table that exists in `catalog_before`.
//! CASCADE silently drops all dependent objects (views, foreign keys, triggers, rules)
//! that reference the dropped table, amplifying the blast radius beyond a simple DROP.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "DROP TABLE CASCADE on existing table";

pub(super) const EXPLAIN: &str = "PGM202 — DROP TABLE CASCADE on existing table\n\
         \n\
         What it detects:\n\
         A DROP TABLE ... CASCADE statement targeting a table that already exists\n\
         in the database.\n\
         \n\
         Why it matters:\n\
         CASCADE silently drops all dependent objects — foreign keys, views,\n\
         triggers, and rules — that reference the dropped table. The developer\n\
         may not be aware of all dependencies, leading to unexpected breakage\n\
         in other tables and application code.\n\
         \n\
         A plain DROP TABLE (without CASCADE) would fail if dependencies exist,\n\
         which is a safer default. CASCADE bypasses that safety net.\n\
         \n\
         Example:\n\
           DROP TABLE customers CASCADE;\n\
         \n\
         If the 'orders' table has a FK referencing 'customers', CASCADE will\n\
         silently drop that FK constraint on 'orders'.\n\
         \n\
         Recommended approach:\n\
         1. Identify all dependent objects before dropping.\n\
         2. Explicitly drop or alter dependencies in separate migration steps.\n\
         3. Use plain DROP TABLE (without CASCADE) so PostgreSQL will error\n\
            if unexpected dependencies remain.\n\
         \n\
         This rule is MAJOR severity because CASCADE silently destroys\n\
         dependent objects the developer may not be aware of.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::DropTable(ref dt) = stmt.node {
            if !dt.cascade {
                continue;
            }

            let table_key = dt.name.catalog_key();

            if !ctx.is_existing_table(table_key) {
                continue;
            }

            // Find FK dependencies: tables whose FKs reference the dropped table
            let mut dependents: Vec<String> = Vec::new();
            for table in ctx.catalog_before.tables() {
                // Skip the table being dropped itself
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
                    "DROP TABLE CASCADE on '{}' will silently drop dependent objects. \
                     Views, triggers, and rules referencing this table are not tracked \
                     by the catalog and may also be affected.",
                    dt.name.display_name()
                )
            } else {
                dependents.sort();
                format!(
                    "DROP TABLE CASCADE on '{}' will silently drop dependent objects. \
                     Known FK dependencies from: {}.",
                    dt.name.display_name(),
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
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{DestructiveRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn rule_id() -> RuleId {
        RuleId::Destructive(DestructiveRule::Pgm202)
    }

    #[test]
    fn test_drop_cascade_existing_fires() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("customers"))
                .with_if_exists(false)
                .with_cascade(true),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_cascade_with_fk_deps_lists_them() {
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
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("customers"))
                .with_if_exists(false)
                .with_cascade(true),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_cascade_new_table_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("customers".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("customers"))
                .with_if_exists(false)
                .with_cascade(true),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_no_cascade_no_finding() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("customers")).with_if_exists(false),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_cascade_nonexistent_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/010.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified("customers"))
                .with_if_exists(true)
                .with_cascade(true),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
