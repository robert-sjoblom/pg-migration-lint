#!/usr/bin/env bash
#
# Build the Liquibase bridge JAR using Docker.
# No local Java or Maven installation required.
#
# Usage:
#   ./bridge/build.sh                  # Build and extract JAR to tools/
#   ./bridge/build.sh --image-only     # Only build the Docker image
#
# The resulting JAR is placed at tools/liquibase-bridge.jar
# which matches the default config path in pg-migration-lint.toml.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
IMAGE_NAME="pg-migration-lint-bridge"
CONTAINER_NAME="bridge-extract-$$"
OUTPUT_DIR="$PROJECT_ROOT/tools"
JAR_NAME="liquibase-bridge.jar"

echo "Building Liquibase bridge Docker image..."
docker build -t "$IMAGE_NAME" "$SCRIPT_DIR"

if [[ "${1:-}" == "--image-only" ]]; then
    echo "Docker image built: $IMAGE_NAME"
    exit 0
fi

echo "Extracting JAR from container..."
mkdir -p "$OUTPUT_DIR"

# Create a temporary container, copy the JAR out, then clean up.
docker create --name "$CONTAINER_NAME" "$IMAGE_NAME" >/dev/null 2>&1
docker cp "$CONTAINER_NAME:/app/$JAR_NAME" "$OUTPUT_DIR/$JAR_NAME"
docker rm "$CONTAINER_NAME" >/dev/null 2>&1

echo "JAR extracted to: $OUTPUT_DIR/$JAR_NAME"
echo ""
echo "To use with pg-migration-lint, set in pg-migration-lint.toml:"
echo "  [liquibase]"
echo "  bridge_jar_path = \"tools/$JAR_NAME\""
