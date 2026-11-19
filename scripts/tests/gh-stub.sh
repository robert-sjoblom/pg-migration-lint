#!/usr/bin/env bash
#
# gh-stub.sh — fake `gh` CLI for test-download-binary.sh. Simulates
# `gh release view` / `gh release download` / `gh api repos/<repo>/releases`
# without any network access, and logs every invocation for the test to
# assert against.
#
# Required environment (set by the test harness, not a real caller):
#   GH_STUB_LOG          file to append "gh <args...>" invocation lines to.
#   GH_STUB_KNOWN_TAG    a ref that this stub treats as an existing release
#                        tag (i.e. `gh release view <that ref>` succeeds).
#   GH_STUB_LATEST_TAG   the tag name returned as the "latest release" when
#                        queried without a positional ref.
#   GH_STUB_SHA256_MODE  how this stub responds to a `--pattern *.sha256`
#                        download request:
#                          match    - writes a correct .sha256 file for the
#                                     tarball already written into $dir.
#                          mismatch - writes a .sha256 file with a
#                                     deliberately wrong digest.
#                          missing  - exits non-zero, mirroring real gh's
#                                     "no assets matched patterns" behavior
#                                     for a release published before
#                                     checksums existed.
#
# Optional environment:
#   GH_STUB_RELEASES_RESPONSE  the JSON array that
#                              `gh api repos/<repo>/releases` responds with.
#                              The stub applies the caller's `--jq` filter to
#                              it with the real `jq`, so the filter itself is
#                              under test. Any value that is not a JSON array,
#                              such as `HTTP 502: Bad Gateway`, makes the call
#                              fail with that error. Without it, `gh api`
#                              counts as unstubbed.

set -euo pipefail

: "${GH_STUB_LOG:?}"
: "${GH_STUB_KNOWN_TAG:?}"
: "${GH_STUB_LATEST_TAG:?}"
: "${GH_STUB_SHA256_MODE:?}"

printf 'gh %s\n' "$*" >> "$GH_STUB_LOG"

if [[ "${1:-}" == "api" && "${2:-}" == repos/*/releases* && -n "${GH_STUB_RELEASES_RESPONSE:-}" ]]; then
  if [[ "$GH_STUB_RELEASES_RESPONSE" != \[* ]]; then
    echo "gh: $GH_STUB_RELEASES_RESPONSE" >&2
    exit 1
  fi
  shift 2
  jq_filter="."
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --jq|-q) jq_filter="$2"; shift 2 ;;
      *) shift ;;
    esac
  done
  jq -r "$jq_filter" <<<"$GH_STUB_RELEASES_RESPONSE"
  exit
fi

case "${1:-} ${2:-}" in
  "release view")
    shift 2
    if [[ "${1:-}" != --* ]]; then
      # Positional ref form: gh release view <ref> --repo <repo>
      ref="$1"
      [[ "$ref" == "$GH_STUB_KNOWN_TAG" ]] && exit 0 || exit 1
    fi
    # No positional ref: gh release view --repo <repo> --json tagName -q .tagName
    printf '%s\n' "$GH_STUB_LATEST_TAG"
    exit 0
    ;;
  "release download")
    shift 2
    tag="$1"
    shift
    repo=""
    pattern=""
    dir=""
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --repo) repo="$2"; shift 2 ;;
        --pattern) pattern="$2"; shift 2 ;;
        --dir) dir="$2"; shift 2 ;;
        --clobber) shift ;;
        *) shift ;;
      esac
    done
    : "$tag" "$repo" # asserted by the caller via the invocation log, not here
    mkdir -p "$dir"

    if [[ "$pattern" == *.sha256 ]]; then
      tarball_name="${pattern%.sha256}"
      case "$GH_STUB_SHA256_MODE" in
        missing)
          exit 1
          ;;
        match)
          (cd "$dir" && sha256sum "$tarball_name") > "$dir/$pattern"
          exit 0
          ;;
        mismatch)
          real_line="$(cd "$dir" && sha256sum "$tarball_name")"
          bad_line="0${real_line:1}"
          [[ "$bad_line" == "$real_line" ]] && bad_line="1${real_line:1}"
          printf '%s\n' "$bad_line" > "$dir/$pattern"
          exit 0
          ;;
        *)
          echo "gh-stub: unknown GH_STUB_SHA256_MODE: $GH_STUB_SHA256_MODE" >&2
          exit 99
          ;;
      esac
    fi

    workdir="$(mktemp -d)"
    printf 'fake-binary-contents' > "$workdir/pg-migration-lint"
    chmod 644 "$workdir/pg-migration-lint"
    tar -czf "$dir/$pattern" -C "$workdir" pg-migration-lint
    rm -rf "$workdir"
    exit 0
    ;;
  *)
    echo "gh-stub: unstubbed invocation: $*" >&2
    exit 99
    ;;
esac
