//! Renames onto a name that is already taken. Postgres fails the statement
//! and changes nothing, so replay changes nothing either; the conditions are
//! measured in `verify/tests/naming/rename_taken.sql`.

use rstest::rstest;

use super::rename_constraint::before_and_after;
use crate::catalog::builder::{CatalogAssertions, rel, rel_in};
use crate::catalog::types::{ConstraintState, RenameClash};

const NEIGHBOURS_AND_CON: &[&str] = &[
    "CREATE TABLE neighbour (
        n int,
        CONSTRAINT neighbour_check CHECK (n > 0),
        CONSTRAINT neighbour_key UNIQUE (n),
        CONSTRAINT neighbour_excl EXCLUDE USING btree (n WITH =));",
    "CREATE INDEX neighbour_idx ON neighbour (n);",
    "CREATE TABLE other.remote (r int);",
    "CREATE INDEX remote_idx ON other.remote (r);",
    "CREATE TABLE con (
        a int,
        b int,
        CONSTRAINT con_pk PRIMARY KEY (a),
        CONSTRAINT con_uq UNIQUE (b),
        CONSTRAINT con_chk CHECK (a > 0),
        CONSTRAINT con_chk2 CHECK (b > 0),
        CONSTRAINT con_fk FOREIGN KEY (b) REFERENCES neighbour (n),
        CONSTRAINT con_excl EXCLUDE USING btree (b WITH =));",
    "CREATE INDEX con_idx ON con (b);",
];

#[rstest]
#[case::check_onto_a_check_of_its_table("con_chk", "con_chk2", RenameClash::Constraint)]
#[case::check_onto_a_unique_of_its_table("con_chk", "con_uq", RenameClash::Constraint)]
#[case::check_onto_its_own_name("con_chk", "con_chk", RenameClash::Constraint)]
#[case::foreign_key_onto_a_check_of_its_table("con_fk", "con_chk2", RenameClash::Constraint)]
#[case::unique_onto_another_tables_index("con_uq", "neighbour_idx", RenameClash::Relation)]
#[case::unique_onto_a_table("con_uq", "neighbour", RenameClash::Relation)]
#[case::unique_onto_a_plain_index_of_its_table("con_uq", "con_idx", RenameClash::Relation)]
#[case::unique_onto_its_own_name("con_uq", "con_uq", RenameClash::Relation)]
#[case::unique_onto_a_check_of_its_table("con_uq", "con_chk2", RenameClash::Constraint)]
#[case::unique_onto_another_tables_exclude("con_uq", "neighbour_excl", RenameClash::Relation)]
#[case::primary_key_onto_another_tables_unique("con_pk", "neighbour_key", RenameClash::Relation)]
#[case::exclude_onto_the_primary_key_of_its_table("con_excl", "con_pk", RenameClash::Relation)]
fn rename_constraint_onto_a_taken_name_changes_nothing(
    #[case] constraint: &str,
    #[case] new_name: &str,
    #[case] expected_clash: RenameClash,
) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER TABLE con RENAME CONSTRAINT {constraint} TO {new_name};"),
    );

    assert_eq!(
        before.constraint_rename_clash(&rel("con"), constraint, new_name),
        Some(expected_clash)
    );
    after.assert_same_state_as(&before);
}

#[rstest]
#[case::check_onto_another_tables_check("con_chk", "neighbour_check")]
#[case::check_onto_an_index("con_chk", "neighbour_idx")]
#[case::check_onto_a_table("con_chk", "neighbour")]
#[case::foreign_key_onto_another_tables_unique("con_fk", "neighbour_key")]
#[case::unique_onto_another_tables_check("con_uq", "neighbour_check")]
#[case::unique_onto_an_index_in_another_schema("con_uq", "remote_idx")]
fn rename_constraint_onto_a_free_name_applies(#[case] constraint: &str, #[case] new_name: &str) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER TABLE con RENAME CONSTRAINT {constraint} TO {new_name};"),
    );

    assert_eq!(
        before.constraint_rename_clash(&rel("con"), constraint, new_name),
        None
    );
    let table = after.table("con");
    assert!(table.get_constraint(constraint).is_none());
    assert!(table.get_constraint(new_name).is_some());
}

