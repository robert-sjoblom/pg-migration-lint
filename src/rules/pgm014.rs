//! PGM014 — `DROP COLUMN` silently removes primary key
//!
//! Detects `ALTER TABLE ... DROP COLUMN col` where `col` participates in the
//! table's primary key (in `catalog_before`). Dropping a PK column (with
//! `CASCADE`) silently removes the primary key constraint. The table loses its
//! row identity, which affects replication, ORMs, query planning, and data
//! integrity.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags dropping a column that participates in the table's primary key.
pub struct Pgm014;

impl Rule for Pgm014 {
    fn id(&self) -> &'static str {
        "PGM014"
    }

    fn default_severity(&self) -> Severity {
        Severity::Major
    }

    fn description(&self) -> &'static str {
        "DROP COLUMN silently removes primary key"
    }

    fn explain(&self) -> &'static str {
        "PGM014 — DROP COLUMN silently removes primary key\n\
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
         necessary."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::AlterTable(ref at) = stmt.node {
                let table_key = at.name.catalog_key();

                // Only check if the table exists in catalog_before.
                let table = match ctx.catalog_before.get_table(table_key) {
                    Some(t) => t,
                    None => continue,
                };

                for action in &at.actions {
                    if let AlterTableAction::DropColumn { name } = action {
                        // Check PrimaryKey constraints that include this column.
                        for constraint in &table.constraints {
                            if let ConstraintState::PrimaryKey { columns } = constraint
                                && columns.iter().any(|c| c == name)
                            {
                                findings.push(Finding::new(
                                    self.id(),
                                    self.default_severity(),
                                    format!(
                                        "Dropping column '{col}' from table '{table}' \
                                         silently removes the primary key. The table will \
                                         have no row identity. Add a new primary key or \
                                         reconsider the column drop.",
                                        col = name,
                                        table = at.name,
                                    ),
                                    ctx.file,
                                    &stmt.span,
                                ));
                            }
                        }
                    }
                }
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
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

        let findings = Pgm014.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM014");
        assert_eq!(findings[0].severity, Severity::Major);
        assert!(findings[0].message.contains("id"));
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("primary key"));
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

        let findings = Pgm014.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM014");
        assert!(findings[0].message.contains("'a'"));
        assert!(findings[0].message.contains("order_items"));
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

        let findings = Pgm014.check(&stmts, &ctx);
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

        let findings = Pgm014.check(&stmts, &ctx);
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

        let findings = Pgm014.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
