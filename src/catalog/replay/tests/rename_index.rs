//! `ALTER INDEX ... RENAME TO`. The Postgres behaviour pinned here is
//! measured in `verify/tests/naming/rename.sql`.

use rstest::rstest;

use super::replay_sql;
use crate::catalog::builder::CatalogAssertions;
use crate::catalog::types::{ConstraintState, TableState};

fn index_names(table: &TableState) -> Vec<&str> {
    table.indexes.iter().map(|idx| idx.name.as_str()).collect()
}

fn constraint_names(table: &TableState) -> Vec<&str> {
    table
        .constraints
        .iter()
        .map(ConstraintState::name)
        .collect()
}

#[test]
fn rename_index_then_drop_by_the_new_name_removes_it() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int, customer_id int);",
        "CREATE INDEX idx_customer ON orders (customer_id);",
        "ALTER INDEX idx_customer RENAME TO idx_buyer;",
        "DROP INDEX idx_buyer;",
    ]);

    assert_eq!(index_names(catalog.table("orders")), Vec::<&str>::new());
    catalog.assert_index_unregistered("idx_buyer");
    catalog.assert_index_unregistered("idx_customer");
}

#[test]
fn rename_index_unregisters_the_old_name_and_keeps_the_definition() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int, customer_id int, active bool);",
        "CREATE INDEX idx_customer ON orders USING hash (customer_id) WHERE active;",
        "ALTER INDEX idx_customer RENAME TO idx_buyer;",
    ]);

    catalog.assert_index_unregistered("idx_customer");
    catalog.assert_index_owner("idx_buyer", "orders");
    let [index] = catalog.table("orders").indexes.as_slice() else {
        panic!(
            "expected one index, got {:?}",
            catalog.table("orders").indexes
        );
    };
    assert_eq!(index.name, "idx_buyer");
    assert_eq!(index.column_names().collect::<Vec<_>>(), ["customer_id"]);
    assert_eq!(index.where_clause.as_deref(), Some("active"));
    assert_eq!(index.access_method, "hash");
}

#[test]
fn drop_index_by_the_old_name_after_a_rename_changes_nothing() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int, customer_id int);",
        "CREATE INDEX idx_customer ON orders (customer_id);",
        "ALTER INDEX idx_customer RENAME TO idx_buyer;",
        "DROP INDEX idx_customer;",
    ]);

    assert_eq!(index_names(catalog.table("orders")), ["idx_buyer"]);
    catalog.assert_index_owner("idx_buyer", "orders");
}

#[rstest]
#[case::qualified(
    "analytics.orders",
    "data.orders",
    "ALTER INDEX analytics.idx_customer RENAME TO idx_buyer;"
)]
#[case::unqualified_in_the_default_schema(
    "orders",
    "analytics.orders",
    "ALTER INDEX idx_customer RENAME TO idx_buyer;"
)]
fn rename_index_in_one_schema_leaves_the_same_named_index_in_another(
    #[case] renamed_on: &str,
    #[case] untouched_on: &str,
    #[case] rename_sql: &str,
) {
    let mut steps = Vec::new();
    for table in [renamed_on, untouched_on] {
        steps.push(format!("CREATE TABLE {table} (id int, customer_id int);"));
        steps.push(format!(
            "CREATE INDEX idx_customer ON {table} (customer_id);"
        ));
    }
    steps.push(rename_sql.to_string());
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();

    let catalog = replay_sql(&steps);

    let renamed_schema = &catalog.table(renamed_on).name.schema;
    let untouched_schema = &catalog.table(untouched_on).name.schema;
    assert_eq!(index_names(catalog.table(renamed_on)), ["idx_buyer"]);
    catalog.assert_index_owner(&format!("{renamed_schema}.idx_buyer"), renamed_on);
    catalog.assert_index_unregistered(&format!("{renamed_schema}.idx_customer"));
    assert_eq!(index_names(catalog.table(untouched_on)), ["idx_customer"]);
    catalog.assert_index_owner(&format!("{untouched_schema}.idx_customer"), untouched_on);
    catalog.assert_index_unregistered(&format!("{untouched_schema}.idx_buyer"));
}

