#!/usr/bin/env bash
# =============================================================================
# test-docker.sh -- Build and run the full pg-migration-lint test suite in
# Docker, including the Liquibase bridge JAR.
#
# Usage:
#   ./scripts/test-docker.sh              # Build and run tests
#   ./scripts/test-docker.sh --no-cache   # Force a clean rebuild
#   ./scripts/test-docker.sh --build-only # Build the image but don't run tests
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

IMAGE_NAME="pg-migration-lint-test"
DOCKERFILE="Dockerfile.test"

# Parse arguments
BUILD_ARGS=()
RUN_TESTS=true

for arg in "$@"; do
    case "$arg" in
        --no-cache)
            BUILD_ARGS+=("--no-cache")
            ;;
        --build-only)
            RUN_TESTS=false
            ;;
        --help|-h)
            echo "Usage: $0 [--no-cache] [--build-only] [--help]"
            echo ""
            echo "Options:"
            echo "  --no-cache    Force a clean Docker build (no layer caching)"
            echo "  --build-only  Build the image but skip running the test suite"
            echo "  --help        Show this help message"
            exit 0
            ;;
        *)
            echo "Unknown argument: $arg"
            echo "Run $0 --help for usage"
            exit 1
            ;;
    esac
done

cd "$PROJECT_ROOT"

echo "==> Building test image: $IMAGE_NAME"
echo "    Dockerfile: $DOCKERFILE"
echo "    Context:    $PROJECT_ROOT"
echo ""

docker build \
    -f "$DOCKERFILE" \
    -t "$IMAGE_NAME" \
    "${BUILD_ARGS[@]+"${BUILD_ARGS[@]}"}" \
    .

echo ""
echo "==> Build succeeded."

if [ "$RUN_TESTS" = true ]; then
    echo ""
    echo "==> Running tests..."
    echo ""
    docker run --rm "$IMAGE_NAME"
    echo ""
    echo "==> All tests passed."
else
    echo ""
    echo "==> Skipping test run (--build-only)."
fi
