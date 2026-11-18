//! Generated names get a collision suffix when they are already taken, as
//! measured in `verify/tests/naming/collisions.sql`, `collision_scope.sql`,
//! `collision_order.sql` and `truncation.sql`.

use rstest::rstest;

use super::generated_names::assert_names;
use super::replay_sql;
use crate::catalog::builder::CatalogAssertions;
use crate::catalog::types::{Catalog, ConstraintState};

fn constraint_names<'a>(catalog: &'a Catalog, table: &str) -> Vec<&'a str> {
    catalog
        .table(table)
        .constraints
        .iter()
        .map(ConstraintState::name)
        .collect()
}

fn check_names_and_expressions<'a>(catalog: &'a Catalog, table: &str) -> Vec<(&'a str, &'a str)> {
    catalog
        .table(table)
        .constraints
        .iter()
        .filter_map(|c| match c {
            ConstraintState::Check {
                name, expression, ..
            } => Some((name.as_str(), expression.as_str())),
            _ => None,
        })
        .collect()
}

const D63: &str = "d23456789012345678901234567890123456789012345678901234567890123";

#[test]
fn separate_add_unique_statements_count_key_key1_key2() {
    let catalog = replay_sql(&[
        "CREATE TABLE seq_unique (a int);",
        "ALTER TABLE seq_unique ADD UNIQUE (a);",
        "ALTER TABLE seq_unique ADD UNIQUE (a);",
        "ALTER TABLE seq_unique ADD UNIQUE (a);",
    ]);
    let names = ["seq_unique_a_key", "seq_unique_a_key1", "seq_unique_a_key2"];
    assert_names(&catalog, "seq_unique", &names, &names);
}

#[test]
fn separate_create_index_statements_count_idx_idx1_idx2() {
    let catalog = replay_sql(&[
        "CREATE TABLE seq_index (a int);",
        "CREATE INDEX ON seq_index (a);",
        "CREATE INDEX ON seq_index (a);",
        "CREATE INDEX ON seq_index (a);",
    ]);
    assert_names(
        &catalog,
        "seq_index",
        &[],
        &["seq_index_a_idx", "seq_index_a_idx1", "seq_index_a_idx2"],
    );
}

#[test]
fn identical_checks_in_one_create_table_count_check_check1() {
    let catalog = replay_sql(&["CREATE TABLE seq_check (a int CHECK (a > 0), CHECK (a > 0));"]);
    assert_names(
        &catalog,
        "seq_check",
        &["seq_check_a_check", "seq_check_a_check1"],
        &[],
    );
}

#[test]
fn check_suffixes_follow_textual_order() {
    let catalog = replay_sql(&[
        "CREATE TABLE seq_check_order (a int, b int, CHECK (a < b), CHECK (true), CHECK (b > a + 1));",
    ]);
    assert_eq!(
        check_names_and_expressions(&catalog, "seq_check_order"),
        [
            ("seq_check_order_check", "a < b"),
            ("seq_check_order_check1", "true"),
            ("seq_check_order_check2", "b > (a + 1)"),
        ]
    );
}

#[test]
fn duplicate_foreign_keys_in_one_create_table_count_fkey_fkey1() {
    let catalog = replay_sql(&[
        "CREATE TABLE ref (id int PRIMARY KEY);",
        "CREATE TABLE seq_fk (a int REFERENCES ref, FOREIGN KEY (a) REFERENCES ref);",
    ]);
    assert_names(
        &catalog,
        "seq_fk",
        &["seq_fk_a_fkey", "seq_fk_a_fkey1"],
        &[],
    );
}

#[test]
fn two_add_unique_in_one_alter_table_count_key_key1() {
    let catalog = replay_sql(&[
        "CREATE TABLE seq_one_alter (a int);",
        "ALTER TABLE seq_one_alter ADD UNIQUE (a), ADD UNIQUE (a);",
    ]);
    let names = ["seq_one_alter_a_key", "seq_one_alter_a_key1"];
    assert_names(&catalog, "seq_one_alter", &names, &names);
}

