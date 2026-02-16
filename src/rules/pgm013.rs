//! PGM013 — `DROP COLUMN` silently removes unique constraint
//!
//! Detects `ALTER TABLE ... DROP COLUMN col` where `col` participates in a
//! `UNIQUE` constraint or unique index on the table in `catalog_before`.
//! PostgreSQL automatically drops any index or constraint that depends on the
//! column, silently removing uniqueness guarantees.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes unique constraint";

pub(super) const EXPLAIN: &str = "PGM013 — DROP COLUMN silently removes unique constraint\n\
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
            // Skip PK indexes (named *_pkey) since PGM014 handles those.
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
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{MigrationRule, RuleId};
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_pk_column_does_not_fire_pgm013() {
        // PK columns are handled by PGM014, not PGM013.
        // PGM013 checks UNIQUE constraints and unique indexes, not PKs.
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].rule_id,
            RuleId::Migration(MigrationRule::Pgm013)
        );
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

        let findings = RuleId::Migration(MigrationRule::Pgm013).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }
}
