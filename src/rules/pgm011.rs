//! PGM011 — `DROP COLUMN` on existing table
//!
//! Detects `ALTER TABLE ... DROP COLUMN` on tables that already exist.
//! While the DDL itself is cheap (PostgreSQL marks the column as dropped
//! without rewriting the table), the risk is application-level: queries
//! referencing the column will break.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags dropping a column from an existing table.
pub struct Pgm011;

impl Rule for Pgm011 {
    fn id(&self) -> &'static str {
        "PGM011"
    }

    fn default_severity(&self) -> Severity {
        Severity::Info
    }

    fn description(&self) -> &'static str {
        "DROP COLUMN on existing table"
    }

    fn explain(&self) -> &'static str {
        "PGM011 — DROP COLUMN on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... DROP COLUMN on a table that already exists in the\n\
         database (not created in the same set of changed files).\n\
         \n\
         Why it matters:\n\
         PostgreSQL marks the column as dropped without rewriting the table,\n\
         so the DDL operation itself is cheap and fast. However, the risk is\n\
         application-level: any queries, views, functions, or ORM mappings\n\
         that reference the dropped column will fail at runtime.\n\
         \n\
         Example:\n\
           ALTER TABLE orders DROP COLUMN legacy_status;\n\
         \n\
         Recommended approach:\n\
         1. First remove all application references to the column.\n\
         2. Deploy the application change.\n\
         3. Then drop the column in a subsequent migration.\n\
         \n\
         This rule is informational (INFO severity) to increase visibility\n\
         of column drops in code review."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::AlterTable(ref at) = stmt.node {
                let table_key = at.name.catalog_key();

                // Only flag if table exists in catalog_before and is not newly created.
                if !ctx.is_existing_table(table_key) {
                    continue;
                }

                for action in &at.actions {
                    if let AlterTableAction::DropColumn { name } = action {
                        findings.push(Finding::new(
                            self.id(),
                            self.default_severity(),
                            format!(
                                "Dropping column '{col}' from existing table '{table}'. \
                                 The DDL is cheap but ensure no application code references \
                                 this column.",
                                col = name,
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
    fn test_drop_column_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("legacy_status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "legacy_status".to_string(),
            }],
        }))];

        let findings = Pgm011.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM011");
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("legacy_status"));
        assert!(findings[0].message.contains("orders"));
    }

    #[test]
    fn test_drop_column_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::DropColumn {
                name: "tmp_col".to_string(),
            }],
        }))];

        let findings = Pgm011.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
