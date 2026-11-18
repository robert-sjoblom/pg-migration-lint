#!/usr/bin/env bash
#
# download-binary.sh — download the pg-migration-lint release binary that
# matches this composite action's own pinned ref, extract it, and add its
# directory to PATH so later composite-action steps can call
# `pg-migration-lint` directly.
#
# Required environment:
#   ACTION_REPOSITORY  owner/repo hosting *this action* (NOT the consumer
#                      repo that invokes it). Must be forwarded explicitly
#                      by the calling step's `env:` block from
#                      `${{ github.action_repository }}`.
#
#                      Two gotchas stack up here:
#                        1. Reading `${{ github.action_repository }}`
#                           directly inside a composite action's `run:`
#                           shell resolves to an empty string — a known,
#                           confirmed-not-fixed runner bug
#                           (actions/runner#2525, closed as "not planned").
#                           The expression only evaluates correctly when
#                           assigned in a step's `env:` block, so
#                           action.yml captures it there instead.
#                        2. The captured env var must NOT be named
#                           `GITHUB_ACTION_REPOSITORY` (or anything else
#                           starting with `GITHUB_`/`RUNNER_`): those
#                           prefixes are reserved for the runner's own
#                           variables, and GitHub's docs state that a
#                           step's `env:` assignment to a reserved name is
#                           silently ignored in favor of the runner's own
#                           value for that name --
#                           https://docs.github.com/en/actions/reference/workflows-and-actions/variables#default-environment-variables
#                           Using a reserved name here would silently
#                           reproduce bug #1, just one level removed. Hence
#                           the unprefixed `ACTION_REPOSITORY` name.
#   ACTION_REF         Same two caveats as above, for
#                      `${{ github.action_ref }}`. This is the ref the
#                      consumer pinned the action to (the moving
#                      major-version tag like `v2`, a released tag like
#                      `v1.2.3`, or a branch like `main`).
#   GH_TOKEN           Token for `gh` CLI API auth (forwarded from this
#                      action's `github-token` input).
#   GITHUB_PATH        Runner-provided file; appended to so the extracted
#                      binary's directory ends up on PATH.
#   RUNNER_OS          Runner-provided, like GITHUB_PATH. Releases ship
#   RUNNER_ARCH        only a Linux x86_64 binary, for runners such as
#                      `ubuntu-latest`, so anything but `Linux` / `X64`
#                      exits with an error before any `gh` call. Outside a
#                      runner, set them by hand.
#
# Optional environment:
#   RUNNER_TEMP               Preferred scratch directory. Falls back to
#                             `mktemp -d` when unset (e.g. under manual
#                             testing outside a real runner).
#
# Checksum verification: .github/workflows/release-please.yml publishes a
# `.sha256` asset alongside pg-migration-lint-x86_64-linux.tar.gz, and this
# script downloads and verifies it (`sha256sum -c`) before extracting.
# Releases published before this verification step was added have no
# `.sha256` asset; when that download fails, this script warns loudly and
# falls back to installing unverified, matching this script's original
# behavior. A `.sha256` asset that is present but fails verification is
# treated as fatal — the script exits before extracting anything.
# A major-version ref such as `v2` only resolves to a release that has both
# assets, so that fallback applies only to an exact release tag or to the
# latest release.

set -euo pipefail

readonly ASSET_PATTERN='pg-migration-lint-x86_64-linux.tar.gz'
readonly ASSET_RUNNER_OS='Linux'
readonly ASSET_RUNNER_ARCH='X64'
readonly BINARY_NAME='pg-migration-lint'

: "${ACTION_REPOSITORY:?ACTION_REPOSITORY is required — see header comment}"
: "${ACTION_REF:?ACTION_REF is required — see header comment}"
: "${GITHUB_PATH:?GITHUB_PATH is required (normally set by the GitHub Actions runner)}"
: "${RUNNER_OS:?RUNNER_OS is required (normally set by the GitHub Actions runner)}"
: "${RUNNER_ARCH:?RUNNER_ARCH is required (normally set by the GitHub Actions runner)}"

