//! PGM505 — RENAME COLUMN on existing table
//!
//! Detects `ALTER TABLE ... RENAME COLUMN ... TO ...` on tables that already
//! exist. Renaming a column breaks any queries, views, or application code
//! that references the old column name.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "RENAME COLUMN on existing table";

pub(super) const EXPLAIN: &str = "PGM505 — RENAME COLUMN on existing table\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... RENAME COLUMN old_name TO new_name on a table that\n\
         already exists in the database (not created in the same set of changed\n\
         files).\n\
         \n\
         Why it matters:\n\
         Renaming a column is a backwards-incompatible schema change. Any\n\
         queries, views, stored procedures, or application code that reference\n\
         the old column name will break immediately after the migration runs.\n\
         Unlike adding or dropping a column, a rename silently invalidates\n\
         existing references without any compile-time or startup-time error.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders RENAME COLUMN status TO order_status;\n\
           -- All queries using 'status' will fail with 'column does not exist'\n\
         \n\
         Fix:\n\
         Consider a multi-step approach:\n\
         1. Add the new column with the desired name.\n\
         2. Backfill data from the old column to the new column.\n\
         3. Update application code to use the new column name.\n\
         4. Drop the old column once all references have been migrated.\n\
         \n\
         This rule does NOT fire when the table is created in the same set of\n\
         changed files, because renaming a column on a new table has no\n\
         external consumers.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::RenameColumn {
            ref table,
            ref old_name,
            ref new_name,
        } = stmt.node
        {
            let table_key = table.catalog_key();

            // Only flag if table exists in catalog_before and is not newly created.
            if !ctx.is_existing_table(table_key) {
                continue;
            }

            findings.push(rule.make_finding(
                format!(
                    "Renaming column '{old_name}' to '{new_name}' on existing table \
                         '{table}' will break queries referencing the old column name.",
                    old_name = old_name,
                    new_name = new_name,
                    table = table.display_name(),
                ),
                ctx.file,
                &stmt.span,
            ));
        }
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, SchemaDesignRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_rename_column_on_existing_table_fires() {
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

        let stmts = vec![located(IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        })];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm505).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_rename_column_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("order_status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::RenameColumn {
            table: QualifiedName::unqualified("orders"),
            old_name: "status".to_string(),
            new_name: "order_status".to_string(),
        })];

        let findings = RuleId::SchemaDesign(SchemaDesignRule::Pgm505).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
