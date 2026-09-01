//! Claim under test: a lock request that is merely *queued* blocks
//! traffic the current lock holder would have let through on its own. The "lock
//! convoy" / "lock queue" effect.
//!
//! The only OBSERVED fact is that a migration containing 192 `DROP TABLE <child>`
//! statements errored and rolled back.
//!
//! Claims under test in this file:
//! - While a `DROP TABLE <child>` sits in the lock queue for ACCESS EXCLUSIVE on the
//!   parent, a *new* reader arriving at the parent is blocked, even though its ACCESS
//!   SHARE is compatible with every lock that is actually granted.
//! - CONTROL: with the same holder present but nothing queued behind it, that same
//!   reader succeeds. This is what pins the blocking on the queued request rather than
//!   on the holder.
//! - The queued `DROP` is only ever waiting: when the holder releases, it completes.
//!
//! A blocked probe only demonstrates a convoy if no *granted* lock could have
//! blocked it:
//!   * MEASURED here on 14-18: while `DROP TABLE <child>` is queued for the parent it
//!     holds **no lock at all** on the child it is dropping -- PostgreSQL takes the
//!     parent lock *before* the child's (`RangeVarCallbackForDropRelation` locks a
//!     partition's parent first). So the doomed child cannot be the reason a probe
//!     blocks. This is the opposite of the lock order one would assume from
//!     `heap_drop_with_catalog` alone (child first, parent second).
//!   * Belt and braces, the probes avoid the doomed child anyway: `LIVE_READ` prunes to
//!     the surviving partition (footprint checked by
//!     `live_read_lock_footprint_excludes_the_doomed_partition`) and
//!     `PARENT_ONLY_READ` needs nothing but the parent, assuming no pruning at all.

use pg_lock_tests::{TestDb, assert_lock_allows, assert_lock_blocks, expect_lock_timeout};
use rstest::rstest;

/// A miniature of the real schema: one range-partitioned parent, an old partition the
/// migration wants to drop, and a current partition live traffic reads.
const SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE txns_q1 PARTITION OF txns
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    INSERT INTO txns VALUES (1, '2027-02-01'), (2, '2027-05-01');
";

/// Application traffic: a read through the parent that lands on the *surviving*
/// partition. Locks `txns` and `txns_q2` in ACCESS SHARE and nothing else.
const LIVE_READ: &str = "SELECT count(*) FROM txns WHERE partition_key = '2027-05-01'";

/// A read whose whole lock footprint is ACCESS SHARE on the parent -- `ONLY` suppresses
/// partition expansion, so no child is touched and no pruning is assumed.
const PARENT_ONLY_READ: &str = "SELECT count(*) FROM ONLY txns";

/// A plain read through the parent, which does touch every partition.
const UNPRUNED_READ: &str = "SELECT count(*) FROM txns";

/// A blocked probe waits only this long.
const PROBE_LOCK_TIMEOUT: &str = "250ms";

const ACCESS_SHARE: &str = "AccessShareLock";

/// The convoy claim, with realistic traffic on both ends: the holder and the probe are
/// the same ordinary read of the current partition.
#[rstest]
fn queued_drop_blocks_a_reader_the_holder_alone_would_allow(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_queued_blocks_read") else {
        return;
    };
    db.session().run(SETUP);

    // Traffic already in flight, holding ACCESS SHARE on the parent.
    let holder = db.hold(LIVE_READ);
    assert!(
        db.holds_lock("txns", ACCESS_SHARE),
        "pg{pg}: precondition -- the holder must hold ACCESS SHARE on the parent"
    );
    assert!(
        !db.holds_lock("txns_q1", ACCESS_SHARE),
        "pg{pg}: the holder's read prunes to txns_q2, so the doomed partition carries \
         no lock from the holder either -- the parent is the only contended relation"
    );

    // The migration's DROP cannot get ACCESS EXCLUSIVE on the parent, so it parks in
    // the lock queue. `spawn_blocked` returns only once it is observably waiting.
    let waiter = db.spawn_blocked("DROP TABLE txns_q1");

    // The queued DROP holds *nothing* on the partition it is about to drop.
    // PostgreSQL asks for the parent's ACCESS EXCLUSIVE before the child's, so
    // a DROP stuck on the parent has not locked the child at all.
    assert_eq!(
        db.locks_on("txns_q1"),
        Vec::<String>::new(),
        "pg{pg}: a DROP queued for the parent holds no lock on the child it is \
         dropping, so the child lock cannot be what blocks a third session"
    );

    // Third session. Its ACCESS SHARE conflicts with nothing that is granted anywhere:
    // the holder's ACCESS SHARE is self-compatible, and the queued DROP holds no
    // relation lock in this database. If it is blocked, the only remaining explanation
    // is the DROP's *queued* request on the parent.
    let mut newcomer = db.session();
    newcomer.set_lock_timeout(PROBE_LOCK_TIMEOUT);
    expect_lock_timeout(
        &mut newcomer,
        LIVE_READ,
        "CONVOY: a queued ACCESS EXCLUSIVE request on the parent blocks a new reader \
         whose lock is compatible with everything actually granted",
    );

    // Release the holder so the DROP can finish, and join so no thread is left stuck.
    drop(holder);
    assert!(
        waiter.join().is_ok(),
        "pg{pg}: the DROP was only ever waiting -- once the holder released it completed"
    );
}

