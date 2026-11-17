//! An inline `PRIMARY KEY` is one constraint with one backing index, named
//! or not, whether CREATE TABLE or ADD COLUMN declares it.

use rstest::rstest;

use super::generated_names::assert_names;
use super::replay_sql;
use crate::catalog::builder::CatalogAssertions;

#[rstest]
#[case::create_table(&["CREATE TABLE t (id int PRIMARY KEY);"])]
#[case::add_column(&[
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD COLUMN id int PRIMARY KEY;",
])]
fn unnamed_inline_primary_key_is_one_pkey_constraint_and_index(#[case] sql: &[&str]) {
    let catalog = replay_sql(sql);

    assert_names(&catalog, "t", &["t_pkey"], &["t_pkey"]);
    assert!(catalog.table("t").has_primary_key);
}

#[rstest]
#[case::create_table(&["CREATE TABLE t (id int CONSTRAINT foo PRIMARY KEY);"])]
#[case::add_column(&[
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD COLUMN id int CONSTRAINT foo PRIMARY KEY;",
])]
fn named_inline_primary_key_is_only_its_own_constraint_and_index(#[case] sql: &[&str]) {
    let catalog = replay_sql(sql);

    assert_names(&catalog, "t", &["foo"], &["foo"]);
    assert!(catalog.table("t").has_primary_key);
    catalog.assert_index_unregistered("t_pkey");
}

#[rstest]
#[case::create_table(&["CREATE TABLE t (id int CONSTRAINT foo PRIMARY KEY);"])]
#[case::add_column(&[
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD COLUMN id int CONSTRAINT foo PRIMARY KEY;",
])]
fn drop_constraint_of_a_named_inline_primary_key_leaves_no_primary_key(#[case] sql: &[&str]) {
    let mut steps = sql.to_vec();
    steps.push("ALTER TABLE t DROP CONSTRAINT foo;");
    let catalog = replay_sql(&steps);

    assert_names(&catalog, "t", &[], &[]);
    assert!(!catalog.table("t").has_primary_key);
    catalog.assert_index_unregistered("foo");
    catalog.assert_index_unregistered("t_pkey");
}

#[rstest]
#[case::after_renaming_the_table_away(&[
    "CREATE TABLE t (id int CONSTRAINT foo PRIMARY KEY);",
    "ALTER TABLE t RENAME TO t_old;",
    "CREATE TABLE t (id int PRIMARY KEY);",
])]
#[case::after_dropping_the_named_key(&[
    "CREATE TABLE t (id int CONSTRAINT foo PRIMARY KEY);",
    "ALTER TABLE t DROP CONSTRAINT foo;",
    "ALTER TABLE t ADD PRIMARY KEY (id);",
])]
fn named_inline_primary_key_leaves_the_pkey_name_free_for_a_later_unsuffixed_key(
    #[case] sql: &[&str],
) {
    let catalog = replay_sql(sql);

    assert_names(&catalog, "t", &["t_pkey"], &["t_pkey"]);
}
