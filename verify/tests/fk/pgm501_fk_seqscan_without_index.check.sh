#!/usr/bin/env bash
# pgm501_fk_seqscan_without_index.check.sh: real enforcement-path check for PGM501.
#
# The RI trigger's own query is invisible to plain EXPLAIN — only auto_explain with
# log_nested_statements reveals it, and it lands in the container's stderr rather than
# in a query result. That means this claim cannot be verified with a plain
# assert_explain_contains() call from the .sql file; it needs to watch `docker logs`
# around a real triggering DELETE. Invoked by runner.sh after the sibling .sql file,
# against the same container/db, with results fed back into _verify_results so they
# show up alongside that file's own assertions.
#
# Usage: pgm501_fk_seqscan_without_index.check.sh <container> <db_name>

set -euo pipefail

CONTAINER="$1"
DB_NAME="$2"
LABEL_PREFIX="fk/pgm501_fk_seqscan_without_index.sql"

run_psql() {
    docker exec -i "$CONTAINER" psql -U postgres -d "$DB_NAME" -v ON_ERROR_STOP=1 -q "$@"
}

record() {
    local passed="$1"
    local label="$2"
    run_psql -c "SELECT assert_true(${passed}, '${label}');" > /dev/null
}

# Capture the RI trigger's plan for a real DELETE on the referenced table by diffing
# docker logs around the statement. auto_explain logs one nested-statement block per
# internal query in this transaction — including a row-lock fetch against parent_e
# itself and the outer DELETE's own plan — so this scopes strictly to the block whose
# Query Text mentions child_e: start at that line, keep only the tab-indented
# continuation lines that belong to the same block (postgres's multi-line log format
# prefixes every continuation line with a tab), and stop at the first non-tab line
# (the CONTEXT line that closes the block). A fixed `grep -A N` would either truncate
# a long block or, if N is generous enough to avoid that, overrun into the next
# block — confirmed live: with -A 15 the short no-index block bled into the
# following DELETE's "Index Scan using parent_e_pkey" line, producing a false pass.
capture_child_e_plan() {
    local before after
    before=$(docker logs "$CONTAINER" 2>&1 | wc -l)
    run_psql -q <<'SQL' > /dev/null
LOAD 'auto_explain';
SET auto_explain.log_min_duration = 0;
SET auto_explain.log_nested_statements = on;
SET auto_explain.log_analyze = on;
BEGIN;
DELETE FROM parent_e WHERE id = 1;
ROLLBACK;
SQL
    after=$(docker logs "$CONTAINER" 2>&1 | wc -l)
    docker logs "$CONTAINER" 2>&1 | tail -n +"$((before + 1))" | head -n "$((after - before))" \
        | awk '
            capture && !/^\t/ { exit }
            /Query Text:.*child_e/ { capture = 1 }
            capture { print }
          '
}

run_psql <<'SQL' > /dev/null
DROP TABLE IF EXISTS child_e, parent_e CASCADE;
CREATE TABLE parent_e(id int PRIMARY KEY);
CREATE TABLE child_e(id int, parent_id int REFERENCES parent_e(id));
INSERT INTO parent_e SELECT g FROM generate_series(1, 10000) g;
-- children only for ids 2..N: deleting id=1 has no dependents, so the FK
-- (not deferrable by default) doesn't reject the DELETE outright.
INSERT INTO child_e SELECT g, g FROM generate_series(2, 10000) g;
ANALYZE parent_e;
ANALYZE child_e;
SQL

# Case 1: no index on child_e.parent_id -> RI check does a Seq Scan
PLAN=$(capture_child_e_plan)
if echo "$PLAN" | grep -q 'Seq Scan'; then
    record true "${LABEL_PREFIX}: real DELETE enforcement path uses Seq Scan without index"
else
    record false "${LABEL_PREFIX}: real DELETE enforcement path uses Seq Scan without index"
fi

# Case 2: with a covering index -> RI check uses an Index Scan
run_psql -q <<'SQL' > /dev/null
CREATE INDEX idx_child_e_parent ON child_e(parent_id);
ANALYZE child_e;
SQL

PLAN=$(capture_child_e_plan)
if echo "$PLAN" | grep -q 'Index'; then
    record true "${LABEL_PREFIX}: real DELETE enforcement path uses Index Scan with covering index"
else
    record false "${LABEL_PREFIX}: real DELETE enforcement path uses Index Scan with covering index"
fi

run_psql -q <<'SQL' > /dev/null
DROP TABLE child_e;
DROP TABLE parent_e;
SQL
