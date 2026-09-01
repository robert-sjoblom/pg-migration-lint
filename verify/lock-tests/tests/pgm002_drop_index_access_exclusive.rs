//! PGM002 — DROP INDEX acquires ACCESS EXCLUSIVE, blocking reads *and* writes.
//!
//! Ported from `verify/tests/locks/pgm002_drop_index_access_exclusive.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_di(id int, val text);
    INSERT INTO test_di SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
    CREATE INDEX idx_di_val ON test_di(val);
";

#[rstest]
fn drop_index_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm002_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "DROP INDEX idx_di_val",
        "INSERT INTO test_di VALUES (999999, 'probe')",
        "DROP INDEX blocks INSERT (AccessExclusive vs RowExclusive)",
    );
}

#[rstest]
fn drop_index_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm002_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "DROP INDEX idx_di_val",
        "SELECT count(*) FROM test_di",
        "DROP INDEX blocks SELECT (AccessExclusive vs AccessShare)",
    );
}
