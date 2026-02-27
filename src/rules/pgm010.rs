//! PGM010 — `DROP COLUMN` silently removes unique constraint
//!
//! Detects `ALTER TABLE ... DROP COLUMN col` where `col` participates in a
//! `UNIQUE` constraint or unique index on the table in `catalog_before`.
//! PostgreSQL automatically drops any index or constraint that depends on the
//! column, silently removing uniqueness guarantees.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, drop_column_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes unique constraint";

pub(super) const EXPLAIN: &str = "PGM010 — DROP COLUMN silently removes unique constraint\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... DROP COLUMN where the dropped column participates\n\
         in a UNIQUE constraint or unique index on the table.\n\
         \n\
         Why it matters:\n\
         PostgreSQL automatically drops any index or constraint that depends\n\
         on a dropped column. If the column was part of a UNIQUE constraint\n\
         or unique index, the uniqueness guarantee is silently lost. This can\n\
         lead to duplicate data being inserted without any error.\n\
         \n\
         Example (bad):\n\
           -- Table has UNIQUE(email)\n\
           ALTER TABLE users DROP COLUMN email;\n\
           -- The unique constraint on email is silently removed.\n\
         \n\
         Fix:\n\
         Verify that the uniqueness guarantee provided by the constraint or\n\
         index is no longer needed before dropping the column. If uniqueness\n\
         is still required on the remaining columns, create a new constraint\n\
         or index covering those columns.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    drop_column_check::check_drop_column_constraints(
        statements,
        ctx,
        |name, at, table, stmt, ctx| {
            let mut findings = Vec::new();

            // Check UNIQUE constraints that include this column.
            for constraint in table.constraints_involving_column(name) {
                if let ConstraintState::Unique {
                    name: constraint_name,
                    columns,
                } = constraint
                {
                    let constraint_description = match constraint_name {
                        Some(n) => format!("'{n}'"),
                        None => format!("UNIQUE({})", columns.join(", ")),
                    };
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes unique constraint {constraint}. Verify that \
                             the uniqueness guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                            constraint = constraint_description,
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                }
            }

            // Check unique indexes that include this column.
            // Skip PK indexes (named *_pkey) since PGM011 handles those.
            for idx in table.indexes_involving_column(name) {
                if idx.unique && !idx.name.ends_with("_pkey") {
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' silently \
                             removes unique constraint '{constraint}'. Verify that \
                             the uniqueness guarantee is no longer needed.",
                            col = name,
                            table = at.name.display_name(),
                            constraint = idx.name,
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_drop_column_with_unique_constraint_fires() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_column_with_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("products", |t| {
                t.column("id", "integer", false)
                    .column("code", "text", false)
                    .pk(&["id"])
                    .index("idx_products_code_unique", &["code"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("products"),
            actions: vec![AlterTableAction::DropColumn {
                name: "code".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_column_not_in_unique_no_finding() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .column("name", "text", true)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "name".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_column_nonexistent_table_no_finding() {
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

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_pk_column_does_not_fire_pgm010() {
        // PK columns are handled by PGM011, not PGM010.
        // PGM010 checks UNIQUE constraints and unique indexes, not PKs.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        // PK constraints are ConstraintState::PrimaryKey, not Unique, so PGM013 ignores them.
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fires_even_when_table_in_created_set() {
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .unique("uq_users_email", &["email"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let mut created = HashSet::new();
        created.insert("users".to_string()); // table was created in an earlier changed file
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, RuleId::Pgm010);
    }

    #[test]
    fn test_multi_column_unique_drop_one_fires() {
        let before = CatalogBuilder::new()
            .table("subscriptions", |t| {
                t.column("id", "integer", false)
                    .column("a", "text", false)
                    .column("b", "text", false)
                    .pk(&["id"])
                    .unique("uq_subscriptions_a_b", &["a", "b"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("subscriptions"),
            actions: vec![AlterTableAction::DropColumn {
                name: "a".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_unique_column_created_via_using_index_fires() {
        // UNIQUE constraint was created via ADD UNIQUE USING INDEX — replay resolves
        // the index columns into the constraint so DROP COLUMN detects it.
        use crate::catalog::replay::apply;
        use crate::input::MigrationUnit;

        // Step 1: build a table with an index (no unique constraint yet).
        let mut catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .index("idx_orders_email", &["email"], true);
            })
            .build();

        // Step 2: replay ADD UNIQUE USING INDEX to get resolved columns.
        let unit = MigrationUnit {
            id: "add_unique".to_string(),
            statements: vec![Located {
                node: IrNode::AlterTable(AlterTable {
                    name: QualifiedName::unqualified("orders"),
                    actions: vec![AlterTableAction::AddConstraint(TableConstraint::Unique {
                        name: Some("uq_orders_email".to_string()),
                        columns: vec![], // empty with USING INDEX
                        using_index: Some("idx_orders_email".to_string()),
                    })],
                }),
                span: SourceSpan {
                    start_line: 1,
                    end_line: 1,
                    start_offset: 0,
                    end_offset: 0,
                },
            }],
            source_file: PathBuf::from("migrations/001.sql"),
            source_line_offset: 1,
            run_in_transaction: true,
            is_down: false,
        };
        apply(&mut catalog, &unit);

        let before = catalog;
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "email".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        // Fires twice: once for the UNIQUE constraint (resolved columns), once for
        // the backing unique index — both involve the dropped column.
        assert_eq!(
            findings.len(),
            2,
            "Should detect unique constraint removal even when created via USING INDEX"
        );
    }

    #[test]
    fn test_drop_multi_column_unique_partial_drop_fires() {
        // Multi-column unique constraint, drop only one column still fires
        let before = CatalogBuilder::new()
            .table("products", |t| {
                t.column("id", "integer", false)
                    .column("category", "text", false)
                    .column("code", "text", false)
                    .pk(&["id"])
                    .unique("uq_products_cat_code", &["category", "code"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("products"),
            actions: vec![AlterTableAction::DropColumn {
                name: "code".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_drop_pkey_index_column_not_pgm010() {
        // Primary key indexes (named *_pkey) should NOT fire PGM010
        // They should be handled by PGM011 or not at all
        let before = CatalogBuilder::new()
            .table("users", |t| {
                t.column("id", "integer", false)
                    .column("email", "text", false)
                    .pk(&["id"])
                    .index("users_pkey", &["id"], true); // PK index with _pkey suffix
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("users"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm010.check(&stmts, &ctx);
        // PGM010 should NOT fire for _pkey indexes (those are PK, not UNIQUE constraints)
        // This test ensures the `!idx.name.ends_with("_pkey")` filter works
        assert!(
            findings.is_empty(),
            "PK index ending in _pkey should not trigger PGM010"
        );
    }
}
