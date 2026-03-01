#!/usr/bin/env bash
# gen-fn-volatility.sh: Generate src/rules/fn_volatility.rs from pg_proc.
#
# Queries a real PostgreSQL instance (PG 18 via docker compose) for all
# pg_catalog regular functions, deduplicates overloaded names by taking
# MAX(provolatile) (most volatile wins: 'i' < 's' < 'v'), and generates
# a Rust module with three sorted const arrays and a binary-search lookup.
#
# Usage:
#   ./verify/gen-fn-volatility.sh           # Generate src/rules/fn_volatility.rs
#   ./verify/gen-fn-volatility.sh --check   # Check that committed file is up-to-date

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_FILE="$PROJECT_ROOT/src/rules/fn_volatility.rs"
CHECK_MODE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case "$1" in
        --check)
            CHECK_MODE=true
            shift
            ;;
        *)
            echo "Unknown option: $1"
            echo "Usage: $0 [--check]"
            exit 2
            ;;
    esac
done

cd "$SCRIPT_DIR"

# Start PG 18 container
echo "Starting PG 18 container..." >&2
docker compose up -d pg18 2>/dev/null

# Wait for container to be ready
CONTAINER=$(docker compose ps -q pg18 2>/dev/null)
if [ -z "$CONTAINER" ]; then
    echo "Error: pg18 container not found" >&2
    exit 1
fi

echo -n "Waiting for PG 18..." >&2
for i in $(seq 1 30); do
    if docker exec "$CONTAINER" pg_isready -U postgres > /dev/null 2>&1; then
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "" >&2
        echo "Error: PG 18 not ready after 30 seconds" >&2
        exit 1
    fi
    echo -n "." >&2
    sleep 1
done
echo " ready." >&2

# Query pg_proc for all pg_catalog regular functions.
# LOWER(proname) normalizes mixed-case internal names (e.g., RI_FKey_*) so the
# generated arrays are sorted in lowercase order (matching binary search lookups).
# MAX(provolatile) deduplicates overloads: 'i' < 's' < 'v' lexicographically.
SQL="
SELECT LOWER(proname) AS name, MAX(provolatile) AS vol
FROM pg_proc
WHERE pronamespace = 'pg_catalog'::regnamespace
  AND prokind = 'f'
GROUP BY LOWER(proname)
ORDER BY LOWER(proname);
"

echo "Querying pg_proc..." >&2
RAW=$(docker exec -i "$CONTAINER" psql -U postgres -d postgres -t -A -F '|' -c "$SQL")

# Count results
TOTAL=$(echo "$RAW" | wc -l)
echo "Found $TOTAL unique function names." >&2

# Separate into three lists
VOLATILE_LIST=()
STABLE_LIST=()
IMMUTABLE_LIST=()

while IFS='|' read -r name vol; do
    [ -z "$name" ] && continue
    case "$vol" in
        v) VOLATILE_LIST+=("$name") ;;
        s) STABLE_LIST+=("$name") ;;
        i) IMMUTABLE_LIST+=("$name") ;;
        *) echo "Warning: unknown volatility '$vol' for function '$name'" >&2 ;;
    esac
done <<< "$RAW"

echo "Volatile: ${#VOLATILE_LIST[@]}, Stable: ${#STABLE_LIST[@]}, Immutable: ${#IMMUTABLE_LIST[@]}" >&2

# Get PG version for the header comment
PG_VERSION=$(docker exec "$CONTAINER" psql -U postgres -t -A -c "SELECT version();" | head -1)
GEN_DATE=$(date -u +%Y-%m-%d)

