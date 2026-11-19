#![doc = include_str!("docs/pgm017.md")]

use crate::catalog::Catalog;
use crate::catalog::replay::map_with_state_before_if_needed;
use crate::parser::ir::{AlterTable, AlterTableAction, IrNode, Located, TableConstraint};
use crate::rules::using_index::{UsingIndex, has_using_index};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "ADD UNIQUE on existing table without USING INDEX";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm017.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

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
                TableConstraint::Unique {
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
                "ADD UNIQUE on existing table '{table}' without USING INDEX \
                 on column(s) [{columns}]. Create a unique index CONCURRENTLY \
                 first, then use ADD CONSTRAINT ... UNIQUE USING INDEX.",
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

/// The message for `ALTER TABLE at ADD UNIQUE USING INDEX idx_name`, where
/// `target` is what `idx_name` refers to just before the statement. `None`
/// when there is nothing to report: attaching the index takes no long lock,
/// or PostgreSQL rejects the statement and PGM027 reports it.
fn using_index_message(at: &AlterTable, idx_name: &str, target: UsingIndex<'_>) -> Option<String> {
    let table = at.name.display_name();
    match target {
        // Only while PGM027 judges the unit. Postgres rejects the statement
        // and builds nothing, so there is no lock to report; PGM027 reports it.
        UsingIndex::OfAnotherTable { .. } => None,
        UsingIndex::Missing => Some(format!(
            "ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': \
             referenced index does not exist."
        )),
        UsingIndex::OfTable { index, .. } if !index.unique => Some(format!(
            "ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': \
             referenced index is not UNIQUE."
        )),
        UsingIndex::OfTable { index, .. } if !index.is_btree() => Some(format!(
            "ADD UNIQUE USING INDEX '{idx_name}' on table '{table}': \
             referenced index uses access method '{}', but only btree \
             indexes can back a UNIQUE constraint.",
            index.access_method,
        )),
        UsingIndex::OfTable { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::{CatalogAssertions, CatalogBuilder};
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized};
    use rstest::rstest;

    #[test]
    fn test_add_unique_using_index_looks_up_the_index_in_the_tables_schema() {
        let before = catalog_from_sql(
            "CREATE TABLE analytics.orders (id int, email text NOT NULL);
             CREATE UNIQUE INDEX idx_email ON analytics.orders (email);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE analytics.orders ADD CONSTRAINT con_email UNIQUE USING INDEX idx_email;",
        ));

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[rstest]
    #[case::index_in_catalog_before(
        "CREATE TABLE customers (email text);
         CREATE INDEX idx_customers_email ON customers (email);
         CREATE TABLE orders (email text);",
        ""
    )]
    #[case::index_created_in_the_same_unit(
        "CREATE TABLE customers (email text);
         CREATE TABLE orders (email text);",
        "CREATE INDEX idx_customers_email ON customers (email);"
    )]
    #[case::index_name_moved_to_another_table_earlier_in_the_unit(
        "CREATE TABLE customers (email text);
         CREATE TABLE orders (email text);
         CREATE INDEX idx_customers_email ON orders (email);",
        "DROP INDEX idx_customers_email;
         CREATE UNIQUE INDEX idx_customers_email ON customers (email);"
    )]
    fn test_add_unique_using_index_of_another_tables_index_is_left_to_pgm027(
        #[case] history: &str,
        #[case] earlier_in_unit: &str,
    ) {
        let change = format!(
            "{earlier_in_unit}\nALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX idx_customers_email;"
        );
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let findings = RuleId::Pgm017.check(&normalized(parse_sql(&change)), &ctx);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn test_add_unique_using_index_of_a_same_named_index_in_another_schema_fires() {
        let before = catalog_from_sql(
            "CREATE TABLE analytics.orders (id int, email text NOT NULL);
             CREATE UNIQUE INDEX idx_email ON analytics.orders (email);
             CREATE TABLE data.orders (id int, email text NOT NULL);",
        );
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = normalized(parse_sql(
            "ALTER TABLE data.orders ADD CONSTRAINT con_email UNIQUE USING INDEX idx_email;",
        ));

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1, "got {findings:?}");
        assert_eq!(
            findings[0].message,
            "ADD UNIQUE USING INDEX 'idx_email' on table 'data.orders': \
             referenced index does not exist."
        );
    }

    fn add_unique_stmt(table: &str, columns: &[&str]) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::add_constraint(TableConstraint::Unique {
                name: Some(format!("uq_{}", columns.join("_"))),
                columns: columns.iter().map(|s| s.to_string()).collect(),
                include_columns: vec![],
                using_index: None,
            })],
        }))
    }

    fn add_unique_using_index_stmt(table: &str, idx_name: &str) -> Located<IrNode> {
        located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified(table),
            actions: vec![AlterTableAction::add_constraint(TableConstraint::Unique {
                name: Some(format!("uq_{}", idx_name)),
                columns: vec![], // empty with USING INDEX
                using_index: Some(idx_name.to_string()),
                include_columns: vec![],
            })],
        }))
    }

    #[test]
    fn test_add_unique_no_existing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_index_still_fires() {
        // Even with a pre-existing unique index, without USING INDEX PostgreSQL
        // builds a NEW index under ACCESS EXCLUSIVE lock.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_unique_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_stmt("nonexistent", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_non_unique_index_still_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_with_existing_unique_constraint_still_fires() {
        // Even with a pre-existing UNIQUE constraint, without USING INDEX
        // PostgreSQL builds a NEW index.
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .unique("uq_orders_email", &["email"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_stmt("orders", &["email"])];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn test_add_unique_using_index_with_backing_unique_index_no_finding() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_add_unique_using_index_non_unique_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index("idx_orders_email", &["email"], false); // NOT unique
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_using_index_no_backing_index_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_nonexistent")];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_add_unique_using_index_non_btree_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .index_with_method("idx_orders_email", &["email"], true, "hash");
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_using_index_stmt("orders", "idx_orders_email")];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert_eq!(findings.len(), 1);
        assert!(findings[0].message.contains("hash"));
        assert!(findings[0].message.contains("btree"));
    }

    const ORDERS: &str = "CREATE TABLE orders (id bigint NOT NULL, email text NOT NULL);";

    fn findings_for_change(history: &str, change: &str, run_in_transaction: bool) -> Vec<Finding> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: run_in_transaction);
        RuleId::Pgm017.check(&normalized(parse_sql(change)), &ctx)
    }

    #[rstest]
    #[case::unnamed_index(true, "CREATE UNIQUE INDEX ON orders (email);")]
    #[case::unnamed_index_built_concurrently(
        false,
        "CREATE UNIQUE INDEX CONCURRENTLY ON orders (email);"
    )]
    fn test_add_unique_using_an_index_created_earlier_in_the_unit_no_finding(
        #[case] run_in_transaction: bool,
        #[case] create_index: &str,
    ) {
        let change = format!(
            "{create_index}\n\
             ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX orders_email_idx;"
        );
        let after = catalog_from_sql(&format!("{ORDERS}\n{change}"));
        after.assert_index_unregistered("orders_email_idx");
        after.assert_index_owner("uq_email", "orders");

        let findings = findings_for_change(ORDERS, &change, run_in_transaction);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[test]
    fn test_add_unique_using_an_index_from_an_earlier_unit_no_finding() {
        let history = format!("{ORDERS}\nCREATE UNIQUE INDEX orders_email_idx ON orders (email);");

        let findings = findings_for_change(
            &history,
            "ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX orders_email_idx;",
            true,
        );
        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[rstest]
    #[case::dropped_earlier_in_the_unit(
        "CREATE UNIQUE INDEX orders_email_idx ON orders (email);",
        "DROP INDEX orders_email_idx;"
    )]
    #[case::created_and_dropped_earlier_in_the_unit(
        "",
        "CREATE UNIQUE INDEX orders_email_idx ON orders (email);\nDROP INDEX orders_email_idx;"
    )]
    #[case::never_created("", "")]
    fn test_add_unique_using_an_index_missing_at_that_point_fires(
        #[case] earlier_units: &str,
        #[case] earlier_in_unit: &str,
    ) {
        let history = format!("{ORDERS}\n{earlier_units}");
        let change = format!(
            "{earlier_in_unit}\n\
             ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX orders_email_idx;"
        );

        let messages: Vec<String> = findings_for_change(&history, &change, true)
            .into_iter()
            .map(|finding| finding.message)
            .collect();
        assert_eq!(
            messages,
            [
                "ADD UNIQUE USING INDEX 'orders_email_idx' on table 'orders': \
              referenced index does not exist."
            ]
        );
    }

    #[test]
    fn test_add_unique_using_a_non_unique_index_created_earlier_in_the_unit_fires() {
        let messages: Vec<String> = findings_for_change(
            ORDERS,
            "CREATE INDEX orders_email_idx ON orders (email);
             ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX orders_email_idx;",
            true,
        )
        .into_iter()
        .map(|finding| finding.message)
        .collect();
        assert_eq!(
            messages,
            [
                "ADD UNIQUE USING INDEX 'orders_email_idx' on table 'orders': \
              referenced index is not UNIQUE."
            ]
        );
    }

    #[test]
    fn test_each_using_index_sees_the_index_as_the_earlier_statements_of_the_unit_leave_it() {
        let findings = findings_for_change(
            ORDERS,
            "CREATE UNIQUE INDEX orders_email_idx ON orders (email);\n\
             ALTER TABLE orders ADD CONSTRAINT uq_email UNIQUE USING INDEX orders_email_idx;\n\
             ALTER TABLE orders ADD CONSTRAINT uq_email_again UNIQUE USING INDEX orders_email_idx;",
            true,
        );
        let lines: Vec<usize> = findings.iter().map(|finding| finding.start_line).collect();
        assert_eq!(lines, [3], "got {findings:?}");
        assert!(
            findings[0]
                .message
                .contains("referenced index does not exist")
        );
    }

    #[test]
    fn test_add_unique_using_index_multi_column_no_finding() {
        // Multi-column unique index, can still use USING INDEX
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "bigint", false)
                    .column("email", "text", false)
                    .column("domain", "text", false)
                    .index("idx_orders_email_domain", &["email", "domain"], true);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![add_unique_using_index_stmt(
            "orders",
            "idx_orders_email_domain",
        )];

        let findings = RuleId::Pgm017.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }
}
