//! The low-lock alternative to the migration in
//! `partition_drop_blocked_by_traffic.rs`: what a rule could actually recommend
//! instead of `DROP TABLE <child>` / `CREATE TABLE <child> PARTITION OF <parent>`.
//!
//! Claims under test:
//! - `ALTER TABLE <parent> DETACH PARTITION <child> CONCURRENTLY` (PG 14+) followed by
//!   `DROP TABLE <child>` lands on the same catalog and data end state as a bare
//!   `DROP TABLE <child>`.
//! - Once the child is detached it is a standalone table, so its `DROP TABLE` no longer
//!   needs any lock on the parent: it succeeds with a reader parked on the parent,
//!   while the same drop against a still-attached sibling gives up with 55P03.
//! - `CREATE TABLE <child> (LIKE <parent> INCLUDING ALL)` + `ALTER TABLE <parent> ATTACH
//!   PARTITION <child> FOR VALUES ...` only takes SHARE UPDATE EXCLUSIVE on the parent,
//!   so reads and writes through the parent survive it, and it survives them --
//!   whereas `CREATE TABLE <child> PARTITION OF <parent>` blocks both directions.
//! - That two-step recreate reaches the same end state (columns, bound, indexes,
//!   index attachment, constraints) as `CREATE TABLE ... PARTITION OF`.
//! - An interrupted `DETACH ... CONCURRENTLY` leaves the partition detach-pending, and
//!   that state does not resolve itself: re-running the detach is refused outright
//!   (55000, naming FINALIZE), so the only exits are `DETACH ... FINALIZE` and
//!   `DROP TABLE <child>`.
//! - Until the detach is resolved the child is still in `pg_inherits`, so its
//!   drop still wants AccessExclusive on the parent and still loses to the same
//! traffic a fully detached partition's drop sails past.
//!
//! What this file deliberately does NOT claim: that `DETACH PARTITION ... CONCURRENTLY`
//! is immune to live traffic. It is not.
//! `detach_concurrently_waits_for_traffic_and_a_timeout_leaves_a_pending_detach`
//! measures what really happens.

use pg_lock_tests::{
    Holder, SqlState, TestDb, assert_lock_allows, assert_lock_blocks, detail, expect_lock_timeout,
    expect_sqlstate,
};
use rstest::rstest;

// `DETACH PARTITION ... CONCURRENTLY` arrived in PG 14, which is the oldest version the
// harness covers, so every case here runs the full 14–18 matrix.

/// The drop half of the migration, in miniature: one range-partitioned parent, two
/// quarterly children, traffic routed through the parent.
const DROP_SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE txns_q1 PARTITION OF txns
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    INSERT INTO txns VALUES (1, '2027-02-01'), (2, '2027-05-01');
";

/// The recreate half: a parent that already has one child, plus the replacement child
/// built as a standalone table so it can be attached rather than created in place.
///
/// The parent carries a primary key, a secondary index and a CHECK constraint because
/// the migration under study recreated partitions with `ADD CONSTRAINT` and
/// `CREATE INDEX` too, and `LIKE ... INCLUDING ALL` is what replaces those statements.
const RECREATE_SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date, amount numeric,
        CONSTRAINT txns_amount_positive CHECK (amount > 0),
        PRIMARY KEY (id, partition_key)) PARTITION BY RANGE (partition_key);
    CREATE INDEX txns_amount_idx ON txns(amount);
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    INSERT INTO txns VALUES (2, '2027-05-01', 10);
    CREATE TABLE txns_q1 (LIKE txns INCLUDING ALL);
";

/// Two identical parents in one database, `plain` for the direct route and `safe` for
/// the low-lock route, so the two end states can be compared against each other rather
/// than against a hand-written expectation.
const TWO_PARENTS_SETUP: &str = "
    CREATE TABLE plain(id int, partition_key date, amount numeric,
        CONSTRAINT amount_positive CHECK (amount > 0),
        PRIMARY KEY (id, partition_key)) PARTITION BY RANGE (partition_key);
    CREATE INDEX plain_amount_idx ON plain(amount);
    CREATE TABLE plain_q2 PARTITION OF plain
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');

    CREATE TABLE safe(id int, partition_key date, amount numeric,
        CONSTRAINT amount_positive CHECK (amount > 0),
        PRIMARY KEY (id, partition_key)) PARTITION BY RANGE (partition_key);
    CREATE INDEX safe_amount_idx ON safe(amount);
    CREATE TABLE safe_q2 PARTITION OF safe
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
";

