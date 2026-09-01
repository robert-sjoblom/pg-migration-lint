//! PGM016 — ADD PRIMARY KEY takes ACCESS EXCLUSIVE and builds a new index.
//!
//! Also covers the escape hatch: `ADD PRIMARY KEY USING INDEX` adopts an existing
//! index instead of building another one.
//!
//! Ported from `verify/tests/locks/pgm016_add_pk_lock.sql`.

use pg_lock_tests::{TestDb, assert_lock_blocks};
use rstest::rstest;

const SETUP: &str = "
    CREATE TABLE test_pk(id int NOT NULL, val text);
    INSERT INTO test_pk SELECT g, md5(g::text) FROM generate_series(1, 1000) g;
";

const INDEX_COUNT: &str = "SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_pk'";

#[rstest]
fn add_pk_blocks_select(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm016_blocks_select") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_pk ADD PRIMARY KEY (id)",
        "SELECT count(*) FROM test_pk",
        "ADD PRIMARY KEY blocks SELECT (AccessExclusive)",
    );
}

#[rstest]
fn add_pk_blocks_insert(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm016_blocks_insert") else {
        return;
    };
    db.session().run(SETUP);

    assert_lock_blocks(
        &db,
        "ALTER TABLE test_pk ADD PRIMARY KEY (id)",
        "INSERT INTO test_pk VALUES (999999, 'probe')",
        "ADD PRIMARY KEY blocks INSERT (AccessExclusive)",
    );
}

/// A matching unique index is not adopted implicitly; PostgreSQL builds a second one.
#[rstest]
fn add_pk_builds_new_index_despite_existing_unique(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm016_new_index") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run("CREATE UNIQUE INDEX idx_pk_id ON test_pk(id)");

    assert_eq!(s.scalar_i64(INDEX_COUNT), 1, "one index before ADD PK");

    s.run("ALTER TABLE test_pk ADD PRIMARY KEY (id)");

    assert_eq!(
        s.scalar_i64(INDEX_COUNT),
        2,
        "pg{pg}: ADD PK should build its own index even though a matching \
         unique index already exists"
    );
}

/// `USING INDEX` is the way to avoid that second build.
#[rstest]
fn add_pk_using_index_reuses_existing(#[values(14, 15, 16, 17, 18)] pg: u32) {
    let Some(db) = TestDb::new(pg, "pgm016_using_index") else {
        return;
    };
    let mut s = db.session();
    s.run(SETUP);
    s.run("CREATE UNIQUE INDEX idx_pk_reuse ON test_pk(id)");

    s.run("ALTER TABLE test_pk ADD CONSTRAINT test_pk_pkey PRIMARY KEY USING INDEX idx_pk_reuse");

    assert_eq!(
        s.scalar_i64(INDEX_COUNT),
        1,
        "pg{pg}: ADD PK USING INDEX should adopt the existing index, not build another"
    );
}
