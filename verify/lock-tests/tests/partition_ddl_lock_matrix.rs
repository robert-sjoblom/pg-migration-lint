//! The partition-DDL lock matrix: which partition operations lock the *parent*,
//! and what that costs traffic routed through the parent.
//!
//! Background. One migration ran a single explicit transaction containing 192
//! `DROP TABLE` statements against child partitions of 12 range-partitioned parents,
//! then ~200 `CREATE TABLE ... PARTITION OF ...` statements to recreate coarser
//! partitions. The only *observed* fact is that the migration errored and rolled
//! back; nothing here establishes that application traffic stalled or that the
//! service went down. What this file establishes is the mechanism that makes such
//! a failure possible, and the shape of the safe alternative.
//!
//! Claims under test -- the lock mode granted on the PARENT:
//! - `DROP TABLE <child>`                            -> AccessExclusiveLock on parent
//! - `CREATE TABLE <child> PARTITION OF <parent>`    -> AccessExclusiveLock on parent
//! - `ALTER TABLE <parent> DETACH PARTITION <child>` -> AccessExclusiveLock on parent
//! - `ALTER TABLE <parent> ATTACH PARTITION <std>`   -> ShareUpdateExclusiveLock on
//!   parent, and *not* AccessExclusiveLock
//!
//! Claims under test:
//! - While the first three sit uncommitted, a `SELECT` through the parent and an
//!   `INSERT` through the parent both fail to get their lock (SQLSTATE 55P03).
//! - While `ATTACH` sits uncommitted, both survive. The identical end state is
//!   reachable without ever taking AccessExclusive on the parent.
//! - `ATTACH` is not free of AccessExclusive -- it takes it on the table *being
//!   attached*, which blocks traffic addressed directly at that table.
//!
//! Every lock-mode assertion is made from inside the transaction that ran the DDL:
//! PostgreSQL holds DDL locks until commit, and `pg_locks` only reports what is held
//! right now, so an uncommitted transaction is the only place the mode is visible.

use pg_lock_tests::{TestDb, assert_lock_allows, assert_lock_blocks};
use rstest::rstest;

/// A miniature of the real schema: one range-partitioned parent, two children,
/// plus a free-standing table shaped like a partition so ATTACH has something
/// to attach.
///
/// Deliberately no DEFAULT partition: ATTACH also takes AccessExclusive on the
/// default partition when one exists, which would confound the parent-only claim
/// below.
const SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE txns_q1 PARTITION OF txns
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    INSERT INTO txns VALUES (1, '2027-02-01'), (2, '2027-05-01');
    CREATE TABLE txns_q3_standalone(id int, partition_key date);
";

const PARENT: &str = "txns";
const STANDALONE: &str = "txns_q3_standalone";

const ACCESS_EXCLUSIVE: &str = "AccessExclusiveLock";
const SHARE_UPDATE_EXCLUSIVE: &str = "ShareUpdateExclusiveLock";

// The four operations under test. Each is a single statement so that the lock it
// takes is unambiguous.
const DROP_CHILD: &str = "DROP TABLE txns_q1";
const CREATE_CHILD: &str = "CREATE TABLE txns_q4 PARTITION OF txns \
     FOR VALUES FROM ('2027-10-01') TO ('2028-01-01')";
const DETACH_CHILD: &str = "ALTER TABLE txns DETACH PARTITION txns_q1";
const ATTACH_STANDALONE: &str = "ALTER TABLE txns ATTACH PARTITION txns_q3_standalone \
     FOR VALUES FROM ('2027-07-01') TO ('2027-10-01')";

/// Application traffic, expressed as the two things that must keep working.
///
/// Both are addressed at the parent, which is how the application reaches its data.
/// The INSERT targets the q2 range, a partition that exists and is untouched by every
/// operation under test -- so if it fails, it failed on a lock and not on routing.
const SELECT_THROUGH_PARENT: &str = "SELECT count(*) FROM txns";
const INSERT_THROUGH_PARENT: &str = "INSERT INTO txns VALUES (99, '2027-05-01')";

/// A holder statement that takes no lock worth mentioning, for the no-DDL control.
const NO_DDL: &str = "SELECT 1";

