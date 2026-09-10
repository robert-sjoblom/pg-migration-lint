//! PGM024 — DROP TABLE or CREATE TABLE PARTITION OF locks the parent
//!
//! Detects two statement shapes that both take ACCESS EXCLUSIVE on a
//! pre-existing partitioned parent: dropping a live partition child, and
//! creating a new child directly as `PARTITION OF` an existing parent.

use std::collections::HashSet;

use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "DROP TABLE or CREATE TABLE PARTITION OF locks the parent partitioned table";

pub(super) const EXPLAIN: &str = "\
PGM024 — DROP TABLE or CREATE TABLE PARTITION OF locks the parent

What it detects:
  Two statement shapes that both acquire ACCESS EXCLUSIVE on a
  pre-existing partitioned parent table:
    1. DROP TABLE child, where child is a partition of a parent that
       already existed before this change.
    2. CREATE TABLE child PARTITION OF parent, where parent already
       existed before this change.

Why it's dangerous:
  Both statements lock the parent, not just the child, and hold that
  lock until commit — blocking every reader and writer routed through
  the parent (and therefore every sibling partition) for the duration.
  A reader or writer already holding the parent does not make either
  statement fail outright: it queues behind them, and while queued it
  blocks new readers the current holder would have allowed. DROP TABLE
  also destroys the child's data irreversibly, with no CONCURRENTLY
  variant at all.

Safe alternative (DROP TABLE):
  DETACH PARTITION child CONCURRENTLY (PostgreSQL 14+) first — this
  takes only SHARE UPDATE EXCLUSIVE on the parent — then DROP the
  now-standalone table. See PGM004.

Safe alternative (CREATE TABLE ... PARTITION OF):
  There is no CONCURRENTLY form of CREATE TABLE ... PARTITION OF.
  Instead, create the table standalone, then ATTACH PARTITION it:
  ATTACH takes only SHARE UPDATE EXCLUSIVE on the parent (plus a brief
  ACCESS EXCLUSIVE on the new table itself, which nothing is using
  yet, so it is uncontended). See PGM005 for the CHECK constraint that
  lets the attach skip a full table scan of the child.

Example (bad):
  DROP TABLE measurements_2023;
  CREATE TABLE measurements_2025 PARTITION OF measurements
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

Fix (safe):
  ALTER TABLE measurements DETACH PARTITION measurements_2023 CONCURRENTLY;
  DROP TABLE measurements_2023;

  CREATE TABLE measurements_2025 (LIKE measurements INCLUDING ALL);
  ALTER TABLE measurements_2025 ADD CONSTRAINT measurements_2025_bound
    CHECK (ts >= '2025-01-01' AND ts < '2026-01-01') NOT VALID;
  ALTER TABLE measurements_2025 VALIDATE CONSTRAINT measurements_2025_bound;
  ALTER TABLE measurements ATTACH PARTITION measurements_2025
    FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');

Known limitation:
  If the same migration unit both DETACHes a child and then DROPs it,
  this rule does not fire on the DROP — the DETACH already removes the
  lock hazard this rule exists to catch, so suppressing there avoids a
  false positive on the exact fix this rule recommends. The reverse
  order (ATTACH then immediately DROP the same child, in the same
  unit) is not specially detected either way; this is a narrow,
  unusual shape and the rule falls back to whatever catalog_before
  already recorded.

