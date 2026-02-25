#!/usr/bin/env bash
# runner.sh: Execute a single test file against a single PG container.
#
# Usage: runner.sh <pg_version> <port> <test_file> [verbose]
#
# Uses docker exec to run psql inside the container (no local psql needed).

set -euo pipefail

PG_VERSION="$1"
PORT="$2"
TEST_FILE="$3"
VERBOSE="${4:-}"

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
VERIFY_DIR="$(dirname "$SCRIPT_DIR")"
FRAMEWORK_SQL="$SCRIPT_DIR/framework.sql"
CONTAINER="$(cd "$VERIFY_DIR" && docker compose ps -q "pg${PG_VERSION}" 2>/dev/null)"

if [ -z "$CONTAINER" ]; then
    echo "ERROR: container pg${PG_VERSION} not running"
    exit 1
fi

# Helper: run psql inside the container
run_psql() {
    local db="$1"
    shift
    docker exec -i "$CONTAINER" psql -U postgres -d "$db" -v ON_ERROR_STOP=0 "$@"
}

# Extract @min_version from test file header
MIN_VERSION=$(grep -oP '@min_version:\s*\K[0-9]+' "$TEST_FILE" 2>/dev/null || echo "0")

if [ "$PG_VERSION" -lt "$MIN_VERSION" ]; then
    echo "SKIP"
    exit 0
fi

# Compute relative test name (from tests/ directory)
TEST_NAME=$(echo "$TEST_FILE" | sed 's|.*/tests/||')

# Create a fresh database for each test to avoid state leakage
DB_NAME="verify_$(echo "$TEST_NAME" | sed 's|[/.]|_|g')"
# Truncate to 63 chars (PG identifier limit)
DB_NAME="${DB_NAME:0:63}"

run_psql postgres -c "DROP DATABASE IF EXISTS \"$DB_NAME\";" 2>/dev/null || true
run_psql postgres -c "CREATE DATABASE \"$DB_NAME\";" 2>/dev/null

# Load framework (pipe file content into container's psql)
cat "$FRAMEWORK_SQL" | run_psql "$DB_NAME" > /dev/null 2>&1

# Set test file name
run_psql "$DB_NAME" -c "SELECT _set_test_file('$TEST_NAME');" > /dev/null 2>&1

# Run test (pipe file content into container's psql)
if [ -n "$VERBOSE" ]; then
    cat "$TEST_FILE" | run_psql "$DB_NAME" 2>&1
else
    cat "$TEST_FILE" | run_psql "$DB_NAME" > /dev/null 2>&1
fi

# Collect results
RESULTS=$(run_psql "$DB_NAME" -t -A -F '|' -c "SELECT label, passed, detail FROM _verify_results ORDER BY id;")

# Clean up test database
run_psql postgres -c "DROP DATABASE IF EXISTS \"$DB_NAME\";" 2>/dev/null || true

if [ -z "$RESULTS" ]; then
    echo "NO_RESULTS"
    exit 0
fi

# Output results
PASS_COUNT=0
FAIL_COUNT=0

while IFS='|' read -r label passed detail; do
    if [ "$passed" = "t" ]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        if [ -n "$VERBOSE" ]; then
            echo "  PASS: $label"
        fi
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo "  FAIL: $label — $detail"
    fi
done <<< "$RESULTS"

if [ "$FAIL_COUNT" -gt 0 ]; then
    echo "FAIL:${FAIL_COUNT}:${PASS_COUNT}"
    exit 1
else
    echo "PASS:${PASS_COUNT}"
    exit 0
fi
