//! PGM004 — DETACH PARTITION without CONCURRENTLY
//!
//! Detects detaching a partition from a pre-existing partitioned table
//! without the CONCURRENTLY option. Without CONCURRENTLY, PostgreSQL
//! acquires ACCESS EXCLUSIVE on the entire parent table.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str = "DETACH PARTITION on existing table without CONCURRENTLY";

pub(super) const EXPLAIN: &str = "\
PGM004 — DETACH PARTITION without CONCURRENTLY

What it detects:
  ALTER TABLE parent DETACH PARTITION child where the parent table
  already exists and the CONCURRENTLY option is not used.

Why it's dangerous:
  Plain DETACH PARTITION acquires ACCESS EXCLUSIVE on both the parent
  partitioned table and the child partition for the full duration of
  the operation. This blocks all reads and writes on the parent (and
  therefore all its partitions) until detach completes.

Safe alternative:
  Use DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+), which uses
  SHARE UPDATE EXCLUSIVE instead, allowing concurrent reads and writes.

Example (bad):
  ALTER TABLE measurements DETACH PARTITION measurements_2023;

Fix (safe):
  ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;

Note: DETACH PARTITION CONCURRENTLY requires PostgreSQL 14+.";

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    alter_table_check::check_alter_actions(
        statements,
        ctx,
        TableScope::ExcludeCreatedInChange,
        |at, action, stmt, ctx| {
            if let AlterTableAction::DetachPartition {
                concurrent: false, ..
            } = action
            {
                vec![rule.make_finding(
                    format!(
                        "DETACH PARTITION on existing partitioned table '{}' \
                         without CONCURRENTLY acquires ACCESS EXCLUSIVE on the \
                         entire table, blocking all reads and writes. Use \
                         DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+).",
                        at.name.display_name(),
                    ),
                    ctx.file,
                    &stmt.span,
                )]
            } else {
                vec![]
            }
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Helper to build an ALTER TABLE ... DETACH PARTITION statement.
    fn detach_stmt(parent: &str, child: &str, concurrent: bool) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(parent),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified(child),
                concurrent,
            }],
        }))
    }

    fn rule_id() -> RuleId {
        RuleId::Pgm004
    }

    #[test]
    fn test_fires_on_existing_parent() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![detach_stmt("measurements", "measurements_2023", false)];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_with_concurrently() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![detach_stmt("measurements", "measurements_2023", true)];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_new_parent() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("measurements".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![detach_stmt("measurements", "measurements_2023", false)];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_parent_not_in_catalog() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![detach_stmt("measurements", "measurements_2023", false)];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
