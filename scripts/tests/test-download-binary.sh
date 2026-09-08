#!/usr/bin/env bash
#
# test-download-binary.sh — manual test for scripts/download-binary.sh.
#
# There's no shell test framework in this repo, so this is a small
# self-contained bash script: it shadows `gh` on PATH with gh-stub.sh (no
# network access, no real GitHub API calls), runs download-binary.sh across
# four scenarios (tag resolution x2, checksum verification x2 more), and
# asserts:
#   1. the script resolves the right tag in each case;
#   2. it invokes `gh` with the right --repo / --pattern;
#   3. it extracts the tarball and chmods the binary executable, unless
#      checksum verification failed, in which case it must not.
#   4. checksum verification behaves correctly: a matching .sha256 lets the
#      install proceed, a mismatching one hard-fails before extraction, and
#      a missing .sha256 (old release) warns and falls back to installing
#      unverified.
#
# Run manually: bash scripts/tests/test-download-binary.sh
# Not wired into CI.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
download_binary="$repo_root/scripts/download-binary.sh"
gh_stub="$script_dir/gh-stub.sh"

failures=0

pass() { printf '  PASS: %s\n' "$1"; }
fail() { printf '  FAIL: %s\n' "$1"; failures=$((failures + 1)); }

assert_contains() {
  local haystack_file="$1" needle="$2" description="$3"
  if grep -qF -- "$needle" "$haystack_file"; then
    pass "$description"
  else
    fail "$description (expected to find: $needle)"
    echo "    --- actual log ---"
    sed 's/^/    /' "$haystack_file"
    echo "    -------------------"
  fi
}

assert_not_contains() {
  local haystack_file="$1" needle="$2" description="$3"
  if grep -qF -- "$needle" "$haystack_file"; then
    fail "$description (did not expect to find: $needle)"
  else
    pass "$description"
  fi
}

# Runs download-binary.sh once, in a fresh sandbox, with the given
# ACTION_REF and GH_STUB_SHA256_MODE. Leaves behind (as globals for
# the caller to inspect):
#   SANDBOX        sandbox root for this run
#   GH_LOG         path to this run's gh-invocation log
#   INSTALL_DIR    where download-binary.sh should have installed the binary
#   GITHUB_PATH_FILE  path standing in for the runner's $GITHUB_PATH
#   RUN_EXIT       exit code of download-binary.sh
run_scenario() {
  local ref="$1" sha256_mode="$2"

  SANDBOX="$(mktemp -d)"
  GH_LOG="$SANDBOX/gh-invocations.log"
  : > "$GH_LOG"
  GITHUB_PATH_FILE="$SANDBOX/github-path"
  : > "$GITHUB_PATH_FILE"
  INSTALL_DIR="$SANDBOX/runner-temp/pg-migration-lint-bin"

  local stub_bin_dir="$SANDBOX/bin"
  mkdir -p "$stub_bin_dir"
  ln -sf "$gh_stub" "$stub_bin_dir/gh"

  set +e
  PATH="$stub_bin_dir:$PATH" \
    GH_STUB_LOG="$GH_LOG" \
    GH_STUB_KNOWN_TAG="v2.15.0" \
    GH_STUB_LATEST_TAG="v9.9.9" \
    GH_STUB_SHA256_MODE="$sha256_mode" \
    ACTION_REPOSITORY="test-owner/test-repo" \
    ACTION_REF="$ref" \
    GH_TOKEN="fake-token-for-test" \
    GITHUB_PATH="$GITHUB_PATH_FILE" \
    RUNNER_TEMP="$SANDBOX/runner-temp" \
    "$download_binary" > "$SANDBOX/stdout.log" 2>"$SANDBOX/stderr.log"
  RUN_EXIT=$?
  set -e
}

echo "Scenario A: ACTION_REF is itself an existing release tag (v2.15.0); no .sha256 asset published (old release)"
run_scenario "v2.15.0" "missing"

if [[ "$RUN_EXIT" -eq 0 ]]; then
  pass "script exits 0"
else
  fail "script exits 0 (got $RUN_EXIT)"
  sed 's/^/    stderr: /' "$SANDBOX/stderr.log"
fi

assert_contains "$GH_LOG" "gh release view v2.15.0 --repo test-owner/test-repo" \
  "resolves the pinned ref directly when it is already a release tag"
assert_not_contains "$GH_LOG" "--json tagName" \
  "does not query for the latest release when the pinned ref already resolved"
