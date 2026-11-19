#![doc = include_str!("docs/pgm016.md")]

use crate::catalog::Catalog;
use crate::catalog::replay::map_with_state_before_if_needed;
use crate::parser::ir::{AlterTable, AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::using_index::{UsingIndex, has_using_index};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "ADD PRIMARY KEY on existing table without USING INDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm016.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    map_with_state_before_if_needed(
        ctx.catalog_before,
        statements,
        has_using_index,
        |state, stmt| statement_findings(&rule, state, stmt, ctx),
    )
    .into_iter()
    .flatten()
    .collect()
}

/// The findings for `stmt`, where `state` is the catalog just before it.
fn statement_findings(
    rule: &impl Rule,
    state: &Catalog,
    stmt: &Located<IrNode>,
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    let IrNode::AlterTable(at) = &stmt.node else {
        return findings;
    };
    if !ctx.is_existing_table(at.name.relation()) {
        return findings;
    }
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

        let message = match using_index {
            Some(idx_name) => using_index_message(
                at,
                idx_name,
                UsingIndex::for_rule_deferring_to_pgm027(ctx, state, at.name.relation(), idx_name),
            ),
            None => Some(format!(
                "ADD PRIMARY KEY on existing table '{table}' without USING INDEX \
                 on column(s) [{columns}]. Create a UNIQUE index CONCURRENTLY \
                 first, then use ADD PRIMARY KEY USING INDEX.",
                table = at.name.display_name(),
                columns = columns.join(", "),
            )),
        };
        if let Some(message) = message {
            findings.push(rule.make_finding(message, ctx.file, &stmt.span));
        }
    }
    findings
}

