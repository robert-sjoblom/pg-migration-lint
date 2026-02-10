//! PGM012 — `ADD PRIMARY KEY` on existing table without prior UNIQUE constraint
//!
//! Detects `ALTER TABLE ... ADD PRIMARY KEY (col)` on tables that already exist
//! where the target columns don't already have a UNIQUE constraint or unique index.
//! The safe pattern is to first create a unique index CONCURRENTLY, then add the
//! primary key using that index.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags adding a PRIMARY KEY to an existing table without a prior
/// unique index or UNIQUE constraint on the PK columns.
pub struct Pgm012;

impl Rule for Pgm012 {
    fn id(&self) -> &'static str {
        "PGM012"
    }

    fn default_severity(&self) -> Severity {
        Severity::Major
    }

    fn description(&self) -> &'static str {
        "ADD PRIMARY KEY on existing table without prior UNIQUE constraint"
    }

    fn explain(&self) -> &'static str {
        "PGM012 — ADD PRIMARY KEY on existing table without prior UNIQUE constraint\n\
         \n\
         What it detects:\n\
         ALTER TABLE ... ADD PRIMARY KEY (columns) where the table already exists\n\
         and the target columns don't have a pre-existing UNIQUE constraint or\n\
         unique index.\n\
         \n\
         Why it's dangerous:\n\
         Adding a primary key without a pre-existing unique index causes\n\
         PostgreSQL to build a unique index inline (not concurrently). This\n\
         takes an ACCESS EXCLUSIVE lock on the table for the duration of the\n\
         index build. If duplicates exist, the command fails at deploy time.\n\
         \n\
         If the columns already have a unique index or UNIQUE constraint,\n\
         uniqueness is already enforced and the PK addition is logically safe\n\
         (though still takes a brief lock).\n\
         \n\
         Example (bad):\n\
           ALTER TABLE orders ADD PRIMARY KEY (id);\n\
         \n\
         Fix (safe pattern — build unique index concurrently first):\n\
           CREATE UNIQUE INDEX CONCURRENTLY idx_orders_pk ON orders (id);\n\
           ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_orders_pk;"
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
                    if let AlterTableAction::AddConstraint(TableConstraint::PrimaryKey { columns }) =
                        action
                        && let Some(table) = ctx.catalog_before.get_table(table_key)
                        && !table.has_unique_covering(columns)
                    {
                        findings.push(Finding {
                            rule_id: self.id().to_string(),
                            severity: self.default_severity(),
                            message: format!(
                                "ADD PRIMARY KEY on existing table '{table}' without a \
                                         prior UNIQUE constraint or unique index on column(s) \
                                         [{columns}]. Create a unique index CONCURRENTLY first, \
                                         then use ADD PRIMARY KEY USING INDEX.",
                                table = at.name,
                                columns = columns.join(", "),
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
    use std::collections::HashSet;
    use std::path::PathBuf;

    fn make_ctx<'a>(
        before: &'a Catalog,
        after: &'a Catalog,
        file: &'a PathBuf,
        created: &'a HashSet<String>,
    ) -> LintContext<'a> {
        LintContext {
            catalog_before: before,
            catalog_after: after,
            tables_created_in_change: created,
            run_in_transaction: true,
            is_down: false,
            file,
        }
    }

    fn located(node: IrNode) -> Located<IrNode> {
        Located {
            node,
            span: SourceSpan {
                start_line: 1,
                end_line: 1,
                start_offset: 0,
                end_offset: 0,
            },
        }
    }

    fn add_pk_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: columns.iter().map(|s| s.to_string()).collect(),
                },
            )],
        }))
    }

    #[test]
    fn test_add_pk_no_unique_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM012");
        assert_eq!(findings[0].severity, Severity::Major);
        assert!(findings[0].message.contains("orders"));
        assert!(findings[0].message.contains("id"));
    }

    #[test]
    fn test_add_pk_with_unique_constraint_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .unique("uq_orders_id", &["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_with_unique_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("orders".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("nonexistent", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = Pgm012.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM012");
    }
}
