//! PGM501 — Foreign key without covering index
//!
//! Detects foreign key constraints where the referencing table has no index
//! whose leading columns match the FK columns. Without such an index,
//! deletes and updates on the referenced table cause sequential scans on
//! the referencing table, leading to severe performance degradation.

use crate::parser::ir::{IrNode, Located, SourceSpan, TableConstraint};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str = "Foreign key without covering index on referencing columns";

pub(super) const EXPLAIN: &str = "PGM501 — Foreign key without covering index\n\
         \n\
         What it detects:\n\
         A FOREIGN KEY constraint where the referencing table has no index\n\
         whose leading columns match the FK columns in order.\n\
         \n\
         Why it's dangerous:\n\
         When a row is deleted or updated in the referenced (parent) table,\n\
         PostgreSQL must check that no rows in the referencing (child) table\n\
         still reference the old value. Without an index on the FK columns,\n\
         this check performs a sequential scan of the entire child table —\n\
         once per affected parent row. This can cause severe performance\n\
         degradation and lock contention.\n\
         \n\
         Example (bad):\n\
           ALTER TABLE order_items\n\
             ADD CONSTRAINT fk_order\n\
             FOREIGN KEY (order_id) REFERENCES orders(id);\n\
           -- No index on order_items(order_id)\n\
         \n\
         Fix:\n\
           CREATE INDEX idx_order_items_order_id\n\
             ON order_items (order_id);\n\
           ALTER TABLE order_items\n\
             ADD CONSTRAINT fk_order\n\
             FOREIGN KEY (order_id) REFERENCES orders(id);\n\
         \n\
         Prefix matching: FK columns (a, b) are covered by index (a, b) or\n\
         (a, b, c) but NOT by (b, a) or (a). Column order matters.\n\
         \n\
         The check uses the catalog state AFTER the entire file is processed,\n\
         so creating the index later in the same file avoids a false positive.\n\
         \n\
         Partitioned tables:\n\
         For partitioned parent tables, a recursive index (one not created\n\
         with ON ONLY) covers all partitions and satisfies this check. An\n\
         ON ONLY index is just a stub and does NOT provide FK coverage until\n\
         child indexes are attached via ALTER INDEX ... ATTACH PARTITION.\n\
         \n\
         For partition children, the check first looks for an index on the\n\
         child itself, then delegates to the parent's indexes.";

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
    // For partitioned tables, has_covering_index already excludes ON ONLY indexes.
    // For partition children, delegate to the parent's indexes if the child has none.
    let mut findings = Vec::new();
    for fk in &fks {
        let has_index = match ctx.catalog_after.get_table(&fk.table_name) {
            Some(table) if table.is_partitioned => table.has_covering_index(&fk.columns),
            Some(table) if table.parent_table.is_some() => {
                if table.has_covering_index(&fk.columns) {
                    true
                } else {
                    // Delegate to parent — a recursive parent index covers all children.
                    match table
                        .parent_table
                        .as_ref()
                        .and_then(|k| ctx.catalog_after.get_table(k))
                    {
                        Some(parent) => parent.has_covering_index(&fk.columns),
                        None => continue, // parent not in catalog: suppress conservatively
                    }
                }
            }
            Some(table) => table.has_covering_index(&fk.columns),
            None => false,
        };

        if !has_index {
            let cols_display = fk.columns.join(", ");
            findings.push(rule.make_finding(
                format!(
                    "Foreign key on '{table}({cols})' has no covering index. \
                         Sequential scans on the referencing table during deletes/updates \
                         on the referenced table will cause performance issues.",
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
    use std::collections::HashSet;
    use std::path::PathBuf;

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
    fn test_fk_wrong_index_order_fires() {
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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        insta::assert_yaml_snapshot!(findings);
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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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

    // -----------------------------------------------------------------------
    // Partition-aware tests
    // -----------------------------------------------------------------------

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

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
