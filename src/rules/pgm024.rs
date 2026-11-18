#![doc = include_str!("docs/pgm024.md")]

use crate::parser::ir::{IrNode, Located};
use crate::rules::partition_parent_lock::{ParentLockingStatement, ParentLocks};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "DROP TABLE or CREATE TABLE PARTITION OF locks the parent partitioned table";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm024.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let parent_locks = ParentLocks::of_unit(statements);
    statements
        .iter()
        .filter_map(|stmt| {
            let lock = parent_locks.taken_by(&stmt.node, ctx)?;
            let child = lock.child.display_name();
            let parent = &lock.parent.display_name;
            let message = match lock.statement {
                ParentLockingStatement::DropPartition => format!(
                    "DROP TABLE '{child}' is a partition of '{parent}': this acquires ACCESS \
                     EXCLUSIVE on '{parent}' for the duration, blocking all reads and writes \
                     routed through it until commit — and the data is permanently lost. \
                     DETACH PARTITION '{child}' CONCURRENTLY first, then drop the \
                     now-standalone table.",
                ),
                ParentLockingStatement::CreatePartitionOf => format!(
                    "CREATE TABLE '{child}' PARTITION OF '{parent}' acquires ACCESS EXCLUSIVE on \
                     '{parent}' for the duration, blocking all reads and writes routed \
                     through it until commit. Create '{child}' as a standalone table and \
                     ATTACH PARTITION it instead — ATTACH only takes SHARE UPDATE \
                     EXCLUSIVE on the parent.",
                ),
            };
            Some(
                rule.make_finding(message, ctx.file, &stmt.span)
                    .with_dedup_key(lock.parent.key),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::{CatalogBuilder, rel};
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
                .all(|f| f.dedup_key == Some(rel("measurements"))),
            "all three findings must share the parent's key so the pipeline's dedup_findings collapses them to one, got: {findings:?}"
        );
    }
}