/// What a migration would realistically set, and short enough to keep the suite fast.
const MIGRATION_LOCK_TIMEOUT: &str = "250ms";

/// The FROM/TO of the partition every test creates or drops.
const Q1_BOUND: &str = "FOR VALUES FROM ('2027-01-01') TO ('2027-04-01')";

/// The statement whose interruption leaves a half-detached state.
const DETACH_Q1_CONCURRENTLY: &str = "ALTER TABLE txns DETACH PARTITION txns_q1 CONCURRENTLY";

/// `DETACH PARTITION ... CONCURRENTLY` + `DROP TABLE` is not a different outcome, just a
/// different lock profile: the catalog and the data end up where the plain `DROP TABLE`
/// would have left them.
#[rstest]
fn detach_concurrently_then_drop_reaches_the_same_end_state_as_a_plain_drop(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_drop_end_state") else {
        return;
    };
    let mut s = db.session();
    s.run(TWO_PARENTS_SETUP);
    s.run(&format!(
        "CREATE TABLE plain_q1 PARTITION OF plain {Q1_BOUND};
         CREATE TABLE safe_q1 PARTITION OF safe {Q1_BOUND};
         INSERT INTO plain VALUES (1, '2027-02-01', 5), (2, '2027-05-01', 5);
         INSERT INTO safe  VALUES (1, '2027-02-01', 5), (2, '2027-05-01', 5);"
    ));

    // Route A, what the migration did.
    s.run("DROP TABLE plain_q1");

    // Route B. DETACH ... CONCURRENTLY cannot run inside a transaction block, so it gets
    // its own `run` call; no Holder is alive, so it has nothing to wait for.
    s.run("ALTER TABLE safe DETACH PARTITION safe_q1 CONCURRENTLY");
    s.run("DROP TABLE safe_q1");

    for child in ["plain_q1", "safe_q1"] {
        assert_eq!(
            s.scalar_i64(&format!(
                "SELECT count(*)::bigint FROM pg_class WHERE relname = '{child}'"
            )),
            0,
            "pg{pg}: {child} must be gone from pg_class -- both routes end in DROP TABLE"
        );
    }

    // Non-vacuity: each parent must still have exactly its q2 child, so the comparison
    // below is comparing one name against one name and not NULL against NULL.
    for parent in ["plain", "safe"] {
        assert_eq!(
            s.scalar_i64(&format!(
                "SELECT count(*)::bigint FROM pg_inherits
                  WHERE inhparent = '{parent}'::regclass"
            )),
            1,
            "pg{pg}: {parent} must be left with exactly one attached partition"
        );
    }

    assert!(
        s.scalar_bool(
            "SELECT (SELECT array_agg(n ORDER BY n) FROM (
                       SELECT replace(c.relname, 'plain', 'X') AS n
                         FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
                        WHERE i.inhparent = 'plain'::regclass) t)
                  = (SELECT array_agg(n ORDER BY n) FROM (
                       SELECT replace(c.relname, 'safe', 'X') AS n
                         FROM pg_inherits i JOIN pg_class c ON c.oid = i.inhrelid
                        WHERE i.inhparent = 'safe'::regclass) t)"
        ),
        "pg{pg}: after DETACH CONCURRENTLY + DROP, pg_inherits must list the same \
         remaining partitions as after a plain DROP"
    );

    assert!(
        s.scalar_bool(
            "SELECT NOT EXISTS (SELECT 1 FROM pg_inherits
                                 WHERE inhparent IN ('plain'::regclass, 'safe'::regclass)
                                   AND inhdetachpending)"
        ),
        "pg{pg}: the completed DETACH CONCURRENTLY must not leave a partition marked \
         detach-pending"
    );

    assert_eq!(
        s.scalar_i64("SELECT count(*)::bigint FROM safe"),
        s.scalar_i64("SELECT count(*)::bigint FROM plain"),
        "pg{pg}: both routes must discard the dropped partition's rows"
    );
    assert_eq!(
        s.scalar_i64("SELECT count(*)::bigint FROM safe"),
        1,
        "pg{pg}: only the q2 row may survive, so the row-count comparison is not vacuous"
    );
}

