//! PGM019 — RENAME TABLE on existing table
//!
//! Renaming a table breaks all queries, views, and functions that reference
//! the old name. This is a high-risk operation in production.
//!
//! **Replacement detection**: if the same migration file creates a new table
//! with the old name (rename away + create replacement pattern), the finding
//! is suppressed.

use std::collections::HashSet;

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags `ALTER TABLE ... RENAME TO` on existing tables.
pub struct Pgm019;

impl Rule for Pgm019 {
    fn id(&self) -> &'static str {
        "PGM019"
    }

    fn default_severity(&self) -> Severity {
        Severity::Info
    }

    fn description(&self) -> &'static str {
        "RENAME TABLE on existing table"
    }

    fn explain(&self) -> &'static str {
        "PGM019 — RENAME TABLE on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... RENAME TO ... on a table that already exists in the\n\
         database (i.e., the table was not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         Renaming a table breaks all queries, views, and functions that reference\n\
         the old name. While the rename itself is instant DDL (metadata-only),\n\
         the downstream breakage can be severe.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders RENAME TO orders_archive;\n\
           -- All queries referencing 'orders' will now fail.\n\
         \n\
         Example (safe — replacement pattern):\n\
           ALTER TABLE orders RENAME TO orders_old;\n\
           CREATE TABLE orders (...);\n\
           -- The old name is re-created, so existing queries still work.\n\
         \n\
         Fix:\n\
         Use a view to maintain backward compatibility during the transition:\n\
           ALTER TABLE orders RENAME TO orders_v2;\n\
           CREATE VIEW orders AS SELECT * FROM orders_v2;\n\
         \n\
         This rule does NOT fire when a replacement table with the old name\n\
         is created in the same migration unit."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        // Pass 1: collect all CREATE TABLE catalog keys in this unit.
        // These represent "replacement" tables that re-create a renamed-away name.
        let created_in_unit: HashSet<&str> = statements
            .iter()
            .filter_map(|stmt| match &stmt.node {
                IrNode::CreateTable(ct) => Some(ct.name.catalog_key()),
                _ => None,
            })
            .collect();

        // Pass 2: find RenameTable on existing tables.
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::RenameTable {
                ref name,
                ref new_name,
            } = stmt.node
            {
                let table_key = name.catalog_key();

                // Only flag if the table pre-exists (not created in the current changeset).
                if !ctx.is_existing_table(table_key) {
                    continue;
                }

                // Replacement detection: if a CREATE TABLE in this unit re-creates
                // the old name, the rename is part of a safe swap pattern.
                if created_in_unit.contains(table_key) {
                    continue;
                }

                findings.push(Finding::new(
                    self.id(),
                    self.default_severity(),
                    format!(
                        "Renaming existing table '{}' to '{}' will break all \
                         queries, views, and functions referencing the old name.",
                        name.display_name(),
                        new_name,
                    ),
                    ctx.file,
                    &stmt.span,
                ));
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
    fn test_rename_existing_table_fires() {
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

        let stmts = vec![located(IrNode::RenameTable {
            name: QualifiedName::unqualified("orders"),
            new_name: "orders_archive".to_string(),
        })];

        let findings = Pgm019.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM019");
        assert_eq!(findings[0].severity, Severity::Info);
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("orders_archive"));
        assert!(
            findings[0]
                .message
                .contains("queries, views, and functions")
        );
    }

    #[test]
    fn test_rename_new_table_no_finding() {
        // Table does not exist in catalog_before, so the rename is harmless.
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::RenameTable {
            name: QualifiedName::unqualified("temp_table"),
            new_name: "real_table".to_string(),
        })];

        let findings = Pgm019.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_rename_with_replacement_table_no_finding() {
        // Pattern: rename orders -> orders_old, then CREATE TABLE orders.
        // The old name is re-created, so existing queries still work.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![
            located(IrNode::RenameTable {
                name: QualifiedName::unqualified("orders"),
                new_name: "orders_old".to_string(),
            }),
            located(IrNode::CreateTable(CreateTable {
                name: QualifiedName::unqualified("orders"),
                columns: vec![ColumnDef {
                    name: "id".to_string(),
                    type_name: TypeName::simple("integer"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: true,
                    is_serial: false,
                }],
                constraints: vec![],
                temporary: false,
            })),
        ];

        let findings = Pgm019.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Expected no findings for replacement pattern, got: {:?}",
            findings.iter().map(|f| &f.message).collect::<Vec<_>>(),
        );
    }
}
