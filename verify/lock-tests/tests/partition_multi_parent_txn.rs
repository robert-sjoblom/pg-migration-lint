//! What the single wrapping transaction adds when a migration touches *many*
//! partitioned parents.
//!
//! The incident: one explicit `BEGIN; ... COMMIT;` containing ~192 `DROP TABLE`
//! statements against child partitions of **12 different** range-partitioned
//! parents, plus ~200 `CREATE TABLE ... PARTITION OF` / `ADD CONSTRAINT` /
//! `CREATE INDEX` statements to recreate coarser partitions. It errored and rolled
//! back. That rollback is the only *observed* fact — whether application traffic
//! actually stalled is a claim, and it is tested here as a claim: what these tests
//! establish is which locks the transaction *holds*, and what a concurrent read
//! through an affected parent does while they are held.
//!
//! Claims under test:
//! - Inside one transaction, dropping a child of parent A and then a child of parent
//!   B leaves the transaction holding ACCESS EXCLUSIVE on *both* parents at the same
//!   time (`pg_locks`), and a read through A is refused while the transaction is
//!   still working on B. Exposure is therefore cumulative over every parent the file
//!   has reached — 12 parents means 12 parents locked at once by the end.
//! - Contrast: the same statements in autocommit hold no lock on A once A's statement
//!   has returned; a read through A goes through with a 250 ms `lock_timeout`. Same
//!   total work, blast radius bounded to one statement at a time.
//! - Dropping several children of the *same* parent inside one transaction takes
//!   ACCESS EXCLUSIVE on that parent and keeps it for the rest of the transaction,
//!   rather than releasing it between statements.
//!
//! Relied on (already measured on 14–18, not re-derived here): `DROP TABLE <child>`
//! and `CREATE TABLE <child> PARTITION OF <parent>` both take
//! `AccessExclusiveLock` on the *parent*.
//!
//! Three parents stand in for twelve; nothing in the lock manager makes the fourth
//! parent behave differently from the third, and the assertions below are about
//! accumulation, which is what scales.

use pg_lock_tests::{TestDb, expect_lock_timeout};
use rstest::rstest;