/// The same reader, the same  `DROP TABLE`, but the child is standalone by then
/// so there is no parent lock to fail to get.
#[rstest]
fn dropping_a_detached_partition_succeeds_while_traffic_holds_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_detached_drop_ok") else {
        return;
    };
    let mut setup = db.session();
    setup.run(DROP_SETUP);
    // Detached first, with no traffic in flight
    setup.run("ALTER TABLE txns DETACH PARTITION txns_q1 CONCURRENTLY");

    // Only now does live traffic park itself on the parent. Its SELECT locks the parent
    // and every *attached* partition; txns_q1 is no longer one of them.
    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: dropping a DETACHED table needs no lock on the parent, so a reader \
         holding the parent does not stop it"
    );
}

/// It is not that AccessShare happens to be compatible with what the drop wants
/// on the parent, it is that the drop wants nothing on the parent at all. Even
/// ACCESS EXCLUSIVE on the parent -- the most conflicting lock there is -- does
/// not stand in its way.
#[rstest]
fn dropping_a_detached_partition_needs_no_lock_on_the_parent_at_all(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_detached_drop_no_parent_lock") else {
        return;
    };
    let mut setup = db.session();
    setup.run(DROP_SETUP);
    setup.run("ALTER TABLE txns DETACH PARTITION txns_q1 CONCURRENTLY");

    // LOCK TABLE on a partitioned table recurses to its partitions -- but txns_q1 is not
    // one of them any more, which is the whole point.
    let _blocker = db.hold("LOCK TABLE txns IN ACCESS EXCLUSIVE MODE");
    assert!(
        db.holds_lock("txns", "AccessExclusiveLock"),
        "pg{pg}: the blocker must really hold AccessExclusive on the parent, or the \
         drop below proves nothing"
    );

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: once detached, the table is standalone -- dropping it takes no lock on \
         the former parent whatsoever"
    );
}

/// Control for the claim above: with the child still attached, that identical
/// drop under identical traffic gives up.
#[rstest]
fn dropping_a_still_attached_partition_fails_under_the_same_traffic(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_attached_drop_fails") else {
        return;
    };
    db.session().run(DROP_SETUP);

    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");

    expect_lock_timeout(
        &mut migration,
        "DROP TABLE txns_q1",
        "without the DETACH first, DROP TABLE <partition> still needs AccessExclusive \
         on the parent and still gives up -- the DETACH is what makes the difference",
    );
}

/// Direction 1: does the DDL block the application?
#[rstest]
fn attach_partition_allows_select_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_attach_allows_select") else {
        return;
    };
    db.session().run(RECREATE_SETUP);

    assert_lock_allows(
        &db,
        &format!("ALTER TABLE txns ATTACH PARTITION txns_q1 {Q1_BOUND}"),
        "SELECT count(*) FROM txns",
        "ATTACH PARTITION only takes ShareUpdateExclusive on the parent, which is \
         compatible with the AccessShare a reader takes",
    );
}

#[rstest]
fn attach_partition_allows_insert_through_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_attach_allows_insert") else {
        return;
    };
    db.session().run(RECREATE_SETUP);

    // The row must land in the already-attached q2 partition: the ATTACH holding the
    // lock has not committed, so the prober cannot yet see txns_q1, and a row in
    // txns_q1's range would fail routing rather than fail on a lock -- which would prove
    // nothing about locking.
    assert_lock_allows(
        &db,
        &format!("ALTER TABLE txns ATTACH PARTITION txns_q1 {Q1_BOUND}"),
        "INSERT INTO txns VALUES (3, '2027-05-02', 7)",
        "ATTACH PARTITION's ShareUpdateExclusive on the parent is compatible with the \
         RowExclusive a writer takes",
    );
}

/// Control: the statement the migration actually used blocks the same reader.
#[rstest]
fn create_table_partition_of_blocks_select_through_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_partof_blocks_select") else {
        return;
    };
    db.session().run(RECREATE_SETUP);

    assert_lock_blocks(
        &db,
        &format!("CREATE TABLE txns_q0 PARTITION OF txns {Q1_BOUND}"),
        "SELECT count(*) FROM txns",
        "CREATE TABLE ... PARTITION OF takes AccessExclusive on the parent, so it \
         blocks reads that the equivalent ATTACH would have let through",
    );
}

