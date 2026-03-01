//! PGM016 — `ADD PRIMARY KEY` on existing table without `USING INDEX`
//!
//! Detects `ALTER TABLE ... ADD PRIMARY KEY` on existing tables that doesn't
//! use `USING INDEX` to reference a pre-built unique index. Even if a matching
//! unique index already exists, PostgreSQL will build a **new** index under
//! ACCESS EXCLUSIVE lock unless `USING INDEX` is explicit.
//!
//! Additionally, even with `USING INDEX`, if any PK columns are nullable,
//! PostgreSQL implicitly runs `SET NOT NULL` under ACCESS EXCLUSIVE lock.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "ADD PRIMARY KEY on existing table without USING INDEX";

pub(super) const EXPLAIN: &str = "PGM016 — ADD PRIMARY KEY on existing table without USING INDEX\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD PRIMARY KEY on an existing table that does not\n\
         use USING INDEX, or where the referenced index does not exist, is\n\
         not UNIQUE, or covers nullable columns.\n\
         \n\
         Why it's dangerous:\n\
         Without USING INDEX, PostgreSQL always builds a new unique index\n\
         inline under an ACCESS EXCLUSIVE lock, even if a matching unique\n\
         index already exists. For large tables this causes extended downtime.\n\
         \n\
         Even with USING INDEX, if any of the PK columns are nullable,\n\
         PostgreSQL implicitly runs ALTER COLUMN SET NOT NULL which requires\n\
         a full table scan under ACCESS EXCLUSIVE lock.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD PRIMARY KEY (id);\n\
         \n\
         Fix (safe pattern — build unique index concurrently first):\n\
           -- Ensure columns are NOT NULL (use CHECK constraint trick if needed)\n\
           CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);\n\
           ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::ExcludeCreatedInChange,
        |at, action, stmt, ctx| {
            let AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                columns,
                using_index,
            }) = action
            else {
                return vec![];
            };

            let table_key = at.name.catalog_key();
            // Table must exist in catalog_before (i.e. pre-existing).
            if ctx.catalog_before.get_table(table_key).is_none() {
                return vec![];
            }

            let message = match using_index {
                Some(idx_name) => {
                    match ctx.get_index(idx_name) {
                        None => format!(
                            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
                             referenced index does not exist.",
                            table = at.name.display_name(),
                        ),
                        Some(idx) if !idx.unique => format!(
                            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
                             referenced index is not UNIQUE.",
                            table = at.name.display_name(),
                        ),
                        Some(idx) if !idx.is_btree() => format!(
                            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
                             referenced index uses access method '{}', but only btree \
                             indexes can back a PRIMARY KEY constraint.",
                            idx.access_method,
                            table = at.name.display_name(),
                        ),
                        Some(idx) => {
                            // Check nullability using the INDEX's columns (constraint
                            // columns are empty with USING INDEX).
                            if let Some(table) = ctx.catalog_before.get_table(table_key) {
                                let nullable_cols: Vec<&str> = idx
                                    .column_names()
                                    .filter(|c| table.get_column(c).is_some_and(|col| col.nullable))
                                    .collect();
                                if !nullable_cols.is_empty() {
                                    format!(
                                        "ADD PRIMARY KEY USING INDEX '{idx_name}' on table \
                                         '{table}': column(s) [{cols}] are nullable. PostgreSQL \
                                         will implicitly SET NOT NULL under ACCESS EXCLUSIVE \
                                         lock. Run ALTER COLUMN ... SET NOT NULL with a CHECK \
                                         constraint first.",
                                        table = at.name.display_name(),
                                        cols = nullable_cols.join(", "),
                                    )
                                } else {
                                    return vec![]; // safe
                                }
                            } else {
                                return vec![]; // table not in catalog_before, skip
                            }
                        }
                    }
                }
                None => format!(
                    "ADD PRIMARY KEY on existing table '{table}' without USING INDEX \
                     on column(s) [{columns}]. Create a UNIQUE index CONCURRENTLY \
                     first, then use ADD PRIMARY KEY USING INDEX.",
                    table = at.name.display_name(),
                    columns = columns.join(", "),
                ),
            };

            vec![rule.make_finding(message, ctx.file, &stmt.span)]
        },
    )
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

    fn add_pk_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: columns.iter().map(|s| s.to_string()).collect(),
                    using_index: None,
                },
            )],
        }))
    }

    fn add_pk_using_index_stmt(table: &str, idx_name: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec![], // empty with USING INDEX
                    using_index: Some(idx_name.to_string()),
                },
            )],
        }))
    }

    #[test]
    fn test_add_pk_no_unique_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_with_unique_constraint_still_fires() {
        // Even with a pre-existing UNIQUE constraint, without USING INDEX
        // PostgreSQL builds a NEW index under ACCESS EXCLUSIVE lock.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .unique("uq_orders_id", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_pk_with_unique_index_still_fires() {
        // Even with a pre-existing unique index, without USING INDEX
        // PostgreSQL builds a NEW index.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("nonexistent", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_with_backing_unique_index_not_null_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_using_index_non_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_pk", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_no_backing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_nonexistent")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_nullable_columns_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", true) // nullable!
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_created_in_same_migration_no_finding() {
        // Index exists in catalog_after but not in catalog_before
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_using_index_multiple_columns_not_null_no_finding() {
        // Multi-column PK with all NOT NULL columns
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("order_id", "bigint", false)
                    .column("item_id", "bigint", false)
                    .index("idx_order_items_pk", &["order_id", "item_id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("order_items", "idx_order_items_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_using_index_column_made_nullable_by_drop_not_null_fires() {
        // Column was NOT NULL originally but became nullable via DROP NOT NULL
        // in an earlier migration. catalog_before reflects nullable column,
        // so ADD PK USING INDEX should warn about implicit SET NOT NULL.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", true) // nullable after DROP NOT NULL
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Should fire when column became nullable via DROP NOT NULL"
        );
        assert!(findings[0].message.contains("nullable"));
    }

    #[test]
    fn test_add_pk_using_index_non_btree_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false).index_with_method(
                    "idx_orders_pk",
                    &["id"],
                    true,
                    "hash",
                );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("hash"));
        assert!(findings[0].message.contains("btree"));
    }

    #[test]
    fn test_add_pk_using_index_one_nullable_one_not_fires() {
        // Multi-column index where one column is nullable
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("order_id", "bigint", false)
                    .column("item_id", "bigint", true) // nullable
                    .index("idx_order_items_pk", &["order_id", "item_id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_using_index_stmt("order_items", "idx_order_items_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("nullable"));
    }
}
