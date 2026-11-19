//! PGM003: a `COMMIT` in a migration sent as one query does not get the
//! statements after it out of a transaction block.
//!
//! golang-migrate's default mode sends a whole migration file as one
//! simple-protocol query, as `batch_execute` does here.
//!
//! Claims under test:
//! - In one query of several statements, a statement after a `COMMIT` runs in a
//!   new implicit transaction block, so `CREATE INDEX CONCURRENTLY` there fails
//!   with SQLSTATE 25001, even as the query's last statement.
//! - The `COMMIT` does end the transaction before it: what ran before it stays
//!   committed after the later statement fails.
//! - The same `CREATE INDEX CONCURRENTLY` sent as a query of its own runs
//!   outside a transaction block and succeeds.

use pg_lock_tests::{SqlState, TestDb, expect_sqlstate};
use rstest::rstest;

const SETUP: &str = "CREATE TABLE orders (id bigint, status text);";

const ORDERS_INDEXES: &str = "SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'orders'";

#[rstest]
fn concurrently_after_a_commit_in_one_query_runs_in_a_new_implicit_transaction_block(
    #[values(14, 15, 16, 17, 18, 19)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pgm003_concurrently_after_commit") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);

    expect_sqlstate(
        &mut s,
        "INSERT INTO orders VALUES (1, 'new');
         COMMIT;
         CREATE INDEX CONCURRENTLY orders_status_idx ON orders (status);",
        &SqlState::ACTIVE_SQL_TRANSACTION,
        "a statement after COMMIT in one multi-statement query is still inside an \
         implicit transaction block, where CREATE INDEX CONCURRENTLY is refused",
    );

    assert_eq!(
        s.scalar_i64("SELECT count(*)::bigint FROM orders"),
        1,
        "pg{pg}: the COMMIT ended the first implicit transaction, so its INSERT stays \
         committed after the later statement failed"
    );
    assert_eq!(
        s.scalar_i64(ORDERS_INDEXES),
        0,
        "pg{pg}: the refused CREATE INDEX CONCURRENTLY built no index"
    );
}

#[rstest]
fn concurrently_alone_in_its_query_runs_outside_a_transaction_block(
    #[values(14, 15, 16, 17, 18, 19)] pg: u32,
) {
    let Some(db) = TestDb::new(pg, "pgm003_concurrently_alone") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);

    s.run("CREATE INDEX CONCURRENTLY orders_status_idx ON orders (status);");

    assert_eq!(
        s.scalar_i64(ORDERS_INDEXES),
        1,
        "pg{pg}: CREATE INDEX CONCURRENTLY as a query of its own built the index"
    );
}