#[rstest]
#[case::plain_onto_another_index("con_idx", "neighbour_idx", RenameClash::Relation)]
#[case::plain_onto_a_table("con_idx", "neighbour", RenameClash::Relation)]
#[case::plain_onto_its_own_name("con_idx", "con_idx", RenameClash::Relation)]
#[case::plain_onto_another_tables_exclude("con_idx", "neighbour_excl", RenameClash::Relation)]
#[case::primary_key_index_onto_a_check_of_its_table("con_pk", "con_chk2", RenameClash::Constraint)]
fn rename_index_onto_a_taken_name_changes_nothing(
    #[case] index: &str,
    #[case] new_name: &str,
    #[case] expected_clash: RenameClash,
) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER INDEX {index} RENAME TO {new_name};"),
    );

    assert_eq!(
        before.index_rename_clash(&rel(index), new_name),
        Some(expected_clash)
    );
    after.assert_same_state_as(&before);
}

#[rstest]
#[case::primary_key_index_onto_another_tables_check("con_pk", "neighbour_check")]
#[case::plain_onto_a_check_of_its_table("con_idx", "con_chk2")]
#[case::plain_onto_an_index_in_another_schema("con_idx", "remote_idx")]
fn rename_index_onto_a_free_name_applies(#[case] index: &str, #[case] new_name: &str) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER INDEX {index} RENAME TO {new_name};"),
    );

    assert_eq!(before.index_rename_clash(&rel(index), new_name), None);
    after.assert_index_unregistered(index);
    after.assert_index_owner(new_name, "con");
}

#[test]
fn rename_index_onto_a_taken_name_keeps_both_registrations() {
    let (_, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        "ALTER INDEX con_idx RENAME TO neighbour_idx;",
    );

    after.assert_index_owner("con_idx", "con");
    after.assert_index_owner("neighbour_idx", "neighbour");
}

#[rstest]
#[case::onto_another_table("neighbour")]
#[case::onto_an_index("neighbour_idx")]
#[case::onto_a_unique_constraint("neighbour_key")]
#[case::onto_an_exclude_constraint("neighbour_excl")]
#[case::onto_its_own_name("con")]
fn rename_table_onto_a_taken_name_changes_nothing(#[case] new_name: &str) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER TABLE con RENAME TO {new_name};"),
    );

    assert_eq!(
        before.table_rename_clash(&rel("con"), new_name),
        Some(RenameClash::Relation)
    );
    after.assert_same_state_as(&before);
}

#[rstest]
#[case::onto_a_check_constraint("neighbour_check")]
#[case::onto_a_table_in_another_schema("remote")]
fn rename_table_onto_a_free_name_applies(#[case] new_name: &str) {
    let (before, after) = before_and_after(
        NEIGHBOURS_AND_CON,
        &format!("ALTER TABLE con RENAME TO {new_name};"),
    );

    assert_eq!(before.table_rename_clash(&rel("con"), new_name), None);
    after.assert_no_table("con");
    after.assert_index_owner("con_idx", new_name);
}

#[test]
fn rename_table_onto_a_taken_name_keeps_the_other_table() {
    let (_, after) = before_and_after(NEIGHBOURS_AND_CON, "ALTER TABLE con RENAME TO neighbour;");

    let neighbour = after.table("neighbour");
    assert!(neighbour.get_column("n").is_some());
    assert!(neighbour.get_column("a").is_none());
    after.assert_index_owner("neighbour_idx", "neighbour");
    after.assert_index_owner("con_idx", "con");
}

