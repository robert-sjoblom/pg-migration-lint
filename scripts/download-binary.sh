#!/usr/bin/env bash
#
# download-binary.sh — download the pg-migration-lint release binary that
# matches this composite action's own pinned ref, extract it, and add its
# directory to PATH so later composite-action steps can call
# `pg-migration-lint` directly.
#
# Required environment:
#   GITHUB_ACTION_REPOSITORY  owner/repo hosting *this action* (NOT the
#                             consumer repo that invokes it). Must be
#                             forwarded explicitly by the calling step's
#                             `env:` block from `${{ github.action_repository }}`.
#                             Reading `${{ github.action_repository }}` (or
#                             the ambient $GITHUB_ACTION_REPOSITORY) directly
#                             inside a composite action's `run:` shell
#                             resolves to an empty string — this is a known,
#                             confirmed-not-fixed runner bug
#                             (actions/runner#2525, closed as "not planned").
#                             The expression only evaluates correctly when
#                             assigned in `env:`, so action.yml captures it
#                             there and this script just reads the env var.
#   GITHUB_ACTION_REF         Same caveat as above, for
#                             `${{ github.action_ref }}`. This is the ref the
#                             consumer pinned the action to (a released tag
#                             like `v1.2.3`, or a branch like `main`).
#   GH_TOKEN                  Token for `gh` CLI API auth (forwarded from
#                             this action's `github-token` input).
#   GITHUB_PATH               Runner-provided file; appended to so the
#                             extracted binary's directory ends up on PATH.
#
# Optional environment:
#   RUNNER_TEMP               Preferred scratch directory. Falls back to
#                             `mktemp -d` when unset (e.g. under manual
#                             testing outside a real runner).
#
# Checksum verification: NONE. .github/workflows/release-please.yml does not
# publish a .sha256 (or other digest) asset alongside
# pg-migration-lint-x86_64-linux.tar.gz, so this script has no way to verify
# the downloaded binary's integrity beyond `gh`'s own authenticated HTTPS
# transport. This is a known, accepted gap — not silently treated as
# "verified".

set -euo pipefail

readonly ASSET_PATTERN='pg-migration-lint-x86_64-linux.tar.gz'
readonly BINARY_NAME='pg-migration-lint'

: "${GITHUB_ACTION_REPOSITORY:?GITHUB_ACTION_REPOSITORY is required — see header comment}"
: "${GITHUB_ACTION_REF:?GITHUB_ACTION_REF is required — see header comment}"
: "${GITHUB_PATH:?GITHUB_PATH is required (normally set by the GitHub Actions runner)}"

# Resolve the release tag to download. Prefer the ref the action itself was
# pinned to (e.g. a version tag like `v1.2.3`); if that ref isn't itself a
# release tag — e.g. someone pinned `@main` — fall back to the repo's latest
# release.
resolve_tag() {
  local repo="$1" ref="$2"

  if gh release view "$ref" --repo "$repo" >/dev/null 2>&1; then
    printf '%s\n' "$ref"
    return 0
  fi

  gh release view --repo "$repo" --json tagName -q .tagName
}

tag="$(resolve_tag "$GITHUB_ACTION_REPOSITORY" "$GITHUB_ACTION_REF")"

install_dir="${RUNNER_TEMP:-$(mktemp -d)}/pg-migration-lint-bin"
mkdir -p "$install_dir"

gh release download "$tag" \
  --repo "$GITHUB_ACTION_REPOSITORY" \
  --pattern "$ASSET_PATTERN" \
  --dir "$install_dir" \
  --clobber

tar -xzf "$install_dir/$ASSET_PATTERN" -C "$install_dir"
chmod +x "$install_dir/$BINARY_NAME"

echo "$install_dir" >> "$GITHUB_PATH"

echo "Installed ${BINARY_NAME} (${tag}) to ${install_dir}"
