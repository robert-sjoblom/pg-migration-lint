//! PGM004 — detaching a partition locks the parent.
//!
//! Claims under test:
//! - DETACH PARTITION acquires ACCESS EXCLUSIVE lock on parent (PGM004)
//! - DETACH PARTITION CONCURRENTLY uses SHARE UPDATE EXCLUSIVE (PG14+)
//!
//! Ported from `verify/tests/locks/pgm004_detach_partition_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_part(id int, ts date) PARTITION BY RANGE(ts);
    CREATE TABLE test_part_2024 PARTITION OF test_part
        FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
    INSERT INTO test_part VALUES (1, '2024-06-15');
";

#[rstest]
fn detach_partition_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm004_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_part DETACH PARTITION test_part_2024",
        "SELECT count(*) FROM test_part",
        "DETACH PARTITION blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn detach_partition_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm004_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_part DETACH PARTITION test_part_2024",
        "INSERT INTO test_part VALUES (2, '2024-07-01')",
        "DETACH PARTITION blocks INSERT (AccessExclusive)",
    );
}

/// DETACH CONCURRENTLY uses a weaker lock.
#[rstest]
fn detach_concurrently_detaches_partition(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm004_concurrently") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);

    // Its own `run` call: a multi-statement batch is an implicit transaction block,
    // which DETACH ... CONCURRENTLY refuses to run inside.
    s.run("ALTER TABLE test_part DETACH PARTITION test_part_2024 CONCURRENTLY");

    assert_eq!(
        s.scalar_i64(
            "SELECT count(*)::bigint FROM pg_inherits \
             WHERE inhparent = 'test_part'::regclass"
        ),
        0,
        "pg{pg}: DETACH CONCURRENTLY successfully detaches partition"
    );
}
