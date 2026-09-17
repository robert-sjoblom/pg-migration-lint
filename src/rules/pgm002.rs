#![doc = include_str!("docs/pgm002.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Missing CONCURRENTLY on DROP INDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm002.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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

            let table = ctx.catalog_before.get_table(table_name);
            let display_name = table.map(|t| t.display_name.as_str()).unwrap_or(table_name);
            let is_partitioned = table.map(|t| t.is_partitioned).unwrap_or(false);
            let idx_is_only = ctx
                .catalog_before
                .get_index(&di.index_name)
                .map(|idx| idx.only)
                .unwrap_or(false);

            if is_partitioned && idx_is_only {
                // Case A: ON ONLY stub on partitioned parent — safe to drop without
                // CONCURRENTLY; it only affects the invalid parent stub.
                continue;
            }

            if is_partitioned {
                // Case B: Recursive/attached index on partitioned parent.
                // PostgreSQL does NOT support DROP INDEX CONCURRENTLY on partitioned
                // parent indexes. Emit a partition-specific warning.
                findings.push(rule.make_finding(
                    format!(
                        "DROP INDEX '{}' on partitioned table '{}' will lock all \
                            partitions. CONCURRENTLY is not supported for partitioned \
                            parent indexes.",
                        di.index_name, display_name,
                    ),
                    ctx.file,
                    &stmt.span,
                ));
            } else {
                // Case C: Non-partitioned table — standard CONCURRENTLY advice.
                findings.push(rule.make_finding(
                    format!(
                        "DROP INDEX '{}' on existing table '{}' should use CONCURRENTLY \
                            to avoid holding an exclusive lock.",
                        di.index_name, display_name,
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
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

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
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_index_on_partitioned_table_partition_message() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .index("idx_orders_status", &["status"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_drop_only_index_on_partitioned_table_suppressed() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .only_index("idx_orders_status", &["status"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "ON ONLY index on partitioned table should be safe to drop"
        );
    }

    #[test]
    fn test_drop_only_index_on_non_partitioned_table_fires() {
        // ON ONLY on a non-partitioned table is unusual but should still fire
        // the standard CONCURRENTLY message.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .only_index("idx_orders_status", &["status"], false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("CONCURRENTLY"),
            "Non-partitioned table should get standard CONCURRENTLY message"
        );
    }

    #[test]
    fn test_drop_index_after_attach_fires() {
        // ON ONLY index was created, then ALTER INDEX ATTACH flipped only to false.
        // Now dropping should fire the partition-specific message.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", false)
                    .index("idx_orders_status", &["status"], false) // only=false after attach
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/005.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status").with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].message.contains("partitioned table"),
            "After ATTACH, should get partition-specific message"
        );
        assert!(
            findings[0]
                .message
                .contains("CONCURRENTLY is not supported"),
            "Should explain CONCURRENTLY not supported for partitioned indexes"
        );
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
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![located(IrNode::DropIndex(
            DropIndex::test("idx_orders_status")
                .with_concurrent(true)
                .with_if_exists(false),
        ))];

        let findings = RuleId::Pgm002.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
