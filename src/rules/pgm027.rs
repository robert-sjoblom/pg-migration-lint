#![doc = include_str!("docs/pgm027.md")]

use crate::parser::ir::{IrNode, Located, SourceSpan};
use crate::rules::rejected_statements::{Rejection, Renamed, TakenBy, rejections};
use crate::rules::{Finding, LintContext, Rule, Severity};

pub(super) const DESCRIPTION: &str = "Statement fails against the schema from earlier migrations";

pub(super) const EXPLAIN: &str = include_str!("docs/pgm027.md");

pub(super) const DEFAULT_SEVERITY: Severity = Severity::Major;

pub(super) fn check(
    rule: impl Rule,
    statements: &[Located<IrNode>],
    ctx: &LintContext<'_>,
) -> Vec<Finding> {
    let mut findings = Vec::new();
    // `DROP INDEX a, b` is one statement split into one node per index, all
    // with the statement's span. Postgres stops at the first failure.
    let mut last_reported_span: Option<&SourceSpan> = None;
    for (stmt, rejection) in statements
        .iter()
        .zip(rejections(ctx.catalog_before, statements))
    {
        let Some(rejection) = rejection else {
            continue;
        };
        if last_reported_span == Some(&stmt.span) {
            continue;
        }
        last_reported_span = Some(&stmt.span);
        findings.push(rule.make_finding(message(&rejection), ctx.file, &stmt.span));
    }
    findings
}

