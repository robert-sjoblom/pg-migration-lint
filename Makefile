# =============================================================================
# Makefile -- pg-migration-lint build and test targets
# =============================================================================

IMAGE_NAME    := pg-migration-lint-test
CONTAINER_NAME := pgml-bridge-gen

.PHONY: bridge-generate bridge-verify bridge-clean test

# ---------------------------------------------------------------------------
# bridge-generate: Build everything in Docker, then extract golden files
#                  and insta snapshots back to the host for committing.
#
# Use this when:
#   - First-time setup of bridge golden files / snapshots
#   - After changing XML fixtures or bridge Java code
#   - After changing lint rules that affect bridge test output
# ---------------------------------------------------------------------------
bridge-generate:
	docker build -f Dockerfile.test -t $(IMAGE_NAME) .
	@# Remove any leftover container from a previous run
	@docker rm $(CONTAINER_NAME) 2>/dev/null || true
	@# Run with INSTA_UPDATE=always to generate/update snapshots
	docker run --name $(CONTAINER_NAME) $(IMAGE_NAME) \
		sh -c 'INSTA_UPDATE=always cargo test --features bridge-tests -- --test-threads=4 bridge'
	@# Extract Java golden file
	docker cp $(CONTAINER_NAME):/app/bridge/src/test/resources/fixtures/full-changelog.expected.json \
		bridge/src/test/resources/fixtures/full-changelog.expected.json
	@# Extract insta snapshots (bridge tests only)
	@mkdir -p tests/snapshots
	docker cp $(CONTAINER_NAME):/app/tests/snapshots/. tests/snapshots/
	docker rm $(CONTAINER_NAME)
	@echo ""
	@echo "==> Golden files and snapshots extracted. Review and commit."

# ---------------------------------------------------------------------------
# bridge-verify: Build and run all tests in Docker (CI mode).
#                Assumes golden files / snapshots are already committed.
# ---------------------------------------------------------------------------
bridge-verify:
	docker build -f Dockerfile.test -t $(IMAGE_NAME) .
	docker run --rm $(IMAGE_NAME)

# ---------------------------------------------------------------------------
# bridge-clean: Remove Docker artifacts.
# ---------------------------------------------------------------------------
bridge-clean:
	@docker rm $(CONTAINER_NAME) 2>/dev/null || true
	@docker rmi $(IMAGE_NAME) 2>/dev/null || true
	@echo "==> Docker artifacts cleaned."

# ---------------------------------------------------------------------------
# test: Run standard (non-bridge) tests locally.
# ---------------------------------------------------------------------------
test:
	cargo test -- --test-threads=4
	cargo clippy -- -D warnings
