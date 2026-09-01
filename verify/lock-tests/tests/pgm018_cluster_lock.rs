//! PGM018 — CLUSTER acquires ACCESS EXCLUSIVE lock and rewrites table (PGM018)
//!
//! Ported from `verify/tests/locks/pgm018_cluster_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_cl(id int, val text);
    INSERT INTO test_cl SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
    CREATE INDEX idx_cl_val ON test_cl(val);
";

#[rstest]
fn cluster_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm018_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "CLUSTER test_cl USING idx_cl_val",
        "SELECT count(*) FROM test_cl",
        "CLUSTER blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn cluster_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm018_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "CLUSTER test_cl USING idx_cl_val",
        "INSERT INTO test_cl VALUES (999999, 'probe')",
        "CLUSTER blocks INSERT (AccessExclusive)",
    );
}