fn message(rejection: &Rejection) -> String {
    match rejection {
        Rejection::UsingIndexOfAnotherTable {
            constraint_kind,
            index,
            table,
            owner,
        } => format!(
            "ADD {constraint_kind} USING INDEX '{index}' on table '{table}' fails: \
             the index belongs to table '{owner}'."
        ),
        Rejection::RenameOntoTakenName {
            renamed,
            new_name,
            taken_by,
        } => {
            let statement = match renamed {
                Renamed::Table { table } => format!("Renaming table '{table}' to '{new_name}'"),
                Renamed::Index { index } => format!("Renaming index '{index}' to '{new_name}'"),
                Renamed::Constraint { constraint, table } => {
                    format!("Renaming constraint '{constraint}' on table '{table}' to '{new_name}'")
                }
            };
            let renamed_along = match (renamed, taken_by) {
                (Renamed::Index { .. }, TakenBy::Constraint { .. }) => {
                    "the index backs a constraint that is renamed with it, and "
                }
                (Renamed::Constraint { .. }, TakenBy::Table { .. } | TakenBy::Index { .. }) => {
                    "the constraint's index is renamed with it, and "
                }
                _ => "",
            };
            let holder = match taken_by {
                TakenBy::Table { table } => format!("table '{table}' already has that name"),
                TakenBy::Index { table } => {
                    format!("index '{new_name}' on table '{table}' already has that name")
                }
                TakenBy::Constraint { table } => {
                    format!("table '{table}' already has a constraint named '{new_name}'")
                }
            };
            format!("{statement} fails: {renamed_along}{holder}.")
        }
        Rejection::DropIndexOfConstraint {
            index,
            constraint,
            table,
        } => format!(
            "DROP INDEX '{index}' fails: constraint '{constraint}' on table '{table}' \
             requires it. Drop the constraint instead."
        ),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use crate::parser::pg_query::parse_sql;
    use crate::rules::test_helpers::{catalog_from_sql, lint_ctx, normalized};
    use crate::rules::{Rule, RuleId};

    fn messages_for_change(history: &str, change: &str) -> Vec<String> {
        let before = catalog_from_sql(history);
        let after = catalog_from_sql(&format!("{history}\n{change}"));
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");
        let stmts = normalized(parse_sql(change));
        RuleId::Pgm027
            .check(&stmts, &ctx)
            .into_iter()
            .map(|finding| finding.message)
            .collect()
    }

    const CUSTOMERS_AND_ORDERS: &str = "
        CREATE TABLE customers (id bigint PRIMARY KEY, email text NOT NULL);
        CREATE UNIQUE INDEX customers_email_idx ON customers (email);
        CREATE TABLE orders (id bigint NOT NULL, email text NOT NULL);
        CREATE UNIQUE INDEX orders_id_idx ON orders (id);
        CREATE UNIQUE INDEX orders_email_idx ON orders (email);";

    #[rstest]
    #[case::primary_key(
        "ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_email_idx;",
        "ADD PRIMARY KEY USING INDEX 'customers_email_idx' on table 'orders' fails: \
         the index belongs to table 'customers'."
    )]
    #[case::named_unique(
        "ALTER TABLE orders ADD CONSTRAINT orders_email_key UNIQUE USING INDEX customers_email_idx;",
        "ADD UNIQUE USING INDEX 'customers_email_idx' on table 'orders' fails: \
         the index belongs to table 'customers'."
    )]
    #[case::index_backing_another_tables_constraint(
        "ALTER TABLE orders ADD UNIQUE USING INDEX customers_pkey;",
        "ADD UNIQUE USING INDEX 'customers_pkey' on table 'orders' fails: \
         the index belongs to table 'customers'."
    )]
    fn using_index_of_another_table_fires(#[case] change: &str, #[case] expected: &str) {
        assert_eq!(
            messages_for_change(CUSTOMERS_AND_ORDERS, change),
            [expected]
        );
    }

    #[rstest]
    #[case::own_index("ALTER TABLE orders ADD PRIMARY KEY USING INDEX orders_id_idx;")]
    #[case::own_index_for_unique("ALTER TABLE orders ADD UNIQUE USING INDEX orders_email_idx;")]
    #[case::index_missing_from_the_catalog(
        "ALTER TABLE orders ADD PRIMARY KEY USING INDEX made_outside_migrations_idx;"
    )]
    #[case::without_using_index("ALTER TABLE orders ADD PRIMARY KEY (id);")]
    fn using_index_without_evidence_of_another_table_is_silent(#[case] change: &str) {
        let messages = messages_for_change(CUSTOMERS_AND_ORDERS, change);
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn using_index_looks_the_index_up_in_the_altered_tables_schema_only() {
        let history = "
            CREATE TABLE analytics.customers (id bigint, email text NOT NULL);
            CREATE UNIQUE INDEX customers_email_idx ON analytics.customers (email);
            CREATE TABLE orders (id bigint, email text NOT NULL);";

        let messages = messages_for_change(
            history,
            "ALTER TABLE orders ADD UNIQUE USING INDEX customers_email_idx;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn using_index_of_an_index_created_on_another_table_earlier_in_the_unit_fires() {
        let messages = messages_for_change(
            CUSTOMERS_AND_ORDERS,
            "CREATE UNIQUE INDEX customers_id_idx ON customers (id);
             ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_id_idx;",
        );
        assert_eq!(
            messages,
            [
                "ADD PRIMARY KEY USING INDEX 'customers_id_idx' on table 'orders' fails: \
              the index belongs to table 'customers'."
            ]
        );
    }

    #[test]
    fn using_index_of_an_index_created_on_the_table_earlier_in_the_unit_is_silent() {
        let messages = messages_for_change(
            "CREATE TABLE orders (id bigint NOT NULL);",
            "CREATE UNIQUE INDEX orders_id_idx ON orders (id);
             ALTER TABLE orders ADD CONSTRAINT orders_pk PRIMARY KEY USING INDEX orders_id_idx;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    const NEIGHBOURS_AND_CON: &str = "
        CREATE TABLE neighbour (
            n int,
            CONSTRAINT neighbour_check CHECK (n > 0),
            CONSTRAINT neighbour_key UNIQUE (n));
        CREATE INDEX neighbour_idx ON neighbour (n);
        CREATE TABLE other.remote (r int);
        CREATE INDEX remote_idx ON other.remote (r);
        CREATE TABLE con (
            a int,
            b int,
            CONSTRAINT con_pk PRIMARY KEY (a),
            CONSTRAINT con_uq UNIQUE (b),
            CONSTRAINT con_chk CHECK (a > 0),
            CONSTRAINT con_chk2 CHECK (b > 0));
        CREATE INDEX con_idx ON con (b);";

    #[rstest]
    #[case::table_onto_a_table(
        "ALTER TABLE con RENAME TO neighbour;",
        "Renaming table 'con' to 'neighbour' fails: table 'neighbour' already has that name."
    )]
    #[case::table_onto_an_index(
        "ALTER TABLE con RENAME TO neighbour_idx;",
        "Renaming table 'con' to 'neighbour_idx' fails: \
         index 'neighbour_idx' on table 'neighbour' already has that name."
    )]
    #[case::table_onto_a_constraints_index(
        "ALTER TABLE con RENAME TO neighbour_key;",
        "Renaming table 'con' to 'neighbour_key' fails: \
         index 'neighbour_key' on table 'neighbour' already has that name."
    )]
    #[case::table_onto_its_own_name(
        "ALTER TABLE con RENAME TO con;",
        "Renaming table 'con' to 'con' fails: table 'con' already has that name."
    )]
    #[case::index_onto_an_index(
        "ALTER INDEX con_idx RENAME TO neighbour_idx;",
        "Renaming index 'con_idx' to 'neighbour_idx' fails: \
         index 'neighbour_idx' on table 'neighbour' already has that name."
    )]
    #[case::index_onto_a_table(
        "ALTER INDEX con_idx RENAME TO neighbour;",
        "Renaming index 'con_idx' to 'neighbour' fails: table 'neighbour' already has that name."
    )]
    #[case::constraint_index_onto_a_constraint_of_its_table(
        "ALTER INDEX con_pk RENAME TO con_chk2;",
        "Renaming index 'con_pk' to 'con_chk2' fails: the index backs a constraint that is \
         renamed with it, and table 'con' already has a constraint named 'con_chk2'."
    )]
    #[case::check_onto_a_check_of_its_table(
        "ALTER TABLE con RENAME CONSTRAINT con_chk TO con_chk2;",
        "Renaming constraint 'con_chk' on table 'con' to 'con_chk2' fails: \
         table 'con' already has a constraint named 'con_chk2'."
    )]
    #[case::unique_onto_a_table(
        "ALTER TABLE con RENAME CONSTRAINT con_uq TO neighbour;",
        "Renaming constraint 'con_uq' on table 'con' to 'neighbour' fails: the constraint's \
         index is renamed with it, and table 'neighbour' already has that name."
    )]
    #[case::unique_onto_a_plain_index_of_its_table(
        "ALTER TABLE con RENAME CONSTRAINT con_uq TO con_idx;",
        "Renaming constraint 'con_uq' on table 'con' to 'con_idx' fails: the constraint's \
         index is renamed with it, and index 'con_idx' on table 'con' already has that name."
    )]
    #[case::unique_onto_a_check_of_its_table(
        "ALTER TABLE con RENAME CONSTRAINT con_uq TO con_chk;",
        "Renaming constraint 'con_uq' on table 'con' to 'con_chk' fails: \
         table 'con' already has a constraint named 'con_chk'."
    )]
    fn rename_onto_a_taken_name_fires(#[case] change: &str, #[case] expected: &str) {
        assert_eq!(messages_for_change(NEIGHBOURS_AND_CON, change), [expected]);
    }

    #[rstest]
    #[case::table_onto_a_constraint_name("ALTER TABLE con RENAME TO con_chk2;")]
    #[case::table_onto_a_table_name_in_another_schema("ALTER TABLE con RENAME TO remote;")]
    #[case::index_onto_a_relation_name_in_another_schema(
        "ALTER INDEX con_idx RENAME TO remote_idx;"
    )]
    #[case::plain_index_onto_a_constraint_of_its_table("ALTER INDEX con_idx RENAME TO con_chk2;")]
    #[case::constraint_index_onto_another_tables_constraint(
        "ALTER INDEX con_pk RENAME TO neighbour_check;"
    )]
    #[case::check_onto_another_tables_constraint(
        "ALTER TABLE con RENAME CONSTRAINT con_chk TO neighbour_check;"
    )]
    #[case::check_onto_an_index_name("ALTER TABLE con RENAME CONSTRAINT con_chk TO neighbour_idx;")]
    #[case::unique_onto_a_relation_name_in_another_schema(
        "ALTER TABLE con RENAME CONSTRAINT con_uq TO remote_idx;"
    )]
    #[case::missing_table("ALTER TABLE made_outside_migrations RENAME TO neighbour;")]
    #[case::missing_index("ALTER INDEX made_outside_migrations_idx RENAME TO neighbour;")]
    #[case::missing_constraint("ALTER TABLE con RENAME CONSTRAINT made_outside TO con_chk2;")]
    fn rename_onto_a_free_name_or_of_a_missing_object_is_silent(#[case] change: &str) {
        let messages = messages_for_change(NEIGHBOURS_AND_CON, change);
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::table_dropped("DROP TABLE neighbour;", "ALTER TABLE con RENAME TO neighbour;")]
    #[case::index_dropped(
        "DROP INDEX neighbour_idx;",
        "ALTER INDEX con_idx RENAME TO neighbour_idx;"
    )]
    #[case::table_renamed_away(
        "ALTER TABLE neighbour RENAME TO neighbour_old;",
        "ALTER TABLE con RENAME TO neighbour;"
    )]
    #[case::constraint_dropped(
        "ALTER TABLE con DROP CONSTRAINT con_chk2;",
        "ALTER TABLE con RENAME CONSTRAINT con_chk TO con_chk2;"
    )]
    fn rename_onto_a_name_an_earlier_statement_of_the_unit_freed_is_silent(
        #[case] freeing_statement: &str,
        #[case] rename: &str,
    ) {
        let messages = messages_for_change(
            NEIGHBOURS_AND_CON,
            &format!("{freeing_statement}\n{rename}"),
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[rstest]
    #[case::table_created(
        "CREATE TABLE con_archive (id int);",
        "ALTER TABLE con RENAME TO con_archive;",
        "Renaming table 'con' to 'con_archive' fails: table 'con_archive' already has that name."
    )]
    #[case::index_created(
        "CREATE INDEX con_a_idx ON con (a);",
        "ALTER INDEX con_idx RENAME TO con_a_idx;",
        "Renaming index 'con_idx' to 'con_a_idx' fails: \
         index 'con_a_idx' on table 'con' already has that name."
    )]
    #[case::constraint_added(
        "ALTER TABLE con ADD CONSTRAINT con_chk3 CHECK (b < 100);",
        "ALTER TABLE con RENAME CONSTRAINT con_chk TO con_chk3;",
        "Renaming constraint 'con_chk' on table 'con' to 'con_chk3' fails: \
         table 'con' already has a constraint named 'con_chk3'."
    )]
    #[case::table_renamed_onto_the_name(
        "ALTER TABLE neighbour RENAME TO con_archive;",
        "ALTER TABLE con RENAME TO con_archive;",
        "Renaming table 'con' to 'con_archive' fails: table 'con_archive' already has that name."
    )]
    fn rename_onto_a_name_an_earlier_statement_of_the_unit_took_fires(
        #[case] taking_statement: &str,
        #[case] rename: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            messages_for_change(NEIGHBOURS_AND_CON, &format!("{taking_statement}\n{rename}")),
            [expected]
        );
    }

    #[test]
    fn rename_judges_each_statement_against_the_renames_before_it() {
        let messages = messages_for_change(
            NEIGHBOURS_AND_CON,
            "ALTER TABLE con RENAME TO con_new;
             ALTER TABLE neighbour RENAME TO con;
             ALTER TABLE con RENAME TO con_new;",
        );
        assert_eq!(
            messages,
            ["Renaming table 'con' to 'con_new' fails: table 'con_new' already has that name."]
        );
    }

    #[rstest]
    #[case::taken_in_the_same_schema("analytics", 1)]
    #[case::taken_in_another_schema("data", 0)]
    fn rename_table_looks_the_new_name_up_in_the_tables_schema(
        #[case] schema_with_orders_archive: &str,
        #[case] expected_findings: usize,
    ) {
        let history = format!(
            "CREATE TABLE analytics.orders (id int);
             CREATE TABLE {schema_with_orders_archive}.orders_archive (id int);"
        );
        let messages = messages_for_change(
            &history,
            "ALTER TABLE analytics.orders RENAME TO orders_archive;",
        );
        assert_eq!(messages.len(), expected_findings, "got {messages:?}");
    }

    #[test]
    fn rename_in_a_named_schema_shows_the_names_as_written() {
        let history = "
            CREATE TABLE analytics.orders (id int);
            CREATE TABLE analytics.orders_archive (id int);";
        assert_eq!(
            messages_for_change(
                history,
                "ALTER TABLE analytics.orders RENAME TO orders_archive;"
            ),
            [
                "Renaming table 'analytics.orders' to 'orders_archive' fails: \
              table 'analytics.orders_archive' already has that name."
            ]
        );
    }

    const ORDERS_WITH_CONSTRAINTS: &str = "
        CREATE TABLE orders (
            id bigint PRIMARY KEY,
            code text CONSTRAINT orders_code_key UNIQUE,
            ref bigint);
        CREATE UNIQUE INDEX orders_ref_idx ON orders (ref);";

    #[rstest]
    #[case::primary_key(
        "DROP INDEX orders_pkey;",
        "DROP INDEX 'orders_pkey' fails: constraint 'orders_pkey' on table 'orders' requires it. \
         Drop the constraint instead."
    )]
    #[case::unique_concurrently(
        "DROP INDEX CONCURRENTLY orders_code_key;",
        "DROP INDEX 'orders_code_key' fails: constraint 'orders_code_key' on table 'orders' \
         requires it. Drop the constraint instead."
    )]
    #[case::unique_if_exists(
        "DROP INDEX IF EXISTS orders_code_key;",
        "DROP INDEX 'orders_code_key' fails: constraint 'orders_code_key' on table 'orders' \
         requires it. Drop the constraint instead."
    )]
    #[case::constraint_added_earlier_in_the_unit(
        "ALTER TABLE orders ADD CONSTRAINT orders_ref_key UNIQUE USING INDEX orders_ref_idx;
         DROP INDEX orders_ref_key;",
        "DROP INDEX 'orders_ref_key' fails: constraint 'orders_ref_key' on table 'orders' \
         requires it. Drop the constraint instead."
    )]
    fn drop_index_of_a_constraints_index_fires(#[case] change: &str, #[case] expected: &str) {
        assert_eq!(
            messages_for_change(ORDERS_WITH_CONSTRAINTS, change),
            [expected]
        );
    }

    #[rstest]
    #[case::unique_index_without_a_constraint("DROP INDEX orders_ref_idx;")]
    #[case::constraint_dropped_earlier_in_the_unit(
        "ALTER TABLE orders DROP CONSTRAINT orders_code_key;
         DROP INDEX IF EXISTS orders_code_key;"
    )]
    #[case::missing_index("DROP INDEX made_outside_migrations_idx;")]
    fn drop_index_without_a_constraint_requiring_it_is_silent(#[case] change: &str) {
        let messages = messages_for_change(ORDERS_WITH_CONSTRAINTS, change);
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn drop_index_of_an_exclude_constraints_index_is_silent_while_the_catalog_registers_none() {
        let messages = messages_for_change(
            "CREATE TABLE rooms (r int, CONSTRAINT rooms_r_excl EXCLUDE USING btree (r WITH =));",
            "DROP INDEX rooms_r_excl;",
        );
        assert!(messages.is_empty(), "got {messages:?}");
    }

    #[test]
    fn drop_index_naming_two_constraints_indexes_fires_once() {
        let messages = messages_for_change(
            ORDERS_WITH_CONSTRAINTS,
            "DROP INDEX orders_pkey, orders_code_key;",
        );
        assert_eq!(
            messages,
            [
                "DROP INDEX 'orders_pkey' fails: constraint 'orders_pkey' on table 'orders' \
              requires it. Drop the constraint instead."
            ]
        );
    }

    #[test]
    fn each_failing_statement_of_a_unit_fires() {
        let lines: Vec<usize> = {
            let history = ORDERS_WITH_CONSTRAINTS;
            let change =
                "DROP INDEX orders_pkey;\nALTER INDEX orders_ref_idx RENAME TO orders_code_key;";
            let before = catalog_from_sql(history);
            let after = catalog_from_sql(&format!("{history}\n{change}"));
            lint_ctx!(ctx, &before, &after, "migrations/002.sql");
            RuleId::Pgm027
                .check(&normalized(parse_sql(change)), &ctx)
                .iter()
                .map(|finding| finding.start_line)
                .collect()
        };
        assert_eq!(lines, [1, 2]);
    }

    #[test]
    fn findings_use_the_default_severity() {
        let before = catalog_from_sql(ORDERS_WITH_CONSTRAINTS);
        let after = before.clone();
        lint_ctx!(ctx, &before, &after, "migrations/002.sql");
        let findings =
            RuleId::Pgm027.check(&normalized(parse_sql("DROP INDEX orders_pkey;")), &ctx);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, RuleId::Pgm027.default_severity());
    }
}
