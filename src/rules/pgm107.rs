//! PGM107 — Integer Primary Key
//!
//! Detects primary key columns that use `int4` (`integer`) or `int2`
//! (`smallint`) instead of `int8` (`bigint`). High-write tables routinely
//! exhaust the 2.1 billion (`integer`) or 32 000 (`smallint`) limit.
//! Migrating to `bigint` later requires an ACCESS EXCLUSIVE lock and full
//! table rewrite.

use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::{Finding, LintContext, Rule};

pub(super) const DESCRIPTION: &str =
    "Primary key column uses integer or smallint instead of bigint";

pub(super) const EXPLAIN: &str = "PGM107 — Integer Primary Key\n\
         \n\
         What it detects:\n\
         A primary key column whose type is integer (int4) or smallint (int2).\n\
         Detected in CREATE TABLE (inline PK or table-level PRIMARY KEY) and\n\
         ALTER TABLE ... ADD PRIMARY KEY.\n\
         \n\
         Why it matters:\n\
         integer maxes out at ~2.1 billion rows, smallint at ~32 000.\n\
         High-write tables routinely exhaust these ranges. Migrating a\n\
         primary key column from integer to bigint requires an ACCESS\n\
         EXCLUSIVE lock and full table rewrite — a painful, high-risk\n\
         operation on production tables.\n\
         \n\
         Example (flagged):\n\
           CREATE TABLE orders (\n\
             id integer PRIMARY KEY\n\
           );\n\
         \n\
         Fix:\n\
           CREATE TABLE orders (\n\
             id bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY\n\
           );";

/// Returns `true` for integer types that are too small for a primary key.
fn is_small_int_type(type_name: &str) -> bool {
    matches!(type_name, "int2" | "int4" | "smallint" | "integer")
}

/// Human-readable label for the flagged type.
fn display_type(type_name: &str) -> &str {
    match type_name {
        "int2" | "smallint" => "smallint",
        "int4" | "integer" => "integer",
        _ => type_name,
    }
}

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();

    for stmt in statements {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                // Collect PK column names from table-level constraints
                let pk_columns: Vec<&str> = ct
                    .constraints
                    .iter()
                    .filter_map(|c| match c {
                        TableConstraint::PrimaryKey { columns, .. } => Some(columns.as_slice()),
                        _ => None,
                    })
                    .flatten()
                    .map(String::as_str)
                    .collect();

                for col in &ct.columns {
                    if !is_small_int_type(&col.type_name.name) {
                        continue;
                    }

                    let is_pk = col.is_inline_pk || pk_columns.iter().any(|&pk| pk == col.name);

                    if is_pk {
                        findings.push(rule.make_finding(
                            format!(
                                "Primary key column '{}' on '{}' uses {}. \
                                 Consider using bigint to avoid exhausting the \
                                 integer range on high-write tables.",
                                col.name,
                                ct.name.display_name(),
                                display_type(&col.type_name.name),
                            ),
                            ctx.file,
                            &stmt.span,
                        ));
                    }
                }
            }
            IrNode::AlterTable(at) => {
                for action in &at.actions {
                    let AlterTableAction::AddConstraint(TableConstraint::PrimaryKey {
                        columns,
                        using_index,
                    }) = action
                    else {
                        continue;
                    };

                    // Look up column types from catalog (before or after)
                    let table_key = at.name.catalog_key();
                    let table = ctx
                        .catalog_before
                        .get_table(table_key)
                        .or_else(|| ctx.catalog_after.get_table(table_key));

                    let Some(table) = table else {
                        continue;
                    };

                    // When USING INDEX is specified, columns is empty — resolve
                    // the PK columns from the referenced index instead.
                    let resolved_columns: Vec<String>;
                    let col_names: &[String] = if columns.is_empty() {
                        if let Some(idx_name) = using_index {
                            if let Some(idx) = ctx.get_index(idx_name) {
                                resolved_columns = idx.column_names().map(String::from).collect();
                                &resolved_columns
                            } else {
                                continue;
                            }
                        } else {
                            continue;
                        }
                    } else {
                        columns
                    };

                    for col_name in col_names {
                        if let Some(col) = table.get_column(col_name)
                            && is_small_int_type(&col.type_name.name)
                        {
                            findings.push(rule.make_finding(
                                format!(
                                    "Primary key column '{}' on '{}' uses {}. \
                                     Consider using bigint to avoid exhausting the \
                                     integer range on high-write tables.",
                                    col_name,
                                    at.name.display_name(),
                                    display_type(&col.type_name.name),
                                ),
                                ctx.file,
                                &stmt.span,
                            ));
                        }
                    }
                }
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
    use crate::rules::test_helpers::{located, make_ctx};
    use std::collections::HashSet;
    use std::path::PathBuf;

    #[test]
    fn test_inline_pk_int4_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("id", "int4")
                    .with_nullable(false)
                    .with_inline_pk(),
            ]),
        ))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_inline_pk_int2_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("counters")).with_columns(vec![
                ColumnDef::test("id", "int2")
                    .with_nullable(false)
                    .with_inline_pk(),
            ]),
        ))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_table_level_pk_int4_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![ColumnDef::test("id", "int4").with_nullable(false)])
                .with_constraints(vec![TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                }]),
        ))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("integer"));
    }

    #[test]
    fn test_bigint_pk_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("id", "int8")
                    .with_nullable(false)
                    .with_inline_pk(),
            ]),
        ))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_int4_non_pk_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/001.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders")).with_columns(vec![
                ColumnDef::test("quantity", "int4").with_nullable(true),
            ]),
        ))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_alter_table_add_pk_int4_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int4", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_alter_table_add_pk_bigint_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int8", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_composite_pk_one_small_fires() {
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("order_id", "int8", false)
                    .column("item_id", "int4", false);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("order_items"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec!["order_id".to_string(), "item_id".to_string()],
                    using_index: None,
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "should flag only the int4 column");
        assert!(findings[0].message.contains("item_id"));
    }

    #[test]
    fn test_add_pk_using_index_int4_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int4", false)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec![],
                    using_index: Some("idx_orders_pk".to_string()),
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("id"));
        assert!(findings[0].message.contains("integer"));
    }

    #[test]
    fn test_add_pk_using_index_bigint_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "int8", false)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec![],
                    using_index: Some("idx_orders_pk".to_string()),
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        let file = PathBuf::from("migrations/002.sql");
        let created = HashSet::new();
        let ctx = make_ctx(&before, &after, &file, &created);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("nonexistent"),
            actions: vec![AlterTableAction::AddConstraint(
                TableConstraint::PrimaryKey {
                    columns: vec!["id".to_string()],
                    using_index: None,
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