#[test]
fn rename_of_a_schema_qualified_index_keeps_it_in_that_schema() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (id int, customer_id int);",
        "CREATE INDEX idx_customer ON analytics.orders (customer_id);",
        "ALTER INDEX analytics.idx_customer RENAME TO idx_buyer;",
    ]);

    catalog.assert_index_owner("analytics.idx_buyer", "analytics.orders");
    catalog.assert_index_unregistered("idx_buyer");
}

#[rstest]
#[case::inline_primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::table_level_primary_key("CREATE TABLE t (id int, PRIMARY KEY (id));", "t_pkey")]
#[case::named_primary_key("CREATE TABLE t (id int, CONSTRAINT my_pk PRIMARY KEY (id));", "my_pk")]
#[case::unnamed_unique("CREATE TABLE t (a int UNIQUE);", "t_a_key")]
#[case::named_unique("CREATE TABLE t (a int, CONSTRAINT my_uq UNIQUE (a));", "my_uq")]
#[case::unique_added_using_index(
    "CREATE TABLE t (a int);
     CREATE UNIQUE INDEX t_a_idx ON t (a);
     ALTER TABLE t ADD UNIQUE USING INDEX t_a_idx;",
    "t_a_idx"
)]
fn rename_index_renames_the_constraint_it_backs(
    #[case] create_sql: &str,
    #[case] index_name: &str,
) {
    let catalog = replay_sql(&[
        create_sql,
        &format!("ALTER INDEX {index_name} RENAME TO t_renamed;"),
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), ["t_renamed"]);
    assert_eq!(index_names(table), ["t_renamed"]);
    catalog.assert_index_owner("t_renamed", "t");
    catalog.assert_index_unregistered(index_name);
}

#[rstest]
#[case::primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::unique("CREATE TABLE t (a int UNIQUE);", "t_a_key")]
fn drop_constraint_by_the_new_name_drops_a_renamed_constraint_index(
    #[case] create_sql: &str,
    #[case] index_name: &str,
) {
    let catalog = replay_sql(&[
        create_sql,
        &format!("ALTER INDEX {index_name} RENAME TO t_renamed;"),
        "ALTER TABLE t DROP CONSTRAINT t_renamed;",
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), Vec::<&str>::new());
    assert_eq!(index_names(table), Vec::<&str>::new());
    assert!(!table.has_primary_key);
    catalog.assert_index_unregistered("t_renamed");
}

#[test]
fn rename_index_keeps_a_check_constraint_of_the_same_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE ix (a int, CONSTRAINT ix_a_idx CHECK (a > 0));",
        "CREATE INDEX ON ix (a);",
        "ALTER INDEX ix_a_idx RENAME TO ix_a_renamed;",
    ]);

    let table = catalog.table("ix");
    assert_eq!(constraint_names(table), ["ix_a_idx"]);
    assert_eq!(index_names(table), ["ix_a_renamed"]);
}

#[rstest]
#[case::if_exists_on_a_name_no_schema_has("ALTER INDEX IF EXISTS idx_missing RENAME TO idx_new;")]
#[case::a_name_no_schema_has("ALTER INDEX idx_missing RENAME TO idx_new;")]
#[case::if_exists_on_a_name_only_another_schema_has(
    "ALTER INDEX IF EXISTS idx_customer RENAME TO idx_new;"
)]
#[case::a_name_only_another_schema_has("ALTER INDEX idx_customer RENAME TO idx_new;")]
fn rename_of_an_index_missing_from_its_schema_changes_nothing(#[case] rename_sql: &str) {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (id int, customer_id int);",
        "CREATE INDEX idx_customer ON analytics.orders (customer_id);",
        rename_sql,
    ]);

    assert_eq!(
        index_names(catalog.table("analytics.orders")),
        ["idx_customer"]
    );
    catalog.assert_index_owner("analytics.idx_customer", "analytics.orders");
    catalog.assert_index_unregistered("analytics.idx_new");
    catalog.assert_index_unregistered("idx_new");
    catalog.assert_index_unregistered("idx_customer");
}