const MIGRATION_LOCK_TIMEOUT: &str = "250ms";

/// Lock modes granted on `relation` while `ddl` sits in an open, uncommitted
/// transaction.
fn modes_while_running(db: &TestDb, ddl: &str, relation: &str) -> Vec<String> {
    let _ddl_in_flight = db.hold(ddl);
    db.locks_on(relation)
}

#[rstest]
fn drop_child_takes_access_exclusive_on_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_drop_child_parent_mode") else {
        return;
    };
    db.session().run(SETUP);

    let modes = modes_while_running(&db, DROP_CHILD, PARENT);

    assert_eq!(
        modes,
        [ACCESS_EXCLUSIVE],
        "pg{pg}: `{DROP_CHILD}` must hold exactly {ACCESS_EXCLUSIVE} on the parent \
         `{PARENT}` -- dropping a child is a parent-wide operation"
    );
}

#[rstest]
fn create_child_takes_access_exclusive_on_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_create_child_parent_mode") else {
        return;
    };
    db.session().run(SETUP);

    let modes = modes_while_running(&db, CREATE_CHILD, PARENT);

    assert_eq!(
        modes,
        [ACCESS_EXCLUSIVE],
        "pg{pg}: `CREATE TABLE ... PARTITION OF {PARENT}` must hold exactly \
         {ACCESS_EXCLUSIVE} on the parent -- creating a partition in place attaches it \
         in the same statement"
    );
}

#[rstest]
fn detach_partition_takes_access_exclusive_on_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_detach_parent_mode") else {
        return;
    };
    db.session().run(SETUP);

    let modes = modes_while_running(&db, DETACH_CHILD, PARENT);

    assert_eq!(
        modes,
        [ACCESS_EXCLUSIVE],
        "pg{pg}: `{DETACH_CHILD}` must hold exactly {ACCESS_EXCLUSIVE} on the parent"
    );
}

#[rstest]
fn attach_partition_takes_only_share_update_exclusive_on_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pdlm_attach_parent_mode") else {
        return;
    };
    db.session().run(SETUP);

    let _attaching = db.hold(ATTACH_STANDALONE);
    let modes = db.locks_on(PARENT);

    assert_eq!(
        modes,
        [SHARE_UPDATE_EXCLUSIVE],
        "pg{pg}: `ALTER TABLE {PARENT} ATTACH PARTITION ...` must hold exactly \
         {SHARE_UPDATE_EXCLUSIVE} on the parent"
    );
    assert!(
        !db.holds_lock(PARENT, ACCESS_EXCLUSIVE),
        "pg{pg}: ATTACH PARTITION must NOT take {ACCESS_EXCLUSIVE} on the parent -- \
         that is exactly what separates it from DROP/CREATE/DETACH. \
         Granted modes on `{PARENT}`: {modes:?}"
    );
}

/// ATTACH is not lock-free; the AccessExclusive moves to the table being attached.
#[rstest]
fn attach_takes_access_exclusive_on_the_table_being_attached(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pdlm_attach_child_mode") else {
        return;
    };
    db.session().run(SETUP);

    let _attaching = db.hold(ATTACH_STANDALONE);
    let modes = db.locks_on(STANDALONE);

    assert!(
        db.holds_lock(STANDALONE, ACCESS_EXCLUSIVE),
        "pg{pg}: ATTACH PARTITION must hold {ACCESS_EXCLUSIVE} on `{STANDALONE}`, the \
         table being attached, even though the parent only gets \
         {SHARE_UPDATE_EXCLUSIVE}. Granted modes on `{STANDALONE}`: {modes:?}"
    );
    assert!(
        !db.holds_lock(PARENT, ACCESS_EXCLUSIVE),
        "pg{pg}: the AccessExclusive is on the attached table only -- the parent must \
         still be free of it in the same transaction. \
         Granted modes on `{PARENT}`: {:?}",
        db.locks_on(PARENT)
    );
}

#[rstest]
fn drop_child_blocks_traffic_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_drop_child_blocks_parent") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        DROP_CHILD,
        SELECT_THROUGH_PARENT,
        "an uncommitted DROP TABLE <child> holds AccessExclusive on the parent, so a \
         SELECT routed through the parent cannot get AccessShare",
    );
    assert_lock_blocks(
        &db,
        DROP_CHILD,
        INSERT_THROUGH_PARENT,
        "an uncommitted DROP TABLE <child> also stops an INSERT routed through the \
         parent, even one destined for an untouched sibling partition",
    );
}