# Generate Rust file
generate() {
    cat <<'HEADER'
//! Auto-generated function volatility classifications from PostgreSQL `pg_proc`.
//!
//! DO NOT EDIT BY HAND. Regenerate with:
//!   ./verify/gen-fn-volatility.sh
//!
HEADER
    echo "//! Source: $PG_VERSION"
    echo "//! Generated: $GEN_DATE"
    cat <<'BODY'

/// PostgreSQL function volatility classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FnVolatility {
    /// Function always returns the same result for the same arguments.
    /// Safe as column default — no table rewrite.
    Immutable,
    /// Function returns the same result within a single statement/transaction.
    /// Safe as column default on PG 11+ — evaluated once at ALTER TABLE time.
    Stable,
    /// Function can return different results on successive calls.
    /// Forces a full table rewrite when used as ADD COLUMN default.
    Volatile,
}

BODY

    # Emit volatile array
    echo "/// Volatile functions from pg_catalog (sorted for binary search)."
    echo "const VOLATILE: &[&str] = &["
    for fn_name in "${VOLATILE_LIST[@]}"; do
        echo "    \"$fn_name\","
    done
    echo "];"
    echo ""

    # Emit stable array
    echo "/// Stable functions from pg_catalog (sorted for binary search)."
    echo "const STABLE: &[&str] = &["
    for fn_name in "${STABLE_LIST[@]}"; do
        echo "    \"$fn_name\","
    done
    echo "];"
    echo ""

    # Emit immutable array
    echo "/// Immutable functions from pg_catalog (sorted for binary search)."
    echo "const IMMUTABLE: &[&str] = &["
    for fn_name in "${IMMUTABLE_LIST[@]}"; do
        echo "    \"$fn_name\","
    done
    echo "];"
    echo ""

    cat <<'FOOTER'
/// Look up the volatility of a PostgreSQL built-in function.
///
/// Returns `None` for unrecognized functions (e.g., user-defined or extension functions).
/// The caller should treat `None` as "unknown — possibly volatile".
///
/// Function names are matched case-insensitively (lowercased before lookup).
pub(crate) fn lookup(name: &str) -> Option<FnVolatility> {
    let lower = name.to_lowercase();
    let key = lower.as_str();

    // Binary search across the three sorted arrays
    if VOLATILE.binary_search(&key).is_ok() {
        return Some(FnVolatility::Volatile);
    }
    if STABLE.binary_search(&key).is_ok() {
        return Some(FnVolatility::Stable);
    }
    if IMMUTABLE.binary_search(&key).is_ok() {
        return Some(FnVolatility::Immutable);
    }

    None
}
FOOTER
}

GENERATED=$(generate)

if [ "$CHECK_MODE" = true ]; then
    if [ ! -f "$OUTPUT_FILE" ]; then
        echo "Error: $OUTPUT_FILE does not exist. Run without --check to generate." >&2
        exit 1
    fi
    # Format the generated output in a temp file so we compare formatted-to-formatted
    TMPFILE=$(mktemp /tmp/fn_volatility.XXXXXX.rs)
    echo "$GENERATED" > "$TMPFILE"
    cargo fmt -- "$TMPFILE" 2>/dev/null
    if diff "$TMPFILE" "$OUTPUT_FILE" > /dev/null 2>&1; then
        rm -f "$TMPFILE"
        echo "OK: fn_volatility.rs is up-to-date." >&2
        exit 0
    else
        echo "Error: fn_volatility.rs is out of date. Regenerate with:" >&2
        echo "  ./verify/gen-fn-volatility.sh" >&2
        diff "$TMPFILE" "$OUTPUT_FILE" >&2 || true
        rm -f "$TMPFILE"
        exit 1
    fi
else
    echo "$GENERATED" > "$OUTPUT_FILE"
    cargo fmt -- "$OUTPUT_FILE" 2>/dev/null
    echo "Wrote $OUTPUT_FILE" >&2
    echo "  Volatile:  ${#VOLATILE_LIST[@]} functions" >&2
    echo "  Stable:    ${#STABLE_LIST[@]} functions" >&2
    echo "  Immutable: ${#IMMUTABLE_LIST[@]} functions" >&2
fi
