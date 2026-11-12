use rstest::rstest;

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

fn constraint_names(table: &TableState) -> Vec<&str> {
    table
        .constraints
        .iter()
        .map(ConstraintState::name)
        .collect()
}

fn index_names(table: &TableState) -> Vec<&str> {
    table.indexes.iter().map(|idx| idx.name.as_str()).collect()
}

fn assert_names(catalog: &Catalog, table_key: &str, constraints: &[&str], indexes: &[&str]) {
    let table = table(catalog, table_key);
    assert_eq!(constraint_names(table), constraints, "constraint names");
    assert_eq!(index_names(table), indexes, "index names");
    for index_name in indexes {
        catalog.assert_index_owner(index_name, table_key);
    }
}

#[rstest]
#[case::unnamed_primary_key(
    "CREATE TABLE pk_table_level (id int, PRIMARY KEY (id));",
    "public.pk_table_level",
    &["pk_table_level_pkey"],
    &["pk_table_level_pkey"]
)]
#[case::named_primary_key(
    "CREATE TABLE pk_named (id int, CONSTRAINT my_pk PRIMARY KEY (id));",
    "public.pk_named",
    &["my_pk"],
    &["my_pk"]
)]
#[case::unnamed_unique_with_include(
    "CREATE TABLE uq_include (a int, b int, UNIQUE (a) INCLUDE (b));",
    "public.uq_include",
    &["uq_include_a_b_key"],
    &["uq_include_a_b_key"]
)]
#[case::unnamed_unique_inline(
    "CREATE TABLE uq_inline (a int UNIQUE);",
    "public.uq_inline",
    &["uq_inline_a_key"],
    &["uq_inline_a_key"]
)]
#[case::named_unique(
    "CREATE TABLE uq_named (a int, CONSTRAINT my_uq UNIQUE (a));",
    "public.uq_named",
    &["my_uq"],
    &["my_uq"]
)]
#[case::unnamed_check_on_one_column(
    "CREATE TABLE ck_table_one (a int, b int, CHECK (b > 0));",
    "public.ck_table_one",
    &["ck_table_one_b_check"],
    &[]
)]
#[case::unnamed_check_on_two_columns(
    "CREATE TABLE ck_table_two (a int, b int, CHECK (a < b));",
    "public.ck_table_two",
    &["ck_table_two_check"],
    &[]
)]
#[case::named_check(
    "CREATE TABLE ck_named (a int, CONSTRAINT my_check CHECK (a > 0));",
    "public.ck_named",
    &["my_check"],
    &[]
)]
#[case::unnamed_exclude_on_function(
    "CREATE TABLE ex_function (e text, EXCLUDE USING btree (lower(e) WITH =));",
    "public.ex_function",
    &["ex_function_lower_excl"],
    &[]
)]
#[case::named_exclude(
    "CREATE TABLE ex_named (a int, CONSTRAINT my_excl EXCLUDE USING btree (a WITH =));",
    "public.ex_named",
    &["my_excl"],
    &[]
)]
#[case::quoted_identifiers_keep_case(
    r#"CREATE TABLE "Orders" ("Code" text UNIQUE);"#,
    "public.Orders",
    &["Orders_Code_key"],
    &["Orders_Code_key"]
)]
#[case::long_table_name_truncated(
    "CREATE TABLE u23456789012345678901234567890123456789012345678901234567890123 (a int UNIQUE);",
    "public.u23456789012345678901234567890123456789012345678901234567890123",
    &["u23456789012345678901234567890123456789012345678901234567_a_key"],
    &["u23456789012345678901234567890123456789012345678901234567_a_key"]
)]
fn create_table_constraint_gets_postgres_name(
    #[case] sql: &str,
    #[case] table_key: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    assert_names(&replay_sql(&[sql]), table_key, constraints, indexes);
}

#[rstest]
#[case::unnamed_foreign_key_multi_column(
    "CREATE TABLE fk_multi (px int, pid int, FOREIGN KEY (pid, px) REFERENCES parent (id, x));",
    "public.fk_multi",
    "fk_multi_pid_px_fkey"
)]
#[case::unnamed_foreign_key_inline(
    "CREATE TABLE fk_inline (parent_id int REFERENCES parent);",
    "public.fk_inline",
    "fk_inline_parent_id_fkey"
)]
#[case::named_foreign_key(
    "CREATE TABLE fk_named (pid int, CONSTRAINT my_fk FOREIGN KEY (pid) REFERENCES parent);",
    "public.fk_named",
    "my_fk"
)]
fn create_table_foreign_key_gets_postgres_name(
    #[case] sql: &str,
    #[case] table_key: &str,
    #[case] constraint: &str,
) {
    let catalog = replay_sql(&[
        "CREATE TABLE parent (id int PRIMARY KEY, x int, UNIQUE (id, x));",
        sql,
    ]);
    assert_names(&catalog, table_key, &[constraint], &[]);
}

