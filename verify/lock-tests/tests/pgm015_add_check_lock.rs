//! PGM015 — ADD CHECK without NOT VALID acquires ACCESS EXCLUSIVE lock and scans table (PGM015)
//!
//! Ported from `verify/tests/locks/pgm015_add_check_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks, detail};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_chk(id int, val int);
    INSERT INTO test_chk SELECT g, g FROM generate_series(1, 1000) g;
";

/// A row that violates `val > 0`, so a validating ADD CHECK would have to fail.
const VIOLATING_ROW: &str = "INSERT INTO test_chk VALUES (9999, -1)";

#[rstest]
fn add_check_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm015_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_chk ADD CONSTRAINT chk_val CHECK (val > 0)",
        "SELECT count(*) FROM test_chk",
        "ADD CHECK blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn add_check_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm015_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_chk ADD CONSTRAINT chk_val2 CHECK (val > 0)",
        "INSERT INTO test_chk VALUES (9999, 1)",
        "ADD CHECK blocks INSERT (AccessExclusive)",
    );
}

/// `NOT VALID` skips the scan, so existing violating rows do not block the DDL.
#[rstest]
fn add_check_not_valid_succeeds_with_violating_data(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm015_not_valid_ok") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run(VIOLATING_ROW);

    s.try_run("ALTER TABLE test_chk ADD CONSTRAINT chk_nv CHECK (val > 0) NOT VALID")
        .unwrap_or_else(|e| {
            panic!(
                "pg{pg}: ADD CHECK NOT VALID succeeds with violating data: {}",
                detail(&e)
            )
        });
}

#[rstest]
fn not_valid_check_is_marked_unvalidated(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm015_not_valid_flag") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run(VIOLATING_ROW);
    s.run("ALTER TABLE test_chk ADD CONSTRAINT chk_nv CHECK (val > 0) NOT VALID");

    assert!(
        !s.scalar_bool("SELECT convalidated FROM pg_constraint WHERE conname = 'chk_nv'"),
        "pg{pg}: NOT VALID CHECK is marked unvalidated"
    );
}
