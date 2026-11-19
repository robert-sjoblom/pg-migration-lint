#![doc = include_str!("docs/pgm013.md")]

use crate::catalog::Catalog;
use crate::catalog::replay::map_with_state_before_if_needed;
use crate::parser::ir::{AlterTableAction, IrNode, Located};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str =
    "SET NOT NULL on existing table requires ACCESS EXCLUSIVE lock";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm013.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Critical;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    map_with_state_before_if_needed(
        ctx.catalog_before,
        statements,
        sets_not_null,
        |state, stmt| statement_findings(&rule, state, stmt, ctx),
    )
    .into_iter()
    .flatten()
    .collect()
}

/// True for an `ALTER TABLE` with a `SET NOT NULL` action, so a unit without
/// one needs no replay: [`statement_findings`] reports nothing else.
fn sets_not_null(node: &IrNode) -> bool {
    matches!(
        node,
        IrNode::AlterTable(at)
            if at
                .actions
                .iter()
                .any(|action| matches!(action, AlterTableAction::SetNotNull { .. }))
    )
}

/// The findings for `stmt`, where `state` is the catalog just before it. A
/// valid CHECK that proves the column is not null, from an earlier migration
/// or from earlier in this one, lets PostgreSQL skip the scan. A CHECK the
/// statement itself drops does not: PostgreSQL runs every `DROP CONSTRAINT`
/// of an `ALTER TABLE` before its `SET NOT NULL`, whatever the written order.
fn statement_findings(
    rule: &impl Rule,
    state: &Catalog,
    stmt: &Located<IrNode>,
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let IrNode::AlterTable(at) = &stmt.node else {
        return Vec::new();
    };
    if !ctx.is_existing_table(at.name.relation()) {
        return Vec::new();
    }
    let table = state.get_table(at.name.relation());
    let dropped_constraints: Vec<&str> = at
        .actions
        .iter()
        .filter_map(|action| match action {
            AlterTableAction::DropConstraint { constraint_name } => Some(constraint_name.as_str()),
            _ => None,
        })
        .collect();
    let proven_not_null = |column: &str| {
        table.is_some_and(|t| {
            t.valid_checks_proving_not_null(column)
                .any(|check| !dropped_constraints.contains(&check))
        })
    };
    at.actions
        .iter()
        .filter_map(|action| match action {
            AlterTableAction::SetNotNull { column_name } if !proven_not_null(column_name) => {
                Some(rule.make_finding(
                    format!(
                        "SET NOT NULL on column '{col}' of existing table '{table}' \
                         requires an ACCESS EXCLUSIVE lock and full table scan. \
                         Use a CHECK constraint with NOT VALID, validate it, \
                         then set NOT NULL.",
                        col = column_name,
                        table = at.name.display_name(),
                    ),
                    ctx.file,
                    &stmt.span,
                ))
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use crate::catalog::builder::CatalogBuilder;
    use crate::parser::ir::*;
    use crate::parser::pg_query::parse_sql;
    use crate::rules::RuleId;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, located, normalized, replayed};
    use rstest::rstest;

    const STATUS_SET_NOT_NULL_MESSAGE: &str = "SET NOT NULL on column 'status' of existing table \
        'orders' requires an ACCESS EXCLUSIVE lock and full table scan. Use a CHECK constraint \
        with NOT VALID, validate it, then set NOT NULL.";

    #[rstest]
    #[case::unnamed("ALTER TABLE orders ADD NOT NULL status;", &[STATUS_SET_NOT_NULL_MESSAGE])]
    #[case::named(
        "ALTER TABLE orders ADD CONSTRAINT orders_status_nn NOT NULL status;",
        &[STATUS_SET_NOT_NULL_MESSAGE]
    )]
    #[case::not_valid(
        "ALTER TABLE orders ADD CONSTRAINT orders_status_nn NOT NULL status NOT VALID;",
        &[]
    )]
    fn add_not_null_table_constraint_on_existing_table_is_set_not_null_unless_not_valid(
        #[case] sql: &str,
        #[case] expected: &[&str],
    ) {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let stmts = normalized(parse_sql(sql));
        let after = replayed(&before, &stmts);
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let messages: Vec<String> = RuleId::Pgm013
            .check(&stmts, &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect();
        assert_eq!(messages, expected);
    }

    #[test]
    fn test_set_not_null_on_existing_table_fires() {
        let before = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
        insta::assert_yaml_snapshot!(findings);
    }

    #[test]
    fn test_set_not_null_on_new_table_no_finding() {
        let before = Catalog::new();
        let after = CatalogBuilder::new()
            .table("orders", |t| {
                t.column("id", "integer", false)
                    .column("status", "text", true)
                    .pk(&["id"]);
            })
            .build();
        lint_ctx!(ctx, &before, &after, "migrations/001.sql", created: ["orders"]);

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    #[test]
    fn test_set_not_null_table_not_in_catalog_no_finding() {
        let before = Catalog::new();
        let after = Catalog::new();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");

        let stmts = vec![located(IrNode::AlterTable(AlterTable {
            name: QualifiedName::unqualified("orders"),
            actions: vec![AlterTableAction::SetNotNull {
                column_name: "status".to_string(),
            }],
        }))];

        let findings = RuleId::Pgm013.check(&stmts, &ctx);
        assert!(findings.is_empty());
    }

    const ORDERS: &str =
        "CREATE TABLE orders (id bigint PRIMARY KEY, status text, region text, \"Note\" text);";
    const ADD_STATUS_CHECK_NOT_VALID: &str = "ALTER TABLE orders ADD CONSTRAINT orders_status_nn \
         CHECK (status IS NOT NULL) NOT VALID;";
    const VALIDATE_STATUS_CHECK: &str = "ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn;";
    const SET_STATUS_NOT_NULL: &str = "ALTER TABLE orders ALTER COLUMN status SET NOT NULL;";
    const INVOICES_WITH_VALID_STATUS_CHECK: &str = "
        CREATE TABLE invoices (id bigint PRIMARY KEY, status text);
        ALTER TABLE invoices ADD CONSTRAINT invoices_status_nn
            CHECK (status IS NOT NULL) NOT VALID;
        ALTER TABLE invoices VALIDATE CONSTRAINT invoices_status_nn;";
    const AUDIT_ORDERS_WITH_VALID_STATUS_CHECK: &str = "
        CREATE SCHEMA audit;
        CREATE TABLE audit.orders (id bigint PRIMARY KEY, status text);
        ALTER TABLE audit.orders ADD CONSTRAINT orders_status_nn
            CHECK (status IS NOT NULL) NOT VALID;
        ALTER TABLE audit.orders VALIDATE CONSTRAINT orders_status_nn;";

    fn findings_for_change(history: &str, change: &str, run_in_transaction: bool) -> Vec<Finding> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql", txn: run_in_transaction);
        RuleId::Pgm013.check(&normalized(parse_sql(change)), &ctx)
    }

    #[rstest]
    fn test_set_not_null_after_check_validated_earlier_in_the_unit_no_finding(
        #[values(true, false)] run_in_transaction: bool,
    ) {
        let change =
            format!("{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n{SET_STATUS_NOT_NULL}");

        let findings = findings_for_change(ORDERS, &change, run_in_transaction);

        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[rstest]
    #[case::validated_in_an_earlier_unit(
        &format!("{ORDERS}\n{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}"),
        SET_STATUS_NOT_NULL.to_string(),
    )]
    #[case::added_in_an_earlier_unit_and_validated_in_this_one(
        &format!("{ORDERS}\n{ADD_STATUS_CHECK_NOT_VALID}"),
        format!("{VALIDATE_STATUS_CHECK}\n{SET_STATUS_NOT_NULL}"),
    )]
    #[case::added_valid_earlier_in_the_unit(
        ORDERS,
        format!(
            "ALTER TABLE orders ADD CONSTRAINT orders_status_nn CHECK (status IS NOT NULL);\n\
             {SET_STATUS_NOT_NULL}"
        ),
    )]
    #[case::column_renamed_after_the_check_was_validated(
        &format!("{ORDERS}\n{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}"),
        "ALTER TABLE orders RENAME COLUMN status TO state;\n\
         ALTER TABLE orders ALTER COLUMN state SET NOT NULL;"
            .to_string(),
    )]
    #[case::quoted_column_name(
        ORDERS,
        "ALTER TABLE orders ADD CONSTRAINT orders_note_nn CHECK (\"Note\" IS NOT NULL) NOT VALID;\n\
         ALTER TABLE orders VALIDATE CONSTRAINT orders_note_nn;\n\
         ALTER TABLE orders ALTER COLUMN \"Note\" SET NOT NULL;"
            .to_string(),
    )]
    #[case::redundantly_parenthesized_check(
        ORDERS,
        format!(
            "ALTER TABLE orders ADD CONSTRAINT orders_status_nn \
             CHECK ((status IS NOT NULL)) NOT VALID;\n\
             {VALIDATE_STATUS_CHECK}\n{SET_STATUS_NOT_NULL}"
        ),
    )]
    #[case::postfix_notnull_check(
        ORDERS,
        format!(
            "ALTER TABLE orders ADD CONSTRAINT orders_status_nn CHECK (status NOTNULL) NOT VALID;\n\
             {VALIDATE_STATUS_CHECK}\n{SET_STATUS_NOT_NULL}"
        ),
    )]
    #[case::schema_qualified_table(
        AUDIT_ORDERS_WITH_VALID_STATUS_CHECK,
        "ALTER TABLE audit.orders ALTER COLUMN status SET NOT NULL;".to_string(),
    )]
    #[case::statement_drops_another_constraint(
        &format!(
            "{ORDERS}\n{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_region_nn CHECK (region <> '');"
        ),
        "ALTER TABLE orders ALTER COLUMN status SET NOT NULL, DROP CONSTRAINT orders_region_nn;"
            .to_string(),
    )]
    fn test_set_not_null_proven_by_a_valid_check_no_finding(
        #[case] history: &str,
        #[case] change: String,
    ) {
        let findings = findings_for_change(history, &change, true);

        assert!(findings.is_empty(), "got {findings:?}");
    }

    #[rstest]
    #[case::check_left_not_valid(
        "check_left_not_valid",
        ORDERS,
        format!("{ADD_STATUS_CHECK_NOT_VALID}\n{SET_STATUS_NOT_NULL}"),
    )]
    #[case::validated_check_on_another_column(
        "validated_check_on_another_column",
        ORDERS,
        format!(
            "ALTER TABLE orders ADD CONSTRAINT orders_region_nn \
             CHECK (region IS NOT NULL) NOT VALID;\n\
             ALTER TABLE orders VALIDATE CONSTRAINT orders_region_nn;\n\
             {SET_STATUS_NOT_NULL}"
        ),
    )]
    #[case::validated_check_on_another_table(
        "validated_check_on_another_table",
        &format!("{ORDERS}\n{INVOICES_WITH_VALID_STATUS_CHECK}"),
        SET_STATUS_NOT_NULL.to_string(),
    )]
    #[case::validated_check_on_a_table_of_the_same_name_in_another_schema(
        "validated_check_on_a_table_of_the_same_name_in_another_schema",
        &format!("{ORDERS}\n{AUDIT_ORDERS_WITH_VALID_STATUS_CHECK}"),
        SET_STATUS_NOT_NULL.to_string(),
    )]
    #[case::validated_check_dropped_before(
        "validated_check_dropped_before",
        ORDERS,
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders DROP CONSTRAINT orders_status_nn;\n\
             {SET_STATUS_NOT_NULL}"
        ),
    )]
    #[case::validated_check_dropped_by_the_statement_after_set_not_null(
        "validated_check_dropped_by_the_statement_after_set_not_null",
        ORDERS,
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders ALTER COLUMN status SET NOT NULL, \
             DROP CONSTRAINT orders_status_nn;"
        ),
    )]
    #[case::validated_check_dropped_by_the_statement_before_set_not_null(
        "validated_check_dropped_by_the_statement_before_set_not_null",
        ORDERS,
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders DROP CONSTRAINT orders_status_nn, \
             ALTER COLUMN status SET NOT NULL;"
        ),
    )]
    #[case::check_validated_by_the_statement(
        "check_validated_by_the_statement",
        ORDERS,
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n\
             ALTER TABLE orders VALIDATE CONSTRAINT orders_status_nn, \
             ALTER COLUMN status SET NOT NULL;"
        ),
    )]
    #[case::check_added_by_the_statement(
        "check_added_by_the_statement",
        ORDERS,
        "ALTER TABLE orders ADD CONSTRAINT orders_status_nn CHECK (status IS NOT NULL), \
         ALTER COLUMN status SET NOT NULL;"
            .to_string(),
    )]
    #[case::validated_check_with_further_conditions(
        "validated_check_with_further_conditions",
        ORDERS,
        format!(
            "ALTER TABLE orders ADD CONSTRAINT orders_status_nn \
             CHECK (status IS NOT NULL AND region IS NOT NULL) NOT VALID;\n\
             {VALIDATE_STATUS_CHECK}\n{SET_STATUS_NOT_NULL}"
        ),
    )]
    fn test_set_not_null_without_a_valid_check_proving_it_fires(
        #[case] name: &str,
        #[case] history: &str,
        #[case] change: String,
    ) {
        let findings = findings_for_change(history, &change, true);

        insta::assert_yaml_snapshot!(format!("fires_{name}"), findings);
    }

    const ADD_STATUS_NOT_NULL: &str = "ALTER TABLE orders ADD NOT NULL status;";

    #[rstest]
    #[case::unnamed_after_check_validated_earlier_in_the_unit(
        format!("{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n{ADD_STATUS_NOT_NULL}"),
        &[],
    )]
    #[case::named_after_check_validated_earlier_in_the_unit(
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_status_not_null NOT NULL status;"
        ),
        &[],
    )]
    #[case::after_check_left_not_valid(
        format!("{ADD_STATUS_CHECK_NOT_VALID}\n{ADD_STATUS_NOT_NULL}"),
        &[STATUS_SET_NOT_NULL_MESSAGE],
    )]
    #[case::statement_drops_the_validated_check(
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders ADD NOT NULL status, DROP CONSTRAINT orders_status_nn;"
        ),
        &[STATUS_SET_NOT_NULL_MESSAGE],
    )]
    #[case::not_valid_after_check_left_not_valid(
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_status_not_null NOT NULL status NOT VALID;"
        ),
        &[],
    )]
    #[case::not_valid_after_check_validated_earlier_in_the_unit(
        format!(
            "{ADD_STATUS_CHECK_NOT_VALID}\n{VALIDATE_STATUS_CHECK}\n\
             ALTER TABLE orders ADD CONSTRAINT orders_status_not_null NOT NULL status NOT VALID;"
        ),
        &[],
    )]
    fn add_not_null_table_constraint_is_reported_unless_a_valid_check_proves_it_or_it_is_not_valid(
        #[case] change: String,
        #[case] expected: &[&str],
    ) {
        let messages: Vec<String> = findings_for_change(ORDERS, &change, true)
            .into_iter()
            .map(|finding| finding.message)
            .collect();

        assert_eq!(messages, expected);
    }
}
