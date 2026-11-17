//! `DROP INDEX` of the index behind a PRIMARY KEY or UNIQUE constraint.
//! Postgres rejects it with 2BP01 and changes nothing
//! (`verify/tests/other/pgm027_drop_constraint_index.sql`), so replay
//! changes nothing either.

use rstest::rstest;

use super::rename_constraint::before_and_after;
use crate::catalog::builder::CatalogAssertions;

const ORDERS: &[&str] = &["CREATE TABLE orders (
        id int PRIMARY KEY,
        code text CONSTRAINT orders_code_key UNIQUE,
        ref int,
        email text);
    CREATE UNIQUE INDEX orders_email_idx ON orders (email);
    ALTER TABLE orders ADD CONSTRAINT orders_email_key UNIQUE USING INDEX orders_email_idx;
    CREATE UNIQUE INDEX orders_ref_idx ON orders (ref);"];

#[rstest]
#[case::primary_key("DROP INDEX orders_pkey;")]
#[case::unique("DROP INDEX orders_code_key;")]
#[case::unique_added_using_index("DROP INDEX orders_email_key;")]
#[case::concurrently("DROP INDEX CONCURRENTLY orders_pkey;")]
#[case::if_exists("DROP INDEX IF EXISTS orders_code_key;")]
fn drop_index_of_a_constraints_index_changes_nothing(#[case] drop_sql: &str) {
    let (before, after) = before_and_after(ORDERS, drop_sql);

    after.assert_same_state_as(&before);
}

#[test]
fn drop_index_of_a_unique_index_no_constraint_uses_drops_it() {
    let (before, after) = before_and_after(ORDERS, "DROP INDEX orders_ref_idx;");

    before.assert_index_owner("orders_ref_idx", "orders");
    after.assert_index_unregistered("orders_ref_idx");
    assert!(
        after
            .table("orders")
            .indexes
            .iter()
            .all(|idx| idx.name != "orders_ref_idx")
    );
}

#[test]
fn drop_index_after_drop_constraint_in_the_same_unit_finds_nothing_to_drop() {
    let (before, after) = before_and_after(
        ORDERS,
        "ALTER TABLE orders DROP CONSTRAINT orders_code_key;
         DROP INDEX orders_code_key;",
    );

    before.assert_index_owner("orders_code_key", "orders");
    after.assert_index_unregistered("orders_code_key");
    assert!(after.table("orders").get_constraint("orders_code_key").is_none());
}
