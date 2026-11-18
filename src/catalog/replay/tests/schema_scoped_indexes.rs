//! Indexes are keyed by schema and name. Each schema has its own index
//! namespace, so same-named indexes in two schemas are separate objects, and
//! a name resolves in exactly one schema.

use rstest::rstest;

use super::{replay_sql, steps_from_sql};
use crate::catalog::builder::{CatalogAssertions, TEST_DEFAULT_SCHEMA, test_relation};
use crate::catalog::types::{Catalog, IndexState, TableState};
use crate::parser::ir::{IrNode, Reindex, ReindexTarget};

fn index_names(table: &TableState) -> Vec<&str> {
    let mut names: Vec<&str> = table.indexes.iter().map(|idx| idx.name.as_str()).collect();
    names.sort_unstable();
    names
}

#[track_caller]
fn index_state<'a>(catalog: &'a Catalog, table: &str, index: &str) -> &'a IndexState {
    let table_state = catalog.table(table);
    match table_state.indexes.iter().find(|idx| idx.name == index) {
        Some(index_state) => index_state,
        None => panic!(
            "{table} has no index {index}; it has {:?}",
            index_names(table_state)
        ),
    }
}

/// The test name of `index` in `table`'s schema, as [`test_relation`] reads it.
fn index_in_schema_of(catalog: &Catalog, table: &str, index: &str) -> String {
    format!("{}.{index}", catalog.table(table).name.schema)
}

/// Panics unless `table` has `index` and the reverse lookup maps `index`, in
/// the table's schema, back to `table`.
#[track_caller]
fn assert_index_of(catalog: &Catalog, index: &str, table: &str) {
    index_state(catalog, table, index);
    catalog.assert_index_owner(&index_in_schema_of(catalog, table, index), table);
}

/// Panics if `table` has `index` or the reverse lookup maps `index`, in the
/// table's schema, to any table.
#[track_caller]
fn assert_no_index_of(catalog: &Catalog, index: &str, table: &str) {
    let table_state = catalog.table(table);
    assert!(
        table_state.indexes.iter().all(|idx| idx.name != index),
        "{table} should not have index {index}; it has {:?}",
        index_names(table_state)
    );
    catalog.assert_index_unregistered(&index_in_schema_of(catalog, table, index));
}

/// Replay two `orders` tables, written as `first` and `second`, that each
/// get a primary key and an index named `idx_customer`, in that order; then
/// replay `steps`.
fn replay_two_orders_tables_then(first: &str, second: &str, steps: &[&str]) -> Catalog {
    let setup = [
        format!("CREATE TABLE {first} (id int PRIMARY KEY, customer_id int);"),
        format!("CREATE TABLE {second} (id int PRIMARY KEY, customer_id int);"),
        format!("CREATE INDEX idx_customer ON {first} (customer_id);"),
        format!("CREATE INDEX idx_customer ON {second} (customer_id);"),
    ];
    let all_steps: Vec<&str> = setup
        .iter()
        .map(String::as_str)
        .chain(steps.iter().copied())
        .collect();
    replay_sql(&all_steps)
}

#[rstest]
#[case::both_schemas_qualified("analytics.orders", "data.orders")]
#[case::one_schema_qualified("analytics.orders", "orders")]
fn same_named_indexes_in_two_schemas_each_stay_with_their_own_table(
    #[case] first: &str,
    #[case] second: &str,
) {
    let catalog = replay_two_orders_tables_then(first, second, &[]);

    for table in [first, second] {
        assert_index_of(&catalog, "idx_customer", table);
        assert_index_of(&catalog, "orders_pkey", table);
    }
}

#[rstest]
#[case::qualified_with_the_first_schema(
    "analytics.orders",
    "data.orders",
    "DROP INDEX analytics.idx_customer;",
    "analytics.orders",
    "data.orders"
)]
#[case::qualified_with_the_second_schema(
    "analytics.orders",
    "data.orders",
    "DROP INDEX data.idx_customer;",
    "data.orders",
    "analytics.orders"
)]
#[case::unqualified_in_the_default_schema(
    "orders",
    "analytics.orders",
    "DROP INDEX idx_customer;",
    "orders",
    "analytics.orders"
)]
fn drop_index_removes_only_the_index_in_its_resolved_schema(
    #[case] first: &str,
    #[case] second: &str,
    #[case] drop_sql: &str,
    #[case] dropped_from: &str,
    #[case] kept_on: &str,
) {
    let catalog = replay_two_orders_tables_then(first, second, &[drop_sql]);

    assert_no_index_of(&catalog, "idx_customer", dropped_from);
    assert_index_of(&catalog, "idx_customer", kept_on);
}

#[test]
fn drop_index_unqualified_skips_same_named_indexes_outside_the_default_schema() {
    let catalog = replay_two_orders_tables_then(
        "analytics.orders",
        "data.orders",
        &["DROP INDEX idx_customer;"],
    );

    assert_index_of(&catalog, "idx_customer", "analytics.orders");
    assert_index_of(&catalog, "idx_customer", "data.orders");
}

#[test]
fn drop_table_keeps_the_same_named_indexes_of_another_schema_registered() {
    let catalog = replay_two_orders_tables_then(
        "analytics.orders",
        "data.orders",
        &["DROP TABLE analytics.orders;"],
    );

    catalog.assert_no_table("analytics.orders");
    catalog.assert_index_unregistered("analytics.idx_customer");
    catalog.assert_index_unregistered("analytics.orders_pkey");
    assert_index_of(&catalog, "idx_customer", "data.orders");
    assert_index_of(&catalog, "orders_pkey", "data.orders");
}