#[test]
fn explicit_check_name_earlier_in_create_table_pushes_the_generated_one_to_check1() {
    let catalog = replay_sql(&[
        "CREATE TABLE explicit_first (CONSTRAINT explicit_first_a_check CHECK (a < 10), a int CHECK (a > 0));",
    ]);
    assert_eq!(
        check_names_and_expressions(&catalog, "explicit_first"),
        [
            ("explicit_first_a_check", "a < 10"),
            ("explicit_first_a_check1", "a > 0"),
        ]
    );
}

#[rstest]
#[case::check(
    "CREATE TABLE bar_check (a int, CONSTRAINT foo_check_a_check CHECK (a > 0));",
    "CREATE TABLE foo_check (a int CHECK (a > 0));",
    "foo_check",
    &["foo_check_a_check1"],
    &[]
)]
#[case::foreign_key(
    "CREATE TABLE bar_fk (a int, CONSTRAINT foo_fk_a_fkey FOREIGN KEY (a) REFERENCES ref);",
    "CREATE TABLE foo_fk (a int REFERENCES ref);",
    "foo_fk",
    &["foo_fk_a_fkey1"],
    &[]
)]
#[case::unique(
    "CREATE TABLE bar_uq (a int, CONSTRAINT foo_uq_a_key UNIQUE (a));",
    "CREATE TABLE foo_uq (a int UNIQUE);",
    "foo_uq",
    &["foo_uq_a_key1"],
    &["foo_uq_a_key1"]
)]
#[case::unique_avoids_another_tables_check(
    "CREATE TABLE bar_kind1 (a int, CONSTRAINT foo_kind1_a_key CHECK (a > 0));",
    "CREATE TABLE foo_kind1 (a int UNIQUE);",
    "foo_kind1",
    &["foo_kind1_a_key1"],
    &["foo_kind1_a_key1"]
)]
#[case::check_avoids_another_tables_unique(
    "CREATE TABLE bar_kind2 (a int, CONSTRAINT foo_kind2_a_check UNIQUE (a));",
    "CREATE TABLE foo_kind2 (a int CHECK (a > 0));",
    "foo_kind2",
    &["foo_kind2_a_check1"],
    &[]
)]
#[case::primary_key_avoids_another_tables_check(
    "CREATE TABLE bar_kind3 (a int, CONSTRAINT foo_kind3_pkey CHECK (a > 0));",
    "CREATE TABLE foo_kind3 (a int, PRIMARY KEY (a));",
    "foo_kind3",
    &["foo_kind3_pkey1"],
    &["foo_kind3_pkey1"]
)]
#[case::exclude_avoids_another_tables_check(
    "CREATE TABLE bar_kind4 (a int, CONSTRAINT foo_kind4_a_excl CHECK (a > 0));",
    "CREATE TABLE foo_kind4 (a int, EXCLUDE USING btree (a WITH =));",
    "foo_kind4",
    &["foo_kind4_a_excl1"],
    &[]
)]
#[case::table_and_column_split_differently(
    "CREATE TABLE app_user (id int UNIQUE);",
    "CREATE TABLE app (user_id int UNIQUE);",
    "app",
    &["app_user_id_key1"],
    &["app_user_id_key1"]
)]
fn generated_constraint_name_avoids_constraint_names_of_other_tables_in_the_schema(
    #[case] other_table: &str,
    #[case] table_sql: &str,
    #[case] table: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    let catalog = replay_sql(&[
        "CREATE TABLE ref (id int PRIMARY KEY);",
        other_table,
        table_sql,
    ]);
    assert_names(&catalog, table, constraints, indexes);
}

#[test]
fn constraint_and_relation_names_in_another_schema_are_ignored() {
    let catalog = replay_sql(&[
        "CREATE TABLE ref (id int PRIMARY KEY);",
        "CREATE TABLE other.bar_iso (a int,
            CONSTRAINT foo_iso_a_check CHECK (a > 0),
            CONSTRAINT foo_iso_a_key UNIQUE (a),
            CONSTRAINT foo_iso_a_fkey FOREIGN KEY (a) REFERENCES ref);",
        "CREATE TABLE other.foo_iso_pk_pkey (x int);",
        "CREATE TABLE foo_iso (a int CHECK (a > 0) UNIQUE REFERENCES ref);",
        "CREATE TABLE foo_iso_pk (a int, PRIMARY KEY (a));",
    ]);
    assert_names(
        &catalog,
        "foo_iso",
        &["foo_iso_a_check", "foo_iso_a_key", "foo_iso_a_fkey"],
        &["foo_iso_a_key"],
    );
    assert_names(
        &catalog,
        "foo_iso_pk",
        &["foo_iso_pk_pkey"],
        &["foo_iso_pk_pkey"],
    );
}