assert_contains "$GH_LOG" "gh release download v2.15.0 --repo test-owner/test-repo --pattern pg-migration-lint-x86_64-linux.tar.gz --dir $INSTALL_DIR --clobber" \
  "invokes gh release download with the right tag/repo/pattern"
assert_contains "$GH_LOG" "gh release download v2.15.0 --repo test-owner/test-repo --pattern pg-migration-lint-x86_64-linux.tar.gz.sha256 --dir $INSTALL_DIR --clobber" \
  "invokes gh release download for the .sha256 asset too"
assert_contains "$SANDBOX/stderr.log" "predates checksum publishing" \
  "warns that this release predates checksum publishing"

if [[ -x "$INSTALL_DIR/pg-migration-lint" ]]; then
  pass "extracted binary exists and is executable"
else
  fail "extracted binary exists and is executable (not found or not +x at $INSTALL_DIR/pg-migration-lint)"
fi

assert_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
  "adds the install directory to \$GITHUB_PATH"

echo
echo "Scenario B: ACTION_REF is not a release tag (main), falls back to latest; no .sha256 asset published (old release)"
run_scenario "main" "missing"

if [[ "$RUN_EXIT" -eq 0 ]]; then
  pass "script exits 0"
else
  fail "script exits 0 (got $RUN_EXIT)"
  sed 's/^/    stderr: /' "$SANDBOX/stderr.log"
fi

assert_contains "$GH_LOG" "gh release view main --repo test-owner/test-repo" \
  "attempts to resolve the pinned ref (main) directly first"
assert_contains "$GH_LOG" "gh release view --repo test-owner/test-repo --json tagName -q .tagName" \
  "falls back to querying the latest release when the pinned ref isn't a tag"
assert_contains "$GH_LOG" "gh release download v9.9.9 --repo test-owner/test-repo --pattern pg-migration-lint-x86_64-linux.tar.gz --dir $INSTALL_DIR --clobber" \
  "downloads using the resolved latest tag (v9.9.9), not the literal ref (main)"
assert_contains "$SANDBOX/stderr.log" "predates checksum publishing" \
  "warns that this release predates checksum publishing"

if [[ -x "$INSTALL_DIR/pg-migration-lint" ]]; then
  pass "extracted binary exists and is executable"
else
  fail "extracted binary exists and is executable (not found or not +x at $INSTALL_DIR/pg-migration-lint)"
fi

assert_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
  "adds the install directory to \$GITHUB_PATH"

echo
echo "Scenario C: .sha256 asset present and matches (normal case going forward)"
run_scenario "v2.15.0" "match"

if [[ "$RUN_EXIT" -eq 0 ]]; then
  pass "script exits 0"
else
  fail "script exits 0 (got $RUN_EXIT)"
  sed 's/^/    stderr: /' "$SANDBOX/stderr.log"
fi

assert_contains "$GH_LOG" "gh release download v2.15.0 --repo test-owner/test-repo --pattern pg-migration-lint-x86_64-linux.tar.gz.sha256 --dir $INSTALL_DIR --clobber" \
  "invokes gh release download for the .sha256 asset"
assert_not_contains "$SANDBOX/stderr.log" "predates checksum publishing" \
  "does not warn about a missing checksum when one was verified"

if [[ -x "$INSTALL_DIR/pg-migration-lint" ]]; then
  pass "extracted binary exists and is executable"
else
  fail "extracted binary exists and is executable (not found or not +x at $INSTALL_DIR/pg-migration-lint)"
fi

assert_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
  "adds the install directory to \$GITHUB_PATH"

echo
echo "Scenario D: .sha256 asset present but does not match (tampered/corrupted download)"
run_scenario "v2.15.0" "mismatch"

if [[ "$RUN_EXIT" -ne 0 ]]; then
  pass "script exits non-zero"
else
  fail "script exits non-zero (got $RUN_EXIT)"
fi

assert_contains "$SANDBOX/stderr.log" "checksum verification failed" \
  "prints a clear checksum verification failure error"

if [[ -e "$INSTALL_DIR/pg-migration-lint" ]]; then
  fail "does not extract the binary when checksum verification fails (found $INSTALL_DIR/pg-migration-lint)"
else
  pass "does not extract the binary when checksum verification fails"
fi

assert_not_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
  "does not add the install directory to \$GITHUB_PATH when verification fails"

echo
if [[ "$failures" -eq 0 ]]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) failed."
  exit 1
fi
