#!/usr/bin/env bash
# run.sh: PostgreSQL behavior verification test runner.
#
# Runs SQL test files against real PostgreSQL instances (14–18) via Docker Compose
# to verify the factual claims made in pg-migration-lint rule explanations.
#
# Usage:
#   ./verify/run.sh                         # Run all tests against all PG versions
#   ./verify/run.sh --pg 17                 # Run against PG 17 only
#   ./verify/run.sh --test locks/pgm001*    # Run only matching tests
#   ./verify/run.sh --verbose               # Show individual assertion results
#   ./verify/run.sh --no-teardown           # Don't stop containers after run

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

# Defaults
PG_FILTER=""
TEST_FILTER=""
VERBOSE=""
TEARDOWN=true

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --pg)
            PG_FILTER="$2"
            shift 2
            ;;
        --test)
            TEST_FILTER="$2"
            shift 2
            ;;
        --verbose)
            VERBOSE="1"
            shift
            ;;
        --no-teardown)
            TEARDOWN=false
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--pg VERSION] [--test PATTERN] [--verbose] [--no-teardown]"
            exit 2
            ;;
    esac
done

# PG versions and their ports
declare -A PG_PORTS=(
    [14]=54314
    [15]=54315
    [16]=54316
    [17]=54317
    [18]=54318
)

# Filter PG versions if requested
if [ -n "$PG_FILTER" ]; then
    if [ -z "${PG_PORTS[$PG_FILTER]+x}" ]; then
        echo "Error: Unknown PG version $PG_FILTER (available: ${!PG_PORTS[*]})"
        exit 2
    fi
    VERSIONS=("$PG_FILTER")
else
    VERSIONS=(14 15 16 17 18)
fi

# Collect test files
if [ -n "$TEST_FILTER" ]; then
    # Allow glob patterns relative to tests/
    mapfile -t TEST_FILES < <(find tests/ -name '*.sql' -path "*${TEST_FILTER}*" | sort)
else
    mapfile -t TEST_FILES < <(find tests/ -name '*.sql' | sort)
fi

if [ ${#TEST_FILES[@]} -eq 0 ]; then
    echo "No test files found matching: $TEST_FILTER"
    exit 2
fi

echo "=== PostgreSQL Behavior Verification ==="
echo "PG versions: ${VERSIONS[*]}"
echo "Test files:  ${#TEST_FILES[@]}"
echo ""

# Start containers
echo "Starting PostgreSQL containers..."
docker compose up -d "${VERSIONS[@]/#/pg}" 2>/dev/null

# Wait for all containers to be healthy (uses docker exec, no local psql needed)
echo -n "Waiting for health checks"
for ver in "${VERSIONS[@]}"; do
    container=$(docker compose ps -q "pg${ver}" 2>/dev/null)
    if [ -z "$container" ]; then
        echo ""
        echo "Error: container pg${ver} not found"
        exit 1
    fi
    for i in $(seq 1 30); do
        if docker exec "$container" pg_isready -U postgres > /dev/null 2>&1; then
            break
        fi
        if [ "$i" -eq 30 ]; then
            echo ""
            echo "Error: PG $ver not ready after 30 seconds"
            exit 1
        fi
        echo -n "."
        sleep 1
    done
done
echo " ready."
echo ""

# Result tracking
declare -A RESULTS  # key: "test|version" value: "PASS" | "FAIL" | "SKIP" | "ERROR"
TOTAL_PASS=0
TOTAL_FAIL=0
TOTAL_SKIP=0

# Run tests
for test_file in "${TEST_FILES[@]}"; do
    test_name=$(echo "$test_file" | sed 's|^tests/||')

    for ver in "${VERSIONS[@]}"; do
        port="${PG_PORTS[$ver]}"
        key="${test_name}|${ver}"

        if [ -n "$VERBOSE" ]; then
            echo "--- PG${ver}: ${test_name} ---"
        fi

        output=$(bash lib/runner.sh "$ver" "$port" "$test_file" "$VERBOSE" 2>&1) || true

        if echo "$output" | grep -q "^SKIP$"; then
            RESULTS[$key]="SKIP"
            TOTAL_SKIP=$((TOTAL_SKIP + 1))
        elif echo "$output" | grep -q "^PASS:"; then
            count=$(echo "$output" | grep "^PASS:" | cut -d: -f2)
            RESULTS[$key]="PASS($count)"
            TOTAL_PASS=$((TOTAL_PASS + 1))
        elif echo "$output" | grep -q "^FAIL:"; then
            fails=$(echo "$output" | grep "^FAIL:" | cut -d: -f2)
            passes=$(echo "$output" | grep "^FAIL:" | cut -d: -f3)
            RESULTS[$key]="FAIL($fails/$((fails + passes)))"
            TOTAL_FAIL=$((TOTAL_FAIL + 1))
            # Print failure details
            echo "$output" | grep "^  FAIL:" | while read -r line; do
                echo "  PG${ver} ${test_name}: ${line}"
            done
        elif echo "$output" | grep -q "^NO_RESULTS$"; then
            RESULTS[$key]="EMPTY"
            TOTAL_FAIL=$((TOTAL_FAIL + 1))
            echo "  PG${ver} ${test_name}: no assertions ran"
        else
            RESULTS[$key]="ERROR"
            TOTAL_FAIL=$((TOTAL_FAIL + 1))
            echo "  PG${ver} ${test_name}: unexpected output:"
            echo "$output" | head -5 | sed 's/^/    /'
        fi
    done
done

# Print matrix summary
echo ""
echo "=== Results Matrix ==="
echo ""

# Header
printf "%-50s" "Test"
for ver in "${VERSIONS[@]}"; do
    printf "  PG%-4s" "$ver"
done
echo ""

printf "%-50s" "----"
for ver in "${VERSIONS[@]}"; do
    printf "  ------"
done
echo ""

for test_file in "${TEST_FILES[@]}"; do
    test_name=$(echo "$test_file" | sed 's|^tests/||')
    printf "%-50s" "$test_name"

    for ver in "${VERSIONS[@]}"; do
        key="${test_name}|${ver}"
        result="${RESULTS[$key]:-???}"

        case "$result" in
            PASS*)  printf "  \033[32m%-6s\033[0m" "PASS" ;;
            FAIL*)  printf "  \033[31m%-6s\033[0m" "FAIL" ;;
            SKIP)   printf "  \033[33m%-6s\033[0m" "--"   ;;
            EMPTY)  printf "  \033[33m%-6s\033[0m" "EMPTY" ;;
            ERROR)  printf "  \033[31m%-6s\033[0m" "ERR"  ;;
            *)      printf "  %-6s" "???"   ;;
        esac
    done
    echo ""
done

echo ""
echo "=== Summary ==="
echo "Passed: $TOTAL_PASS  Failed: $TOTAL_FAIL  Skipped: $TOTAL_SKIP"
echo ""

# Teardown
if [ "$TEARDOWN" = true ]; then
    echo "Stopping containers..."
    docker compose down > /dev/null 2>&1
fi

# Exit code
if [ "$TOTAL_FAIL" -gt 0 ]; then
    exit 1
else
    exit 0
fi
