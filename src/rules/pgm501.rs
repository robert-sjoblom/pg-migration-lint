#![doc = include_str!("docs/pgm501.md")]

use crate::parser::ir::{IrNode, Located, SourceSpan, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Foreign key without covering index on referencing columns";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm501.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    // Collect all FKs added in this unit.
    let mut fks: Vec<FkInfo> = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                for constraint in &ct.constraints {
                    if let TableConstraint::ForeignKey { columns, .. } = constraint {
                        fks.push(FkInfo {
                            table_name: ct.name.catalog_key().to_string(),
                            display_name: ct.name.display_name(),
                            columns: columns.clone(),
                            span: stmt.span.clone(),
                        });
                    }
                }
                // Also check inline FK from column definitions (is_inline_pk is for PK;
                // inline FK would be in constraints). The IR puts inline FKs into
                // the constraints list, so they are already handled above.
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    if let crate::parser::ir::AlterTableAction::AddConstraint(
                        TableConstraint::ForeignKey { columns, .. },
                    ) = action
                    {
                        fks.push(FkInfo {
                            table_name: at.name.catalog_key().to_string(),
                            display_name: at.name.display_name(),
                            columns: columns.clone(),
                            span: stmt.span.clone(),
                        });
                    }
                }
            }
            _ => {}
        }
    }

    // Post-file check: for each FK, check catalog_after for a covering index.
    // For partitioned tables, has_indexed_fk_column already excludes ON ONLY indexes.
    // For partition children, delegate to the parent's indexes if the child has none.
    let mut findings = Vec::new();
    for fk in &fks {
        let has_index = match ctx.catalog_after.get_table(&fk.table_name) {
            Some(table) if table.is_partitioned => table.has_indexed_fk_column(&fk.columns),
            Some(table) if table.parent_table.is_some() => {
                if table.has_indexed_fk_column(&fk.columns) {
                    true
                } else {
                    // Delegate to parent — a recursive parent index covers all children.
                    match table
                        .parent_table
                        .as_ref()
                        .and_then(|k| ctx.catalog_after.get_table(k))
                    {
                        Some(parent) => parent.has_indexed_fk_column(&fk.columns),
                        None => continue, // parent not in catalog: suppress conservatively
                    }
                }
            }
            Some(table) => table.has_indexed_fk_column(&fk.columns),
            None => false,
        };

        if !has_index {
            let cols_display = fk.columns.join(", ");
            findings.push(rule.make_finding(
                format!(
                    "Foreign key on '{table}({cols})' has no covering index. Queries \
                         joining or filtering on these columns, and referential integrity \
                         checks, cause sequential scans on the referencing table.",
                    table = fk.display_name,
                    cols = cols_display,
                ),
                ctx.file,
                &fk.span,
            ));
        }
    }

    findings
}

/// Represents a foreign key found in the current migration unit, with
/// enough context to report a finding.
struct FkInfo {
    table_name: String,
    display_name: String,
    columns: Vec<String>,
    span: SourceSpan,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::*;