#[rstest]
#[case::unique_avoids_a_table(
    "CREATE TABLE rel_uq_a_key (x int);",
    "CREATE TABLE rel_uq (a int UNIQUE);",
    "rel_uq",
    &["rel_uq_a_key1"],
    &["rel_uq_a_key1"]
)]
#[case::primary_key_avoids_a_table(
    "CREATE TABLE rel_pk_pkey (x int);",
    "CREATE TABLE rel_pk (a int, PRIMARY KEY (a));",
    "rel_pk",
    &["rel_pk_pkey1"],
    &["rel_pk_pkey1"]
)]
#[case::exclude_avoids_an_index(
    "CREATE TABLE holder (x int); CREATE INDEX rel_ex_a_excl ON holder (x);",
    "CREATE TABLE rel_ex (a int, EXCLUDE USING btree (a WITH =));",
    "rel_ex",
    &["rel_ex_a_excl1"],
    &[]
)]
#[case::check_ignores_a_table(
    "CREATE TABLE rel_ck_a_check (x int);",
    "CREATE TABLE rel_ck (a int CHECK (a > 0));",
    "rel_ck",
    &["rel_ck_a_check"],
    &[]
)]
#[case::foreign_key_ignores_an_index(
    "CREATE TABLE rel_fk_holder (x int); CREATE INDEX rel_fk_a_fkey ON rel_fk_holder (x);",
    "CREATE TABLE rel_fk (a int REFERENCES ref);",
    "rel_fk",
    &["rel_fk_a_fkey"],
    &[]
)]
fn index_backed_constraints_avoid_relation_names_and_others_do_not(
    #[case] relation_sql: &str,
    #[case] table_sql: &str,
    #[case] table: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    let catalog = replay_sql(&[
        "CREATE TABLE ref (id int PRIMARY KEY);",
        relation_sql,
        table_sql,
    ]);
    assert_names(&catalog, table, constraints, indexes);
}

#[test]
fn create_index_ignores_constraint_names() {
    let catalog = replay_sql(&[
        "CREATE TABLE ix (a int, CONSTRAINT ix_a_idx CHECK (a > 0));",
        "CREATE INDEX ON ix (a);",
    ]);
    assert_names(&catalog, "ix", &["ix_a_idx"], &["ix_a_idx"]);
}

#[test]
fn create_index_skips_a_name_taken_by_a_table() {
    let catalog = replay_sql(&[
        "CREATE TABLE ci (a int);",
        "CREATE TABLE ci_a_idx (x int);",
        "CREATE INDEX ON ci (a);",
    ]);
    assert_names(&catalog, "ci", &[], &["ci_a_idx1"]);
}

#[test]
fn create_index_skips_names_taken_by_a_table_and_another_tables_index() {
    let catalog = replay_sql(&[
        "CREATE TABLE ci2 (a int);",
        "CREATE TABLE ci2_a_idx (x int);",
        "CREATE INDEX ci2_a_idx1 ON ci2_a_idx (x);",
        "CREATE INDEX ON ci2 (a);",
    ]);
    assert_names(&catalog, "ci2", &[], &["ci2_a_idx2"]);
    catalog.assert_index_owner("ci2_a_idx1", "ci2_a_idx");
}

#[test]
fn create_index_skips_the_name_of_an_exclude_constraints_index() {
    let catalog = replay_sql(&[
        "CREATE TABLE ex (a int, CONSTRAINT ex_a_idx EXCLUDE USING btree (a WITH =));",
        "CREATE INDEX ON ex (a);",
    ]);
    assert_names(&catalog, "ex", &["ex_a_idx"], &["ex_a_idx1"]);
}