#[rstest]
fn create_table_partition_of_blocks_insert_through_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_partof_blocks_insert") else {
        return;
    };
    db.session().run(RECREATE_SETUP);

    assert_lock_blocks(
        &db,
        &format!("CREATE TABLE txns_q0 PARTITION OF txns {Q1_BOUND}"),
        "INSERT INTO txns VALUES (3, '2027-05-02', 7)",
        "CREATE TABLE ... PARTITION OF takes AccessExclusive on the parent, so it \
         blocks writes that the equivalent ATTACH would have let through",
    );
}

/// Direction 2, the one the incident was about: does the application block the DDL?
#[rstest]
fn like_plus_attach_succeeds_while_traffic_holds_the_parent(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_attach_under_traffic") else {
        return;
    };
    let mut setup = db.session();
    setup.run(RECREATE_SETUP);
    setup.run("DROP TABLE txns_q1");

    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");
    migration.run("CREATE TABLE txns_q1 (LIKE txns INCLUDING ALL)");
    migration.run(&format!(
        "ALTER TABLE txns ATTACH PARTITION txns_q1 {Q1_BOUND}"
    ));
    migration.run("COMMIT");

    assert_eq!(
        migration.scalar_i64(
            "SELECT count(*)::bigint FROM pg_inherits
              WHERE inhparent = 'txns'::regclass AND inhrelid = 'txns_q1'::regclass"
        ),
        1,
        "pg{pg}: LIKE + ATTACH completes with a reader parked on the parent, because \
         ShareUpdateExclusive does not conflict with AccessShare"
    );
}

/// Control: the migration's own recreate statement, under the same traffic, fails.
#[rstest]
fn create_table_partition_of_fails_under_the_same_traffic(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_partof_under_traffic") else {
        return;
    };
    let mut setup = db.session();
    setup.run(RECREATE_SETUP);
    setup.run("DROP TABLE txns_q1");

    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("BEGIN");

    expect_lock_timeout(
        &mut migration,
        &format!("CREATE TABLE txns_q1 PARTITION OF txns {Q1_BOUND}"),
        "CREATE TABLE ... PARTITION OF needs AccessExclusive on the parent, so the same \
         reader that LIKE + ATTACH tolerates makes it give up",
    );
}