/// Miniature of the real schema: three range-partitioned parents, two or three
/// children each, one row per child so counts are deterministic.
const SETUP: &str = "
    CREATE TABLE alpha(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE alpha_q1 PARTITION OF alpha
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE alpha_q2 PARTITION OF alpha
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');

    CREATE TABLE beta(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE beta_q1 PARTITION OF beta
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE beta_q2 PARTITION OF beta
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    CREATE TABLE beta_q3 PARTITION OF beta
        FOR VALUES FROM ('2027-07-01') TO ('2027-10-01');

    CREATE TABLE gamma(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE gamma_q1 PARTITION OF gamma
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE gamma_q2 PARTITION OF gamma
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');

    INSERT INTO alpha VALUES (1, '2027-02-01'), (2, '2027-05-01');
    INSERT INTO beta  VALUES (1, '2027-02-01'), (2, '2027-05-01'), (3, '2027-08-01');
    INSERT INTO gamma VALUES (1, '2027-02-01'), (2, '2027-05-01');
";

/// Bounds how long a probe waits before reporting 55P03 instead of hanging. Also set
/// on migration sessions, so a statement that unexpectedly blocks fails loudly rather
/// than wedging the suite.
const LOCK_TIMEOUT: &str = "250ms";

/// How many of the three parents currently have `AccessExclusiveLock` granted on them
/// in *this* database. `pg_locks` is cluster-wide and tests run in parallel, hence the
/// database filter.
const LOCKED_PARENTS: &str = "
    SELECT count(DISTINCT c.relname)::bigint
      FROM pg_locks l
      JOIN pg_class c ON c.oid = l.relation
     WHERE l.locktype = 'relation'
       AND l.granted
       AND l.mode = 'AccessExclusiveLock'
       AND c.relname IN ('alpha', 'beta', 'gamma')
       AND l.database = (SELECT oid FROM pg_database
                          WHERE datname = current_database())
";

const AEL: &str = "AccessExclusiveLock";

/// Claim 1: the locks accumulate. One transaction, two parents touched, both parents
/// held at ACCESS EXCLUSIVE simultaneously — and a read through the first parent is
/// refused while the transaction is still busy with the second.
#[rstest]
fn one_transaction_holds_every_parent_it_has_touched(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pmulti_two_parents_txn") else {
        return;
    };
    db.session().run(SETUP);

    // The migration session must stay open under our control, so it is a plain
    // session with an explicit BEGIN rather than a `Holder`.
    let mut migration = db.session();
    migration.set_lock_timeout(LOCK_TIMEOUT);
    migration.run("BEGIN");

    migration.run("DROP TABLE alpha_q1");
    assert!(
        db.holds_lock("alpha", AEL),
        "pg{pg}: dropping a child of alpha must leave ACCESS EXCLUSIVE held on alpha; \
         granted modes were {:?}",
        db.locks_on("alpha")
    );

    migration.run("DROP TABLE beta_q1");
    assert!(
        db.holds_lock("alpha", AEL) && db.holds_lock("beta", AEL),
        "pg{pg}: after touching two parents in one transaction both must be held at \
         ACCESS EXCLUSIVE at the same time; alpha={:?} beta={:?}",
        db.locks_on("alpha"),
        db.locks_on("beta")
    );

    // A reader that arrives now, addressed at the first parent, joins the lock queue
    // and stays there. `spawn_blocked` only returns once it is observably waiting.
    let reader_on_alpha = db.spawn_blocked("SELECT count(*) FROM alpha");

    // Meanwhile the transaction moves on to the rest of beta's rework — the shape the
    // real file had: coarser partitions replacing the dropped ones.
    migration.run("DROP TABLE beta_q2");
    migration.run(
        "CREATE TABLE beta_h1 PARTITION OF beta
             FOR VALUES FROM ('2027-01-01') TO ('2027-07-01')",
    );
    migration.run("CREATE INDEX beta_h1_key_idx ON beta_h1(partition_key)");

    // Nothing about working on beta released alpha.
    assert!(
        db.holds_lock("alpha", AEL),
        "pg{pg}: alpha must still be held at ACCESS EXCLUSIVE while the transaction \
         works on beta; granted modes were {:?}",
        db.locks_on("alpha")
    );

    let mut probe = db.session();
    probe.set_lock_timeout(LOCK_TIMEOUT);

    assert!(
        probe.ungranted_relation_locks() >= 1,
        "pg{pg}: the reader on alpha must still be queued (not granted) while the \
         transaction is working on beta"
    );

    assert_eq!(
        probe.scalar_i64(LOCKED_PARENTS),
        2,
        "pg{pg}: exactly the two parents the transaction reached must be held at \
         ACCESS EXCLUSIVE — the exposure is the union of every parent so far, \
         which is why 12 parents means 12 locked parents at commit time"
    );

    expect_lock_timeout(
        &mut probe,
        "SELECT count(*) FROM alpha",
        "a read routed through the first parent is refused while the transaction is \
         mid-flight on a different parent",
    );

    // Control: the parent this transaction never touched is unaffected, so the
    // refusal above is the accumulated lock and not a database-wide stall.
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM gamma"),
        2,
        "pg{pg}: a parent the transaction never touched stays readable, so the \
         blocked read is attributable to the locks this transaction accumulated"
    );

    migration.run("COMMIT");

    // The queued reader waited from before the beta work until COMMIT: alpha was held
    // continuously across every intervening statement.
    reader_on_alpha
        .join()
        .unwrap_or_else(|e| panic!("pg{pg}: reader on alpha should proceed after COMMIT: {e}"));
}

/// Claim 2 (contrast): the same statements without the wrapping transaction. Each
/// parent is released the moment its own statement returns, so a read through an
/// already-processed parent goes through while the migration is still running.
#[rstest]
fn autocommit_releases_each_parent_when_its_statement_returns(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pmulti_autocommit_release") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.set_lock_timeout(LOCK_TIMEOUT);

    let mut probe = db.session();
    probe.set_lock_timeout(LOCK_TIMEOUT);

    // No BEGIN: one `run` per statement, each its own transaction.
    migration.run("DROP TABLE alpha_q1");
    assert!(
        !db.holds_lock("alpha", AEL),
        "pg{pg}: in autocommit the lock on alpha must be gone once the statement has \
         returned; granted modes were {:?}",
        db.locks_on("alpha")
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM alpha"),
        1,
        "pg{pg}: a read through alpha goes through (within a 250ms lock_timeout) as \
         soon as alpha's own statement has returned"
    );

    migration.run("DROP TABLE beta_q1");
    assert_eq!(
        probe.scalar_i64(LOCKED_PARENTS),
        0,
        "pg{pg}: with no wrapping transaction, no parent is still held after its \
         statement — the locks do not accumulate"
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM alpha"),
        1,
        "pg{pg}: alpha stays readable while the migration works on beta"
    );

    // Same remaining work as the transactional case, statement by statement.
    migration.run("DROP TABLE beta_q2");
    migration.run(
        "CREATE TABLE beta_h1 PARTITION OF beta
             FOR VALUES FROM ('2027-01-01') TO ('2027-07-01')",
    );
    migration.run("CREATE INDEX beta_h1_key_idx ON beta_h1(partition_key)");

    assert_eq!(
        probe.scalar_i64(LOCKED_PARENTS),
        0,
        "pg{pg}: after the same total work, autocommit leaves nothing held"
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM alpha"),
        1,
        "pg{pg}: alpha readable after the whole migration"
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM beta"),
        1,
        "pg{pg}: beta readable after the whole migration"
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM gamma"),
        2,
        "pg{pg}: gamma untouched and readable"
    );
}

/// Claim 3: several children of the *same* parent in one transaction. The parent's
/// ACCESS EXCLUSIVE lock is taken by the first statement and kept for the rest of the
/// transaction; it is not released between statements.
#[rstest]
fn same_parent_stays_locked_across_statements_in_one_transaction(
    #[values(14, 15, 16, 17, 18)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pmulti_same_parent_txn") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.set_lock_timeout(LOCK_TIMEOUT);
    migration.run("BEGIN");

    migration.run("DROP TABLE gamma_q1");
    assert!(
        db.holds_lock("gamma", AEL),
        "pg{pg}: the first drop must take ACCESS EXCLUSIVE on gamma; granted modes \
         were {:?}",
        db.locks_on("gamma")
    );

    // A reader arriving between the two drops. If the lock were released at statement
    // boundaries it would be granted here.
    let reader_on_gamma = db.spawn_blocked("SELECT count(*) FROM gamma");

    // The second child of the same parent: the transaction already holds the parent
    // lock, so it proceeds even though a reader is queued ahead of it.
    migration.run("DROP TABLE gamma_q2");

    assert_eq!(
        db.locks_on("gamma"),
        vec![AEL.to_string()],
        "pg{pg}: between and after the two drops the only lock granted on gamma is \
         the transaction's ACCESS EXCLUSIVE — the queued reader never gets in"
    );

    let mut probe = db.session();
    probe.set_lock_timeout(LOCK_TIMEOUT);
    expect_lock_timeout(
        &mut probe,
        "SELECT count(*) FROM gamma",
        "a read through the parent is still refused after the second drop, so the \
         parent lock is held for the rest of the transaction rather than released \
         between statements",
    );

    migration.run("COMMIT");

    // The reader was enqueued after the first drop and only completes now: the lock
    // was continuous across the statement boundary.
    reader_on_gamma
        .join()
        .unwrap_or_else(|e| panic!("pg{pg}: reader on gamma should proceed after COMMIT: {e}"));

    assert_eq!(
        migration.scalar_i64("SELECT count(*)::bigint FROM gamma"),
        0,
        "pg{pg}: both children were dropped"
    );
}

/// Control for claim 3: the same two drops of the same parent, in autocommit. The
/// parent is free between statements, so a read gets in mid-migration.
#[rstest]
fn autocommit_frees_the_same_parent_between_statements(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pmulti_same_parent_autocommit") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.set_lock_timeout(LOCK_TIMEOUT);

    let mut probe = db.session();
    probe.set_lock_timeout(LOCK_TIMEOUT);

    migration.run("DROP TABLE gamma_q1");
    assert!(
        !db.holds_lock("gamma", AEL),
        "pg{pg}: no wrapping transaction, so gamma is free once the drop returned; \
         granted modes were {:?}",
        db.locks_on("gamma")
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM gamma"),
        1,
        "pg{pg}: a read through gamma goes through between the two drops"
    );

    migration.run("DROP TABLE gamma_q2");
    assert!(
        !db.holds_lock("gamma", AEL),
        "pg{pg}: gamma free again after the second drop; granted modes were {:?}",
        db.locks_on("gamma")
    );
    assert_eq!(
        probe.scalar_i64("SELECT count(*)::bigint FROM gamma"),
        0,
        "pg{pg}: same total work, but the parent was only locked for the duration of \
         each individual statement"
    );
}
