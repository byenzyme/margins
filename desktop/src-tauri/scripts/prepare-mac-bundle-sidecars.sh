#!/usr/bin/env bash
# Ensure the macOS sidecars declared in tauri.conf.json are present before
# Tauri copies them into the .app bundle.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

case "${TAURI_ENV_PLATFORM:-}" in
  "" | darwin | macos) ;;
  *)
    echo "prepare-mac-bundle-sidecars: skipping non-macOS platform ${TAURI_ENV_PLATFORM}"
    exit 0
    ;;
esac

targets=()
case "${1:-${TAURI_ENV_ARCH:-}}" in
  aarch64 | arm64) targets=("aarch64-apple-darwin") ;;
  x86_64) targets=("x86_64-apple-darwin") ;;
  universal | universal-apple-darwin) targets=("aarch64-apple-darwin" "x86_64-apple-darwin") ;;
  "")
    targets=("$(rustc -vV | sed -n 's/^host: //p')")
    ;;
  *)
    targets=("${1:-${TAURI_ENV_ARCH}}")
    ;;
esac

for target in "${targets[@]}"; do
  case "$target" in
    aarch64-apple-darwin | x86_64-apple-darwin) ;;
    *)
      echo "prepare-mac-bundle-sidecars: skipping unsupported target $target"
      continue
      ;;
  esac

  "$SCRIPT_DIR/fetch-enzyme-sidecar.sh" "$target"
  "$SCRIPT_DIR/build-margins-sidecar.sh" "$target"
done
