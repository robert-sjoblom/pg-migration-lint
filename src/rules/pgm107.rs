#![doc = include_str!("docs/pgm107.md")]

use crate::catalog::types::TableState;
use crate::parser::ir::{AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::using_index::{UsingIndex, using_indexes_by_statement};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "Primary key column uses integer or smallint instead of bigint";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm107.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

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

    for (stmt, using_indexes) in statements
        .iter()
        .zip(using_indexes_by_statement(ctx.catalog_before, statements))
    {
        match &stmt.node {
            IrNode::CreateTable(ct) => {
                // The parser promotes an inline PRIMARY KEY to the table's
                // constraints, so these cover both spellings.
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

                    if pk_columns.contains(&col.name.as_str()) {
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
                    let AlterTableAction::AddConstraint {
                        constraint:
                            TableConstraint::PrimaryKey {
                                columns,
                                using_index,
                                ..
                            },
                        ..
                    } = action
                    else {
                        continue;
                    };

                    // With USING INDEX the constraint's columns are empty: the
                    // PK columns are the referenced index's, typed as the table
                    // stands just before the statement. Only an index of the
                    // altered table has them: Postgres rejects another table's
                    // index (PGM027).
                    let (table, col_names): (&TableState, Vec<&str>) = match using_index {
                        Some(idx_name) => {
                            let Some(UsingIndex::OfTable { index, table }) =
                                using_indexes.get(idx_name)
                            else {
                                continue;
                            };
                            (table, index.column_names().collect())
                        }
                        None => {
                            let table_key = at.name.relation();
                            let Some(table) = ctx
                                .catalog_before
                                .get_table(table_key)
                                .or_else(|| ctx.catalog_after.get_table(table_key))
                            else {
                                continue;
                            };
                            (table, columns.iter().map(String::as_str).collect())
                        }
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
    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized};

    #[test]
    fn test_add_pk_using_index_reads_columns_from_the_index_in_the_tables_schema() {
        let before = catalog_from_sql(
            "CREATE TABLE analytics.orders (order_id int NOT NULL);
             CREATE UNIQUE INDEX idx_pk ON analytics.orders (order_id);
             CREATE TABLE data.orders (id bigint NOT NULL);
             CREATE UNIQUE INDEX idx_pk ON data.orders (id);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE analytics.orders ADD PRIMARY KEY USING INDEX idx_pk;",
        ));

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "got {findings:?}");
        assert!(
            findings[0].message.contains("'order_id' on 'analytics.orders'"),
            "got {}",
            findings[0].message
        );
    }

    #[test]
    fn test_add_pk_using_index_of_another_table_reads_none_of_its_columns() {
        let before = catalog_from_sql(
            "CREATE TABLE customers (id int NOT NULL);
             CREATE UNIQUE INDEX idx_customers_id ON customers (id);
             CREATE TABLE orders (id int NOT NULL);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_customers_id;",
        ));

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    fn messages_for_change(history: &str, change: &str) -> Vec<String> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");
        RuleId::Pgm107
            .check(&normalized(parse_sql(change)), &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[test]
    fn test_add_pk_using_an_index_created_earlier_in_the_unit_reads_its_columns() {
        let messages = messages_for_change(
            "CREATE TABLE orders (id int NOT NULL);",
            "CREATE UNIQUE INDEX orders_id_idx ON orders (id);
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert_eq!(messages.len(), 1, "got {messages:?}");
        assert!(
            messages[0].contains("'id' on 'orders' uses integer"),
            "got {messages:?}"
        );
    }

    #[test]
    fn test_add_pk_using_index_reads_column_types_as_the_earlier_statements_of_the_unit_leave_them()
    {
        let messages = messages_for_change(
            "CREATE TABLE orders (id int NOT NULL);
             CREATE UNIQUE INDEX orders_id_idx ON orders (id);",
            "ALTER TABLE orders ALTER COLUMN id TYPE bigint;
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn test_inline_pk_int4_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = normalized(parse_sql("CREATE TABLE orders (id int4 PRIMARY KEY);"));

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_inline_pk_int2_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = normalized(parse_sql("CREATE TABLE counters (id int2 PRIMARY KEY);"));

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_table_level_pk_int4_fires() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = vec![located(IrNode::CreateTable(
            CreateTable::test(QualifiedName::unqualified("orders"))
                .with_columns(vec![ColumnDef::test("id", "int4").with_nullable(false)])
                .with_constraints(vec![TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

        let stmts = normalized(parse_sql("CREATE TABLE orders (id int8 PRIMARY KEY);"));

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_int4_non_pk_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql");

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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("order_items"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["order_id".to_string(), "item_id".to_string()],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec![],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec![],
                    include_columns: vec![],
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
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("nonexistent"),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec!["id".to_string()],
                    include_columns: vec![],
                    using_index: None,
                },
            )],
        }))];

        let findings = RuleId::Pgm107.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
