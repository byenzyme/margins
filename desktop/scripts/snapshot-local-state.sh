#!/usr/bin/env bash
set -euo pipefail

# Dump the real local Margins desktop state (sanitized settings + indexed session
# list + a few sample note bodies) into a gitignored fixture that the browser UX
# harness loads. This makes the CDP previews resemble the actual installed app so
# layout/density can be evaluated against real data.
#
# The fixture is written by the real Rust indexing path (no reimplementation), so
# it cannot drift from what the app shows.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
TAURI_DIR="$DESKTOP_DIR/src-tauri"

OUT="${MARGINS_SNAPSHOT_OUT:-$DESKTOP_DIR/test-harness/local-snapshot.json}"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/margins-cargo-target}"
export CARGO_TARGET_DIR

echo "Building + running snapshot export (this reuses the shared cargo target cache)..."
(
  cd "$TAURI_DIR"
  MARGINS_EXPORT_HARNESS_SNAPSHOT="$OUT" cargo run --quiet
)

if [[ -f "$OUT" ]]; then
  echo "Snapshot written to: $OUT"
else
  echo "Snapshot export did not produce $OUT" >&2
  exit 1
fi