#[test]
fn unnamed_index_gets_a_suffix_when_an_explicit_index_has_its_name() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int, b int);",
        "CREATE INDEX t_a_idx ON t (b);",
        "CREATE INDEX ON t (a);",
    ]);
    assert_names(&catalog, "t", &[], &["t_a_idx", "t_a_idx1"]);
    let index = &catalog.table("t").indexes[1];
    assert_eq!(index.column_names().collect::<Vec<_>>(), ["a"]);
}

#[test]
fn drop_index_by_generated_name_removes_only_that_index() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int);",
        "CREATE INDEX ON t (a);",
        "CREATE INDEX ON t (a);",
        "DROP INDEX t_a_idx;",
    ]);
    assert_names(&catalog, "t", &[], &["t_a_idx1"]);
    catalog.assert_index_unregistered("t_a_idx");
}

#[test]
fn a_dropped_generated_name_is_the_first_free_one_again() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int);",
        "CREATE INDEX ON t (a);",
        "CREATE INDEX ON t (a);",
        "DROP INDEX t_a_idx;",
        "CREATE INDEX ON t (a);",
    ]);
    assert_names(&catalog, "t", &[], &["t_a_idx1", "t_a_idx"]);
}

#[test]
fn new_table_reusing_a_renamed_tables_name_gets_suffix_1_everywhere() {
    let catalog = replay_sql(&[
        "CREATE TABLE customers (id int PRIMARY KEY);",
        "CREATE TABLE orders (id int, code text UNIQUE, qty int CHECK (qty > 0),
            customer_id int REFERENCES customers, PRIMARY KEY (id));",
        "CREATE INDEX ON orders (qty);",
        "ALTER TABLE orders RENAME TO purchases;",
        "CREATE TABLE orders (id int, code text UNIQUE, qty int CHECK (qty > 0),
            customer_id int REFERENCES customers, PRIMARY KEY (id));",
        "CREATE INDEX ON orders (qty);",
    ]);
    assert_names(
        &catalog,
        "orders",
        &[
            "orders_code_key1",
            "orders_qty_check1",
            "orders_customer_id_fkey1",
            "orders_pkey1",
        ],
        &["orders_code_key1", "orders_pkey1", "orders_qty_idx1"],
    );
}

#[test]
fn create_table_overwriting_a_table_frees_the_old_tables_names() {
    let catalog = replay_sql(&[
        "CREATE TABLE t (a int UNIQUE, b int);",
        "CREATE INDEX ON t (b);",
        "CREATE TABLE t (a int UNIQUE, b int);",
        "CREATE INDEX ON t (b);",
    ]);
    assert_names(&catalog, "t", &["t_a_key"], &["t_a_key", "t_b_idx"]);
}

#[test]
fn inline_primary_key_whose_pkey_name_is_taken_is_registered_once_as_pkey1() {
    let catalog = replay_sql(&[
        "CREATE TABLE t_pkey (x int);",
        "CREATE TABLE t (id int PRIMARY KEY);",
        "CREATE TABLE u_pkey (x int);",
        "CREATE TABLE u (a int);",
        "ALTER TABLE u ADD COLUMN id int PRIMARY KEY;",
    ]);
    assert_names(&catalog, "t", &["t_pkey1"], &["t_pkey1"]);
    assert_names(&catalog, "u", &["u_pkey1"], &["u_pkey1"]);
}

