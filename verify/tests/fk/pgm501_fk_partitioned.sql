-- @claim: The RI check on a partitioned referencing table traverses the partitions,
--   not just the (permanently empty) parent relation — deleting a referenced row with
--   a dependent only in a partition is rejected.
-- @claim: Partition key present in the FK column list -> the RI check prunes to
--   exactly the one partition that can hold a match.
-- @claim: Partition key absent from the FK column list -> the RI check locks every
--   partition, with no pruning at all, since pruning is driven by the FK column list
--   and not by any index.
-- @claim: An index on the referencing table's FK columns does not change the above —
--   pruning is unavailable regardless of index shape when the partition key isn't
--   among the FK columns.
-- @claim: ON DELETE CASCADE fans out identically — a cascaded delete removes matching
--   rows from every partition that holds one.
-- @claim: The referenced side carries one trigger pair for the whole FK, not one per
--   partition, even though pg_constraint holds one row per partition plus the parent.
-- @claim: A UNIQUE index on a partitioned table cannot omit the partition key.
-- @claim: Once a prepared statement's plan goes generic, partition pruning becomes a
--   runtime "init pruning" step, visible in EXPLAIN ANALYZE as "Subplans Removed".
-- @min_version: 14

-- Setup: ref_y is the referenced table; kid is RANGE-partitioned on period, which is
-- part of the FK. Checkerboard population (t+p) % 2 = 0 leaves half of all
-- (tenant_id, period) pairs absent from kid, which is what lets the lock-count
-- assertions below prove pruning by absence alone, not by an early match.
CREATE TABLE ref_y (tenant_id int, period int, PRIMARY KEY (tenant_id, period));
INSERT INTO ref_y SELECT t, p FROM generate_series(1, 4) t, generate_series(1, 200) p;

CREATE TABLE kid (tenant_id int, period int NOT NULL, payload text,
    FOREIGN KEY (tenant_id, period) REFERENCES ref_y (tenant_id, period))
    PARTITION BY RANGE (period);
CREATE TABLE kid_p1 PARTITION OF kid FOR VALUES FROM (1) TO (51);
CREATE TABLE kid_p2 PARTITION OF kid FOR VALUES FROM (51) TO (101);
CREATE TABLE kid_p3 PARTITION OF kid FOR VALUES FROM (101) TO (151);
CREATE TABLE kid_p4 PARTITION OF kid FOR VALUES FROM (151) TO (201);
INSERT INTO kid SELECT t, p, 'x' FROM generate_series(1, 4) t, generate_series(1, 200) p,
       generate_series(1, 100) c WHERE (t + p) % 2 = 0;
ANALYZE;

-- (2, 74): present (sum 76, even) — has dependents in kid_p2.
-- (2, 75): absent (sum 77, odd) — no dependents anywhere, used to prove pruning by
--          lock count alone below.

-- Claim: RI check traverses partitions, not just the empty parent
DO $$
BEGIN
    BEGIN
        DELETE FROM ref_y WHERE tenant_id = 2 AND period = 74;
        RAISE EXCEPTION USING ERRCODE = 'triggered_action_exception',
            MESSAGE = 'unexpected: delete of a referenced row with a partition dependent succeeded';
    EXCEPTION
        WHEN foreign_key_violation THEN
            PERFORM assert_true(true,
                'RI check on partitioned referencing table rejects delete with a dependent in a partition');
        WHEN OTHERS THEN
            PERFORM assert_false(true,
                'RI check on partitioned referencing table rejects delete with a dependent in a partition');
    END;
END $$;

-- Claim: partition key present in FK -> pruned to exactly one partition
-- assert_eq's own _verify_results insert must happen AFTER the ROLLBACK, since a
-- ROLLBACK inside this block would otherwise undo the assertion's own recorded row
-- along with the DELETE — \gset captures the count into a psql variable first.
BEGIN;
DELETE FROM ref_y WHERE tenant_id = 2 AND period = 75;
SELECT count(*) AS locked_partitions FROM pg_locks
    WHERE relation::regclass::text ~ '^kid_p[0-9]+$' AND mode = 'RowShareLock';
\gset
ROLLBACK;
SELECT assert_eq(:locked_partitions::bigint, 1::bigint,
    'partition key in FK: RI check locks exactly one partition (pruned)');

-- kid_unpruned: RANGE-partitioned on region, which is NOT part of the FK. Same
-- checkerboard population spread across every region, so (2, 75) stays absent here
-- too, letting the same lock-count technique prove the absence of pruning.
CREATE TABLE kid_unpruned (tenant_id int, period int NOT NULL, region int NOT NULL, payload text,
    FOREIGN KEY (tenant_id, period) REFERENCES ref_y (tenant_id, period))
    PARTITION BY RANGE (region);
CREATE TABLE kid_unpruned_r1 PARTITION OF kid_unpruned FOR VALUES FROM (1) TO (2);
CREATE TABLE kid_unpruned_r2 PARTITION OF kid_unpruned FOR VALUES FROM (2) TO (3);
CREATE TABLE kid_unpruned_r3 PARTITION OF kid_unpruned FOR VALUES FROM (3) TO (4);
CREATE TABLE kid_unpruned_r4 PARTITION OF kid_unpruned FOR VALUES FROM (4) TO (5);
INSERT INTO kid_unpruned SELECT t, p, r, 'x'
    FROM generate_series(1, 4) t, generate_series(1, 200) p, generate_series(1, 4) r
    WHERE (t + p) % 2 = 0;
