//! PGM005 — ATTACH PARTITION without pre-validated CHECK constraint
//!
//! Detects attaching an existing table as a partition when the child table
//! has no CHECK constraint. Without a pre-validated CHECK, PostgreSQL
//! performs a full table scan under ACCESS EXCLUSIVE lock.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str =
    "ATTACH PARTITION of existing table without pre-validated CHECK";

pub(super) const EXPLAIN: &str = "\
PGM005 — ATTACH PARTITION without pre-validated CHECK constraint

What it detects:
  ALTER TABLE parent ATTACH PARTITION child FOR VALUES ... where the
  child table already exists, has existing rows, and has no CHECK
  constraint in the catalog.

Why it's dangerous:
  When attaching a partition, PostgreSQL must verify that every existing
  row in the child satisfies the partition bound. Without a pre-validated
  CHECK constraint whose expression implies the partition bound, PostgreSQL
  performs a full table scan under ACCESS EXCLUSIVE lock on the child
  table. For large child tables this causes extended unavailability.

Safe alternative (3-step pattern):
  -- Step 1: Add a CHECK constraint mirroring the partition bound (NOT VALID)
  ALTER TABLE orders_2024 ADD CONSTRAINT orders_2024_bound_check
      CHECK (created_at >= '2024-01-01' AND created_at < '2025-01-01')
      NOT VALID;

  -- Step 2: Validate separately (SHARE UPDATE EXCLUSIVE — allows reads & writes)
  ALTER TABLE orders_2024 VALIDATE CONSTRAINT orders_2024_bound_check;

  -- Step 3: Attach (scan skipped because constraint is already validated)
  ALTER TABLE orders_partitioned ATTACH PARTITION orders_2024
      FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');

Note: This rule does not verify whether the CHECK expression semantically
implies the partition bound — it only checks for the presence of any CHECK
constraint. A false negative occurs when a CHECK exists but does not match
the bound; this is acceptable for v1.";

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
            if let AlterTableAction::AttachPartition { child } = action {
                let child_key = child.catalog_key();

                if !ctx.catalog_before.has_table(child_key) {
                    return vec![];
                }

                if ctx.tables_created_in_change.contains(child_key) {
                    return vec![];
                }

                // Child has at least one CHECK constraint — trust it implies the bound
                if let Some(table) = ctx.catalog_before.get_table(child_key) {
                    let has_check = table
                        .constraints
                        .iter()
                        .any(|c| matches!(c, ConstraintState::Check { .. }));
                    if has_check {
                        return vec![];
                    }
                }

                vec![rule.make_finding(
                    format!(
                        "ATTACH PARTITION of existing table '{}' to '{}' will \
                         scan the entire child table under ACCESS EXCLUSIVE lock \
                         to verify the partition bound. Add a CHECK constraint \
                         mirroring the partition bound, validate it separately, \
                         then attach.",
                        child.display_name(),
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
    use crate::rules::test_helpers::{located, make_ctx};
    use crate::rules::{RuleId, UnsafeDdlRule};
    use std::collections::HashSet;
    use std::path::PathBuf;

    /// Helper to build an ALTER TABLE ... ATTACH PARTITION statement.
    fn attach_stmt(parent: &str, child: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(parent),
            actions: vec![AlterTableAction::AttachPartition {
                child: QualifiedName::unqualified(child),
            }],
        }))
    }

    fn rule_id() -> RuleId {
        RuleId::UnsafeDdl(UnsafeDdlRule::Pgm005)
    }

    #[test]
    fn test_fires_on_existing_child_without_check() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_no_finding_when_child_has_check() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .check_constraint(Some("measurements_2024_bound"), false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

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
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false);
            })
            .build();
        let file = PathBuf::from("migrations/001.sql");
        let mut created = HashSet::new();
        created.insert("measurements".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_on_new_child() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false);
            })
            .build();
        let file = PathBuf::from("migrations/002.sql");
        let mut created = HashSet::new();
        created.insert("measurements_2024".to_string());
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_no_finding_when_child_not_in_catalog() {
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

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
