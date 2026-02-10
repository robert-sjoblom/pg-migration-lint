//! PGM002 — Missing `CONCURRENTLY` on `DROP INDEX`
//!
//! Detects `DROP INDEX` statements that do not use the `CONCURRENTLY` option.
//! Without `CONCURRENTLY`, PostgreSQL acquires an `ACCESS EXCLUSIVE` lock on
//! the table the index belongs to, blocking all reads and writes.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags `DROP INDEX` without `CONCURRENTLY` on existing tables.
pub struct Pgm002;

impl Rule for Pgm002 {
    fn id(&self) -> &'static str {
        "PGM002"
    }

    fn default_severity(&self) -> Severity {
        Severity::Critical
    }

    fn description(&self) -> &'static str {
        "Missing CONCURRENTLY on DROP INDEX"
    }

    fn explain(&self) -> &'static str {
        "PGM002 — Missing CONCURRENTLY on DROP INDEX\n\
         \n\
         What it detects:\n\
         A DROP INDEX statement that does not use the CONCURRENTLY option,\n\
         where the index belongs to a table that already exists in the database.\n\
         \n\
         Why it's dangerous:\n\
         Without CONCURRENTLY, PostgreSQL acquires an ACCESS EXCLUSIVE lock on\n\
         the table associated with the index for the duration of the drop\n\
         operation. This blocks ALL queries — reads and writes — on the table.\n\
         While DROP INDEX is usually fast, it still briefly blocks concurrent\n\
         access and can queue behind long-running queries, amplifying the impact.\n\
         \n\
         Example (bad):\n\
           DROP INDEX idx_orders_status;\n\
         \n\
         Fix:\n\
           DROP INDEX CONCURRENTLY idx_orders_status;\n\
         \n\
         Note: CONCURRENTLY cannot run inside a transaction. If your migration\n\
         framework wraps each file in a transaction, you must disable that.\n\
         See PGM006."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
        let mut findings = Vec::new();

        for stmt in statements {
            if let IrNode::DropIndex(ref di) = stmt.node {
                if di.concurrent {
                    continue;
                }

                // Find which table this index belongs to by searching catalog_before.
                let belongs_to_existing_table = ctx.catalog_before.tables().any(|table| {
                    // Skip tables created in the current change set.
                    if ctx.tables_created_in_change.contains(&table.name) {
                        return false;
                    }
                    table.indexes.iter().any(|idx| idx.name == di.index_name)
                });

                if belongs_to_existing_table {
                    findings.push(Finding {
                        rule_id: self.id().to_string(),
                        severity: self.default_severity(),
                        message: "DROP INDEX on existing table should use CONCURRENTLY \
                             to avoid holding an exclusive lock."
                            .to_string(),
                        file: ctx.file.clone(),
                        start_line: stmt.span.start_line,
                        end_line: stmt.span.end_line,
                    });
                }
            }
        }

        findings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_drop_index_no_concurrent_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]).index(
                    "idx_orders_status",
                    &["status"],
                    false,
                );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropIndex(DropIndex {
            index_name: "idx_orders_status".to_string(),
            concurrent: false,
        }))];

        let findings = Pgm002.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM002");
        assert_eq!(findings[0].severity, Severity::Critical);
    }

    #[test]
    fn test_drop_index_with_concurrent_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]).index(
                    "idx_orders_status",
                    &["status"],
                    false,
                );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::DropIndex(DropIndex {
            index_name: "idx_orders_status".to_string(),
            concurrent: true,
        }))];

        let findings = Pgm002.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