#[rstest]
#[case::add_primary_key("ALTER TABLE t ADD PRIMARY KEY (a);", &["t_pkey"], &["t_pkey"])]
#[case::add_named_primary_key(
    "ALTER TABLE t ADD CONSTRAINT alter_pk PRIMARY KEY (a);",
    &["alter_pk"],
    &["alter_pk"]
)]
#[case::add_unique("ALTER TABLE t ADD UNIQUE (a, b);", &["t_a_b_key"], &["t_a_b_key"])]
#[case::add_column_unique(
    "ALTER TABLE t ADD COLUMN c int UNIQUE;",
    &["t_c_key"],
    &["t_c_key"]
)]
#[case::add_check_on_one_column("ALTER TABLE t ADD CHECK (a > 0);", &["t_a_check"], &[])]
#[case::add_check_on_two_columns("ALTER TABLE t ADD CHECK (a < b);", &["t_check"], &[])]
#[case::add_column_check_on_other_column(
    "ALTER TABLE t ADD COLUMN c int CHECK (c > a);",
    &["t_check"],
    &[]
)]
#[case::add_foreign_key(
    "ALTER TABLE t ADD FOREIGN KEY (a) REFERENCES parent;",
    &["t_a_fkey"],
    &[]
)]
#[case::add_column_references(
    "ALTER TABLE t ADD COLUMN pid int REFERENCES parent;",
    &["t_pid_fkey"],
    &[]
)]
#[case::add_exclude(
    "ALTER TABLE t ADD EXCLUDE USING btree (a WITH =);",
    &["t_a_excl"],
    &[]
)]
fn alter_table_constraint_gets_postgres_name(
    #[case] sql: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    let catalog = replay_sql(&[
        "CREATE TABLE parent (id int PRIMARY KEY);",
        "CREATE TABLE t (a int, b int);",
        sql,
    ]);
    assert_names(&catalog, "public.t", constraints, indexes);
}

#[rstest]
#[case::single_column("CREATE INDEX ON t (a);", "t_a_idx")]
#[case::index_order("CREATE INDEX ON t (b, a);", "t_b_a_idx")]
#[case::function_operator_column(
    "CREATE INDEX ON t (lower(email), (a + b), b);",
    "t_lower_expr_b_idx"
)]
#[case::include_appended("CREATE INDEX ON t (a) INCLUDE (b);", "t_a_b_idx")]
#[case::repeated_element("CREATE INDEX ON t (a, a);", "t_a_a1_idx")]
#[case::unique_partial_concurrently(
    "CREATE UNIQUE INDEX CONCURRENTLY ON t (a) WHERE a > 0;",
    "t_a_idx"
)]
#[case::hash_with_opclass("CREATE INDEX ON t USING hash (email text_ops);", "t_email_idx")]
#[case::named("CREATE INDEX my_idx ON t (a);", "my_idx")]
fn create_index_gets_postgres_name(#[case] sql: &str, #[case] index: &str) {
    let catalog = replay_sql(&["CREATE TABLE t (a int, b int, email text);", sql]);
    assert_names(&catalog, "public.t", &[], &[index]);
}

#[test]
fn unnamed_create_index_can_be_dropped_by_its_generated_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int);",
        "CREATE INDEX ON t (a);",
        "DROP INDEX t_a_idx;",
    ]);
    assert_names(&catalog, "public.t", &[], &[]);
    catalog.assert_index_unregistered("t_a_idx");
}

#[test]
fn schema_qualified_table_generates_from_its_bare_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE analytics.customers (id int PRIMARY KEY);",
        "CREATE TABLE analytics.orders (
            id int,
            code text UNIQUE,
            qty int CHECK (qty > 0),
            customer_id int REFERENCES analytics.customers,
            slot int,
            EXCLUDE USING btree (slot WITH =),
            PRIMARY KEY (id));",
        "CREATE INDEX ON analytics.orders (qty);",
    ]);
    assert_names(
        &catalog,
        "analytics.orders",
        &[
            "orders_code_key",
            "orders_qty_check",
            "orders_customer_id_fkey",
            "orders_slot_excl",
            "orders_pkey",
        ],
        &["orders_code_key", "orders_pkey", "orders_qty_idx"],
    );
}

#[test]
fn add_primary_key_using_index_takes_the_index_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE ui_pk (a int NOT NULL);",
        "CREATE UNIQUE INDEX ui_pk_a_idx ON ui_pk (a);",
        "ALTER TABLE ui_pk ADD PRIMARY KEY USING INDEX ui_pk_a_idx;",
    ]);
    assert_names(&catalog, "public.ui_pk", &["ui_pk_a_idx"], &["ui_pk_a_idx"]);
}

#[test]
fn add_unique_using_index_takes_the_index_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE ui_uq (a int);",
        "CREATE UNIQUE INDEX ui_uq_a_idx ON ui_uq (a);",
        "ALTER TABLE ui_uq ADD UNIQUE USING INDEX ui_uq_a_idx;",
    ]);
    assert_names(&catalog, "public.ui_uq", &["ui_uq_a_idx"], &["ui_uq_a_idx"]);
}