/// CONTROL for the test above: with the holder present but nothing queued behind it,
/// the identical read succeeds. Without this, "blocked" could be blamed on the holder.
#[rstest]
fn holder_alone_allows_the_same_reader(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_holder_alone_allows") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        LIVE_READ,
        LIVE_READ,
        &format!(
            "pg{pg}: CONTROL -- with no DROP in the queue, a reader arriving at the \
             parent is not blocked by traffic already holding ACCESS SHARE on it"
        ),
    );
}

/// The same convoy claim with a probe that assumes nothing about partition pruning:
/// `SELECT ... FROM ONLY <parent>` needs exactly one lock, ACCESS SHARE on the parent.
#[rstest]
fn queued_drop_blocks_a_read_that_needs_only_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_parent_only_blocked") else {
        return;
    };
    db.session().run(SETUP);

    let holder = db.hold(PARENT_ONLY_READ);
    let waiter = db.spawn_blocked("DROP TABLE txns_q1");

    let mut newcomer = db.session();
    newcomer.set_lock_timeout(PROBE_LOCK_TIMEOUT);
    expect_lock_timeout(
        &mut newcomer,
        PARENT_ONLY_READ,
        "CONVOY: a statement whose entire lock footprint is ACCESS SHARE on the parent \
         is blocked by the DROP's queued ACCESS EXCLUSIVE request on that parent -- no \
         granted lock conflicts with it",
    );

    drop(holder);
    assert!(
        waiter.join().is_ok(),
        "pg{pg}: the DROP completed once the holder released"
    );
}

/// CONTROL for the parent-only probe.
#[rstest]
fn holder_alone_allows_the_parent_only_read(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_parent_only_control") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        PARENT_ONLY_READ,
        PARENT_ONLY_READ,
        &format!(
            "pg{pg}: CONTROL -- two ACCESS SHARE readers of the parent coexist happily; \
             only a queued conflicting request changes that"
        ),
    );
}

/// Pins down the probe's lock footprint: `LIVE_READ` needs no lock on the partition the
/// migration is dropping, while the same read without the partition-key predicate does.
/// Independent of the measurement that a queued DROP holds nothing on the child, this
/// rules out the doomed partition as an explanation for the headline test's timeout.
#[rstest]
fn live_read_lock_footprint_excludes_the_doomed_partition(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_read_footprint") else {
        return;
    };
    db.session().run(SETUP);

    // ACCESS EXCLUSIVE on the child alone -- the parent is deliberately left unlocked,
    // so anything blocked here is blocked by the child lock.
    const EXCLUSIVE_ON_CHILD: &str = "LOCK TABLE txns_q1 IN ACCESS EXCLUSIVE MODE";

    assert_lock_allows(
        &db,
        EXCLUSIVE_ON_CHILD,
        LIVE_READ,
        &format!(
            "pg{pg}: the pruned read does not lock txns_q1, so an exclusive lock on \
             txns_q1 cannot be what blocks it"
        ),
    );

    assert_lock_blocks(
        &db,
        EXCLUSIVE_ON_CHILD,
        UNPRUNED_READ,
        &format!(
            "pg{pg}: the same read without the partition-key predicate does lock \
             txns_q1 -- which is why the headline test probes with the pruned form"
        ),
    );
}

/// The queued DROP is waiting: nothing is wrong with it except the lock.
#[rstest]
fn the_queued_drop_completes_once_the_holder_releases(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pconvoy_waiter_finishes") else {
        return;
    };
    db.session().run(SETUP);

    let holder = db.hold(LIVE_READ);
    let waiter = db.spawn_blocked("DROP TABLE txns_q1");

    drop(holder);
    let outcome = waiter.join();
    assert!(
        outcome.is_ok(),
        "pg{pg}: the queued DROP should complete once the parent lock is free, but it \
         failed: {}",
        outcome
            .err()
            .map(|e| pg_lock_tests::detail(&e))
            .unwrap_or_default()
    );

    assert_eq!(
        db.session()
            .scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: the partition really was dropped after the wait, so the DROP had been \
         held up only by the lock"
    );
}
