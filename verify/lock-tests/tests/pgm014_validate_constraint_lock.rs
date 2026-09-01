//! VALIDATE CONSTRAINT uses SHARE UPDATE EXCLUSIVE lock (PGM014)
//! This allows reads and writes while validation is in progress
//!
//! Ported from `verify/tests/fk/pgm014_validate_constraint_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_allows};
use rstest::rstest;

/// Parent/child tables with an FK added `NOT VALID`, so that
/// `VALIDATE CONSTRAINT` still has work to do.
const SETUP: &str = "
    CREATE TABLE test_vp(id int PRIMARY KEY);
    CREATE TABLE test_vc(id int, parent_id int);
    INSERT INTO test_vp SELECT g FROM generate_series(1, 1000) g;
    INSERT INTO test_vc SELECT g, g FROM generate_series(1, 1000) g;
    ALTER TABLE test_vc ADD CONSTRAINT fk_valid
        FOREIGN KEY (parent_id) REFERENCES test_vp(id) NOT VALID;
";

#[rstest]
fn validate_constraint_allows_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_validate_allows_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        "ALTER TABLE test_vc VALIDATE CONSTRAINT fk_valid",
        "INSERT INTO test_vc VALUES (9999, 1)",
        "VALIDATE CONSTRAINT allows INSERT (ShareUpdateExclusive vs RowExclusive)",
    );
}

#[rstest]
fn validate_constraint_allows_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm014_validate_allows_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_allows(
        &db,
        "ALTER TABLE test_vc VALIDATE CONSTRAINT fk_valid",
        "SELECT count(*) FROM test_vc",
        "VALIDATE CONSTRAINT allows SELECT (ShareUpdateExclusive vs AccessShare)",
    );
}
