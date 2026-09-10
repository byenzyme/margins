#!/usr/bin/env bash
# Stage the margins CLI (built from this repo) into src-tauri/binaries/ as a
# Tauri sidecar (externalBin "binaries/margins-cli"). Named margins-cli because
# Contents/MacOS/ already holds the main "Margins" executable and APFS is
# case-insensitive. Usage: build-margins-sidecar.sh [target-triple]
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$SCRIPT_DIR/../binaries"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
SCRATCH=""

cleanup_scratch() {
  if [ -n "$SCRATCH" ] && [ -d "$SCRATCH" ]; then
    rm -rf -- "$SCRATCH"
  fi
}
trap cleanup_scratch EXIT

TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
DEST="$BIN_DIR/margins-cli-$TRIPLE"
if [ -x "$DEST" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "build-margins-sidecar: $DEST already present (FORCE=1 to rebuild)"
  exit 0
fi

# Reuse an already-built binary when one exists; otherwise build into a
# scratch target dir so this stays safe to call from build.rs (the shared
# target dir is locked by the outer cargo invocation).
TARGET_DIR="$(cd "$REPO_ROOT" && cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
SOURCE=""
if [ "$TRIPLE" = "$HOST_TRIPLE" ]; then
  CANDIDATES=("$TARGET_DIR/release/margins-private" "$TARGET_DIR/debug/margins-private")
else
  CANDIDATES=("$TARGET_DIR/$TRIPLE/release/margins-private" "$TARGET_DIR/$TRIPLE/debug/margins-private")
fi
for candidate in "${CANDIDATES[@]}"; do
  if [ -x "$candidate" ]; then SOURCE="$candidate"; break; fi
done

if [ -z "$SOURCE" ]; then
  SCRATCH="$BIN_DIR/.margins-build"
  echo "build-margins-sidecar: building margins CLI (scratch target dir)"
  (cd "$REPO_ROOT" && CARGO_TARGET_DIR="$SCRATCH" CARGO_BUILD_BUILD_DIR="$SCRATCH" cargo build --release --bin margins-private --target "$TRIPLE")
  SOURCE="$SCRATCH/$TRIPLE/release/margins-private"
fi

mkdir -p "$BIN_DIR"
cp "$SOURCE" "$DEST.tmp"
chmod 0755 "$DEST.tmp"
mv "$DEST.tmp" "$DEST"
echo "build-margins-sidecar: staged $DEST (from $SOURCE)"