#[rstest]
fn create_child_blocks_traffic_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_create_child_blocks_parent") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        CREATE_CHILD,
        SELECT_THROUGH_PARENT,
        "CREATE TABLE ... PARTITION OF <parent> is not additive from a locking point \
         of view: while uncommitted it blocks a SELECT through the parent",
    );
    assert_lock_blocks(
        &db,
        CREATE_CHILD,
        INSERT_THROUGH_PARENT,
        "CREATE TABLE ... PARTITION OF <parent> blocks an INSERT through the parent",
    );
}

#[rstest]
fn detach_blocks_traffic_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_detach_blocks_parent") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        DETACH_CHILD,
        SELECT_THROUGH_PARENT,
        "an uncommitted DETACH PARTITION blocks a SELECT through the parent",
    );
    assert_lock_blocks(
        &db,
        DETACH_CHILD,
        INSERT_THROUGH_PARENT,
        "an uncommitted DETACH PARTITION blocks an INSERT through the parent",
    );
}

#[rstest]
fn attach_allows_traffic_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_attach_allows_parent") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        ATTACH_STANDALONE,
        SELECT_THROUGH_PARENT,
        "ShareUpdateExclusive on the parent is compatible with AccessShare, so a \
         SELECT through the parent proceeds while ATTACH is still uncommitted",
    );
    assert_lock_allows(
        &db,
        ATTACH_STANDALONE,
        INSERT_THROUGH_PARENT,
        "ShareUpdateExclusive on the parent is compatible with RowExclusive, so an \
         INSERT through the parent proceeds while ATTACH is still uncommitted",
    );
}

/// The cost ATTACH does impose, so the safe path is not oversold.
#[rstest]
fn attach_blocks_traffic_addressed_at_the_table_being_attached(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pdlm_attach_blocks_child") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        ATTACH_STANDALONE,
        "SELECT count(*) FROM txns_q3_standalone",
        "ATTACH holds AccessExclusive on the table being attached, so a SELECT \
         addressed directly at that table is blocked -- the weak lock protects the \
         parent, not the new child",
    );
}

/// Control for every `assert_lock_blocks` above: the probes are not inherently
/// failing. With an idle transaction holding nothing relevant, both succeed.
#[rstest]
fn parent_traffic_succeeds_when_no_partition_ddl_is_running(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pdlm_control_no_ddl") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        NO_DDL,
        SELECT_THROUGH_PARENT,
        "with no partition DDL in flight a SELECT through the parent succeeds",
    );
    assert_lock_allows(
        &db,
        NO_DDL,
        INSERT_THROUGH_PARENT,
        "with no partition DDL in flight an INSERT through the parent succeeds",
    );
}

/// Control in the other direction.
///
/// The companion file shows a reader or writer on the parent making `DROP TABLE
/// <child>` give up with 55P03. Here the same live traffic is held and ATTACH commits
/// anyway, which is what "safe building block" has to mean: it survives the workload
/// rather than merely being polite to it.
#[rstest]
fn attach_commits_while_a_reader_and_a_writer_hold_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pdlm_attach_under_traffic") else {
        return;
    };
    db.session().run(SETUP);

    // Live traffic, idle in transaction: AccessShare and RowExclusive on the parent.
    let _reader = db.hold(SELECT_THROUGH_PARENT);
    let _writer = db.hold("INSERT INTO txns VALUES (3, '2027-02-01')");

    let mut migration = db.session();
    // If ATTACH did need AccessExclusive on the parent, this bounds the failure.
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");
    migration.run(ATTACH_STANDALONE);
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64(
            "SELECT count(*)::bigint FROM pg_inherits \
             WHERE inhparent = 'txns'::regclass \
               AND inhrelid = 'txns_q3_standalone'::regclass"
        ),
        1,
        "pg{pg}: ATTACH PARTITION commits while a reader and a writer hold the \
         parent -- the same traffic that makes DROP TABLE <child> give up"
    );
}
