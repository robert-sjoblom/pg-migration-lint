//! Whether ATTACH/DETACH PARTITION on a parent still work once that same parent
//! already holds ACCESS EXCLUSIVE from an earlier statement in the *same* open
//! transaction.
//!
//! Motivation: `partition_multi_parent_txn.rs` only measured repeating the *same*
//! operation (`DROP TABLE <child>`) against a parent that already holds ACCESS
//! EXCLUSIVE. It never measured a *different* partition operation — specifically
//! ATTACH or DETACH — against a parent already locked that way in the same
//! transaction.
//!
//! Claims under test:
//! - After `DROP TABLE <child>` has taken ACCESS EXCLUSIVE on the parent, a
//!   `DETACH PARTITION` of a *different* child of that same parent, later in the
//!   same transaction, either succeeds or fails with a specific, named SQLSTATE.
//! - Same question for `ATTACH PARTITION` of a standalone table onto that parent.
//! - Lock manager theory says a transaction's own already-held lock never
//!   conflicts with a weaker or equal request from that same transaction, so if
//!   either of the above fails, the failure is a catalog/semantic restriction, not
//!   a self-lock-wait.

use pg_lock_tests::TestDb;
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE txns(id int, partition_key date) PARTITION BY RANGE (partition_key);
    CREATE TABLE txns_q1 PARTITION OF txns
        FOR VALUES FROM ('2027-01-01') TO ('2027-04-01');
    CREATE TABLE txns_q2 PARTITION OF txns
        FOR VALUES FROM ('2027-04-01') TO ('2027-07-01');
    CREATE TABLE txns_standalone(id int, partition_key date);
";

const ACCESS_EXCLUSIVE: &str = "AccessExclusiveLock";

#[rstest]
fn detach_after_drop_child_holds_parent(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pahd_detach_after_drop") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");

    assert!(
        db.holds_lock("txns", ACCESS_EXCLUSIVE),
        "pg{pg}: dropping txns_q1 must leave ACCESS EXCLUSIVE on txns; granted \
         modes were {:?}",
        db.locks_on("txns")
    );

    // The parent already holds ACCESS EXCLUSIVE from the DROP above. Does a
    // DETACH of a *different* child, still in the same transaction, succeed?
    let outcome = migration.try_run("ALTER TABLE txns DETACH PARTITION txns_q2");

    match outcome {
        Ok(()) => eprintln!(
            "pg{pg}: DETACH PARTITION of a different child succeeded while the \
             parent already held ACCESS EXCLUSIVE from an earlier DROP in the same \
             transaction"
        ),
        Err(e) => panic!(
            "pg{pg}: DETACH PARTITION of a different child FAILED while the parent \
             already held ACCESS EXCLUSIVE from an earlier DROP in the same \
             transaction: {}",
            pg_lock_tests::detail(&e)
        ),
    }

    migration.run("ROLLBACK");
}

#[rstest]
fn attach_after_drop_child_holds_parent(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pahd_attach_after_drop") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.run("BEGIN");
    migration.run("DROP TABLE txns_q1");

    assert!(
        db.holds_lock("txns", ACCESS_EXCLUSIVE),
        "pg{pg}: dropping txns_q1 must leave ACCESS EXCLUSIVE on txns; granted \
         modes were {:?}",
        db.locks_on("txns")
    );

    // The parent already holds ACCESS EXCLUSIVE from the DROP above. Does an
    // ATTACH of a standalone table, still in the same transaction, succeed?
    let outcome = migration.try_run(
        "ALTER TABLE txns ATTACH PARTITION txns_standalone \
             FOR VALUES FROM ('2027-07-01') TO ('2027-10-01')",
    );

    match outcome {
        Ok(()) => eprintln!(
            "pg{pg}: ATTACH PARTITION succeeded while the parent already held \
             ACCESS EXCLUSIVE from an earlier DROP in the same transaction"
        ),
        Err(e) => panic!(
            "pg{pg}: ATTACH PARTITION FAILED while the parent already held ACCESS \
             EXCLUSIVE from an earlier DROP in the same transaction: {}",
            pg_lock_tests::detail(&e)
        ),
    }

    migration.run("ROLLBACK");
}

#[rstest]
fn detach_after_attach_of_a_different_child(#[values(14, 15, 16, 17, 18, 19)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pahd_detach_after_attach") else {
        return;
    };
    db.session().run(SETUP);

    let mut migration = db.session();
    migration.run("BEGIN");
    migration.run(
        "ALTER TABLE txns ATTACH PARTITION txns_standalone \
             FOR VALUES FROM ('2027-07-01') TO ('2027-10-01')",
    );

    // ATTACH takes ACCESS EXCLUSIVE on the table being attached, not the parent
    // (measured in partition_ddl_lock_matrix.rs) — so this transaction now holds
    // ACCESS EXCLUSIVE on txns_standalone specifically, and only SHARE UPDATE
    // EXCLUSIVE on txns itself.
    let outcome = migration.try_run("ALTER TABLE txns DETACH PARTITION txns_q1");

    match outcome {
        Ok(()) => eprintln!(
            "pg{pg}: DETACH of a different child succeeded after ATTACHing a \
             standalone table onto the same parent in the same transaction"
        ),
        Err(e) => panic!(
            "pg{pg}: DETACH of a different child FAILED after ATTACHing a \
             standalone table onto the same parent in the same transaction: {}",
            pg_lock_tests::detail(&e)
        ),
    }

    migration.run("ROLLBACK");
}
