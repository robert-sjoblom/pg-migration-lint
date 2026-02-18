//! PGM012 — `DROP COLUMN` silently removes foreign key
//!
//! Detects `ALTER TABLE ... DROP COLUMN col` where `col` participates in a
//! `FOREIGN KEY` constraint on the table in `catalog_before`. Dropping a
//! column that is part of a foreign key (with `CASCADE`) silently removes
//! the FK constraint. The referential integrity guarantee is lost.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes foreign key";

pub(super) const EXPLAIN: &str = "PGM012 — DROP COLUMN silently removes foreign key\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... DROP COLUMN where the dropped column participates\n\
         in a FOREIGN KEY constraint on the table.\n\
         \n\
         Why it matters:\n\
         Dropping a column that is part of a foreign key (with CASCADE)\n\
         silently removes the FK constraint. The referential integrity\n\
         guarantee is lost. This can lead to orphaned rows and data\n\
         inconsistency without any error or warning from PostgreSQL.\n\
         \n\
         Example (bad):\n\
           -- Table has FOREIGN KEY (customer_id) REFERENCES customers(id)\n\
           ALTER TABLE orders DROP COLUMN customer_id;\n\
           -- The foreign key constraint is silently removed.\n\
         \n\
         Fix:\n\
         Verify that the referential integrity guarantee provided by the\n\
         foreign key is no longer needed before dropping the column.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::AnyPreExisting,
        |at, action, stmt, ctx| {
            let AlterTableAction::DropColumn { name } = action else {
                return vec![];
            };

            let table_key = at.name.catalog_key();
            let Some(table) = ctx.catalog_before.get_table(table_key) else {
                return vec![];
            };

            let mut findings = Vec::new();

            // Check ForeignKey constraints that include this column.
            for constraint in table.constraints_involving_column(name) {
                if let ConstraintState::ForeignKey {
                    name: constraint_name,
                    columns,
                    ref_table_display,
                    ..
                } = constraint
                {
                    let fk_description = match constraint_name {
                        Some(n) => {
                            format!("'{n}' referencing '{ref_tbl}'", ref_tbl = ref_table_display,)
                        }
                        None => format!(
                            "({cols}) \u{2192} {ref_tbl}",
                            cols = columns.join(", "),
                            ref_tbl = ref_table_display,
                        ),
                    };
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes foreign key {constraint}. Verify that the \
                             referential integrity guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                            constraint = fk_description,
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }

            findings
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, UnsafeDdlRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_drop_fk_column_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_orders_customer", &["customer_id"], "customers", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "customer_id".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fires_even_when_table_in_created_set() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_orders_customer", &["customer_id"], "customers", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/025.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string()); // table was created in an earlier changed file
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "customer_id".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].rule_id,
            RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012)
        );
    }

    #[test]
    fn test_drop_column_from_multi_column_fk_fires() {
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("id", "integer", false)
                    .column("order_id", "integer", false)
                    .column("product_id", "integer", false)
                    .pk(&["id"])
                    .fk(
                        "fk_order_product",
                        &["order_id", "product_id"],
                        "order_products",
                        &["order_id", "product_id"],
                    );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("order_items"),
            actions: vec![AlterTableAction::DropColumn {
                name: "order_id".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_non_fk_column_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .column("notes", "text", true)
                    .pk(&["id"])
                    .fk("fk_orders_customer", &["customer_id"], "customers", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "notes".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_before_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("nonexistent"),
            actions: vec![AlterTableAction::DropColumn {
                name: "col".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_unnamed_fk_shows_column_arrow_description() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"]);
            })
            .build();

        // We need to add the unnamed FK manually since the builder always sets a name.
        let mut before = before;
        let table = before.get_table_mut("orders").unwrap();
        table.constraints.push(ConstraintState::ForeignKey {
            name: None,
            columns: vec!["customer_id".to_string()],
            ref_table: "customers".to_string(),
            ref_table_display: "customers".to_string(),
            ref_columns: vec!["id".to_string()],
            not_valid: false,
        });

        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "customer_id".to_string(),
            }],
        }))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm012).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
