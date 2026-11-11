use std::collections::BTreeSet;

use super::steps_from_sql;
use crate::catalog::builder::CatalogAssertions;
use crate::catalog::replay::apply;
use crate::catalog::types::{Catalog, ConstraintState, TableState};

fn replay_sql(sql_steps: &[&str]) -> Catalog {
    let mut catalog = Catalog::new();
    for unit in steps_from_sql(sql_steps, "public") {
        apply(&mut catalog, &unit);
    }
    catalog
}

#[track_caller]
fn table<'a>(catalog: &'a Catalog, table_key: &str) -> &'a TableState {
    catalog.table(table_key)
}

fn index_names(table: &TableState) -> BTreeSet<&str> {
    table.indexes.iter().map(|idx| idx.name.as_str()).collect()
}

fn assert_index_owned_by(catalog: &Catalog, index_name: &str, table_key: &str) {
    assert_eq!(
        index_names(table(catalog, table_key)),
        BTreeSet::from([index_name]),
    );
    catalog.assert_index_owner(index_name, table_key);
}

fn assert_pk_fully_dropped(catalog: &Catalog, table_key: &str, pk_index_name: &str) {
    let table = table(catalog, table_key);
    assert!(!table.has_primary_key);
    assert!(
        !table
            .constraints
            .iter()
            .any(|c| matches!(c, ConstraintState::PrimaryKey { .. })),
        "PK constraint should be gone, found {:?}",
        table.constraints,
    );
    assert!(
        table.indexes.is_empty(),
        "PK index should be gone, found {:?}",
        index_names(table),
    );
    catalog.assert_index_unregistered(pk_index_name);
}

#[test]
fn drop_constraint_default_pkey_name_drops_unnamed_pk_of_unqualified_table() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int PRIMARY KEY);",
        "ALTER TABLE orders DROP CONSTRAINT orders_pkey;",
    ]);
    assert_pk_fully_dropped(&catalog, "public.orders", "orders_pkey");
}

#[test]
fn drop_constraint_default_pkey_name_drops_unnamed_pk_of_schema_qualified_table() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (id int PRIMARY KEY);",
        "ALTER TABLE analytics.orders DROP CONSTRAINT orders_pkey;",
    ]);
    assert_pk_fully_dropped(&catalog, "analytics.orders", "orders_pkey");
}

#[test]
fn drop_constraint_named_pk_drops_its_constraint_named_index() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int, CONSTRAINT my_pk PRIMARY KEY (id));",
        "ALTER TABLE orders DROP CONSTRAINT my_pk;",
    ]);
    assert_pk_fully_dropped(&catalog, "public.orders", "my_pk");
}

#[test]
fn create_table_named_pk_index_takes_constraint_name() {
    let catalog = replay_sql(&["CREATE TABLE orders (id int, CONSTRAINT my_pk PRIMARY KEY (id));"]);
    assert_index_owned_by(&catalog, "my_pk", "public.orders");
}

#[test]
fn create_table_inline_pk_on_schema_qualified_table_index_uses_bare_table_name() {
    let catalog = replay_sql(&["CREATE TABLE analytics.orders (id int PRIMARY KEY);"]);
    assert_index_owned_by(&catalog, "orders_pkey", "analytics.orders");
}

#[test]
fn create_table_unnamed_unique_on_schema_qualified_table_index_uses_bare_table_name() {
    let catalog = replay_sql(&["CREATE TABLE analytics.orders (email text UNIQUE);"]);
    assert_index_owned_by(&catalog, "orders_email_key", "analytics.orders");
}

#[test]
fn alter_table_add_named_pk_index_takes_constraint_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (id int);",
        "ALTER TABLE analytics.orders ADD CONSTRAINT my_pk PRIMARY KEY (id);",
    ]);
    assert_index_owned_by(&catalog, "my_pk", "analytics.orders");
}

#[test]
fn alter_table_add_unnamed_pk_on_schema_qualified_table_index_uses_bare_table_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (id int);",
        "ALTER TABLE analytics.orders ADD PRIMARY KEY (id);",
    ]);
    assert_index_owned_by(&catalog, "orders_pkey", "analytics.orders");
}

#[test]
fn alter_table_add_column_inline_pk_on_schema_qualified_table_index_uses_bare_table_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (email text);",
        "ALTER TABLE analytics.orders ADD COLUMN id int PRIMARY KEY;",
    ]);
    assert_index_owned_by(&catalog, "orders_pkey", "analytics.orders");
}

#[test]
fn alter_table_add_unnamed_unique_on_schema_qualified_table_index_uses_bare_table_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.orders (email text);",
        "ALTER TABLE analytics.orders ADD UNIQUE (email);",
    ]);
    assert_index_owned_by(&catalog, "orders_email_key", "analytics.orders");
}
