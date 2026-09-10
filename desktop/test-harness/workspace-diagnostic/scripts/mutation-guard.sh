#!/usr/bin/env bash
# mutation-guard.sh — snapshot and verify filesystem state of a fixture directory.
#
# Usage:
#   mutation-guard.sh snapshot <dir> <snapshot-file>
#       Walk <dir> with find+shasum, write sorted manifest to <snapshot-file>.
#
#   mutation-guard.sh verify <dir> <snapshot-file>
#       Re-snapshot <dir> and diff against <snapshot-file>.
#       Exits 0 when identical. Exits non-zero and prints the diff when anything
#       has been added, removed, or changed.
#
# The guard is strict: any file added, removed, or modified is a failure.
# There is no exclude list — the guard is designed to catch ALL residue,
# including .enzyme/enzyme.log writes that the skill must not create during
# read-only diagnosis passes.
#
# Self-test (run from any directory):
#   bash mutation-guard.sh selftest
#
set -euo pipefail

PROG="$(basename "$0")"

usage() {
  echo "Usage:"
  echo "  $PROG snapshot <dir> <snapshot-file>"
  echo "  $PROG verify   <dir> <snapshot-file>"
  echo "  $PROG selftest"
  exit 1
}

# build_manifest <dir> <outfile>
# Walks <dir>, hashes every file with sha256, writes sorted "hash  relpath" lines to <outfile>.
build_manifest() {
  local dir="$1"
  local out="$2"
  find "$dir" -type f \
    | sort \
    | while IFS= read -r f; do
        rel="${f#"$dir"/}"
        sum=$(shasum -a 256 "$f" | awk '{print $1}')
        printf "%s  %s\n" "$sum" "$rel"
      done > "$out"
}

do_snapshot() {
  local dir="$1"
  local out="$2"

  if [[ ! -d "$dir" ]]; then
    echo "$PROG snapshot: directory not found: $dir" >&2
    return 1
  fi

  build_manifest "$dir" "$out"
  echo "$PROG: snapshot written to $out ($(wc -l < "$out" | tr -d ' ') files)"
}

do_verify() {
  local dir="$1"
  local snapshot="$2"

  if [[ ! -d "$dir" ]]; then
    echo "$PROG verify: directory not found: $dir" >&2
    return 1
  fi

  if [[ ! -f "$snapshot" ]]; then
    echo "$PROG verify: snapshot file not found: $snapshot" >&2
    return 1
  fi

  local tmp
  tmp="$(mktemp /tmp/mutation-guard-verify.XXXXXX)"
  # shellcheck disable=SC2064
  trap "rm -f '$tmp'" RETURN

  build_manifest "$dir" "$tmp"

  if diff --unified=3 "$snapshot" "$tmp" > /dev/null 2>&1; then
    echo "$PROG: CLEAN — no filesystem changes detected in $dir"
    return 0
  else
    echo "$PROG: FAIL — filesystem residue detected in $dir" >&2
    echo "" >&2
    echo "--- snapshot (before)" >&2
    echo "+++ current  (after)" >&2
    diff --unified=3 "$snapshot" "$tmp" >&2 || true
    return 1
  fi
}

do_selftest() {
  echo "$PROG: running self-test in /tmp ..."

  local testdir snap
  testdir="$(mktemp -d /tmp/mutation-guard-selftest.XXXXXX)"
  snap="$(mktemp /tmp/mutation-guard-snap.XXXXXX)"

  # Cleanup on any exit
  trap "rm -rf '$testdir' '$snap'" EXIT

  echo "hello" > "$testdir/a.txt"
  echo "world" > "$testdir/b.txt"

  # Snapshot two files
  do_snapshot "$testdir" "$snap"

  # Clean verify — must return 0
  if do_verify "$testdir" "$snap"; then
    echo "$PROG: selftest PASS — clean verify returned 0"
  else
    echo "$PROG: selftest FAIL — clean verify returned non-zero" >&2
    exit 1
  fi

  # Add a residue file — dirty verify must return non-zero
  echo "residue" > "$testdir/c.txt"
  local dirty_rc=0
  do_verify "$testdir" "$snap" > /dev/null 2>&1 || dirty_rc=$?
  if [[ "$dirty_rc" -eq 0 ]]; then
    echo "$PROG: selftest FAIL — dirty verify returned 0 (should have been non-zero)" >&2
    exit 1
  else
    echo "$PROG: selftest PASS — dirty verify correctly returned rc=$dirty_rc"
  fi

  echo "$PROG: self-test complete — all assertions passed"
}

# --- dispatch ---

if [[ $# -lt 1 ]]; then
  usage
fi

CMD="$1"
case "$CMD" in
  snapshot)
    [[ $# -eq 3 ]] || usage
    do_snapshot "$2" "$3"
    ;;
  verify)
    [[ $# -eq 3 ]] || usage
    do_verify "$2" "$3"
    ;;
  selftest|self-test)
    do_selftest
    ;;
  *)
    echo "$PROG: unknown command: $CMD" >&2
    usage
    ;;
esac
