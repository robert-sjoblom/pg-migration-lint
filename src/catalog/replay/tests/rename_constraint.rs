//! `ALTER TABLE ... RENAME CONSTRAINT`. The Postgres behaviour pinned here is
//! measured in `verify/tests/naming/rename.sql`.

use rstest::rstest;

use super::{replay_sql, steps_from_sql};
use crate::catalog::builder::{CatalogAssertions, TEST_DEFAULT_SCHEMA};
use crate::catalog::generated_names::TakenNames;
use crate::catalog::replay::apply;
use crate::catalog::types::{Catalog, ConstraintState, TableState};

fn index_names(table: &TableState) -> Vec<&str> {
    table.indexes.iter().map(|idx| idx.name.as_str()).collect()
}

fn constraint_names(table: &TableState) -> Vec<&str> {
    table.constraints.iter().map(ConstraintState::name).collect()
}

const REFERENCED_TABLE: &str = "CREATE TABLE p (id int PRIMARY KEY);";

#[rstest]
#[case::inline_primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::named_primary_key("CREATE TABLE t (id int, CONSTRAINT my_pk PRIMARY KEY (id));", "my_pk")]
#[case::unnamed_unique("CREATE TABLE t (a int UNIQUE);", "t_a_key")]
#[case::unique_added_using_index(
    "CREATE TABLE t (a int);
     CREATE UNIQUE INDEX t_a_idx ON t (a);
     ALTER TABLE t ADD UNIQUE USING INDEX t_a_idx;",
    "t_a_idx"
)]
fn rename_constraint_renames_the_index_it_is_backed_by(
    #[case] create_sql: &str,
    #[case] constraint_name: &str,
) {
    let catalog = replay_sql(&[
        create_sql,
        &format!("ALTER TABLE t RENAME CONSTRAINT {constraint_name} TO t_renamed;"),
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), ["t_renamed"]);
    assert_eq!(index_names(table), ["t_renamed"]);
    catalog.assert_index_owner("t_renamed", "t");
    catalog.assert_index_unregistered(constraint_name);
}

#[test]
fn rename_constraint_keeps_the_primary_key() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (id int PRIMARY KEY);",
        "ALTER TABLE t RENAME CONSTRAINT t_pkey TO t_renamed;",
    ]);

    let table = catalog.table("t");
    assert!(table.has_primary_key);
    assert!(matches!(
        table.get_constraint("t_renamed"),
        Some(ConstraintState::PrimaryKey { columns, .. }) if columns == &["id"]
    ));
}

#[test]
fn renamed_exclude_constraint_takes_its_unrecorded_index_name_along() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int, EXCLUDE USING btree (a WITH =));",
        "ALTER TABLE t RENAME CONSTRAINT t_a_excl TO t_renamed;",
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), ["t_renamed"]);
    assert_eq!(index_names(table), Vec::<&str>::new());
    catalog.assert_index_unregistered("t_renamed");
    let schema_names = catalog.names_in_schema(TEST_DEFAULT_SCHEMA);
    assert!(schema_names.relation_taken("t_renamed"));
    assert!(!schema_names.relation_taken("t_a_excl"));
}

#[rstest]
#[case::check("CREATE TABLE t (a int CHECK (a > 0));", "t_a_check")]
#[case::foreign_key("CREATE TABLE t (a int REFERENCES p);", "t_a_fkey")]
fn rename_constraint_renames_a_check_or_foreign_key_alone(
    #[case] create_sql: &str,
    #[case] constraint_name: &str,
) {
    let catalog = replay_sql(&[
        REFERENCED_TABLE,
        create_sql,
        &format!("ALTER TABLE t RENAME CONSTRAINT {constraint_name} TO t_renamed;"),
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), ["t_renamed"]);
    assert_eq!(index_names(table), Vec::<&str>::new());
    catalog.assert_index_unregistered("t_renamed");
}

