#!/usr/bin/env bash
# gen-reserved-keywords.sh: Generate src/rules/reserved_keywords.rs from pg_get_keywords().
#
# Queries a real PostgreSQL instance (PG 18 via docker compose) for all
# reserved keywords (catcode 'R' = reserved, 'T' = reserved, can be function
# or type), and generates a Rust module with a const array and a
# contains-based lookup.
#
# Usage:
#   ./verify/gen-reserved-keywords.sh           # Generate src/rules/reserved_keywords.rs
#   ./verify/gen-reserved-keywords.sh --check   # Check that committed file is up-to-date

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OUTPUT_FILE="$PROJECT_ROOT/src/rules/reserved_keywords.rs"
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

# Query pg_get_keywords() for reserved keywords.
# catcode 'R' = reserved, 'T' = reserved (can be function or type).
# These are keywords that cannot be used as table/column names without quoting.
SQL="
SELECT word
FROM pg_get_keywords()
WHERE catcode IN ('R', 'T')
ORDER BY word;
"

echo "Querying pg_get_keywords()..." >&2
RAW=$(docker exec -i "$CONTAINER" psql -U postgres -d postgres -t -A -c "$SQL")

# Collect into array
KEYWORDS=()
while IFS= read -r word; do
    [ -z "$word" ] && continue
    KEYWORDS+=("$word")
done <<< "$RAW"

echo "Found ${#KEYWORDS[@]} reserved keywords." >&2

# Get PG version for the header comment
PG_VERSION=$(docker exec "$CONTAINER" psql -U postgres -t -A -c "SELECT version();" | head -1)
GEN_DATE=$(date -u +%Y-%m-%d)

# Generate Rust file
generate() {
    cat <<'HEADER'
//! Auto-generated PostgreSQL reserved keywords from `pg_get_keywords()`.
//!
//! DO NOT EDIT BY HAND. Regenerate with:
//!   ./verify/gen-reserved-keywords.sh
//!
HEADER
    echo "//! Source: $PG_VERSION"
    echo "//! Generated: $GEN_DATE"
    cat <<'BODY'

/// Sorted array of PostgreSQL reserved keywords (catcode 'R' and 'T').
///
/// These keywords cannot be used as identifiers (table names, column names)
/// without double-quoting.
BODY

    echo "const RESERVED: &[&str] = &["
    for kw in "${KEYWORDS[@]}"; do
        echo "    \"$kw\","
    done
    echo "];"
    echo ""

    cat <<'FOOTER'
/// Check whether a name is a PostgreSQL reserved keyword.
///
/// The caller must pass a **lowercase** name — this function does no case
/// conversion. The `RESERVED` array contains only lowercase entries.
pub(crate) fn is_reserved(name: &str) -> bool {
    RESERVED.contains(&name)
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
    TMPFILE=$(mktemp /tmp/reserved_keywords.XXXXXX.rs)
    echo "$GENERATED" > "$TMPFILE"
    cargo fmt -- "$TMPFILE" 2>/dev/null
    if diff "$TMPFILE" "$OUTPUT_FILE" > /dev/null 2>&1; then
        rm -f "$TMPFILE"
        echo "OK: reserved_keywords.rs is up-to-date." >&2
        exit 0
    else
        echo "Error: reserved_keywords.rs is out of date. Regenerate with:" >&2
        echo "  ./verify/gen-reserved-keywords.sh" >&2
        diff "$TMPFILE" "$OUTPUT_FILE" >&2 || true
        rm -f "$TMPFILE"
        exit 1
    fi
else
    echo "$GENERATED" > "$OUTPUT_FILE"
    cargo fmt -- "$OUTPUT_FILE" 2>/dev/null
    echo "Wrote $OUTPUT_FILE" >&2
    echo "  Reserved keywords: ${#KEYWORDS[@]}" >&2
fi