/// The recreate half of claim 1: swapping `PARTITION OF` for `LIKE INCLUDING ALL` +
/// `ATTACH` is a lock-level change, not a schema change.
#[rstest]
fn like_plus_attach_reaches_the_same_end_state_as_partition_of(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_attach_end_state") else {
        return;
    };
    let mut s = db.session();
    s.run(TWO_PARENTS_SETUP);

    // Route A, what the migration did.
    s.run(&format!(
        "CREATE TABLE plain_q1 PARTITION OF plain {Q1_BOUND}"
    ));

    // Route B, the low-lock replacement.
    s.run(&format!(
        "CREATE TABLE safe_q1 (LIKE safe INCLUDING ALL);
         ALTER TABLE safe ATTACH PARTITION safe_q1 {Q1_BOUND};"
    ));

    // Both children are attached, with the same bound.
    for (parent, child) in [("plain", "plain_q1"), ("safe", "safe_q1")] {
        assert_eq!(
            s.scalar_i64(&format!(
                "SELECT count(*)::bigint FROM pg_inherits
                  WHERE inhparent = '{parent}'::regclass
                    AND inhrelid = '{child}'::regclass"
            )),
            1,
            "pg{pg}: {child} must be an attached partition of {parent}"
        );
    }
    assert!(
        s.scalar_bool(
            "SELECT (SELECT pg_get_expr(relpartbound, oid) FROM pg_class
                      WHERE relname = 'plain_q1')
                  = (SELECT pg_get_expr(relpartbound, oid) FROM pg_class
                      WHERE relname = 'safe_q1')"
        ),
        "pg{pg}: ATTACH ... FOR VALUES must record the same partition bound as \
         PARTITION OF ... FOR VALUES"
    );

    // Columns, types and NOT NULL. `replace(..., 'X')` strips the only thing that is
    // legitimately different between the two routes: the parent's name.
    assert!(
        s.scalar_bool(
            "SELECT (SELECT array_agg(a ORDER BY a) FROM (
                       SELECT attname || ' ' || format_type(atttypid, atttypmod)
                              || ' notnull=' || attnotnull AS a
                         FROM pg_attribute
                        WHERE attrelid = 'plain_q1'::regclass
                          AND attnum > 0 AND NOT attisdropped) t)
                  = (SELECT array_agg(a ORDER BY a) FROM (
                       SELECT attname || ' ' || format_type(atttypid, atttypmod)
                              || ' notnull=' || attnotnull AS a
                         FROM pg_attribute
                        WHERE attrelid = 'safe_q1'::regclass
                          AND attnum > 0 AND NOT attisdropped) t)"
        ),
        "pg{pg}: LIKE ... INCLUDING ALL must reproduce the columns, types and NOT NULL \
         flags that PARTITION OF derives from the parent"
    );

    // Indexes: same definitions, and the same number of them.
    assert_eq!(
        s.scalar_i64("SELECT count(*)::bigint FROM pg_index WHERE indrelid = 'plain_q1'::regclass"),
        2,
        "pg{pg}: the PARTITION OF child is expected to carry the pkey and the amount \
         index, so the index comparison below is not vacuous"
    );
    assert!(
        s.scalar_bool(
            "SELECT (SELECT array_agg(d ORDER BY d) FROM (
                       SELECT replace(pg_get_indexdef(indexrelid), 'plain', 'X') AS d
                         FROM pg_index WHERE indrelid = 'plain_q1'::regclass) t)
                  = (SELECT array_agg(d ORDER BY d) FROM (
                       SELECT replace(pg_get_indexdef(indexrelid), 'safe', 'X') AS d
                         FROM pg_index WHERE indrelid = 'safe_q1'::regclass) t)"
        ),
        "pg{pg}: LIKE ... INCLUDING ALL must reproduce the child indexes that \
         PARTITION OF creates, so no separate CREATE INDEX is needed"
    );

    // Index *attachment*: the parent's partitioned indexes must own both children in
    // both routes, otherwise the parent index would be incomplete after the swap.
    for parent_index in [
        "plain_pkey",
        "plain_amount_idx",
        "safe_pkey",
        "safe_amount_idx",
    ] {
        assert_eq!(
            s.scalar_i64(&format!(
                "SELECT count(*)::bigint FROM pg_inherits
                  WHERE inhparent = '{parent_index}'::regclass"
            )),
            2,
            "pg{pg}: {parent_index} must have both child indexes attached -- ATTACH \
             PARTITION adopts the LIKE-created indexes instead of leaving them loose"
        );
    }
    assert!(
        s.scalar_bool(
            "SELECT bool_and(indisvalid) FROM pg_index
              WHERE indrelid IN ('plain'::regclass, 'safe'::regclass)"
        ),
        "pg{pg}: both parents' partitioned indexes must be valid after the swap"
    );

    // Constraints, including the inheritance bookkeeping (`conislocal`,
    // `coninhcount`) -- ATTACH merges the LIKE-copied CHECK with the parent's instead of
    // leaving a duplicate local one behind.
    assert!(
        s.scalar_bool(
            "SELECT (SELECT array_agg(c ORDER BY c) FROM (
                       SELECT replace(conname, 'plain', 'X') || ' '
                              || pg_get_constraintdef(oid)
                              || ' local=' || conislocal
                              || ' inhcount=' || coninhcount AS c
                         FROM pg_constraint WHERE conrelid = 'plain_q1'::regclass) t)
                  = (SELECT array_agg(c ORDER BY c) FROM (
                       SELECT replace(conname, 'safe', 'X') || ' '
                              || pg_get_constraintdef(oid)
                              || ' local=' || conislocal
                              || ' inhcount=' || coninhcount AS c
                         FROM pg_constraint WHERE conrelid = 'safe_q1'::regclass) t)"
        ),
        "pg{pg}: LIKE ... INCLUDING ALL + ATTACH must leave the same constraints, with \
         the same inheritance bookkeeping, as PARTITION OF"
    );
}

