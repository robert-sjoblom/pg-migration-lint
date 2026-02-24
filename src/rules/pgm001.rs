//! PGM001 — Missing `CONCURRENTLY` on `CREATE INDEX`
//!
//! Detects `CREATE INDEX` statements on existing tables that do not use
//! the `CONCURRENTLY` option. Without `CONCURRENTLY`, PostgreSQL acquires
//! a `SHARE` lock on the table for the duration of the index build,
//! blocking all writes (inserts, updates, deletes).

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
         Without CONCURRENTLY, PostgreSQL acquires a SHARE lock on the table\n\
         for the entire duration of the index build. This blocks all writes\n\
         (inserts, updates, deletes) on the table while allowing reads.\n\
         For large tables, index creation can take minutes or hours, blocking\n\
         all write traffic for that duration.\n\
         \n\
         Example (bad):\n\
           CREATE INDEX idx_orders_status ON orders (status);\n\
         \n\
         Fix:\n\
           CREATE INDEX CONCURRENTLY idx_orders_status ON orders (status);\n\
         \n\
         Note: CONCURRENTLY cannot run inside a transaction. If your migration\n\
         framework wraps each file in a transaction (e.g., Liquibase default),\n\
         you must also disable that. See PGM003.\n\
         \n\
         This rule does NOT fire when the table is created in the same set of\n\
         changed files, because locking an empty/new table is harmless.\n\
         \n\
         Partitioned tables: CREATE INDEX on a partitioned parent propagates\n\
         the index build to every partition, locking all of them. The safe\n\
         pattern is: CREATE INDEX ON ONLY parent (creates an invalid parent-\n\
         only index with no lock on children), then CREATE INDEX CONCURRENTLY\n\
         on each partition, then ALTER INDEX parent_idx ATTACH PARTITION\n\
         child_idx for each. CREATE INDEX ON ONLY is suppressed by this rule\n\
         because it does not lock child partitions.";

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
                // ON ONLY creates an invalid parent-only index without locking children.
                // This is the safe first step of the partition index pattern.
                if ci.only {
                    continue;
                }

                let is_partitioned = ctx
                    .catalog_before
                    .get_table(table_key)
                    .map(|t| t.is_partitioned)
                    .unwrap_or(false);

                if is_partitioned {
                    findings.push(rule.make_finding(
                        format!(
                            "CREATE INDEX on partitioned table '{}' will lock all partitions. \
                             Use CREATE INDEX ON ONLY, then CREATE INDEX CONCURRENTLY on each \
                             partition, then ALTER INDEX ... ATTACH PARTITION.",
                            ci.table_name.display_name()
                        ),
                        ctx.file,
                        &stmt.span,
                    ));
                } else {
                    findings.push(rule.make_finding(
                        format!(
                            "CREATE INDEX on existing table '{}' should use CONCURRENTLY \
                                 to avoid holding a SHARE lock that blocks writes.",
                            ci.table_name.display_name()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::test_helpers::*;
    use crate::rules::{RuleId, UnsafeDdlRule};
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
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
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
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .with_concurrent(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partitioned_table_emits_partition_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
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
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_partitioned_table_only_suppresses() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
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
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .with_only(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partitioned_table_with_concurrent_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
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
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .with_concurrent(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_partitioned_only_is_suppressed() {
        // ON ONLY on a non-partitioned table is suppressed regardless —
        // it signals the partition-safe indexing pattern and is harmless.
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
            .with_columns(vec![IndexColumn::Column("status".to_string())])
            .with_only(true),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_partition_child_suggests_concurrently() {
        // A partition child is a real data-holding table — CREATE INDEX
        // without CONCURRENTLY should suggest CONCURRENTLY, same as any
        // non-partitioned table.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_2024", |t| {
                t.column("id", "integer", false)
                    .pk(&["id"])
                    .partition_of("orders");
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_2024_status".to_string()),
                QualifiedName::unqualified("orders_2024"),
            )
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partition child should suggest CONCURRENTLY"
        );
        assert!(
            findings[0].message.contains("CONCURRENTLY"),
            "Should suggest CONCURRENTLY, not the partitioned-table message"
        );
        assert!(
            !findings[0].message.contains("lock all partitions"),
            "Should NOT mention locking partitions — this is a leaf child"
        );
    }

    #[test]
    fn test_sub_partitioned_table_emits_partition_finding() {
        // A table that is both partitioned AND a partition child
        // (sub-partitioning). The `is_partitioned` check takes priority,
        // so it should emit the partition-specific finding.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("region", "text", false)
                    .pk(&["id", "region"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_2024", |t| {
                t.column("id", "integer", false)
                    .column("region", "text", false)
                    .pk(&["id", "region"])
                    .partition_of("orders")
                    .partitioned_by(crate::parser::ir::PartitionStrategy::List, &["region"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/003.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateIndex(
            CreateIndex::test(
                Some("idx_orders_2024_status".to_string()),
                QualifiedName::unqualified("orders_2024"),
            )
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Sub-partitioned table should emit partition finding"
        );
        assert!(
            findings[0].message.contains("lock all partitions"),
            "Should emit the partition-specific message for sub-partitioned tables"
        );
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
            .with_columns(vec![IndexColumn::Column("status".to_string())]),
        ))];

        let findings = RuleId::UnsafeDdl(UnsafeDdlRule::Pgm001).check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
