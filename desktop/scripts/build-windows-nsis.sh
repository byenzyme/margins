#!/usr/bin/env bash
set -euo pipefail

TARGET="${MARGINS_WINDOWS_TARGET:-x86_64-pc-windows-msvc}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/margins-cargo-target}"
export CARGO_NET_GIT_FETCH_WITH_CLI="${CARGO_NET_GIT_FETCH_WITH_CLI:-true}"

if ! command -v cargo-xwin >/dev/null 2>&1; then
  echo "cargo-xwin is required: cargo install --locked cargo-xwin" >&2
  exit 1
fi

if ! rustup target list --installed | grep -qx "$TARGET"; then
  rustup target add "$TARGET"
fi

if ! command -v makensis >/dev/null 2>&1; then
  echo "NSIS is required on macOS: brew install nsis" >&2
  exit 1
fi

if ! command -v llvm-rc >/dev/null 2>&1 && command -v brew >/dev/null 2>&1; then
  LLVM_PREFIX="$(brew --prefix llvm 2>/dev/null || true)"
  if [ -n "$LLVM_PREFIX" ]; then
    export PATH="$LLVM_PREFIX/bin:$PATH"
  fi
fi

if ! command -v llvm-rc >/dev/null 2>&1; then
  echo "llvm-rc is required on macOS: brew install llvm" >&2
  exit 1
fi

npm run tauri -- build \
  --runner cargo-xwin \
  --target "$TARGET" \
  --features parakeet-asr \
  --config '{"build":{"devUrl":"http://margins.invalid"},"bundle":{"targets":["nsis"]}}' \
  -- \
  --no-default-features