/// `DETACH PARTITION ... CONCURRENTLY` is *not* traffic-proof. It commits a first phase
/// that marks the partition detach-pending, then waits for every transaction that could
/// still see the old partition set. An open holder therefore makes it wait, and a
/// migration-grade `lock_timeout` turns that wait into 55P03 -- the same SQLSTATE the
/// plain drop fails with.
///
/// Two details separate it from the plain drop:
/// - the wait is on the holder's *virtualxid*, not on a relation lock, so it is invisible
///   to a relation-lock queue probe;
/// - phase one has already committed, so giving up does not roll the detach back. The
///   partition stays marked detach-pending, and queries through the parent already skip
///   its rows.
#[rstest]
fn detach_concurrently_waits_for_traffic_and_a_timeout_leaves_a_pending_detach(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_detach_conc_waits") else {
        return;
    };
    db.session().run(DROP_SETUP);

    let _traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    // No BEGIN: DETACH ... CONCURRENTLY refuses to run inside a transaction block.
    expect_lock_timeout(
        &mut migration,
        "ALTER TABLE txns DETACH PARTITION txns_q1 CONCURRENTLY",
        "DETACH PARTITION ... CONCURRENTLY waits for transactions that reference the \
         parent, so live traffic makes it give up too -- it is a weaker lock, not no wait",
    );

    let mut observer = db.session();
    assert!(
        observer.scalar_bool(
            "SELECT inhdetachpending FROM pg_inherits
              WHERE inhrelid = 'txns_q1'::regclass"
        ),
        "pg{pg}: the interrupted DETACH CONCURRENTLY does not roll back -- the partition \
         is left marked detach-pending, needing DETACH ... FINALIZE"
    );
    assert_eq!(
        observer.scalar_i64("SELECT count(*)::bigint FROM txns"),
        1,
        "pg{pg}: while the detach is pending, queries through the parent already skip \
         the partition's rows"
    );
    assert_eq!(
        observer.scalar_i64("SELECT count(*)::bigint FROM txns_q1"),
        1,
        "pg{pg}: ... even though the row is still there in the partition itself"
    );
}

/// Leaves `txns_q1` marked detach-pending, and returns with no traffic in flight.
///
/// Phase one of a concurrent detach commits before the waiting starts, so interrupting
/// the wait does not undo it. Callers therefore begin from a parent that is missing a
/// partition's rows while that partition still holds them.
fn leave_detach_pending(db: &TestDb) {
    drop(leave_detach_pending_under_traffic(db));
}

/// The same fixture, but the traffic that interrupted the detach is handed back still
/// parked on the parent, for the one test that cares what the pending state costs while
/// the parent is busy. Dropping the returned [`Holder`] releases it.
#[must_use = "the Holder is what keeps the parent locked; dropping it immediately \
              makes this the same as leave_detach_pending"]
fn leave_detach_pending_under_traffic(db: &TestDb) -> Holder {
    let traffic = db.hold("SELECT count(*) FROM txns");

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    expect_lock_timeout(
        &mut migration,
        DETACH_Q1_CONCURRENTLY,
        "fixture: the concurrent detach has to be interrupted by traffic for these tests \
         to have anything to assert about",
    );

    traffic
}

/// Nothing cleans this up in the background.
///
/// The window where the parent under-reports its rows lasts until somebody
/// intervenes.
#[rstest]
fn a_pending_detach_does_not_resolve_itself(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_pending_no_self_heal") else {
        return;
    };
    db.session().run(DROP_SETUP);
    leave_detach_pending(&db);

    let mut observer = db.session();
    // Time passes, and the parent is exercised — in case resolution were lazy and
    // triggered by access rather than by a background worker.
    observer.run("SELECT pg_sleep(2)");
    observer.run("SELECT count(*) FROM txns");

    // A brand new backend, in case the state were somehow local to a session.
    let mut fresh = db.session();
    assert!(
        fresh.scalar_bool(
            "SELECT inhdetachpending FROM pg_inherits
              WHERE inhrelid = 'txns_q1'::regclass"
        ),
        "pg{pg}: a pending detach does not resolve itself — not once the blocking traffic \
         has gone, not after time passes, and not after further traffic through the parent"
    );
    assert_eq!(
        fresh.scalar_i64("SELECT count(*)::bigint FROM txns"),
        1,
        "pg{pg}: ... so the parent keeps under-reporting its rows for as long as it lasts"
    );
    assert_eq!(
        fresh.scalar_i64("SELECT count(*)::bigint FROM txns_q1"),
        1,
        "pg{pg}: ... while the row is still sitting in the partition"
    );
}

