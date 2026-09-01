//! PGM017 — ADD UNIQUE takes ACCESS EXCLUSIVE and builds a new index.
//!
//! Claims under test:
//! - ADD UNIQUE acquires ACCESS EXCLUSIVE lock and builds a new index (PGM017)
//! - ADD UNIQUE USING INDEX reuses existing index
//!
//! Ported from `verify/tests/locks/pgm017_add_unique_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_uq(id int, val text);
    INSERT INTO test_uq SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
";

const INDEX_COUNT: &str = "SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_uq'";

#[rstest]
fn add_unique_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm017_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_uq ADD CONSTRAINT uq_val UNIQUE (val)",
        "SELECT count(*) FROM test_uq",
        "ADD UNIQUE blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn add_unique_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm017_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_uq ADD CONSTRAINT uq_val UNIQUE (val)",
        "INSERT INTO test_uq VALUES (999999, 'probe_unique')",
        "ADD UNIQUE blocks INSERT (AccessExclusive)",
    );
}

/// Precondition for the `USING INDEX` case: a lone unique index is the only index.
#[rstest]
fn one_index_before_add_unique_using_index(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm017_one_index_before") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run("CREATE UNIQUE INDEX idx_uq_val ON test_uq(val)");

    assert_eq!(
        s.scalar_i64(INDEX_COUNT),
        1,
        "pg{pg}: One index before ADD UNIQUE USING INDEX"
    );
}

/// `USING INDEX` adopts the existing index rather than building a second one.
#[rstest]
fn add_unique_using_index_reuses_existing(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm017_using_index") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run("CREATE UNIQUE INDEX idx_uq_val ON test_uq(val)");

    s.run("ALTER TABLE test_uq ADD CONSTRAINT uq_reuse UNIQUE USING INDEX idx_uq_val");

    assert_eq!(
        s.scalar_i64(INDEX_COUNT),
        1,
        "pg{pg}: ADD UNIQUE USING INDEX reuses existing index (still 1 index)"
    );
}
