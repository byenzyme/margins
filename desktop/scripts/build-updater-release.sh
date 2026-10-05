#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" && -z "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]; then
  echo "Set TAURI_SIGNING_PRIVATE_KEY or TAURI_SIGNING_PRIVATE_KEY_PATH before building updater artifacts." >&2
  echo "Local key path generated during setup: $HOME/.tauri/margins-updater.key or $HOME/.tauri/aside-updater.key" >&2
  exit 1
fi

if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" && -n "${TAURI_SIGNING_PRIVATE_KEY_PATH:-}" ]]; then
  export TAURI_SIGNING_PRIVATE_KEY="$(cat "$TAURI_SIGNING_PRIVATE_KEY_PATH")"
fi

export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/margins-cargo-target}"

npm run build
rustup target add aarch64-apple-darwin
npx tauri build --target aarch64-apple-darwin
