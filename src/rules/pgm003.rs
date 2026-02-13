//! PGM003 — Foreign key without covering index
//!
//! Detects foreign key constraints where the referencing table has no index
//! whose leading columns match the FK columns. Without such an index,
//! deletes and updates on the referenced table cause sequential scans on
//! the referencing table, leading to severe performance degradation.

use crate::parser::ir::{IrNode, Located, SourceSpan, TableConstraint};
use crate::rules::{Finding, LintContext, Rule, Severity};

/// Rule that flags foreign keys without a covering index on the referencing table.
pub struct Pgm003;

/// Represents a foreign key found in the current migration unit, with
/// enough context to report a finding.
struct FkInfo {
    table_name: String,
    display_name: String,
    columns: Vec<String>,
    span: SourceSpan,
}

impl Rule for Pgm003 {
    fn id(&self) -> &'static str {
        "PGM003"
    }

    fn default_severity(&self) -> Severity {
        Severity::Major
    }

    fn description(&self) -> &'static str {
        "Foreign key without covering index on referencing columns"
    }

    fn explain(&self) -> &'static str {
        "PGM003 — Foreign key without covering index\n\
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
         so creating the index later in the same file avoids a false positive."
    }

    fn check(&self, statements: &[Located<IrNode>], ctx: &LintContext<'_>) -> Vec<Finding> {
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
        let mut findings = Vec::new();
        for fk in &fks {
            let has_index = ctx
                .catalog_after
                .get_table(&fk.table_name)
                .map(|t| t.has_covering_index(&fk.columns))
                .unwrap_or(false);

            if !has_index {
                let cols_display = fk.columns.join(", ");
                findings.push(self.make_finding(
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
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

        let findings = Pgm003.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule_id, "PGM003");
        assert_eq!(findings[0].severity, Severity::Major);
        assert!(findings[0].message.contains("child(pid)"));
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

        let findings = Pgm003.check(&stmts, &ctx);
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

        let stmts = vec![located(IrNode::CreateTable(CreateTable {
            name: QualifiedName::unqualified("child"),
            columns: vec![
                ColumnDef {
                    name: "a".to_string(),
                    type_name: TypeName::simple("integer"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
                ColumnDef {
                    name: "b".to_string(),
                    type_name: TypeName::simple("integer"),
                    nullable: false,
                    default_expr: None,
                    is_inline_pk: false,
                    is_serial: false,
                },
            ],
            constraints: vec![TableConstraint::ForeignKey {
                name: Some("fk_composite".to_string()),
                columns: vec!["a".to_string(), "b".to_string()],
                ref_table: QualifiedName::unqualified("parent"),
                ref_columns: vec!["x".to_string(), "y".to_string()],
                not_valid: false,
            }],
            temporary: false,
        }))];

        let findings = Pgm003.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("child(a, b)"));
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

        let findings = Pgm003.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