ANALYZE kid_unpruned;

-- Claim: partition key absent from FK -> every partition locked, no pruning
BEGIN;
DELETE FROM ref_y WHERE tenant_id = 2 AND period = 75;
SELECT count(*) AS locked_partitions FROM pg_locks
    WHERE relation::regclass::text ~ '^kid_unpruned_r[0-9]+$' AND mode = 'RowShareLock';
\gset
ROLLBACK;
SELECT assert_eq(:locked_partitions::bigint, 4::bigint,
    'partition key absent from FK: RI check locks every partition (no pruning)');

-- Claim: an index on the FK columns does not change the above — pruning is driven by
-- the FK column list, never by an index
CREATE INDEX ON kid_unpruned (tenant_id, period);
ANALYZE kid_unpruned;

BEGIN;
DELETE FROM ref_y WHERE tenant_id = 2 AND period = 75;
SELECT count(*) AS locked_partitions FROM pg_locks
    WHERE relation::regclass::text ~ '^kid_unpruned_r[0-9]+$' AND mode = 'RowShareLock';
\gset
ROLLBACK;
SELECT assert_eq(:locked_partitions::bigint, 4::bigint,
    'an index on the FK columns does not restore pruning when the partition key is absent from the FK');

-- Claim: ON DELETE CASCADE fans out identically
CREATE TABLE kid_cascade (tenant_id int, period int NOT NULL, payload text,
    FOREIGN KEY (tenant_id, period) REFERENCES ref_y (tenant_id, period) ON DELETE CASCADE)
    PARTITION BY RANGE (period);
CREATE TABLE kid_cascade_p1 PARTITION OF kid_cascade FOR VALUES FROM (1) TO (51);
CREATE TABLE kid_cascade_p2 PARTITION OF kid_cascade FOR VALUES FROM (51) TO (101);
CREATE TABLE kid_cascade_p3 PARTITION OF kid_cascade FOR VALUES FROM (101) TO (151);
CREATE TABLE kid_cascade_p4 PARTITION OF kid_cascade FOR VALUES FROM (151) TO (201);

INSERT INTO ref_y VALUES (99, 10);
INSERT INTO kid_cascade VALUES (99, 10, 'x');
DELETE FROM ref_y WHERE tenant_id = 99 AND period = 10;

SELECT assert_eq(
    (SELECT count(*) FROM kid_cascade WHERE tenant_id = 99 AND period = 10),
    0::bigint,
    'ON DELETE CASCADE removes the dependent row from its partition'
);

-- Claim: one trigger pair for the whole FK, not one per partition — contrasted with
-- pg_constraint, which does hold one row per partition plus the parent
SELECT assert_eq(
    (SELECT count(*) FROM pg_trigger t
        JOIN pg_constraint c ON t.tgconstraint = c.oid
        WHERE c.conrelid = 'kid'::regclass AND t.tgrelid = 'ref_y'::regclass),
    2::bigint,
    'referenced side carries exactly one trigger pair for the FK, regardless of partition count'
);

SELECT assert_eq(
    (SELECT count(*) FROM pg_constraint
        WHERE conrelid IN ('kid'::regclass, 'kid_p1'::regclass, 'kid_p2'::regclass,
                            'kid_p3'::regclass, 'kid_p4'::regclass)
          AND contype = 'f'),
    5::bigint,
    'pg_constraint holds one FK row per partition plus the parent (5), unlike pg_trigger'
);

-- Claim: a UNIQUE index on a partitioned table cannot omit the partition key
-- (region). ON_ERROR_STOP=0 in the runner means this failing statement does not
-- abort the rest of the file.
CREATE UNIQUE INDEX bad_unique_idx ON kid_unpruned (tenant_id, period);

SELECT assert_false(
    EXISTS(SELECT 1 FROM pg_indexes WHERE indexname = 'bad_unique_idx'),
    'a UNIQUE index omitting the partition key is refused, not silently created'
);

-- Claim: once the plan goes generic, pruning becomes a runtime "init pruning" step,
-- visible as "Subplans Removed" in EXPLAIN ANALYZE — (2, 75) is absent everywhere,
-- so a custom plan would prune to kid_p2 at plan time; forcing a generic plan is what
-- pushes the pruning decision to execution time.
PREPARE kid_lookup(int, int) AS
    SELECT 1 FROM kid WHERE tenant_id = $1 AND period = $2 FOR KEY SHARE;
SET plan_cache_mode = force_generic_plan;

SELECT assert_explain_contains(
    'ANALYZE EXECUTE kid_lookup(2, 75)',
    'Subplans Removed',
    'generic plan pruning is visible at runtime as Subplans Removed'
);

RESET plan_cache_mode;

-- Cleanup (children before the referenced table; DROP TABLE on a partitioned parent
-- takes every partition with it, no CASCADE needed)
DROP TABLE kid_cascade;
DROP TABLE kid_unpruned;
DROP TABLE kid;
DROP TABLE ref_y;
