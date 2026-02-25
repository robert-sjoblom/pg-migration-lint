-- @claim: ADD PRIMARY KEY acquires ACCESS EXCLUSIVE lock and builds a new index (PGM016)
-- @claim: ADD PRIMARY KEY USING INDEX reuses existing index
-- @min_version: 14

-- Setup
CREATE TABLE test_pk(id int NOT NULL, val text);
INSERT INTO test_pk SELECT g, md5(g::text) FROM generate_series(1, 1000) g;

-- Test 1: ADD PRIMARY KEY blocks SELECT (ACCESS EXCLUSIVE)
SELECT assert_lock_blocks(
    'ALTER TABLE test_pk ADD PRIMARY KEY (id)',
    'SELECT count(*) FROM test_pk',
    'ADD PRIMARY KEY blocks SELECT (AccessExclusive)'
);

ALTER TABLE test_pk DROP CONSTRAINT IF EXISTS test_pk_pkey;

-- Test 2: ADD PRIMARY KEY blocks INSERT
SELECT assert_lock_blocks(
    'ALTER TABLE test_pk ADD PRIMARY KEY (id)',
    'INSERT INTO test_pk VALUES (999999, ''probe'')',
    'ADD PRIMARY KEY blocks INSERT (AccessExclusive)'
);

ALTER TABLE test_pk DROP CONSTRAINT IF EXISTS test_pk_pkey;

-- Test 3: ADD PRIMARY KEY creates a NEW index even if a matching unique index exists
CREATE UNIQUE INDEX idx_pk_id ON test_pk(id);

-- Count indexes before
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_pk'),
    1::bigint,
    'One index before ADD PK'
);

ALTER TABLE test_pk ADD PRIMARY KEY (id);

-- Should have 2 indexes: the original unique + the new PK index
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_pk'),
    2::bigint,
    'ADD PK creates new index even with existing unique index'
);

ALTER TABLE test_pk DROP CONSTRAINT test_pk_pkey;
DROP INDEX idx_pk_id;

-- Test 4: ADD PRIMARY KEY USING INDEX reuses existing index
CREATE UNIQUE INDEX idx_pk_reuse ON test_pk(id);

ALTER TABLE test_pk ADD CONSTRAINT test_pk_pkey PRIMARY KEY USING INDEX idx_pk_reuse;

-- Should still have just 1 index (reused)
SELECT assert_eq(
    (SELECT count(*)::bigint FROM pg_indexes WHERE tablename = 'test_pk'),
    1::bigint,
    'ADD PK USING INDEX reuses existing index (still 1 index)'
);

-- Cleanup
DROP TABLE test_pk;
