//! PGM001 — CREATE INDEX acquires SHARE lock — blocks writes, allows reads (PGM001)
//!
//! Ported from `verify/tests/locks/pgm001_create_index_share_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_allows, assert_lock_blocks};
use rstest::rstest;

/// Enough rows that the index build is real work rather than a no-op.
const SETUP: &str = "
    CREATE TABLE test_ci(id int, val text);
    INSERT INTO test_ci SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
";

#[rstest]
fn create_index_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm001_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "CREATE INDEX idx_ci_val ON test_ci(val)",
        "INSERT INTO test_ci VALUES (999999, 'probe')",
        "CREATE INDEX blocks INSERT (SHARE vs RowExclusive)",
    );
}

#[rstest]
fn create_index_blocks_update(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm001_blocks_update") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "CREATE INDEX idx_ci_val ON test_ci(val)",
        "UPDATE test_ci SET val = 'changed' WHERE id = 1",
        "CREATE INDEX blocks UPDATE (SHARE vs RowExclusive)",
    );
}

#[rstest]
fn create_index_allows_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm001_allows_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        "CREATE INDEX idx_ci_val ON test_ci(val)",
        "SELECT count(*) FROM test_ci",
        "CREATE INDEX allows SELECT (SHARE vs AccessShare)",
    );
}
