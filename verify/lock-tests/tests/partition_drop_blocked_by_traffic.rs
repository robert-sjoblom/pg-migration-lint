//! Reproduction: a migration that drops partitions fails because live traffic holds
//! the parent.
//!
//! Claims under test:
//! - `DROP TABLE <partition>` needs ACCESS EXCLUSIVE on the *parent*, so any session
//!   reading or writing through the parent makes it give up (SQLSTATE 55P03).
//! - Because the migration runs in one explicit transaction, that single failure
//!   discards every statement that already succeeded.
//! - The blocking session does not have to be busy — one query, then idle in
//!   transaction, is enough.
//! - Traffic addressed straight at a *sibling* partition does not block the drop,
//!   which is what identifies the parent lock as the cause.

use pg_lock_tests::{SqlState, TestDb, expect_lock_timeout, expect_sqlstate};
use rstest::rstest;

/// A miniature of the real schema: one range-partitioned parent, two children.
const SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE txns_q1 PARTITION OF txns
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    INSERT INTO txns VALUES (1, '2027-02-01'), (2, '2027-05-01');
";

/// Short enough to keep the suite fast; the holder never lets go, so the wait is
/// bounded only by this.
const MIGRATION_LOCK_TIMEOUT: &str = "250ms";

#[rstest]
fn reader_on_parent_blocks_dropping_a_partition(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "partdrop_reader_blocks") else {
        return;
    };
    db.session().run(SETUP);

    // Live traffic: one SELECT through the parent, then idle in transaction. This holds
    // ACCESS SHARE on the parent, which is all it takes.
    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");

    expect_lock_timeout(
        &mut migration,
        "DROP TABLE txns_q1",
        "DROP TABLE <partition> gives up when a reader holds the parent \
         (it needs AccessExclusive on the parent, not just on the partition)",
    );
}

#[rstest]
fn writer_on_parent_blocks_dropping_a_partition(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "partdrop_writer_blocks") else {
        return;
    };
    db.session().run(SETUP);

    // A writer holds ROW EXCLUSIVE on the parent — likewise incompatible.
    let _traffic = db.hold("INSERT INTO txns VALUES (3, '2027-03-01')");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");

    expect_lock_timeout(
        &mut migration,
        "DROP TABLE txns_q1",
        "DROP TABLE <partition> gives up when a writer holds the parent",
    );
}

/// The compounding factor: one wrapping transaction means one blocked statement
/// throws away everything before it.
#[rstest]
fn one_blocked_drop_discards_the_whole_transaction(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "partdrop_discards_txn") else {
        return;
    };
    db.session().run(SETUP);

    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");

    // Stand-in for the statements that already succeeded before the one that blocks.
    migration.run("CREATE TABLE migration_progress(step int)");
    migration.run("INSERT INTO migration_progress VALUES (1)");

    expect_lock_timeout(
        &mut migration,
        "DROP TABLE txns_q1",
        "the drop gives up while traffic holds the parent",
    );

    // Everything after the failure is refused, so the remaining statements never run.
    expect_sqlstate(
        &mut migration,
        "SELECT 1",
        &SqlState::IN_FAILED_SQL_TRANSACTION,
        "after a blocked DROP the transaction is aborted, so every later statement \
         in the migration is refused",
    );

    migration.run("ROLLBACK");

    assert_eq!(
        migration.scalar_i64(
            "SELECT count(*)::bigint FROM pg_class WHERE relname = 'migration_progress'"
        ),
        0,
        "pg{pg}: work done before the blocked DROP must be gone after rollback — \
         one un-gettable lock costs the entire migration"
    );
}

/// Control: nothing about the drop is inherently slow or blocked.
#[rstest]
fn dropping_a_partition_succeeds_without_traffic(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "partdrop_no_traffic") else {
        return;
    };
    let mut migration = db.session();
    migration.run(SETUP);
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);

    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: with no traffic holding the parent the same DROP succeeds"
    );
}

/// Isolates the cause: it is the lock on the *parent* that the migration cannot get,
/// not a lock on the partition being dropped.
#[rstest]
fn traffic_on_a_sibling_partition_does_not_block(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "partdrop_sibling_ok") else {
        return;
    };
    db.session().run(SETUP);

    // Addressed straight at the sibling, so the parent is never locked.
    let _traffic = db.hold("SELECT count(*) FROM txns_q2");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: traffic that bypasses the parent does not block the drop — \
         routing through the parent is what makes it fail"
    );
}
