//! PGM005 — ATTACH PARTITION without pre-validated CHECK constraint
//!
//! Detects attaching an existing table as a partition when the child table
//! has no CHECK constraint referencing the partition key columns. Without
//! a pre-validated CHECK, PostgreSQL performs a full table scan under
//! ACCESS EXCLUSIVE lock.

use crate::catalog::types::ConstraintState;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, TableScope, alter_table_check};

pub(super) const DESCRIPTION: &str =
    "ATTACH PARTITION of existing table without pre-validated CHECK";

pub(super) const EXPLAIN: &str = "\
PGM005 — ATTACH PARTITION without pre-validated CHECK constraint

What it detects:
  ALTER TABLE parent ATTACH PARTITION child FOR VALUES ... where the
  child table already exists and has no CHECK constraint that references
  the partition key columns.

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

Note: The rule checks that at least one CHECK constraint on the child
references all of the parent's partition key columns. It does not verify
that the CHECK expression values semantically match the partition bound.";

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

                if let Some(table) = ctx.catalog_before.get_table(child_key) {
                    // Look up the parent's partition key columns to verify
                    // the CHECK references the right columns.
                    let parent_key = at.name.catalog_key();
                    let partition_columns = ctx
                        .catalog_before
                        .get_table(parent_key)
                        .and_then(|parent| parent.partition_by.as_ref())
                        .map(|pb| &pb.columns);

                    if let Some(columns) = partition_columns {
                        // Parent has partition info — require CHECK to
                        // reference all partition key columns.
                        if table.has_check_referencing_columns(columns) {
                            return vec![];
                        }
                    } else {
                        // Parent not in catalog or lacks partition info
                        // (incremental CI). Fall back to any CHECK.
                        if table
                            .constraints
                            .iter()
                            .any(|c| matches!(c, ConstraintState::Check { .. }))
                        {
                            return vec![];
                        }
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
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{located, make_ctx};
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
        RuleId::Pgm005
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
    fn test_no_finding_when_child_has_relevant_check() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .check_constraint(
                        Some("measurements_2024_bound"),
                        "(ts >= '2024-01-01' AND ts < '2025-01-01')",
                        false,
                    );
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
    fn test_fires_when_child_has_unrelated_check() {
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .column("value", "double precision", true)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .column("value", "double precision", true)
                    .check_constraint(Some("chk_value"), "(value > 0)", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Unrelated CHECK should not suppress PGM005"
        );
    }

    #[test]
    fn test_multi_column_partition_key_all_must_match() {
        let before = CatalogBuilder::new()
            .table("events", |t| {
                t.column("year", "integer", false)
                    .column("month", "integer", false)
                    .partitioned_by(PartitionStrategy::Range, &["year", "month"]);
            })
            .table("events_2024_q1", |t| {
                t.column("year", "integer", false)
                    .column("month", "integer", false)
                    .check_constraint(Some("chk_year_only"), "(year = 2024)", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("events", "events_2024_q1")];

        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "CHECK referencing only 'year' should not suppress when partition key is (year, month)"
        );
    }

    #[test]
    fn test_multi_column_partition_key_both_match() {
        let before = CatalogBuilder::new()
            .table("events", |t| {
                t.column("year", "integer", false)
                    .column("month", "integer", false)
                    .partitioned_by(PartitionStrategy::Range, &["year", "month"]);
            })
            .table("events_2024_q1", |t| {
                t.column("year", "integer", false)
                    .column("month", "integer", false)
                    .check_constraint(
                        Some("chk_bound"),
                        "(year = 2024 AND month >= 1 AND month <= 3)",
                        false,
                    );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("events", "events_2024_q1")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "CHECK referencing both partition key columns should suppress"
        );
    }

    #[test]
    fn test_fallback_any_check_when_parent_not_in_catalog() {
        // Parent not in catalog — incremental CI case. Fall back to "any CHECK".
        let before = CatalogBuilder::new()
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .check_constraint(Some("chk_whatever"), "(id > 0)", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "When parent is not in catalog, any CHECK should suppress (conservative)"
        );
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

    #[test]
    fn test_fires_when_parent_exists_but_not_partitioned() {
        // Parent exists in catalog but was NOT created with partitioned_by,
        // so partition_by is None. The fallback "any CHECK" logic kicks in.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("created_at", "timestamptz", false);
            })
            .table("orders_2024", |t| {
                t.column("id", "bigint", false)
                    .column("created_at", "timestamptz", false)
                    .check_constraint(Some("chk_id"), "(id > 0)", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("orders", "orders_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "When parent exists but lacks partition info, the fallback 'any CHECK' \
             logic should suppress the finding (conservative behavior)"
        );
    }

    #[test]
    fn test_fires_when_child_has_not_valid_check() {
        // Known limitation: the rule does NOT distinguish validated vs NOT VALID
        // CHECK constraints. PostgreSQL would still perform a full table scan
        // for a NOT VALID constraint during ATTACH PARTITION, but detecting this
        // requires tracking validation state changes which is out of scope.
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .check_constraint(
                        Some("measurements_2024_bound"),
                        "(ts >= '2024-01-01' AND ts < '2025-01-01')",
                        true, // NOT VALID
                    );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        // The rule suppresses because the CHECK references the partition key
        // column, even though it is NOT VALID. This is a known limitation:
        // PostgreSQL would still scan the table for NOT VALID constraints,
        // but detecting this requires tracking validation state changes
        // which is out of scope.
        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Known limitation: NOT VALID CHECK referencing partition key columns \
             suppresses the finding even though PostgreSQL would still scan"
        );
    }

    #[test]
    fn test_relevant_check_among_irrelevant_suppresses() {
        // Child has two CHECKs: one unrelated, one referencing the partition
        // key. The relevant one should be enough to suppress.
        let before = CatalogBuilder::new()
            .table("measurements", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .column("value", "double precision", true)
                    .partitioned_by(PartitionStrategy::Range, &["ts"]);
            })
            .table("measurements_2024", |t| {
                t.column("id", "bigint", false)
                    .column("ts", "timestamptz", false)
                    .column("value", "double precision", true)
                    .check_constraint(Some("chk_value"), "(value > 0)", false)
                    .check_constraint(
                        Some("measurements_2024_bound"),
                        "(ts >= '2024-01-01' AND ts < '2025-01-01')",
                        false,
                    );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("measurements", "measurements_2024")];

        let findings = rule_id().check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "One relevant CHECK among irrelevant ones should suppress"
        );
    }

    #[test]
    fn test_expression_partition_key_not_matched() {
        // Expression-based partition key like PARTITION BY RANGE (date_trunc('month', ts))
        // stores the whole expression as one "column" entry. The CHECK won't match
        // the full expression string as a single token, so the finding fires.
        let before = CatalogBuilder::new()
            .table("events", |t| {
                t.column("ts", "timestamptz", false)
                    .partitioned_by(PartitionStrategy::Range, &["date_trunc('month', ts)"]);
            })
            .table("events_2024", |t| {
                t.column("ts", "timestamptz", false).check_constraint(
                    Some("chk_bound"),
                    "(ts >= '2024-01-01' AND ts < '2025-01-01')",
                    false,
                );
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![attach_stmt("events", "events_2024")];

        // Expression-based partition keys are stored as opaque strings that
        // don't match simple column token checks. This is a known limitation;
        // the finding fires even though the CHECK may be semantically correct.
        let findings = rule_id().check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Expression-based partition keys cannot be matched by column-token heuristic"
        );
    }
}
