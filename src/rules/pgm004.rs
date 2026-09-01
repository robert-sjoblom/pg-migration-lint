//! PGM004 — DETACH PARTITION without CONCURRENTLY
//!
//! Detects detaching a partition from a pre-existing partitioned table
//! without the CONCURRENTLY option. Without CONCURRENTLY, PostgreSQL
//! acquires ACCESS EXCLUSIVE on the entire parent table.

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity, TableScope, alter_table_check};

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
  Use DETACH PARTITION ... CONCURRENTLY (PostgreSQL 14+). It takes
  only SHARE UPDATE EXCLUSIVE on the parent instead of ACCESS
  EXCLUSIVE, so it does not block readers and writers through the
  parent the way the plain form does.

Two things to plan for with CONCURRENTLY:
  1. It cannot run inside a transaction block, so the statement must
     be issued on its own (runInTransaction=\"false\" for Liquibase,
     the equivalent setting elsewhere). PGM003 does not currently
     detect this: it only checks CREATE/DROP INDEX CONCURRENTLY.
  2. It still waits for conflicting traffic on the parent, and an
     interrupted wait does not roll back. The first phase has already
     committed, so the child is left pending-detach in pg_inherits
     (inhdetachpending). Queries routed through the parent already
     skip the partition's rows while the rows remain in it, and the
     state does not self-resolve. A retry of the same DETACH ...
     CONCURRENTLY fails with SQLSTATE 55000 (hint: FINALIZE).
     Recovery is DETACH PARTITION ... FINALIZE or DROP TABLE on the
     child. While pending, that DROP TABLE still takes ACCESS
     EXCLUSIVE on the parent. It can lose to the same traffic that
     caused the interruption.

Example (bad):
  ALTER TABLE measurements DETACH PARTITION measurements_2023;

Fix (safe):
  ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;

Note: DETACH PARTITION CONCURRENTLY requires PostgreSQL 14+.";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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
    use crate::rules::test_helpers::{lint_ctx, located};

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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["measurements"]);

        let stmts = vec![detach_stmt("measurements", "measurements_2023", false)];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_parent_not_in_catalog() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![detach_stmt("measurements", "measurements_2023", false)];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
