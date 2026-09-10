#!/usr/bin/env bash
set -euo pipefail

DESKTOP="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO="$(dirname "$DESKTOP")"
REPO_COMMON_ROOT="$(dirname "$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(dirname "$REPO_COMMON_ROOT")/margins-cargo-target}"

cd "$DESKTOP/src-tauri"
cargo check --no-default-features --features hosted-web --bin margins-server
