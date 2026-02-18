//! PGM002 — Missing `CONCURRENTLY` on `DROP INDEX`
//!
//! Detects `DROP INDEX` statements that do not use the `CONCURRENTLY` option.
//! Without `CONCURRENTLY`, PostgreSQL acquires an `ACCESS EXCLUSIVE` lock on
//! the table the index belongs to, blocking all reads and writes.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Missing CONCURRENTLY on DROP INDEX";

pub(super) const EXPLAIN: &str = "PGM002 — Missing CONCURRENTLY on DROP INDEX\n\
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
         See PGM003.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::DropIndex(ref di) = stmt.node {
            if di.concurrent {
                continue;
            }

            // Look up which table owns this index via the catalog's reverse index.
            let Some(table_name) = ctx.catalog_before.table_for_index(&di.index_name) else {
                continue;
            };

            // Skip tables created in the current change set.
            if ctx.tables_created_in_change.contains(table_name) {
                continue;
            }

            findings.push(rule.make_finding(
                format!(
                    "DROP INDEX '{}' on existing table '{}' should use CONCURRENTLY \
                        to avoid holding an exclusive lock.",
                    di.index_name,
                    ctx.catalog_before
                        .get_table(table_name)
                        .map(|t| t.display_name.as_str())
                        .unwrap_or(table_name)
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
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use crate::rules::{RuleId, UnsafeDdlRule};
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

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm002).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
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

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status")
                .with_concurrent(true)
                .with_if_exists(false),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm002).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