const ORDERS_IN_BILLING_AND_PUBLIC: &[&str] = &[
    "CREATE TABLE billing.orders (id int PRIMARY KEY, amount int);",
    "CREATE INDEX orders_amount_idx ON billing.orders (amount);",
    "CREATE TABLE orders (id int PRIMARY KEY, amount int);",
];

#[rstest]
#[case::rename_constraint(
    "ALTER TABLE billing.orders RENAME CONSTRAINT orders_pkey TO orders_amount_idx;"
)]
#[case::rename_index("ALTER INDEX billing.orders_pkey RENAME TO orders_amount_idx;")]
#[case::rename_table("ALTER TABLE billing.orders RENAME TO orders_amount_idx;")]
fn rename_in_a_schema_onto_a_name_taken_there_changes_nothing(#[case] rename_sql: &str) {
    let (before, after) = before_and_after(ORDERS_IN_BILLING_AND_PUBLIC, rename_sql);

    after.assert_same_state_as(&before);
}

#[test]
fn clash_queries_look_in_the_renamed_objects_schema() {
    let (catalog, _) = before_and_after(ORDERS_IN_BILLING_AND_PUBLIC, "SELECT 1;");
    let billing_orders = rel_in("billing", "orders");

    assert_eq!(
        catalog.constraint_rename_clash(&billing_orders, "orders_pkey", "orders_amount_idx"),
        Some(RenameClash::Relation)
    );
    assert_eq!(
        catalog.index_rename_clash(&rel_in("billing", "orders_pkey"), "orders_amount_idx"),
        Some(RenameClash::Relation)
    );
    assert_eq!(
        catalog.table_rename_clash(&billing_orders, "orders_amount_idx"),
        Some(RenameClash::Relation)
    );
    assert_eq!(
        catalog.constraint_rename_clash(&rel("orders"), "orders_pkey", "orders_amount_idx"),
        None
    );
    assert_eq!(
        catalog.index_rename_clash(&rel("orders_pkey"), "orders_amount_idx"),
        None
    );
    assert_eq!(
        catalog.table_rename_clash(&rel("orders"), "orders_amount_idx"),
        None
    );
}

#[test]
fn rename_constraint_onto_a_name_taken_only_in_another_schema_applies() {
    let (_, after) = before_and_after(
        ORDERS_IN_BILLING_AND_PUBLIC,
        "ALTER TABLE orders RENAME CONSTRAINT orders_pkey TO orders_amount_idx;",
    );

    after.assert_index_owner("orders_amount_idx", "orders");
    after.assert_index_owner("billing.orders_amount_idx", "billing.orders");
}

#[test]
fn rename_table_onto_a_name_taken_only_in_another_schema_applies() {
    let (_, after) = before_and_after(
        ORDERS_IN_BILLING_AND_PUBLIC,
        "ALTER TABLE orders RENAME TO orders_amount_idx;",
    );

    after.assert_no_table("orders");
    after.assert_table("orders_amount_idx");
    after.assert_table("billing.orders");
}

#[test]
fn a_clash_query_on_a_missing_object_is_none() {
    let (before, _) = before_and_after(NEIGHBOURS_AND_CON, "SELECT 1;");

    assert_eq!(
        before.table_rename_clash(&rel("missing"), "neighbour"),
        None
    );
    assert_eq!(
        before.index_rename_clash(&rel("missing_idx"), "neighbour"),
        None
    );
    assert_eq!(
        before.constraint_rename_clash(&rel("missing"), "con_chk", "con_chk2"),
        None
    );
    assert_eq!(
        before.constraint_rename_clash(&rel("con"), "missing", "con_chk2"),
        None
    );
    assert!(matches!(
        before.table("con").get_constraint("con_excl"),
        Some(ConstraintState::Exclude { .. })
    ));
    assert_eq!(
        before.index_rename_clash(&rel("con_excl"), "free_name"),
        None
    );
}