#[test]
fn rename_constraint_on_a_check_leaves_an_index_of_the_same_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE ix (a int, CONSTRAINT ix_a_idx CHECK (a > 0));",
        "CREATE INDEX ON ix (a);",
        "ALTER TABLE ix RENAME CONSTRAINT ix_a_idx TO ix_a_positive;",
    ]);

    let table = catalog.table("ix");
    assert_eq!(constraint_names(table), ["ix_a_positive"]);
    assert_eq!(index_names(table), ["ix_a_idx"]);
    catalog.assert_index_owner("ix_a_idx", "ix");
}

#[rstest]
#[case::primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::unique("CREATE TABLE t (a int UNIQUE);", "t_a_key")]
#[case::exclude("CREATE TABLE t (a int, EXCLUDE USING btree (a WITH =));", "t_a_excl")]
#[case::check("CREATE TABLE t (a int CHECK (a > 0));", "t_a_check")]
#[case::foreign_key("CREATE TABLE t (a int REFERENCES p);", "t_a_fkey")]
fn drop_constraint_by_the_new_name_drops_a_renamed_constraint(
    #[case] create_sql: &str,
    #[case] constraint_name: &str,
) {
    let catalog = replay_sql(&[
        REFERENCED_TABLE,
        create_sql,
        &format!("ALTER TABLE t RENAME CONSTRAINT {constraint_name} TO t_renamed;"),
        "ALTER TABLE t DROP CONSTRAINT t_renamed;",
    ]);

    let table = catalog.table("t");
    assert_eq!(constraint_names(table), Vec::<&str>::new());
    assert_eq!(index_names(table), Vec::<&str>::new());
    assert!(!table.has_primary_key);
    catalog.assert_index_unregistered("t_renamed");
    catalog.assert_index_unregistered(constraint_name);
}

#[rstest]
#[case::primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::unique("CREATE TABLE t (a int UNIQUE);", "t_a_key")]
fn drop_index_by_the_new_name_is_rejected_as_the_renamed_constraint_requires_it(
    #[case] create_sql: &str,
    #[case] constraint_name: &str,
) {
    let (before, after) = before_and_after(
        &[
            create_sql,
            &format!("ALTER TABLE t RENAME CONSTRAINT {constraint_name} TO t_renamed;"),
        ],
        "DROP INDEX t_renamed;",
    );

    before.assert_index_owner("t_renamed", "t");
    assert!(
        before
            .table("t")
            .constraint_backed_by("t_renamed")
            .is_some()
    );
    after.assert_same_state_as(&before);
}

#[rstest]
#[case::primary_key("CREATE TABLE t (id int PRIMARY KEY);", "t_pkey")]
#[case::check("CREATE TABLE t (a int CHECK (a > 0));", "t_a_check")]
fn drop_constraint_by_the_old_name_after_a_rename_changes_nothing(
    #[case] create_sql: &str,
    #[case] constraint_name: &str,
) {
    let catalog = replay_sql(&[
        create_sql,
        &format!("ALTER TABLE t RENAME CONSTRAINT {constraint_name} TO t_renamed;"),
        &format!("ALTER TABLE t DROP CONSTRAINT {constraint_name};"),
    ]);

    assert_eq!(constraint_names(catalog.table("t")), ["t_renamed"]);
}

#[test]
fn rename_table_then_its_primary_key_then_drop_it_by_the_new_name() {
    let steps = [
        "CREATE TABLE legacy (id int PRIMARY KEY);",
        "ALTER TABLE legacy RENAME TO modern;",
        "ALTER TABLE modern RENAME CONSTRAINT legacy_pkey TO modern_pkey;",
    ];
    let renamed = replay_sql(&steps);
    let dropped = replay_sql(&[&steps[..], &["ALTER TABLE modern DROP CONSTRAINT modern_pkey;"]].concat());

    assert_eq!(constraint_names(renamed.table("modern")), ["modern_pkey"]);
    renamed.assert_index_owner("modern_pkey", "modern");
    renamed.assert_index_unregistered("legacy_pkey");
    let table = dropped.table("modern");
    assert_eq!(constraint_names(table), Vec::<&str>::new());
    assert_eq!(index_names(table), Vec::<&str>::new());
    assert!(!table.has_primary_key);
    dropped.assert_index_unregistered("modern_pkey");
}