#[rstest]
#[case::check_before_unique(
    "CREATE TABLE ct_check_first (a int UNIQUE, CONSTRAINT ct_check_first_a_key CHECK (a > 0));",
    "ct_check_first",
    &["ct_check_first_a_key1", "ct_check_first_a_key"],
    &["ct_check_first_a_key1"]
)]
#[case::primary_key_before_unique(
    "CREATE TABLE ct_pk_first (a int UNIQUE, b int, CONSTRAINT ct_pk_first_a_key PRIMARY KEY (b));",
    "ct_pk_first",
    &["ct_pk_first_a_key1", "ct_pk_first_a_key"],
    &["ct_pk_first_a_key1", "ct_pk_first_a_key"]
)]
#[case::unique_and_exclude_in_textual_order(
    "CREATE TABLE ct_unique_textual (a int, b int,
        CONSTRAINT ct_unique_textual_a_excl UNIQUE (b), EXCLUDE USING btree (a WITH =));",
    "ct_unique_textual",
    &["ct_unique_textual_a_excl", "ct_unique_textual_a_excl1"],
    &["ct_unique_textual_a_excl"]
)]
#[case::foreign_key_after_unique(
    "CREATE TABLE ct_fk_after_index (a int REFERENCES ref, CONSTRAINT ct_fk_after_index_a_fkey UNIQUE (a));",
    "ct_fk_after_index",
    &["ct_fk_after_index_a_fkey1", "ct_fk_after_index_a_fkey"],
    &["ct_fk_after_index_a_fkey"]
)]
#[case::foreign_key_after_check(
    "CREATE TABLE ct_fk_after_check (a int REFERENCES ref, CONSTRAINT ct_fk_after_check_a_fkey CHECK (a > 0));",
    "ct_fk_after_check",
    &["ct_fk_after_check_a_fkey1", "ct_fk_after_check_a_fkey"],
    &[]
)]
fn create_table_names_constraints_in_creation_order_and_keeps_textual_order(
    #[case] sql: &str,
    #[case] table: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    let catalog = replay_sql(&["CREATE TABLE ref (id int PRIMARY KEY);", sql]);
    assert_names(&catalog, table, constraints, indexes);
}

#[rstest]
#[case::unique_before_check(
    "CREATE TABLE t (a int, b int);",
    "ALTER TABLE t ADD CHECK (a > 0), ADD CONSTRAINT t_a_check UNIQUE (b);",
    &["t_a_check1", "t_a_check"],
    &["t_a_check"]
)]
#[case::unique_before_foreign_key(
    "CREATE TABLE t (a int, b int);",
    "ALTER TABLE t ADD FOREIGN KEY (a) REFERENCES ref, ADD CONSTRAINT t_a_fkey UNIQUE (a);",
    &["t_a_fkey1", "t_a_fkey"],
    &["t_a_fkey"]
)]
#[case::check_and_foreign_key_in_textual_order(
    "CREATE TABLE t (a int, b int);",
    "ALTER TABLE t ADD CONSTRAINT t_a_check FOREIGN KEY (a) REFERENCES ref, ADD CHECK (a > 0);",
    &["t_a_check", "t_a_check1"],
    &[]
)]
#[case::drop_written_after_add_runs_first(
    "CREATE TABLE t (a int UNIQUE, b int);",
    "ALTER TABLE t ADD UNIQUE (a), DROP CONSTRAINT t_a_key;",
    &["t_a_key"],
    &["t_a_key"]
)]
#[case::drop_written_before_add(
    "CREATE TABLE t (a int UNIQUE, b int);",
    "ALTER TABLE t DROP CONSTRAINT t_a_key, ADD UNIQUE (a);",
    &["t_a_key"],
    &["t_a_key"]
)]
// collision_order.sql:102
#[case::add_column_check_before_add_check(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD CHECK (a > 0), ADD COLUMN c int CONSTRAINT t_a_check CHECK (c > 0);",
    &["t_a_check1", "t_a_check"],
    &[]
)]
// collision_order.sql:108
#[case::add_column_unique_before_add_unique(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD UNIQUE (b) DEFERRABLE, ADD COLUMN b int UNIQUE;",
    &["t_b_key1", "t_b_key"],
    &["t_b_key1", "t_b_key"]
)]
// collision_order.sql:114
#[case::add_column_generated_check_before_add_check(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD CHECK (b > 0), ADD COLUMN b int CHECK (b > 1);",
    &["t_b_check1", "t_b_check"],
    &[]
)]
// collision_order.sql:120
#[case::add_column_foreign_key_before_add_foreign_key(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD FOREIGN KEY (b) REFERENCES ref ON DELETE CASCADE, ADD COLUMN b int REFERENCES ref;",
    &["t_b_fkey1", "t_b_fkey"],
    &[]
)]
// collision_order.sql:126
#[case::add_column_foreign_key_before_add_check(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD CHECK (a > 0), ADD COLUMN c int CONSTRAINT t_a_check REFERENCES ref;",
    &["t_a_check1", "t_a_check"],
    &[]
)]
// collision_order.sql:132
#[case::add_constraint_unique_before_add_column_check(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD CONSTRAINT t_c_check UNIQUE (a), ADD COLUMN c int CHECK (c > 0);",
    &["t_c_check", "t_c_check1"],
    &["t_c_check"]
)]
// collision_order.sql:137
#[case::check_before_foreign_key_within_one_add_column(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD COLUMN c int REFERENCES ref CONSTRAINT t_c_fkey CHECK (c > 0);",
    &["t_c_fkey1", "t_c_fkey"],
    &[]
)]
// collision_order.sql:142
#[case::earlier_add_column_foreign_key_before_later_add_column_check(
    "CREATE TABLE t (a int);",
    "ALTER TABLE t ADD COLUMN c int CONSTRAINT t_d_check REFERENCES ref, ADD COLUMN d int CHECK (d > 0);",
    &["t_d_check", "t_d_check1"],
    &[]
)]
fn alter_table_names_constraints_in_creation_order_and_keeps_textual_order(
    #[case] create_sql: &str,
    #[case] alter_sql: &str,
    #[case] constraints: &[&str],
    #[case] indexes: &[&str],
) {
    let catalog = replay_sql(&[
        "CREATE TABLE ref (id int PRIMARY KEY);",
        create_sql,
        alter_sql,
    ]);
    assert_names(&catalog, "t", constraints, indexes);
}