#[rstest]
#[case::primary_key("PRIMARY KEY")]
#[case::unique("UNIQUE")]
fn add_named_constraint_using_index_renames_the_index(#[case] kind: &str) {
    let catalog = replay_sql(&[
        "CREATE TABLE ui_named (a int NOT NULL);",
        "CREATE UNIQUE INDEX ui_named_idx ON ui_named (a);",
        &format!("ALTER TABLE ui_named ADD CONSTRAINT my_con {kind} USING INDEX ui_named_idx;"),
    ]);
    assert_names(&catalog, "public.ui_named", &["my_con"], &["my_con"]);
    catalog.assert_index_unregistered("ui_named_idx");
    let index = &table(&catalog, "public.ui_named").indexes[0];
    assert_eq!(index.column_names().collect::<Vec<_>>(), ["a"]);
}

#[rstest]
#[case::named_primary_key("PRIMARY KEY")]
#[case::named_unique("UNIQUE")]
fn drop_named_constraint_added_using_index_drops_the_renamed_index(#[case] kind: &str) {
    let catalog = replay_sql(&[
        "CREATE TABLE ui_named (a int NOT NULL);",
        "CREATE UNIQUE INDEX ui_named_idx ON ui_named (a);",
        &format!("ALTER TABLE ui_named ADD CONSTRAINT my_con {kind} USING INDEX ui_named_idx;"),
        "ALTER TABLE ui_named DROP CONSTRAINT my_con;",
    ]);
    assert_names(&catalog, "public.ui_named", &[], &[]);
    catalog.assert_index_unregistered("my_con");
    assert!(!table(&catalog, "public.ui_named").has_primary_key);
}

#[test]
fn rename_table_then_drop_unnamed_pk_by_its_original_generated_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE orders (id int PRIMARY KEY);",
        "ALTER TABLE orders RENAME TO purchases;",
        "ALTER TABLE purchases DROP CONSTRAINT orders_pkey;",
    ]);
    let purchases = table(&catalog, "public.purchases");
    assert!(!purchases.has_primary_key);
    assert_names(&catalog, "public.purchases", &[], &[]);
    catalog.assert_index_unregistered("orders_pkey");
}

#[test]
fn rename_table_and_columns_keep_every_generated_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE customers (id int PRIMARY KEY);",
        "CREATE TABLE orders (
            id int,
            code text UNIQUE,
            qty int CHECK (qty > 0),
            customer_id int REFERENCES customers,
            slot int,
            EXCLUDE USING btree (slot WITH =),
            PRIMARY KEY (id));",
        "CREATE INDEX ON orders (qty);",
        "ALTER TABLE orders RENAME TO purchases;",
        "ALTER TABLE purchases RENAME COLUMN code TO sku;",
        "ALTER TABLE purchases RENAME COLUMN qty TO quantity;",
        "ALTER TABLE purchases RENAME COLUMN customer_id TO buyer_id;",
        "ALTER TABLE purchases RENAME COLUMN slot TO position;",
    ]);
    assert_names(
        &catalog,
        "public.purchases",
        &[
            "orders_code_key",
            "orders_qty_check",
            "orders_customer_id_fkey",
            "orders_slot_excl",
            "orders_pkey",
        ],
        &["orders_code_key", "orders_pkey", "orders_qty_idx"],
    );
}

#[rstest]
#[case::unique("code text UNIQUE", "orders_code_key")]
#[case::foreign_key("customer_id int REFERENCES customers", "orders_customer_id_fkey")]
#[case::check("qty int CHECK (qty > 0)", "orders_qty_check")]
#[case::exclude("slot int, EXCLUDE USING btree (slot WITH =)", "orders_slot_excl")]
fn drop_unnamed_constraint_by_its_generated_name_after_table_rename(
    #[case] column_and_constraint: &str,
    #[case] generated_name: &str,
) {
    let catalog = replay_sql(&[
        "CREATE TABLE customers (id int PRIMARY KEY);",
        &format!("CREATE TABLE orders (id int, {column_and_constraint});"),
        "ALTER TABLE orders RENAME TO purchases;",
        &format!("ALTER TABLE purchases DROP CONSTRAINT {generated_name};"),
    ]);
    assert_names(&catalog, "public.purchases", &[], &[]);
    catalog.assert_index_unregistered(generated_name);
}

#[test]
fn drop_check_constraint_keeps_an_index_of_the_same_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE ix (a int, CONSTRAINT ix_a_idx CHECK (a > 0));",
        "CREATE INDEX ON ix (a);",
        "ALTER TABLE ix DROP CONSTRAINT ix_a_idx;",
    ]);
    assert_names(&catalog, "public.ix", &[], &["ix_a_idx"]);
}

#[test]
fn validate_unnamed_foreign_key_by_its_generated_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE customers (id int PRIMARY KEY);",
        "CREATE TABLE orders (customer_id int);",
        "ALTER TABLE orders ADD FOREIGN KEY (customer_id) REFERENCES customers NOT VALID;",
        "ALTER TABLE orders VALIDATE CONSTRAINT orders_customer_id_fkey;",
    ]);
    let orders = table(&catalog, "public.orders");
    assert!(
        matches!(
            orders.constraints.as_slice(),
            [ConstraintState::ForeignKey {
                not_valid: false,
                ..
            }]
        ),
        "FK should be validated, found {:?}",
        orders.constraints
    );
}
