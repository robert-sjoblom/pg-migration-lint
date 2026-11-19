#!/usr/bin/env bash
#
# test-download-binary.sh — manual test for scripts/download-binary.sh.
#
# There's no shell test framework in this repo, so this is a small
# self-contained bash script: it shadows `gh` on PATH with gh-stub.sh (no
# network access, no real GitHub API calls), runs download-binary.sh across
# fifteen scenarios (tag resolution x2, checksum verification x2 more, runner
# platform x5 more, major-version refs x6 more), and asserts:
#   1. the script resolves the right tag in each case: an exact release tag
#      as is, a branch through the latest release, and a major-version ref
#      such as `v2` to the newest published v2.x.y release that ships both
#      the binary and its .sha256, failing clearly when there is none or the
#      listing fails;
#   2. it invokes `gh` with the right --repo / --pattern;
#   3. it extracts the tarball and chmods the binary executable, unless
#      checksum verification failed, in which case it must not.
#   4. checksum verification behaves correctly: a matching .sha256 lets the
#      install proceed, a mismatching one hard-fails before extraction, and
#      a missing .sha256 (old release) warns and falls back to installing
#      unverified.
#   5. a runner other than Linux x86_64, or a missing RUNNER_OS /
#      RUNNER_ARCH, fails with a clear error before any `gh` call.
#
# Needs `jq`: the gh stub runs the script's own `--jq` filter with it.
#
# Run manually: bash scripts/tests/test-download-binary.sh
# Not wired into CI.

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"
download_binary="$repo_root/scripts/download-binary.sh"
gh_stub="$script_dir/gh-stub.sh"

if ! command -v jq >/dev/null; then
  echo "test-download-binary.sh needs jq: the gh stub runs download-binary.sh's --jq filter with it. Install jq and run it again." >&2
  exit 1
fi

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

# One entry of a `gh api repos/<repo>/releases` response. The kind is
# `published` (the default: a full release with the binary and its
# checksum), `draft`, `prerelease`, `no-binary` (published, but no assets
# uploaded yet) or `no-checksum` (the binary is uploaded, its .sha256 not
# yet, or never, as on releases before v2.16.0).
release_entry() {
  local tag="$1" kind="${2:-published}"
  local draft=false prerelease=false
  local assets='[{"name": "liquibase-bridge.jar"}, {"name": "pg-migration-lint-x86_64-linux.tar.gz"}, {"name": "pg-migration-lint-x86_64-linux.tar.gz.sha256"}]'
  case "$kind" in
    published) ;;
    draft) draft=true ;;
    prerelease) prerelease=true ;;
    no-binary) assets='[]' ;;
    no-checksum) assets='[{"name": "liquibase-bridge.jar"}, {"name": "pg-migration-lint-x86_64-linux.tar.gz"}]' ;;
    *) echo "release_entry: unknown kind: $kind" >&2; exit 99 ;;
  esac
  printf '{"tag_name": "%s", "draft": %s, "prerelease": %s, "assets": %s}' \
    "$tag" "$draft" "$prerelease" "$assets"
}

# Prints the JSON array of the given release_entry outputs, in the order
# given (the API's order).
releases_response() {
  (IFS=,; printf '[%s]\n' "$*")
}

# Runs download-binary.sh once, in a fresh sandbox, with the given
# ACTION_REF, GH_STUB_SHA256_MODE and, optionally, RUNNER_OS / RUNNER_ARCH
# (default Linux / X64; an empty value leaves that variable unset) and a
# GH_STUB_RELEASES_RESPONSE for `gh api` to answer with. Leaves
# behind (as globals for the caller to inspect):
#   SANDBOX        sandbox root for this run
#   GH_LOG         path to this run's gh-invocation log
#   INSTALL_DIR    where download-binary.sh should have installed the binary
#   GITHUB_PATH_FILE  path standing in for the runner's $GITHUB_PATH
#   RUN_EXIT       exit code of download-binary.sh
run_scenario() {
  local ref="$1" sha256_mode="$2" runner_os="${3-Linux}" runner_arch="${4-X64}" releases_response="${5-}"

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
    env -u RUNNER_OS -u RUNNER_ARCH -u GH_STUB_RELEASES_RESPONSE \
    ${runner_os:+"RUNNER_OS=$runner_os"} \
    ${runner_arch:+"RUNNER_ARCH=$runner_arch"} \
    ${releases_response:+"GH_STUB_RELEASES_RESPONSE=$releases_response"} \
    "$download_binary" > "$SANDBOX/stdout.log" 2>"$SANDBOX/stderr.log"
  RUN_EXIT=$?
  set -e
}

