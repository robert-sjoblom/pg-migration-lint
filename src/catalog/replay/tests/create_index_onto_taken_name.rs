//! `CREATE INDEX name` where a relation in the table's schema already has
//! `name`. Postgres fails the statement with 42P07, with or without
//! `CONCURRENTLY`, and `IF NOT EXISTS` makes it a no-op; either way it
//! creates nothing (`verify/tests/other/pgm027_create_index_onto_taken_name.sql`),
//! so replay changes nothing either.

use rstest::rstest;

use super::*;

fn archive_orders_and_analytics_reports() -> Catalog {
    CatalogBuilder::new()
        .table("archive", |t| {
            t.column("id", "int", false)
                .column("code", "text", true)
                .column("slot", "int", true)
                .named_pk("archive_pkey", &["id"])
                .index("archive_pkey", &["id"], true)
                .unique("archive_code_key", &["code"])
                .index("archive_code_key", &["code"], true)
                .exclude_constraint(Some("archive_slot_excl"), &["slot"])
                .index("archive_slot_idx", &["slot"], false);
        })
        .table("orders", |t| {
            t.column("id", "int", false)
                .column("status", "text", true)
                .index("orders_status_idx", &["status"], false);
        })
        .table("analytics.reports", |t| {
            t.column("id", "int", false)
                .index("reports_id_idx", &["id"], false);
        })
        .build()
}

fn index_names_of(catalog: &Catalog, table: &str) -> Vec<String> {
    catalog
        .table(table)
        .indexes
        .iter()
        .map(|idx| idx.name.clone())
        .collect()
}

#[rstest]
#[case::index_of_another_table("CREATE INDEX archive_slot_idx ON orders (status);")]
#[case::index_of_the_same_table("CREATE INDEX orders_status_idx ON orders (id);")]
#[case::primary_key_index("CREATE INDEX archive_pkey ON orders (id);")]
#[case::unique_constraint_index("CREATE UNIQUE INDEX archive_code_key ON orders (status);")]
#[case::exclude_constraint_index("CREATE INDEX archive_slot_excl ON orders (id);")]
#[case::table("CREATE INDEX archive ON orders (id);")]
#[case::concurrently("CREATE INDEX CONCURRENTLY archive_slot_idx ON orders (status);")]
#[case::if_not_exists("CREATE INDEX IF NOT EXISTS archive_slot_idx ON orders (status);")]
#[case::if_not_exists_onto_a_table("CREATE INDEX IF NOT EXISTS archive ON orders (id);")]
#[case::in_a_named_schema("CREATE INDEX reports_id_idx ON analytics.reports (id);")]
fn create_index_onto_a_taken_name_changes_nothing(#[case] sql: &str) {
    let before = archive_orders_and_analytics_reports();

    let after = replayed_on(&before, sql);

    after.assert_same_state_as(&before);
}

#[rstest]
#[case::taken_in_another_schema(
    "CREATE INDEX reports_id_idx ON orders (id);",
    "public.reports_id_idx",
    "orders"
)]
#[case::table_in_a_schema_where_the_name_is_free(
    "CREATE INDEX archive_slot_idx ON analytics.reports (id);",
    "analytics.archive_slot_idx",
    "analytics.reports"
)]
fn create_index_onto_a_name_taken_only_in_another_schema_creates_it(
    #[case] sql: &str,
    #[case] index: &str,
    #[case] table: &str,
) {
    let before = archive_orders_and_analytics_reports();

    let after = replayed_on(&before, sql);

    after.assert_index_owner(index, table);
    assert_eq!(index_names_of(&after, table).len(), 2);
}

#[test]
fn create_index_onto_a_name_created_earlier_in_the_unit_keeps_the_first_index() {
    let before = archive_orders_and_analytics_reports();

    let after = replayed_on(
        &before,
        "CREATE INDEX orders_id_idx ON orders (id);
         CREATE UNIQUE INDEX orders_id_idx ON orders (status);",
    );

    let orders_id_idx: Vec<_> = after
        .table("orders")
        .indexes
        .iter()
        .filter(|idx| idx.name == "orders_id_idx")
        .collect();
    assert_eq!(orders_id_idx.len(), 1, "got {orders_id_idx:?}");
    assert_eq!(orders_id_idx[0].column_names().collect::<Vec<_>>(), ["id"]);
    assert!(!orders_id_idx[0].unique);
}

#[rstest]
#[case::index_dropped("DROP INDEX archive_slot_idx;", "archive_slot_idx")]
#[case::table_renamed_away("ALTER TABLE archive RENAME TO archive_old;", "archive")]
#[case::table_dropped("DROP TABLE archive;", "archive")]
fn create_index_onto_a_name_freed_earlier_in_the_unit_creates_it(
    #[case] freeing_sql: &str,
    #[case] name: &str,
) {
    let before = archive_orders_and_analytics_reports();

    let after = replayed_on(
        &before,
        &format!("{freeing_sql}\nCREATE INDEX {name} ON orders (status);"),
    );

    after.assert_index_owner(name, "orders");
}

#[test]
fn index_of_another_table_keeps_its_registration_and_is_dropped_from_its_own_table() {
    let before = archive_orders_and_analytics_reports();

    let after = replayed_on(
        &before,
        "CREATE INDEX archive_slot_idx ON orders (status);
         DROP INDEX archive_slot_idx;",
    );

    assert!(!index_names_of(&after, "archive").contains(&"archive_slot_idx".to_string()));
    assert_eq!(index_names_of(&after, "orders"), ["orders_status_idx"]);
    after.assert_index_unregistered("archive_slot_idx");
}
