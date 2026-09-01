//! PGM013 — SET NOT NULL requires ACCESS EXCLUSIVE lock (PGM013)
//!
//! Ported from `verify/tests/locks/pgm013_set_not_null_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_snn(id int, val text);
    INSERT INTO test_snn SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
";

#[rstest]
fn set_not_null_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm013_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_snn ALTER COLUMN val SET NOT NULL",
        "SELECT count(*) FROM test_snn",
        "SET NOT NULL blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn set_not_null_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm013_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_snn ALTER COLUMN val SET NOT NULL",
        "INSERT INTO test_snn VALUES (999999, 'probe')",
        "SET NOT NULL blocks INSERT (AccessExclusive)",
    );
}