assert_stopped_before_any_gh_call() {
  if [[ "$RUN_EXIT" -ne 0 ]]; then
    pass "script exits non-zero"
  else
    fail "script exits non-zero (got $RUN_EXIT)"
  fi

  if [[ -s "$GH_LOG" ]]; then
    fail "makes no gh call"
    sed 's/^/    gh log: /' "$GH_LOG"
  else
    pass "makes no gh call"
  fi

  if [[ -e "$INSTALL_DIR/pg-migration-lint" ]]; then
    fail "does not install the binary (found $INSTALL_DIR/pg-migration-lint)"
  else
    pass "does not install the binary"
  fi

  assert_not_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
    "does not add the install directory to \$GITHUB_PATH"
}

assert_resolved_major_ref_to() {
  local expected_tag="$1"

  if [[ "$RUN_EXIT" -eq 0 ]]; then
    pass "script exits 0"
  else
    fail "script exits 0 (got $RUN_EXIT)"
    sed 's/^/    stderr: /' "$SANDBOX/stderr.log"
  fi

  assert_contains "$GH_LOG" "gh api repos/test-owner/test-repo/releases?per_page=100 --paginate --jq" \
    "lists every page of the repo's releases"
  assert_not_contains "$GH_LOG" "--json tagName" \
    "does not fall back to the latest release"
  assert_contains "$GH_LOG" "gh release download $expected_tag --repo test-owner/test-repo --pattern pg-migration-lint-x86_64-linux.tar.gz --dir $INSTALL_DIR --clobber" \
    "downloads $expected_tag"

  if [[ -x "$INSTALL_DIR/pg-migration-lint" ]]; then
    pass "extracted binary exists and is executable"
  else
    fail "extracted binary exists and is executable (not found or not +x at $INSTALL_DIR/pg-migration-lint)"
  fi

  assert_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
    "adds the install directory to \$GITHUB_PATH"
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
assert_not_contains "$GH_LOG" "gh api" \
  "does not list releases when the pinned ref is an exact release tag"
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
assert_not_contains "$GH_LOG" "gh api" \
  "does not list releases when the pinned ref is a branch"
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
echo "Scenario C: Linux x86_64 runner (ubuntu-latest); .sha256 asset present and matches (normal case going forward)"
run_scenario "v2.15.0" "match" "Linux" "X64"

if [[ "$RUN_EXIT" -eq 0 ]]; then
  pass "script exits 0"
else
  fail "script exits 0 (got $RUN_EXIT)"
  sed 's/^/    stderr: /' "$SANDBOX/stderr.log"
fi

assert_not_contains "$SANDBOX/stderr.log" "supports only Linux x86_64" \
  "does not reject the runner"
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
echo "Scenario E: macOS runner"
run_scenario "v2.15.0" "match" "macOS" "X64"

assert_contains "$SANDBOX/stderr.log" "supports only Linux x86_64 runners (got RUNNER_OS=macOS, RUNNER_ARCH=X64)" \
  "names the supported platform and the runner's own"
assert_stopped_before_any_gh_call

echo
echo "Scenario F: Windows runner"
run_scenario "v2.15.0" "match" "Windows" "X64"

assert_contains "$SANDBOX/stderr.log" "supports only Linux x86_64 runners (got RUNNER_OS=Windows, RUNNER_ARCH=X64)" \
  "names the supported platform and the runner's own"
assert_stopped_before_any_gh_call

echo
echo "Scenario G: Linux ARM64 runner"
run_scenario "v2.15.0" "match" "Linux" "ARM64"

assert_contains "$SANDBOX/stderr.log" "supports only Linux x86_64 runners (got RUNNER_OS=Linux, RUNNER_ARCH=ARM64)" \
  "names the supported platform and the runner's own"
assert_stopped_before_any_gh_call

echo
echo "Scenario H: RUNNER_OS and RUNNER_ARCH unset (run outside a runner)"
run_scenario "v2.15.0" "match" "" ""

assert_contains "$SANDBOX/stderr.log" "RUNNER_OS is required" \
  "says RUNNER_OS must be set"
assert_stopped_before_any_gh_call

echo
echo "Scenario I: RUNNER_OS is Linux but RUNNER_ARCH is unset"
run_scenario "v2.15.0" "match" "Linux" ""

assert_contains "$SANDBOX/stderr.log" "RUNNER_ARCH is required" \
  "says RUNNER_ARCH must be set"
assert_stopped_before_any_gh_call

echo
echo "Scenario J: ACTION_REF is a major-version tag (v2); resolves to the newest v2.x.y by version order, not text order"
run_scenario "v2" "match" "Linux" "X64" "$(releases_response \
  "$(release_entry v2.10.0)" \
  "$(release_entry v2.16.2)" \
  "$(release_entry v2.9.0)")"

assert_resolved_major_ref_to "v2.16.2"

echo
echo "Scenario K: ACTION_REF is v2 and newer v3.x / v20.x releases exist"
run_scenario "v2" "match" "Linux" "X64" "$(releases_response \
  "$(release_entry v20.0.0)" \
  "$(release_entry v3.0.0)" \
  "$(release_entry v2.16.2)" \
  "$(release_entry v2.15.0)")"

assert_resolved_major_ref_to "v2.16.2"
assert_not_contains "$GH_LOG" "gh release download v3.0.0" \
  "does not download a newer major (v3.0.0)"
assert_not_contains "$GH_LOG" "gh release download v20.0.0" \
  "does not download v20.0.0, whose tag merely starts with v2"

echo
echo "Scenario L: ACTION_REF is v2; newer v2.x releases are a draft, a prerelease, or have no binary uploaded yet"
run_scenario "v2" "match" "Linux" "X64" "$(releases_response \
  "$(release_entry v2.19.0 draft)" \
  "$(release_entry v2.18.0 prerelease)" \
  "$(release_entry v2.17.0 no-binary)" \
  "$(release_entry v2.16.2)")"

assert_resolved_major_ref_to "v2.16.2"

echo
echo "Scenario M: ACTION_REF is v4 and no v4.x.y release exists"
run_scenario "v4" "match" "Linux" "X64" "$(releases_response \
  "$(release_entry v4.0.0 draft)" \
  "$(release_entry v3.0.0)" \
  "$(release_entry v2.16.2)")"

if [[ "$RUN_EXIT" -ne 0 ]]; then
  pass "script exits non-zero"
else
  fail "script exits non-zero (got $RUN_EXIT)"
fi

assert_contains "$SANDBOX/stderr.log" "test-owner/test-repo has no published v4.x.y release" \
  "says no release matches the major version"
assert_not_contains "$GH_LOG" "--json tagName" \
  "does not fall back to the latest release"
assert_not_contains "$GH_LOG" "gh release download" \
  "downloads nothing"

if [[ -e "$INSTALL_DIR/pg-migration-lint" ]]; then
  fail "does not install the binary (found $INSTALL_DIR/pg-migration-lint)"
else
  pass "does not install the binary"
fi

assert_not_contains "$GITHUB_PATH_FILE" "$INSTALL_DIR" \
  "does not add the install directory to \$GITHUB_PATH"

echo
echo "Scenario N: ACTION_REF is v2 and listing the releases fails (API error)"
run_scenario "v2" "match" "Linux" "X64" "HTTP 502: Bad Gateway"

if [[ "$RUN_EXIT" -ne 0 ]]; then
  pass "script exits non-zero"
else
  fail "script exits non-zero (got $RUN_EXIT)"
fi

assert_contains "$SANDBOX/stderr.log" "could not list the releases of test-owner/test-repo to resolve v2" \
  "says the release listing failed"
assert_not_contains "$GH_LOG" "--json tagName" \
  "does not fall back to the latest release"
assert_not_contains "$GH_LOG" "gh release download" \
  "downloads nothing"

echo
echo "Scenario O: ACTION_REF is v2; the newest v2.x release has its binary uploaded but not its .sha256 yet"
run_scenario "v2" "match" "Linux" "X64" "$(releases_response \
  "$(release_entry v2.17.0 no-checksum)" \
  "$(release_entry v2.16.2)")"

assert_resolved_major_ref_to "v2.16.2"
assert_not_contains "$GH_LOG" "gh release download v2.17.0" \
  "does not download the release whose checksum is not uploaded yet"

echo
if [[ "$failures" -eq 0 ]]; then
  echo "All checks passed."
  exit 0
else
  echo "$failures check(s) failed."
  exit 1
fi
