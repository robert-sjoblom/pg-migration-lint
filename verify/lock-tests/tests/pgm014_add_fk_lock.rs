//! PGM014 — ADD FOREIGN KEY without NOT VALID acquires SHARE ROW EXCLUSIVE lock (PGM014)
//!
//! ADD FK NOT VALID allows invalid data to exist, constraint marked unvalidated
//!
//! Ported from `verify/tests/locks/pgm014_add_fk_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_allows, assert_lock_blocks, detail};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_parent(id int PRIMARY KEY);
    CREATE TABLE test_child(id int, parent_id int);
    INSERT INTO test_parent SELECT g FROM generate_series(1, 100) g;
    INSERT INTO test_child SELECT g, g FROM generate_series(1, 100) g;
";

/// A child row whose `parent_id` has no matching parent, so a plain ADD FK would fail.
const INSERT_INVALID_ROW: &str = "INSERT INTO test_child VALUES (999, 99999)";

#[rstest]
fn add_fk_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_fk_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_child ADD CONSTRAINT fk_parent FOREIGN KEY (parent_id) REFERENCES test_parent(id)",
        "INSERT INTO test_child VALUES (999, 1)",
        "ADD FK blocks INSERT on child (ShareRowExclusive)",
    );
}

#[rstest]
fn add_fk_allows_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_fk_allows_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        "ALTER TABLE test_child ADD CONSTRAINT fk_parent2 FOREIGN KEY (parent_id) REFERENCES test_parent(id)",
        "SELECT count(*) FROM test_child",
        "ADD FK allows SELECT on child (ShareRowExclusive vs AccessShare)",
    );
}

#[rstest]
fn add_fk_not_valid_succeeds_with_invalid_data(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_fk_nv_succeeds") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run(INSERT_INVALID_ROW);

    s.try_run(
        "ALTER TABLE test_child ADD CONSTRAINT fk_nv FOREIGN KEY (parent_id)
             REFERENCES test_parent(id) NOT VALID",
    )
    .unwrap_or_else(|e| {
        panic!(
            "ADD FK NOT VALID succeeds with invalid data\n  pg{pg}: {}",
            detail(&e)
        )
    });
}

#[rstest]
fn not_valid_fk_is_marked_unvalidated(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_fk_nv_unvalidated") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run(INSERT_INVALID_ROW);
    s.run(
        "ALTER TABLE test_child ADD CONSTRAINT fk_nv FOREIGN KEY (parent_id)
             REFERENCES test_parent(id) NOT VALID",
    );

    assert!(
        !s.scalar_bool("SELECT convalidated FROM pg_constraint WHERE conname = 'fk_nv'"),
        "pg{pg}: NOT VALID FK is marked as not validated in pg_constraint"
    );
}
