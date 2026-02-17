//! PGM014 — `DROP COLUMN` silently removes primary key
//!
//! Detects `ALTER TABLE ... DROP COLUMN col` where `col` participates in the
//! table's primary key (in `catalog_before`). Dropping a PK column (with
//! `CASCADE`) silently removes the primary key constraint. The table loses its
//! row identity, which affects replication, ORMs, query planning, and data
//! integrity.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DROP COLUMN silently removes primary key";

pub(super) const EXPLAIN: &str = "PGM014 — DROP COLUMN silently removes primary key\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... DROP COLUMN where the dropped column participates\n\
         in the table's primary key constraint.\n\
         \n\
         Why it matters:\n\
         Dropping a PK column (with CASCADE) silently removes the primary key\n\
         constraint. The table loses its row identity, which affects replication,\n\
         ORMs, query planning, and data integrity.\n\
         \n\
         Example (bad):\n\
           -- Table has PRIMARY KEY (id)\n\
           ALTER TABLE orders DROP COLUMN id;\n\
           -- The primary key constraint is silently removed.\n\
         \n\
         Fix:\n\
         Add a new primary key on the remaining columns before or after\n\
         dropping the column, or reconsider whether the column drop is\n\
         necessary.";

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

            // Check PrimaryKey constraints that include this column.
            for constraint in table.constraints_involving_column(name) {
                if let ConstraintState::PrimaryKey { .. } = constraint {
                    findings.push(rule.make_finding(
                        format!(
                            "Dropping column '{col}' from table '{table}' \
                             silently removes the primary key. The table will \
                             have no row identity. Add a new primary key or \
                             reconsider the column drop.",
                            col = name,
                            table = at.name.display_name(),
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
    fn test_drop_pk_column_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
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

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fires_even_when_table_in_created_set() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
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
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].rule_id,
            RuleId::Migration(MigrationRule::Pgm014)
        );
    }

    #[test]
    fn test_drop_column_from_multi_column_pk_fires() {
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .column("value", "text", true)
                    .pk(&["a", "b"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("order_items"),
            actions: vec![AlterTableAction::DropColumn {
                name: "a".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_non_pk_column_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("name", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "name".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
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

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_without_pk_drop_column_no_finding() {
        let before = CatalogBuilder::new()
            .table("events", |t| {
                t.column("id", "integer", false)
                    .column("payload", "text", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("events"),
            actions: vec![AlterTableAction::DropColumn {
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_drop_pk_column_created_via_using_index_fires() {
        // PK was created via ADD PRIMARY KEY USING INDEX — replay resolves
        // the index columns into the constraint so DROP COLUMN detects it.
        use crate::catalog::replay::apply;
        use crate::input::MigrationUnit;

        // Step 1: build a table with an index (no PK yet).
        let mut catalog = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("status", "text", true)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();

        // Step 2: replay ADD PRIMARY KEY USING INDEX to get resolved columns.
        let unit = MigrationUnit {
            id: "add_pk".to_string(),
            statements: vec![Located {
                node: IrNode::AlterTable(AlterTable {
                    name: QualifiedName::unqualified("orders"),
                    actions: vec![AlterTableAction::AddConstraint(
                        TableConstraint::PrimaryKey {
                            columns: vec![], // empty with USING INDEX
                            using_index: Some("idx_orders_pk".to_string()),
                        },
                    )],
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
                name: "id".to_string(),
            }],
        }))];

        let findings = RuleId::Migration(MigrationRule::Pgm014).check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Should detect PK removal even when PK was created via USING INDEX"
        );
    }
}