See also: PGM004 (DETACH PARTITION without CONCURRENTLY), PGM005
(ATTACH PARTITION without pre-validated CHECK), PGM201 (DROP TABLE —
covers data loss on any table; this rule owns the lock hazard for
partition children specifically).";

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    // Children detached earlier in this same unit — DROP TABLE on one of
    // these is the safe pattern PGM004 recommends, not the hazard this
    // rule exists to catch. Order-insensitive: DROP TABLE <child> before
    // its own DETACH is not valid SQL, so it can't occur the other way.
    let detached_in_unit: HashSet<&str> = statements
        .iter()
        .filter_map(|stmt| match &stmt.node {
            IrNode::AlterTable(at) => at.actions.iter().find_map(|action| match action {
                AlterTableAction::DetachPartition { child, .. } => Some(child.catalog_key()),
                _ => None,
            }),
            _ => None,
        })
        .collect();

    let mut findings = Vec::new();
    for stmt in statements {
        match &stmt.node {
            IrNode::DropTable(dt) => {
                let child_key = dt.name.catalog_key();
                if detached_in_unit.contains(child_key) {
                    continue;
                }
                let Some(parent_key) = ctx
                    .catalog_before
                    .get_table(child_key)
                    .and_then(|t| t.parent_table.clone())
                else {
                    continue;
                };
                if !ctx.is_existing_table(&parent_key) {
                    continue;
                }
                let parent_name = ctx
                    .parent_display_name(child_key)
                    .unwrap_or_else(|| parent_key.clone());
                findings.push(
                    rule.make_finding(
                        format!(
                            "DROP TABLE '{}' is a partition of '{}': this acquires ACCESS \
                             EXCLUSIVE on '{}' for the duration, blocking all reads and writes \
                             routed through it until commit — and the data is permanently lost. \
                             DETACH PARTITION '{}' CONCURRENTLY first, then drop the \
                             now-standalone table.",
                            dt.name.display_name(),
                            parent_name,
                            parent_name,
                            dt.name.display_name(),
                        ),
                        ctx.file,
                        &stmt.span,
                    )
                    .with_dedup_key(parent_key),
                );
            }
            IrNode::CreateTable(ct) => {
                let Some(parent_name) = &ct.partition_of else {
                    continue;
                };
                let parent_key = parent_name.catalog_key();
                if !ctx.is_existing_table(parent_key) {
                    continue;
                }
                findings.push(
                    rule.make_finding(
                        format!(
                            "CREATE TABLE '{}' PARTITION OF '{}' acquires ACCESS EXCLUSIVE on \
                             '{}' for the duration, blocking all reads and writes routed \
                             through it until commit. Create '{}' as a standalone table and \
                             ATTACH PARTITION it instead — ATTACH only takes SHARE UPDATE \
                             EXCLUSIVE on the parent.",
                            ct.name.display_name(),
                            parent_name.display_name(),
                            parent_name.display_name(),
                            ct.name.display_name(),
                        ),
                        ctx.file,
                        &stmt.span,
                    )
                    .with_dedup_key(parent_key.to_string()),
                );
            }
            _ => {}
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{lint_ctx, located};

    fn rule_id() -> RuleId {
        RuleId::Pgm024
    }

    fn drop_stmt(name: &str) -> Located<IrNode> {
        located(IrNode::DropTable(
            DropTable::test(QualifiedName::unqualified(name)).with_if_exists(false),
        ))
    }

    fn detach_stmt(parent: &str, child: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(parent),
            actions: vec![AlterTableAction::DetachPartition {
                child: QualifiedName::unqualified(child),
                concurrent: true,
            }],
        }))
    }

    fn create_partition_of_stmt(child: &str, parent: &str) -> Located<IrNode> {
        located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified(child))
                .with_columns(vec![ColumnDef::test("id", "bigint").with_nullable(false)])
                .with_partition_of(QualifiedName::unqualified(parent)),
        ))
    }

    #[test]
    fn test_fires_on_drop_of_live_partition_child() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2023", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![drop_stmt("measurements_2023")];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_when_same_unit_detaches_first() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2023", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            detach_stmt("measurements", "measurements_2023"),
            drop_stmt("measurements_2023"),
        ];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "DETACH then DROP in the same unit is the safe pattern PGM004 recommends, got: {findings:?}"
        );
    }

    #[test]
    fn test_no_finding_when_parent_created_in_change() {
        // The parent genuinely exists in `catalog_before` (it was already
        // replayed from an earlier file in this same changeset) — the only
        // reason this is suppressed is that `measurements` is still marked
        // as created in this change, which must win over `has_table`.
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2023", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["measurements"]);

        let stmts = vec![drop_stmt("measurements_2023")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_standalone_table_drop() {
        let before = CatalogBuilder::new()
            .table("customers", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![drop_stmt("customers")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_table_not_in_catalog_before() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![drop_stmt("measurements_2023")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fires_on_create_table_partition_of_existing_parent() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![create_partition_of_stmt(
            "measurements_2025",
            "measurements",
        )];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_when_partition_parent_created_in_change() {
        // The parent genuinely exists in `catalog_before` (it was already
        // replayed from an earlier file in this same changeset) — the only
        // reason this is suppressed is that `measurements` is still marked
        // as created in this change, which must win over `has_table`.
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["measurements"]);

        let stmts = vec![create_partition_of_stmt(
            "measurements_2025",
            "measurements",
        )];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_plain_create_table() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("customers"))
                .with_columns(vec![ColumnDef::test("id", "bigint").with_nullable(false)]),
        ))];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_dedup_key_collapses_findings_under_one_parent() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .partitioned_by(PartitionStrategy::Range, &["id"]);
            })
            .table("measurements_2023", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false).partition_of("measurements");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![
            drop_stmt("measurements_2023"),
            drop_stmt("measurements_2024"),
            create_partition_of_stmt("measurements_2025", "measurements"),
        ];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            3,
            "check() itself does not dedup — that's a post-processing step"
        );
        assert!(
            findings
                .iter()
                .all(|f| f.dedup_key.as_deref() == Some("measurements")),
            "all three findings must share the parent's key so the pipeline's dedup_findings collapses them to one, got: {findings:?}"
        );
    }
}