if [[ "$RUNNER_OS" != "$ASSET_RUNNER_OS" || "$RUNNER_ARCH" != "$ASSET_RUNNER_ARCH" ]]; then
  echo "ERROR: the ${BINARY_NAME} action supports only Linux x86_64 runners (got RUNNER_OS=${RUNNER_OS}, RUNNER_ARCH=${RUNNER_ARCH}). Run the job on a Linux x86_64 runner such as ubuntu-latest." >&2
  exit 1
fi

# release-please publishes a release before the release workflow uploads its
# assets, so the newest release can lack the binary or its checksum for a few
# minutes. Requiring both keeps a `vN` ref on the previous release during
# that window instead of installing unverified.
readonly RELEASE_TAGS_WITH_VERIFIABLE_BINARY_JQ=".[] | select((.draft or .prerelease) | not) | select(any(.assets[]; .name == \"${ASSET_PATTERN}\") and any(.assets[]; .name == \"${ASSET_PATTERN}.sha256\")) | .tag_name"

newest_release_in_major() {
  local repo="$1" major="$2" release_tags

  release_tags="$(gh api "repos/${repo}/releases?per_page=100" --paginate --jq "$RELEASE_TAGS_WITH_VERIFIABLE_BINARY_JQ")" || return 1
  { grep -E "^${major}\.[0-9]+\.[0-9]+$" <<<"$release_tags" || true; } | sort -V | tail -n 1
}

# Resolve the release tag to download from the ref the action was pinned to:
#   - a major-version tag such as `v2` (moved to each v2.x.y release by
#     .github/workflows/release-please.yml): the newest published v2.x.y
#     release that ships the binary and its .sha256, so a newer major is
#     never picked up;
#   - an exact release tag such as `v1.2.3`: that release;
#   - anything else, such as `@main` or a commit SHA: the latest release.
resolve_tag() {
  local repo="$1" ref="$2"

  if [[ "$ref" =~ ^v[0-9]+$ ]]; then
    local tag
    if ! tag="$(newest_release_in_major "$repo" "$ref")"; then
      echo "ERROR: could not list the releases of ${repo} to resolve ${ref}." >&2
      return 1
    fi
    if [[ -z "$tag" ]]; then
      echo "ERROR: ${repo} has no published ${ref}.x.y release with both ${ASSET_PATTERN} and ${ASSET_PATTERN}.sha256 assets (drafts and prereleases do not count). Pin the action to an existing release tag instead." >&2
      return 1
    fi
    printf '%s\n' "$tag"
    return 0
  fi

  if gh release view "$ref" --repo "$repo" >/dev/null 2>&1; then
    printf '%s\n' "$ref"
    return 0
  fi

  gh release view --repo "$repo" --json tagName -q .tagName
}

tag="$(resolve_tag "$ACTION_REPOSITORY" "$ACTION_REF")"

install_dir="${RUNNER_TEMP:-$(mktemp -d)}/pg-migration-lint-bin"
mkdir -p "$install_dir"

gh release download "$tag" \
  --repo "$ACTION_REPOSITORY" \
  --pattern "$ASSET_PATTERN" \
  --dir "$install_dir" \
  --clobber

if ! gh release download "$tag" \
  --repo "$ACTION_REPOSITORY" \
  --pattern "$ASSET_PATTERN.sha256" \
  --dir "$install_dir" \
  --clobber; then
  echo "WARNING: no ${ASSET_PATTERN}.sha256 asset found for ${tag}; this release predates checksum publishing. Installing ${BINARY_NAME} without verification." >&2
else
  if ! (cd "$install_dir" && sha256sum -c "$ASSET_PATTERN.sha256"); then
    echo "ERROR: checksum verification failed for ${ASSET_PATTERN} (release ${tag}). Refusing to install." >&2
    exit 1
  fi
fi

tar -xzf "$install_dir/$ASSET_PATTERN" -C "$install_dir"
chmod +x "$install_dir/$BINARY_NAME"

echo "$install_dir" >> "$GITHUB_PATH"

echo "Installed ${BINARY_NAME} (${tag}) to ${install_dir}"