#[test]
fn rename_constraint_on_a_schema_qualified_table_keeps_the_index_in_that_schema() {
    let catalog = replay_sql(&[
        "CREATE TABLE billing.legacy (id int PRIMARY KEY);",
        "ALTER TABLE billing.legacy RENAME TO modern;",
        "ALTER TABLE billing.modern RENAME CONSTRAINT legacy_pkey TO modern_pkey;",
    ]);

    assert_eq!(
        constraint_names(catalog.table("billing.modern")),
        ["modern_pkey"]
    );
    catalog.assert_index_owner("billing.modern_pkey", "billing.modern");
    catalog.assert_index_unregistered("billing.legacy_pkey");
    catalog.assert_index_unregistered("modern_pkey");
}

#[rstest]
#[case::qualified(
    "analytics.orders",
    "data.orders",
    "ALTER TABLE analytics.orders RENAME CONSTRAINT orders_pkey TO orders_pk;"
)]
#[case::unqualified_in_the_default_schema(
    "orders",
    "analytics.orders",
    "ALTER TABLE orders RENAME CONSTRAINT orders_pkey TO orders_pk;"
)]
fn rename_constraint_in_one_schema_leaves_the_same_named_constraint_in_another(
    #[case] renamed_on: &str,
    #[case] untouched_on: &str,
    #[case] rename_sql: &str,
) {
    let catalog = replay_sql(&[
        &format!("CREATE TABLE {renamed_on} (id int PRIMARY KEY);"),
        &format!("CREATE TABLE {untouched_on} (id int PRIMARY KEY);"),
        rename_sql,
    ]);

    let renamed_schema = &catalog.table(renamed_on).name.schema;
    let untouched_schema = &catalog.table(untouched_on).name.schema;
    assert_eq!(constraint_names(catalog.table(renamed_on)), ["orders_pk"]);
    catalog.assert_index_owner(&format!("{renamed_schema}.orders_pk"), renamed_on);
    catalog.assert_index_unregistered(&format!("{renamed_schema}.orders_pkey"));
    assert_eq!(
        constraint_names(catalog.table(untouched_on)),
        ["orders_pkey"]
    );
    catalog.assert_index_owner(&format!("{untouched_schema}.orders_pkey"), untouched_on);
    catalog.assert_index_unregistered(&format!("{untouched_schema}.orders_pk"));
}

/// Replay `setup`, then `statement` on top of it, and return both catalogs.
pub(super) fn before_and_after(setup: &[&str], statement: &str) -> (Catalog, Catalog) {
    let before = replay_sql(setup);
    let mut after = before.clone();
    for unit in steps_from_sql(&[statement], TEST_DEFAULT_SCHEMA) {
        apply(&mut after, &unit);
    }
    (before, after)
}

#[rstest]
#[case::table_missing_with_if_exists(
    "ALTER TABLE IF EXISTS missing RENAME CONSTRAINT orders_pkey TO orders_pk;"
)]
#[case::table_missing("ALTER TABLE missing RENAME CONSTRAINT orders_pkey TO orders_pk;")]
#[case::table_only_another_schema_has(
    "ALTER TABLE orders RENAME CONSTRAINT orders_pkey TO orders_pk;"
)]
#[case::constraint_missing_with_if_exists(
    "ALTER TABLE IF EXISTS analytics.orders RENAME CONSTRAINT missing TO orders_pk;"
)]
#[case::constraint_missing("ALTER TABLE analytics.orders RENAME CONSTRAINT missing TO orders_pk;")]
fn rename_of_a_missing_table_or_constraint_changes_nothing(#[case] rename_sql: &str) {
    let (before, after) = before_and_after(
        &["CREATE TABLE analytics.orders (id int PRIMARY KEY, CHECK (id > 0));"],
        rename_sql,
    );

    after.assert_same_state_as(&before);
}