    #[test]
    fn test_fk_no_index_fires() {
        let before = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"]);
            })
            .build();
        // After: child has FK but no index
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"])
                    .fk("fk_parent", &["pid"], "parent", &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_fk_no_index_message_leads_with_query_cost_and_narrows_update_clause() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        let message = &findings[0].message;
        assert!(
            message.contains("joining or filtering"),
            "message should name query joins/filters as a seq-scan cause, \
             the more frequent one, not just constraint enforcement: {message:?}"
        );
        assert!(
            !message.contains("deletes/updates on the referenced table"),
            "message must not imply any UPDATE on the referenced table triggers \
             the check — only deletes or updates to the referenced key do: {message:?}"
        );
    }

    #[test]
    fn test_fk_with_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"]);
            })
            .build();
        // After: child has FK AND index
        let after = CatalogBuilder::new()
            .table("parent", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("child", |t| {
                t.column("id", "integer", false)
                    .column("pid", "integer", false)
                    .pk(&["id"])
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .index("idx_child_pid", &["pid"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_reversed_index_order_no_finding() {
        let before = Catalog::new();
        // After: child has composite FK (a, b) but index is (b, a)
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .fk("fk_composite", &["a", "b"], "parent", &["x", "y"])
                    .index("idx_wrong_order", &["b", "a"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("child"))
                .with_columns(vec![
                    ColumnDef::test("a", "integer").with_nullable(false),
                    ColumnDef::test("b", "integer").with_nullable(false),
                ])
                .with_constraints(vec![TableConstraint::ForeignKey {
                    name: Some("fk_composite".to_string()),
                    columns: vec!["a".to_string(), "b".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["x".to_string(), "y".to_string()],
                    not_valid: false,
                }]),
        ))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Reversed-order index (b, a) still fully covers FK (a, b) — column order doesn't matter for an equality-only lookup"
        );
    }

    #[test]
    fn test_fk_prefix_match_no_finding() {
        let before = Catalog::new();
        // After: FK (a, b) with index (a, b, c) — prefix covers it
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("a", "integer", false)
                    .column("b", "integer", false)
                    .column("c", "integer", false)
                    .fk("fk_composite", &["a", "b"], "parent", &["x", "y"])
                    .index("idx_abc", &["a", "b", "c"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_composite".to_string()),
                    columns: vec!["a".to_string(), "b".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["x".to_string(), "y".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_on_partitioned_table_no_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "FK on partitioned table without covering index should fire"
        );
    }

    #[test]
    fn test_fk_on_partition_child_suppressed() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_fk_on_partitioned_table_via_create_table_fires() {
        // FK defined inline in CREATE TABLE on a partitioned table, no index.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("ref_table", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "ref_table", &["id"])
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![
                    ColumnDef::test("id", "integer").with_nullable(false),
                    ColumnDef::test("ref_id", "integer").with_nullable(false),
                ])
                .with_constraints(vec![TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("ref_table"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                }])
                .with_partition_by(
                    crate::parser::ir::PartitionStrategy::Range,
                    vec!["id".to_string()],
                ),
        ))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "FK via CREATE TABLE on partitioned table without index should fire"
        );
    }

    #[test]
    fn test_fk_with_partial_index_fires() {
        let before = Catalog::new();
        // After: child has FK and a partial index covering the FK columns,
        // but partial indexes don't count for FK coverage.
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .partial_index("idx_pid_active", &["pid"], false, "active = true");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Partial index should not satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_with_expression_index_fires() {
        let before = Catalog::new();
        // After: child has FK on (pid) but index is on (lower(pid::text)) — expression
        let after = CatalogBuilder::new()
            .table("child", |t| {
                t.column("pid", "integer", false)
                    .fk("fk_parent", &["pid"], "parent", &["id"])
                    .expression_index("idx_pid_expr", &["expr:lower(pid::text)"], false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_parent".to_string()),
                    columns: vec!["pid".to_string()],
                    ref_table: QualifiedName::unqualified("parent"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Expression index should not satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_with_regular_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_orders_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Recursive index on partitioned table should satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_partial_coverage_on_partitioned_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("ref_table", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("postings", |t| {
                t.column("transaction_id", "integer", false)
                    .column("partition_key", "integer", false)
                    .fk(
                        "fk_txn",
                        &["transaction_id", "partition_key"],
                        "ref_table",
                        &["id"],
                    )
                    .index("idx_txn", &["transaction_id"], false)
                    .partitioned_by(
                        crate::parser::ir::PartitionStrategy::Range,
                        &["partition_key"],
                    );
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("postings"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_txn".to_string()),
                    columns: vec!["transaction_id".to_string(), "partition_key".to_string()],
                    ref_table: QualifiedName::unqualified("ref_table"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Index on only one FK column still avoids a seq scan (Index Scan + Filter), \
             so PGM501 should not fire — this is the real production false positive"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_with_only_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .only_index("idx_orders_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "ON ONLY index should NOT satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partitioned_table_after_attach_no_finding() {
        // ON ONLY index was created, then ALTER INDEX ATTACH flipped only to false.
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_orders_ref_id", &["ref_id"], false) // only=false after attach
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "After ATTACH, index should satisfy FK coverage"
        );
    }

    #[test]
    fn test_fk_on_partition_child_with_own_index_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .index("idx_child_ref_id", &["ref_id"], false)
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(findings.is_empty(), "Child with own index should not fire");
    }

    #[test]
    fn test_fk_with_gin_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"]);
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_cust", &["customer_id"], "customers", &["id"])
                    .index_with_method("idx_gin_cust", &["customer_id"], false, "gin");
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_cust".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "GIN index should NOT satisfy FK coverage — only btree indexes can"
        );
    }

    #[test]
    fn test_fk_with_gist_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"]);
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("customer_id", "integer", false)
                    .pk(&["id"])
                    .fk("fk_cust", &["customer_id"], "customers", &["id"])
                    .index_with_method("idx_gist_cust", &["customer_id"], false, "gist");
            })
            .table("customers", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_cust".to_string()),
                    columns: vec!["customer_id".to_string()],
                    ref_table: QualifiedName::unqualified("customers"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "GiST index should NOT satisfy FK coverage — only btree indexes can"
        );
    }

    #[test]
    fn test_fk_on_partition_child_delegates_to_parent() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders_parent", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .index("idx_parent_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_child", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders_child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert!(
            findings.is_empty(),
            "Child should delegate to parent's recursive index"
        );
    }

    #[test]
    fn test_fk_on_partition_child_parent_only_index_fires() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("parent_ref", |t| {
                t.column("id", "integer", false).pk(&["id"]);
            })
            .table("orders_parent", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .only_index("idx_parent_ref_id", &["ref_id"], false)
                    .partitioned_by(crate::parser::ir::PartitionStrategy::Range, &["id"]);
            })
            .table("orders_child", |t| {
                t.column("id", "integer", false)
                    .column("ref_id", "integer", false)
                    .fk("fk_ref", &["ref_id"], "parent_ref", &["id"])
                    .partition_of("orders_parent");
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders_child"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::ForeignKey {
                    name: Some("fk_ref".to_string()),
                    columns: vec!["ref_id".to_string()],
                    ref_table: QualifiedName::unqualified("parent_ref"),
                    ref_columns: vec!["id".to_string()],
                    not_valid: false,
                },
            )],
        }))];

        let findings = RuleId::Pgm501.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Parent's ON ONLY index should NOT satisfy child FK coverage"
        );
    }
}