/// The message for `ALTER TABLE at ADD PRIMARY KEY USING INDEX idx_name`,
/// where `target` is what `idx_name` refers to just before the statement.
/// `None` when there is nothing to report: attaching the index takes no long
/// lock, or PostgreSQL rejects the statement and PGM027 reports it.
fn using_index_message(at: &AlterTable, idx_name: &str, target: UsingIndex<'_>) -> Option<String> {
    let table = at.name.display_name();
    match target {
        // Only while PGM027 judges the unit. Postgres rejects the statement
        // and builds nothing, so there is no lock to report; PGM027 reports it.
        UsingIndex::OfAnotherTable { .. } => None,
        UsingIndex::Missing => Some(format!(
            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
             referenced index does not exist."
        )),
        UsingIndex::OfTable { index, .. } if !index.unique => Some(format!(
            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
             referenced index is not UNIQUE."
        )),
        UsingIndex::OfTable { index, .. } if !index.is_btree() => Some(format!(
            "ADD PRIMARY KEY USING INDEX '{idx_name}' on table '{table}': \
             referenced index uses access method '{}', but only btree \
             indexes can back a PRIMARY KEY constraint.",
            index.access_method,
        )),
        UsingIndex::OfTable {
            index,
            table: table_state,
        } => {
            // The constraint's columns are empty with USING INDEX: the key
            // columns are the index's.
            let nullable_cols: Vec<&str> = index
                .column_names()
                .filter(|c| table_state.get_column(c).is_some_and(|col| col.nullable))
                .collect();
            (!nullable_cols.is_empty()).then(|| {
                format!(
                    "ADD PRIMARY KEY USING INDEX '{idx_name}' on table \
                     '{table}': column(s) [{cols}] are nullable. PostgreSQL \
                     will implicitly SET NOT NULL under ACCESS EXCLUSIVE \
                     lock. Run ALTER COLUMN ... SET NOT NULL with a CHECK \
                     constraint first.",
                    cols = nullable_cols.join(", "),
                )
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized};
    use rstest::rstest;

    #[test]
    fn test_add_pk_using_index_looks_up_the_index_in_the_tables_schema() {
        let before = catalog_from_sql(
            "CREATE TABLE analytics.orders (id int, email text NOT NULL);
             CREATE UNIQUE INDEX idx_email ON analytics.orders (email);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE analytics.orders ADD CONSTRAINT con_email PRIMARY KEY USING INDEX idx_email;",
        ));

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[rstest]
    #[case::index_in_catalog_before(
        "CREATE TABLE customers (id int NOT NULL);
         CREATE UNIQUE INDEX idx_customers_id ON customers (id);
         CREATE TABLE orders (id int);",
        ""
    )]
    #[case::index_created_in_the_same_unit(
        "CREATE TABLE customers (id int NOT NULL);
         CREATE TABLE orders (id int);",
        "CREATE UNIQUE INDEX idx_customers_id ON customers (id);"
    )]
    #[case::index_name_moved_to_another_table_earlier_in_the_unit(
        "CREATE TABLE customers (id int NOT NULL);
         CREATE TABLE orders (id int);
         CREATE INDEX idx_customers_id ON orders (id);",
        "DROP INDEX idx_customers_id;
         CREATE UNIQUE INDEX idx_customers_id ON customers (id);"
    )]
    fn test_add_pk_using_index_of_another_tables_index_is_left_to_pgm027(
        #[case] history: &str,
        #[case] earlier_in_unit: &str,
    ) {
        let change = format!(
            "{earlier_in_unit}\nALTER TABLE orders ADD PRIMARY KEY USING INDEX idx_customers_id;"
        );
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let findings = RuleId::Pgm016.check(&normalized(parse_sql(&change)), &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn test_add_pk_using_index_of_a_same_named_index_in_another_schema_fires() {
        let before = catalog_from_sql(
            "CREATE TABLE analytics.orders (id int, email text NOT NULL);
             CREATE UNIQUE INDEX idx_email ON analytics.orders (email);
             CREATE TABLE data.orders (id int, email text NOT NULL);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE data.orders ADD CONSTRAINT con_email PRIMARY KEY USING INDEX idx_email;",
        ));

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "got {findings:?}");
        assert_eq!(
            findings[0].message,
            "ADD PRIMARY KEY USING INDEX 'idx_email' on table 'data.orders': \
             referenced index does not exist."
        );
    }

    fn add_pk_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: columns.iter().map(|s| s.to_string()).collect(),
                    include_columns: vec![],
                    using_index: None,
                },
            )],
        }))
    }

    fn add_pk_using_index_stmt(table: &str, idx_name: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::add_constraint(
                TableConstraint::PrimaryKey {
                    name: None,
                    columns: vec![], // empty with USING INDEX
                    using_index: Some(idx_name.to_string()),
                    include_columns: vec![],
                },
            )],
        }))
    }

    #[test]
    fn test_add_pk_no_unique_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_with_unique_constraint_still_fires() {
        // Even with a pre-existing UNIQUE constraint, without USING INDEX
        // PostgreSQL builds a NEW index under ACCESS EXCLUSIVE lock.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .unique("uq_orders_id", &["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_pk_with_unique_index_still_fires() {
        // Even with a pre-existing unique index, without USING INDEX
        // PostgreSQL builds a NEW index.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_stmt("nonexistent", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_id", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_stmt("orders", &["id"])];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_with_backing_unique_index_not_null_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_using_index_non_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .index("idx_orders_pk", &["id"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_no_backing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_nonexistent")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_pk_using_index_nullable_columns_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", true) // nullable!
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    const ORDERS: &str = "CREATE TABLE orders (id bigint NOT NULL);";

    fn findings_for_change(history: &str, change: &str, run_in_transaction: bool) -> Vec<Finding> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: run_in_transaction);
        RuleId::Pgm016.check(&normalized(parse_sql(change)), &ctx)
    }

    fn messages_for_change(history: &str, change: &str) -> Vec<String> {
        findings_for_change(history, change, true)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    #[rstest]
    #[case::named_index(true, "CREATE UNIQUE INDEX orders_id_idx ON orders (id);")]
    #[case::unnamed_index_built_concurrently(
        false,
        "CREATE UNIQUE INDEX CONCURRENTLY ON orders (id);"
    )]
    fn test_add_pk_using_an_index_created_earlier_in_the_unit_no_finding(
        #[case] run_in_transaction: bool,
        #[case] create_index: &str,
    ) {
        let change = format!(
            "{create_index}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;"
        );

        let findings = findings_for_change(ORDERS, &change, run_in_transaction);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn test_add_pk_using_an_index_from_an_earlier_unit_no_finding() {
        let history = format!("{ORDERS}\nCREATE UNIQUE INDEX orders_id_idx ON orders (id);");

        let messages = messages_for_change(
            &history,
            "ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::dropped_earlier_in_the_unit(
        "CREATE UNIQUE INDEX orders_id_idx ON orders (id);",
        "DROP INDEX orders_id_idx;"
    )]
    #[case::never_created("", "")]
    fn test_add_pk_using_an_index_missing_at_that_point_fires(
        #[case] earlier_units: &str,
        #[case] earlier_in_unit: &str,
    ) {
        let history = format!("{ORDERS}\n{earlier_units}");
        let change = format!(
            "{earlier_in_unit}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;"
        );

        assert_eq!(
            messages_for_change(&history, &change),
            [
                "ADD PRIMARY KEY USING INDEX 'orders_id_idx' on table 'orders': \
              referenced index does not exist."
            ]
        );
    }

    #[test]
    fn test_add_pk_using_index_after_set_not_null_earlier_in_the_unit_no_finding() {
        let messages = messages_for_change(
            "CREATE TABLE orders (id bigint);
             CREATE UNIQUE INDEX orders_id_idx ON orders (id);",
            "ALTER TABLE orders ALTER COLUMN id SET NOT NULL;
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn test_add_pk_using_index_after_drop_not_null_earlier_in_the_unit_fires() {
        let history = format!("{ORDERS}\nCREATE UNIQUE INDEX orders_id_idx ON orders (id);");

        let messages = messages_for_change(
            &history,
            "ALTER TABLE orders ALTER COLUMN id DROP NOT NULL;
             ALTER TABLE orders ADD CONSTRAINT orders_pkey PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert_eq!(messages.len(), 1, "got {messages:?}");
        assert!(
            messages[0].contains("column(s) [id] are nullable"),
            "got {messages:?}"
        );
    }

    #[test]
    fn test_add_pk_using_index_multiple_columns_not_null_no_finding() {
        // Multi-column PK with all NOT NULL columns
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("order_id", "bigint", false)
                    .column("item_id", "bigint", false)
                    .index("idx_order_items_pk", &["order_id", "item_id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("order_items", "idx_order_items_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_pk_using_index_column_made_nullable_by_drop_not_null_fires() {
        // Column was NOT NULL originally but became nullable via DROP NOT NULL
        // in an earlier migration. catalog_before reflects nullable column,
        // so ADD PK USING INDEX should warn about implicit SET NOT NULL.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", true) // nullable after DROP NOT NULL
                    .index("idx_orders_pk", &["id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/003.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(
            findings.len(),
            1,
            "Should fire when column became nullable via DROP NOT NULL"
        );
        assert!(findings[0].message.contains("nullable"));
    }

    #[test]
    fn test_add_pk_using_index_non_btree_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false).index_with_method(
                    "idx_orders_pk",
                    &["id"],
                    true,
                    "hash",
                );
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("orders", "idx_orders_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("hash"));
        assert!(findings[0].message.contains("btree"));
    }

    #[test]
    fn test_add_pk_using_index_one_nullable_one_not_fires() {
        // Multi-column index where one column is nullable
        let before = CatalogBuilder::new()
            .table("order_items", |t| {
                t.column("order_id", "bigint", false)
                    .column("item_id", "bigint", true) // nullable
                    .index("idx_order_items_pk", &["order_id", "item_id"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_pk_using_index_stmt("order_items", "idx_order_items_pk")];

        let findings = RuleId::Pgm016.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("nullable"));
    }
}