#[test]
fn alter_table_attaches_using_index_constraints_before_naming_other_index_backed_ones() {
    let catalog = replay_sql(&[
        "CREATE TABLE at_using_first (a int, b int NOT NULL);",
        "CREATE UNIQUE INDEX at_using_first_idx ON at_using_first (b);",
        "ALTER TABLE at_using_first ADD UNIQUE (a),
            ADD CONSTRAINT at_using_first_a_key UNIQUE USING INDEX at_using_first_idx;",
    ]);
    assert_names(
        &catalog,
        "at_using_first",
        &["at_using_first_a_key1", "at_using_first_a_key"],
        &["at_using_first_a_key", "at_using_first_a_key1"],
    );
    catalog.assert_index_unregistered("at_using_first_idx");
}

#[test]
fn suffix_on_a_63_byte_table_cuts_the_table_part_one_byte_more() {
    let catalog = replay_sql(&[
        &format!("CREATE TABLE {D63} (a int);"),
        &format!("ALTER TABLE {D63} ADD UNIQUE (a);"),
        &format!("ALTER TABLE {D63} ADD UNIQUE (a);"),
        &format!("CREATE INDEX ON {D63} (a);"),
        &format!("CREATE INDEX ON {D63} (a);"),
    ]);
    assert_names(
        &catalog,
        D63,
        &[
            "d23456789012345678901234567890123456789012345678901234567_a_key",
            "d2345678901234567890123456789012345678901234567890123456_a_key1",
        ],
        &[
            "d23456789012345678901234567890123456789012345678901234567_a_key",
            "d2345678901234567890123456789012345678901234567890123456_a_key1",
            "d23456789012345678901234567890123456789012345678901234567_a_idx",
            "d2345678901234567890123456789012345678901234567890123456_a_idx1",
        ],
    );
}

#[test]
fn long_tables_sharing_a_truncated_prefix_collide_into_pkey1() {
    let aaa = "z23456789012345678901234567890123456789012345678901234567890aaa";
    let bbb = "z23456789012345678901234567890123456789012345678901234567890bbb";
    let catalog = replay_sql(&[
        &format!("CREATE TABLE {aaa} (id int PRIMARY KEY);"),
        &format!("CREATE TABLE {bbb} (id int PRIMARY KEY);"),
    ]);
    let first = "z234567890123456789012345678901234567890123456789012345678_pkey";
    let second = "z23456789012345678901234567890123456789012345678901234567_pkey1";
    catalog.assert_index_owner(first, aaa);
    catalog.assert_index_owner(second, bbb);
    assert_eq!(constraint_names(&catalog, bbb), [second]);
}
