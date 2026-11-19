//! `ADD PRIMARY KEY` or `ADD UNIQUE ... USING INDEX idx` where `idx` belongs
//! to another table. Postgres fails the statement with 55000 and changes
//! nothing (`verify/tests/other/pgm027_using_index_of_another_table.sql`),
//! so replay changes nothing either. An index the catalog does not have in
//! the table's schema may exist outside the tracked migrations, so the
//! constraint is added as before.

use rstest::rstest;

use super::*;

fn customers_in_two_schemas_and_orders_without_a_key() -> Catalog {
    CatalogBuilder::new()
        .table("customers", |t| {
            t.column("id", "bigint", false)
                .index("customers_id_idx", &["id"], true);
        })
        .table("analytics.customers", |t| {
            t.column("email", "text", false)
                .index("customers_email_idx", &["email"], true);
        })
        .table("orders", |t| {
            t.column("id", "bigint", false)
                .column("email", "text", false);
        })
        .build()
}

#[test]
fn primary_key_using_another_tables_index_leaves_the_table_without_one() {
    let before = customers_in_two_schemas_and_orders_without_a_key();

    let after = replayed_on(
        &before,
        "ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_id_idx;",
    );

    let orders = after.table("orders");
    assert!(!orders.has_primary_key);
    assert!(
        orders.constraints.is_empty(),
        "got {:?}",
        orders.constraints
    );
    after.assert_index_owner("customers_id_idx", "customers");
}

#[rstest]
#[case::primary_key("ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_id_idx;")]
#[case::named_primary_key(
    "ALTER TABLE orders ADD CONSTRAINT orders_pk PRIMARY KEY USING INDEX customers_id_idx;"
)]
#[case::unique("ALTER TABLE orders ADD UNIQUE USING INDEX customers_id_idx;")]
#[case::named_unique(
    "ALTER TABLE orders ADD CONSTRAINT orders_id_key UNIQUE USING INDEX customers_id_idx;"
)]
#[case::among_other_subcommands(
    "ALTER TABLE orders
        ADD COLUMN note text,
        DROP COLUMN email,
        ADD CONSTRAINT orders_note_check CHECK (note <> ''),
        ADD PRIMARY KEY USING INDEX customers_id_idx;"
)]
#[case::after_a_using_index_of_its_own_table(
    "CREATE UNIQUE INDEX orders_email_idx ON orders (email);
     ALTER TABLE orders
        ADD UNIQUE USING INDEX orders_email_idx,
        ADD PRIMARY KEY USING INDEX customers_id_idx;
     DROP INDEX orders_email_idx;"
)]
fn statement_using_another_tables_index_changes_nothing(#[case] sql: &str) {
    let before = customers_in_two_schemas_and_orders_without_a_key();

    let after = replayed_on(&before, sql);

    after.assert_same_state_as(&before);
}

#[test]
fn using_an_index_created_on_another_table_earlier_in_the_unit_changes_nothing() {
    let before = customers_in_two_schemas_and_orders_without_a_key();

    let after = replayed_on(
        &before,
        "CREATE UNIQUE INDEX customers_id_key ON customers (id);
         ALTER TABLE orders ADD PRIMARY KEY USING INDEX customers_id_key;",
    );

    assert!(!after.table("orders").has_primary_key);
    after.assert_index_owner("customers_id_key", "customers");
}

#[rstest]
#[case::missing_from_the_catalog("made_outside_migrations_idx")]
#[case::only_in_another_schema("customers_email_idx")]
fn primary_key_using_an_index_the_catalog_lacks_in_the_tables_schema_is_added(#[case] index: &str) {
    let before = customers_in_two_schemas_and_orders_without_a_key();

    let after = replayed_on(
        &before,
        &format!("ALTER TABLE orders ADD PRIMARY KEY USING INDEX {index};"),
    );

    let orders = after.table("orders");
    assert!(orders.has_primary_key);
    assert!(
        matches!(
            orders.get_constraint(index),
            Some(ConstraintState::PrimaryKey { .. })
        ),
        "got {:?}",
        orders.constraints
    );
}