/// Nor can the operation simply be retried: PostgreSQL refuses and names the way out.
#[rstest]
fn retrying_detach_concurrently_on_a_pending_partition_is_refused(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_pending_retry_refused") else {
        return;
    };
    db.session().run(DROP_SETUP);
    leave_detach_pending(&db);

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);

    // No traffic is holding anything now, so this is a flat refusal rather than a wait.
    let err = expect_sqlstate(
        &mut migration,
        DETACH_Q1_CONCURRENTLY,
        &SqlState::OBJECT_NOT_IN_PREREQUISITE_STATE,
        "re-running the same DETACH ... CONCURRENTLY does not resume a pending detach; \
         it is refused outright, so a retry-on-failure migration runner cannot recover",
    );
    assert!(
        detail(&err).contains("FINALIZE"),
        "pg{pg}: PostgreSQL should point at FINALIZE as the way out. Got: {}",
        detail(&err)
    );
}

#[rstest]
fn detach_finalize_completes_a_pending_detach(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "psafe_pending_finalize") else {
        return;
    };
    db.session().run(DROP_SETUP);
    leave_detach_pending(&db);

    let mut migration = db.session();
    migration.run("ALTER TABLE txns DETACH PARTITION txns_q1 FINALIZE");

    assert_eq!(
        migration.scalar_i64(
            "SELECT count(*)::bigint FROM pg_inherits
              WHERE inhrelid = 'txns_q1'::regclass"
        ),
        0,
        "pg{pg}: FINALIZE completes the detach — the inheritance link is gone"
    );
    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM txns_q1"),
        1,
        "pg{pg}: ... and the partition keeps its rows as a standalone table"
    );
}

#[rstest]
fn dropping_a_pending_partition_also_clears_the_pending_state(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_pending_drop") else {
        return;
    };
    db.session().run(DROP_SETUP);
    leave_detach_pending(&db);

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    migration.run("DROP TABLE txns_q1");

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: DROP TABLE resolves a pending detach as well as FINALIZE does — \
         FINALIZE is not the only exit"
    );
    assert_eq!(
        migration.scalar_i64(
            "SELECT count(*)::bigint FROM pg_inherits
              WHERE inhparent = 'txns'::regclass"
        ),
        1,
        "pg{pg}: ... leaving the parent with only its surviving partition"
    );
    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM txns"),
        1,
        "pg{pg}: ... and the parent readable again with no pending state hanging over it"
    );
}

/// A detach-pending partition is still a partition. It is still in `pg_inherits`, so its
/// `DROP TABLE` still takes AccessExclusive on the parent and still loses to the
/// traffic that caused the pending state.
///
/// Contrast `dropping_a_detached_partition_succeeds_while_traffic_holds_the_parent`:
/// once the detach has actually *completed*, the identical drop ignores the identical
/// reader. Half-detached does not buy the half of that benefit.
#[rstest]
fn dropping_a_pending_partition_still_loses_to_traffic_on_the_parent(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "psafe_pending_drop_vs_traffic") else {
        return;
    };
    db.session().run(DROP_SETUP);
    let traffic = leave_detach_pending_under_traffic(&db);

    let mut migration = db.session();
    migration.set_lock_timeout(MIGRATION_LOCK_TIMEOUT);
    expect_lock_timeout(
        &mut migration,
        "DROP TABLE txns_q1",
        "a detach-pending partition is still attached as far as locking is concerned, so \
         dropping it needs AccessExclusive on the parent -- 55P03 here, not the flat \
         refusal that a retried DETACH ... CONCURRENTLY gets",
    );

    // With the holder gone the identical statement goes through.
    drop(traffic);
    migration.run("DROP TABLE txns_q1");
    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM pg_class WHERE relname = 'txns_q1'"),
        0,
        "pg{pg}: the same drop completes once the parent is free, which is what pins the \
         timeout above on the parent lock rather than on the pending state"
    );
}
