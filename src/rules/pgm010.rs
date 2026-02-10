//! PGM010 — `ADD COLUMN NOT NULL` without default on existing table
//!
//! Detects `ALTER TABLE ... ADD COLUMN ... NOT NULL` without a `DEFAULT` clause
//! on tables that already exist. This command will fail outright if the table
//! has any rows.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags adding a NOT NULL column without a DEFAULT to an existing table.
pub struct Pgm010;

impl Rule for Pgm010 {
    fn id(&self) -> &'static str {
        "PGM010"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "ADD COLUMN NOT NULL without DEFAULT on existing table"
    }

    fn explain(&self) -> &'static str {
        "PGM010 — ADD COLUMN NOT NULL without DEFAULT on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD COLUMN ... NOT NULL without a DEFAULT clause,\n\
         where the table already exists in the database (not created in the\n\
         same set of changed files).\n\
         \n\
         Why it's dangerous:\n\
         Adding a NOT NULL column without a default to a table that has\n\
         existing rows will fail immediately with:\n\
           ERROR: column \"x\" of relation \"t\" contains null values\n\
         This is almost always a bug. The migration will fail at deploy time.\n\
         \n\
         On PG 11+, ADD COLUMN ... NOT NULL DEFAULT <value> is safe — the\n\
         default is applied lazily without rewriting the table (for non-volatile\n\
         defaults).\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD COLUMN status text NOT NULL;\n\
         \n\
         Fix (option A — add with default):\n\
           ALTER TABLE orders ADD COLUMN status text NOT NULL DEFAULT 'pending';\n\
         \n\
         Fix (option B — add nullable, backfill, then constrain):\n\
           ALTER TABLE orders ADD COLUMN status text;\n\
           UPDATE orders SET status = 'pending' WHERE status IS NULL;\n\
           ALTER TABLE orders ALTER COLUMN status SET NOT NULL;"
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::AlterTable(ref at) = stmt.node {
                let table_key = at.name.catalog_key();

                // Only flag if table exists in catalog_before and is not newly created.
                if !ctx.catalog_before.has_table(table_key)
                    || ctx.tables_created_in_change.contains(table_key)
                {
                    continue;
                }

                for action in &at.actions {
                    if let AlterTableAction::AddColumn(col) = action
                        && !col.nullable
                        && col.default_expr.is_none()
                    {
                        findings.push(Finding {
                                rule_id: self.id().to_string(),
                                severity: self.default_severity(),
                                message: format!(
                                    "Adding NOT NULL column '{col}' to existing table '{table}' \
                                     without a DEFAULT will fail if the table has any rows. \
                                     Add a DEFAULT value, or add the column as nullable and backfill.",
                                    col = col.name,
                                    table = at.name,
                                ),
                                file: ctx.file.clone(),
                                start_line: stmt.span.start_line,
                                end_line: stmt.span.end_line,
                            });
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
    fn test_not_null_no_default_fires() {
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "status".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm010.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM010");
        assert_eq!(findings[0].severity, Severity::Critical);
        assert!(findings[0].message.contains("status"));
        assert!(findings[0].message.contains("orders"));
    }

    #[test]
    fn test_not_null_with_default_no_finding() {
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "status".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: Some(DefaultExpr::Literal("pending".to_string())),
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_nullable_no_default_no_finding() {
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "notes".to_string(),
                type_name: TypeName::simple("text"),
                nullable: true,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_table_not_null_no_default_no_finding() {
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
            actions: vec![AlterTableAction::AddColumn(ColumnDef {
                name: "status".to_string(),
                type_name: TypeName::simple("text"),
                nullable: false,
                default_expr: None,
                is_inline_pk: false,
                is_serial: false,
            })],
        }))];

        let findings = Pgm010.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