#[test]
fn drop_table_keeps_an_index_entry_that_another_table_took_over() {
    let catalog = replay_sql(&[
        "CREATE TABLE metrics (id int);
         CREATE TABLE events (id int);",
        "CREATE INDEX idx_created_at ON metrics (id);",
        "CREATE INDEX idx_created_at ON events (id);",
        "DROP TABLE metrics;",
    ]);

    assert_index_of(&catalog, "idx_created_at", "events");
}

#[test]
fn drop_column_on_the_table_holding_a_generated_name_keeps_the_suffixed_index_of_another_table() {
    let catalog = replay_sql(&[
        "CREATE TABLE archive (status text);
         CREATE TABLE orders (status text);",
        "CREATE INDEX orders_status_idx ON archive (status);",
        "CREATE INDEX ON orders (status);",
        "ALTER TABLE archive DROP COLUMN status;",
    ]);

    assert_index_of(&catalog, "orders_status_idx1", "orders");
    catalog.assert_index_unregistered("orders_status_idx");
}

#[rstest]
#[case::drop_column(
    "CREATE INDEX shared_idx ON archive (status);",
    "CREATE INDEX shared_idx ON orders (status);",
    "ALTER TABLE archive DROP COLUMN status;"
)]
#[case::drop_constraint(
    "ALTER TABLE archive ADD CONSTRAINT shared_idx UNIQUE (status);",
    "CREATE INDEX shared_idx ON orders (status);",
    "ALTER TABLE archive DROP CONSTRAINT shared_idx;"
)]
#[case::rename_constraint(
    "ALTER TABLE archive ADD CONSTRAINT shared_idx UNIQUE (status);",
    "CREATE INDEX shared_idx ON orders (status);",
    "ALTER TABLE archive RENAME CONSTRAINT shared_idx TO archive_status_key;"
)]
fn unregistering_an_index_keeps_an_entry_another_table_took_over(
    #[case] archive_index_sql: &str,
    #[case] orders_index_sql: &str,
    #[case] archive_change_sql: &str,
) {
    let catalog = replay_sql(&[
        "CREATE TABLE archive (status text);
         CREATE TABLE orders (status text);",
        archive_index_sql,
        orders_index_sql,
        archive_change_sql,
    ]);

    assert_index_of(&catalog, "shared_idx", "orders");
}

#[test]
fn drop_schema_cascade_unregisters_only_that_schemas_indexes() {
    let catalog = replay_two_orders_tables_then(
        "analytics.orders",
        "data.orders",
        &["DROP SCHEMA analytics CASCADE;"],
    );

    catalog.assert_no_table("analytics.orders");
    catalog.assert_index_unregistered("analytics.idx_customer");
    catalog.assert_index_unregistered("analytics.orders_pkey");
    assert_index_of(&catalog, "idx_customer", "data.orders");
    assert_index_of(&catalog, "orders_pkey", "data.orders");
}

#[test]
fn rename_table_moves_its_index_entries_to_the_new_name_in_its_schema() {
    let catalog = replay_two_orders_tables_then(
        "analytics.orders",
        "data.orders",
        &["ALTER TABLE analytics.orders RENAME TO purchases;"],
    );

    assert_index_of(&catalog, "idx_customer", "analytics.purchases");
    assert_index_of(&catalog, "orders_pkey", "analytics.purchases");
    assert_index_of(&catalog, "idx_customer", "data.orders");
    assert_index_of(&catalog, "orders_pkey", "data.orders");
}

#[rstest]
#[case::qualified(
    "analytics.measurements",
    "data.measurements",
    "ALTER INDEX analytics.idx_created ATTACH PARTITION analytics.measurements_2024_created_idx;"
)]
#[case::unqualified_in_the_default_schema(
    "measurements",
    "analytics.measurements",
    "ALTER INDEX idx_created ATTACH PARTITION measurements_2024_created_idx;"
)]
fn attach_partition_validates_only_the_parent_index_in_its_resolved_schema(
    #[case] attached_on: &str,
    #[case] untouched_on: &str,
    #[case] attach_sql: &str,
) {
    let mut steps = Vec::new();
    for table in [attached_on, untouched_on] {
        steps.push(format!(
            "CREATE TABLE {table} (id int, created date) PARTITION BY RANGE (created);"
        ));
        steps.push(format!(
            "CREATE INDEX idx_created ON ONLY {table} (created);"
        ));
    }
    steps.push(attach_sql.to_string());
    let steps: Vec<&str> = steps.iter().map(String::as_str).collect();

    let catalog = replay_sql(&steps);

    assert!(!index_state(&catalog, attached_on, "idx_created").only);
    assert!(index_state(&catalog, untouched_on, "idx_created").only);
}

#[rstest]
#[case::qualified("REINDEX INDEX data.idx_customer;", "data.orders")]
#[case::unqualified_in_the_default_schema("REINDEX INDEX idx_customer;", "orders")]
fn reindex_index_target_resolves_to_the_index_in_its_own_schema(
    #[case] reindex_sql: &str,
    #[case] owner: &str,
) {
    let catalog = replay_two_orders_tables_then("orders", "data.orders", &[]);
    let units = steps_from_sql(&[reindex_sql], TEST_DEFAULT_SCHEMA);
    let IrNode::Reindex(Reindex {
        target: ReindexTarget::Relation(index),
        ..
    }) = &units[0].statements[0].node
    else {
        panic!("expected REINDEX INDEX, got {:?}", units[0].statements);
    };

    assert_eq!(
        catalog.table_for_index(index.relation()),
        Some(&test_relation(owner))
    );
}
