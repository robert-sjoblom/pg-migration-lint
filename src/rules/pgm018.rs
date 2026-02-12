//! PGM018 — ADD CHECK on existing table without NOT VALID
//!
//! Detects `ALTER TABLE ... ADD CONSTRAINT ... CHECK ... ` without `NOT VALID`
//! on tables that already exist. Adding a CHECK constraint without NOT VALID
//! requires scanning the entire table while holding an ACCESS EXCLUSIVE lock,
//! which blocks all concurrent reads and writes.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags adding a CHECK constraint without NOT VALID on an existing table.
pub struct Pgm018;

impl Rule for Pgm018 {
    fn id(&self) -> &'static str {
        "PGM018"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "ADD CHECK on existing table without NOT VALID"
    }

    fn explain(&self) -> &'static str {
        "PGM018 — ADD CHECK on existing table without NOT VALID\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD CONSTRAINT ... CHECK (...) on a table that already\n\
         exists, without the NOT VALID modifier.\n\
         \n\
         Why it's dangerous:\n\
         Adding a CHECK constraint without NOT VALID acquires an ACCESS EXCLUSIVE\n\
         lock and scans the entire table to verify all existing rows satisfy the\n\
         constraint. On large tables this can cause significant downtime.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD CONSTRAINT orders_status_check\n\
             CHECK (status IN ('pending', 'shipped', 'delivered'));\n\
         \n\
         Fix (safe two-step pattern):\n\
           -- Step 1: Add with NOT VALID (instant, no scan)\n\
           ALTER TABLE orders ADD CONSTRAINT orders_status_check\n\
             CHECK (status IN ('pending', 'shipped', 'delivered')) NOT VALID;\n\
           -- Step 2: Validate (SHARE UPDATE EXCLUSIVE lock, concurrent reads OK)\n\
           ALTER TABLE orders VALIDATE CONSTRAINT orders_status_check;"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::AlterTable(ref at) = stmt.node {
                let table_key = at.name.catalog_key();

                // Only flag if the table exists in catalog_before and is not newly created.
                if !ctx.is_existing_table(table_key) {
                    continue;
                }

                for action in &at.actions {
                    if let AlterTableAction::AddConstraint(TableConstraint::Check {
                        not_valid: false,
                        ..
                    }) = action
                    {
                        findings.push(Finding::new(
                            self.id(),
                            self.default_severity(),
                            format!(
                                "Adding CHECK constraint on existing table '{table}' without \
                                 NOT VALID will scan the entire table while holding an ACCESS \
                                 EXCLUSIVE lock. Use ADD CONSTRAINT ... NOT VALID, then \
                                 VALIDATE CONSTRAINT in a separate statement.",
                                table = at.name.display_name(),
                            ),
                            ctx.file,
                            &stmt.span,
                        ));
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
    fn test_check_without_not_valid_on_existing_table_fires() {
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
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = Pgm018.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM018");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("NOT VALID"));
        assert!(findings[0].message.contains("ACCESS EXCLUSIVE"));
    }

    #[test]
    fn test_check_with_not_valid_no_finding() {
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
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: true,
            })],
        }))];

        let findings = Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_check_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(TableConstraint::Check {
                name: Some("orders_status_check".to_string()),
                expression: "status IN ('pending', 'shipped')".to_string(),
                not_valid: false,
            })],
        }))];

        let findings = Pgm018.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
