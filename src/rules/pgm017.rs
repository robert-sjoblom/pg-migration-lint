//! PGM017 — ADD FOREIGN KEY on existing table without NOT VALID
//!
//! Detects adding FK constraints without NOT VALID to tables that already
//! exist. The safe pattern is ADD CONSTRAINT ... NOT VALID, then VALIDATE CONSTRAINT.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity, alter_table_check};

/// Rule that flags adding a FOREIGN KEY constraint on an existing table
/// without the `NOT VALID` modifier.
pub struct Pgm017;

impl Rule for Pgm017 {
    fn id(&self) -> &'static str {
        "PGM017"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "ADD FOREIGN KEY on existing table without NOT VALID"
    }

    fn explain(&self) -> &'static str {
        "PGM017 — ADD FOREIGN KEY on existing table without NOT VALID\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY ... where the table\n\
         already exists and the constraint does not include NOT VALID.\n\
         \n\
         Why it's dangerous:\n\
         Adding a foreign key constraint without NOT VALID causes PostgreSQL\n\
         to immediately validate all existing rows. This acquires a SHARE\n\
         ROW EXCLUSIVE lock on the table and performs a full table scan, blocking\n\
         concurrent data modifications for the duration. On large tables this can\n\
         cause significant downtime.\n\
         \n\
         Safe alternative:\n\
         Add the constraint with NOT VALID first, then validate it in a\n\
         separate statement. VALIDATE CONSTRAINT only requires a SHARE\n\
         UPDATE EXCLUSIVE lock, which allows concurrent reads and writes.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders\n\
             ADD CONSTRAINT fk_customer\n\
             FOREIGN KEY (customer_id) REFERENCES customers (id);\n\
         \n\
         Fix (safe pattern):\n\
           ALTER TABLE orders\n\
             ADD CONSTRAINT fk_customer\n\
             FOREIGN KEY (customer_id) REFERENCES customers (id)\n\
             NOT VALID;\n\
           ALTER TABLE orders\n\
             VALIDATE CONSTRAINT fk_customer;"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        alter_table_check::check_alter_actions(statements, ctx, |at, action, stmt, ctx| {
            if let AlterTableAction::AddConstraint(TableConstraint::ForeignKey {
                not_valid: false,
                ..
            }) = action
            {
                vec![self.make_finding(
                    format!(
                        "Adding FOREIGN KEY constraint on existing table '{}' \
                         without NOT VALID will scan the entire table while \
                         holding a SHARE ROW EXCLUSIVE lock. Use ADD CONSTRAINT \
                         ... NOT VALID, then VALIDATE CONSTRAINT in a separate \
                         statement.",
                        at.name.display_name(),
                    ),
                    ctx.file,
                    &stmt.span,
                )]
            } else {
                vec![]
            }
        })
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

    /// Helper to build an ALTER TABLE ... ADD CONSTRAINT ... FOREIGN KEY statement.
    fn add_fk_stmt(table: &str, not_valid: bool) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_customer".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid,
                },
            )],
        }))
    }

    #[test]
    fn test_fires_on_existing_table_without_not_valid() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("customer_id", "bigint", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_fk_stmt("orders", false)];

        let findings = Pgm017.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM017");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("NOT VALID"));
    }

    #[test]
    fn test_no_finding_when_not_valid_is_true() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("customer_id", "bigint", true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_fk_stmt("orders", true)];

        let findings = Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_new_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("customer_id", "bigint", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_fk_stmt("orders", false)];

        let findings = Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_fk_in_create_table() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("customer_id", "bigint", true);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        // FK inside a CreateTable, not an AlterTable
        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("orders"),
            columns: vec![
                ColumnDef {
                    name: "id".to_string(),
                    type_name: TypeName::simple("bigint"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: true,
                    is_serial: false,
                },
                ColumnDef {
                    name: "customer_id".to_string(),
                    type_name: TypeName::simple("bigint"),
                    nullable: true,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![TableConstraint::ForeignKey {
                name: Some("fk_customer".to_string()),
                columns: vec!["customer_id".to_string()],
                ref_table: QualifiedName::unqualified("customers"),
                ref_columns: vec!["id".to_string()],
                not_valid: false,
            }],
            temporary: false,
        }))];

        let findings = Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
