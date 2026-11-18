//! PGM004 — detaching a partition locks the parent.
//!
//! Claims under test:
//! - DETACH PARTITION acquires ACCESS EXCLUSIVE lock on parent (PGM004). The blocking
//!   is measured here; the exact mode granted on the parent is measured in
//!   `partition_ddl_lock_matrix.rs`.
//! - DETACH PARTITION CONCURRENTLY uses SHARE UPDATE EXCLUSIVE (PG14+): while its
//!   first phase waits for a lock on the partition, the only lock it holds on the
//!   parent is SHARE UPDATE EXCLUSIVE, never ACCESS EXCLUSIVE, and reads and writes
//!   through the parent proceed.
//! - DETACH PARTITION CONCURRENTLY cannot run inside a transaction block (SQLSTATE
//!   25001), so it can never share a transaction with another parent's lock.
//!
//! Ported from `verify/tests/locks/pgm004_detach_partition_lock.sql`.

use pg_lock_tests::{SqlState, TestDb, assert_lock_blocks, expect_sqlstate};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_part(id int, ts date) PARTITION BY RANGE(ts);
    CREATE TABLE test_part_2024 PARTITION OF test_part
        FOR VALUES FROM ('2024-01-01') TO ('2025-01-01');
    INSERT INTO test_part VALUES (1, '2024-06-15');
";

const ATTACHED_PARTITIONS: &str = "SELECT count(*)::bigint FROM pg_inherits \
     WHERE inhparent = 'test_part'::regclass";

/// Bounds how long traffic waits before reporting 55P03 instead of hanging, so a
/// read or write that unexpectedly blocks fails loudly rather than wedging the suite.
const LOCK_TIMEOUT: &str = "250ms";

#[rstest]
fn detach_partition_blocks_select(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
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
fn detach_partition_blocks_insert(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
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
fn detach_concurrently_detaches_partition(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm004_concurrently") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);

    // Its own `run` call: a multi-statement batch is an implicit transaction block,
    // which DETACH ... CONCURRENTLY refuses to run inside.
    s.run("ALTER TABLE test_part DETACH PARTITION test_part_2024 CONCURRENTLY");

    assert_eq!(
        s.scalar_i64(ATTACHED_PARTITIONS),
        0,
        "pg{pg}: DETACH CONCURRENTLY successfully detaches partition"
    );
}

#[rstest]
fn detach_concurrently_takes_only_share_update_exclusive_on_the_parent(
    #[values(14, 15, 16, 17, 18, 19)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pgm004_concurrently_parent_mode") else {
        return;
    };
    db.session().run(SETUP);

    let holder_on_partition = db.hold("LOCK TABLE test_part_2024 IN SHARE UPDATE EXCLUSIVE MODE");
    let detaching =
        db.spawn_blocked("ALTER TABLE test_part DETACH PARTITION test_part_2024 CONCURRENTLY");

    assert_eq!(
        db.locks_on("test_part"),
        ["ShareUpdateExclusiveLock"],
        "pg{pg}: while DETACH ... CONCURRENTLY waits for the partition, the only lock it \
         holds on the parent must be SHARE UPDATE EXCLUSIVE"
    );

    let mut traffic = db.session();
    traffic.set_lock_timeout(LOCK_TIMEOUT);
    assert_eq!(
        traffic.scalar_i64("SELECT count(*)::bigint FROM test_part"),
        1,
        "pg{pg}: a read through the parent proceeds while DETACH ... CONCURRENTLY is \
         in flight"
    );
    traffic.run("INSERT INTO test_part VALUES (2, '2024-07-01')");

    drop(holder_on_partition);
    detaching.join().unwrap_or_else(|e| {
        panic!("pg{pg}: DETACH ... CONCURRENTLY should complete once the partition is free: {e}")
    });
    assert_eq!(
        traffic.scalar_i64(ATTACHED_PARTITIONS),
        0,
        "pg{pg}: DETACH ... CONCURRENTLY detached the partition"
    );
}

#[rstest]
fn detach_concurrently_refuses_to_run_in_a_transaction_block(
    #[values(14, 15, 16, 17, 18, 19)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pgm004_concurrently_txn_block") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);

    s.run("BEGIN");
    expect_sqlstate(
        &mut s,
        "ALTER TABLE test_part DETACH PARTITION test_part_2024 CONCURRENTLY",
        &SqlState::ACTIVE_SQL_TRANSACTION,
        "DETACH PARTITION ... CONCURRENTLY refuses to run inside a transaction block",
    );
    s.run("ROLLBACK");

    assert_eq!(
        s.scalar_i64(ATTACHED_PARTITIONS),
        1,
        "pg{pg}: the refused DETACH ... CONCURRENTLY left the partition attached"
    );
}
