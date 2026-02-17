//! PGM001 — Missing `CONCURRENTLY` on `CREATE INDEX`
//!
//! Detects `CREATE INDEX` statements on existing tables that do not use
//! the `CONCURRENTLY` option. Without `CONCURRENTLY`, PostgreSQL acquires
//! an `ACCESS EXCLUSIVE` lock on the table for the duration of the index
//! build, blocking all reads and writes.

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Missing CONCURRENTLY on CREATE INDEX";

pub(super) const EXPLAIN: &str = "PGM001 — Missing CONCURRENTLY on CREATE INDEX\n\
         \n\
         What it detects:\n\
         A CREATE INDEX statement that does not use the CONCURRENTLY option,\n\
         targeting a table that already exists in the database (i.e., the table\n\
         was not created in the same set of changed files).\n\
         \n\
         Why it's dangerous:\n\
         Without CONCURRENTLY, PostgreSQL acquires an ACCESS EXCLUSIVE lock on\n\
         the table for the entire duration of the index build. This blocks ALL\n\
         queries — reads and writes — on the table. For large tables, index\n\
         creation can take minutes or hours, causing extended downtime.\n\
         \n\
         Example (bad):\n\
           CREATE INDEX idx_orders_status ON orders (status);\n\
         \n\
         Fix:\n\
           CREATE INDEX CONCURRENTLY idx_orders_status ON orders (status);\n\
         \n\
         Note: CONCURRENTLY cannot run inside a transaction. If your migration\n\
         framework wraps each file in a transaction (e.g., Liquibase default),\n\
         you must also disable that. See PGM006.\n\
         \n\
         This rule does NOT fire when the table is created in the same set of\n\
         changed files, because locking an empty/new table is harmless.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        if let IrNode::CreateIndex(ref ci) = stmt.node {
            if ci.concurrent {
                continue;
            }

            let table_key = ci.table_name.catalog_key();

            // Only flag if table exists in catalog_before (pre-existing)
            // AND was not created in the current set of changed files.
            if ctx.is_existing_table(table_key) {
                findings.push(rule.make_finding(
                    format!(
                        "CREATE INDEX on existing table '{}' should use CONCURRENTLY \
                             to avoid holding an exclusive lock.",
                        ci.table_name.display_name()
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            }
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
    use crate::rules::test_helpers::*;
    use crate::rules::{MigrationRule, RuleId};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_existing_table_no_concurrent_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_columns(vec![IndexColumn {
                name: "status".to_string(),
            }]),
        ))];

        let findings = RuleId::Migration(MigrationRule::Pgm001).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_existing_table_with_concurrent_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_columns(vec![IndexColumn {
                name: "status".to_string(),
            }])
            .with_concurrent(true),
        ))];

        let findings = RuleId::Migration(MigrationRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_new_table_in_change_no_finding() {
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

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_status".to_string()),
                QualifiedName::unqualified("orders"),
            )
            .with_columns(vec![IndexColumn {
                name: "status".to_string(),
            }]),
        ))];

        let findings = RuleId::Migration(MigrationRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
