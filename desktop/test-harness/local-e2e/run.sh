#!/usr/bin/env bash
# ---------------------------------------------------------------------------
# Local e2e harness for Margins on a headless Linux VPS.
#
# Boots the real Rust HTTP backend (margins-server) against an isolated,
# pre-seeded workspace, starts the Vite frontend with the dev proxy + token,
# and prints the URL to drive with `agent-browser`. This is the real backend
# (real storage, real distillation), not the ?scenario= mock lane.
#
# Usage:
#   desktop/test-harness/local-e2e/run.sh [--keep-state] [--rebuild] [--hermetic-bin]
#
#   --hermetic-bin  Strip the real ~/.local/bin from the server's PATH so the
#                   CLI-install flow (ensure_cli_tools) installs enzyme/margins
#                   into the isolated temp HOME instead of reusing/polluting the
#                   real one. Use for setup/CLI-install E2E; removed on teardown.
#
# Env overrides:
#   MARGINS_E2E_PORT        server port (default 8787)
#   MARGINS_E2E_VITE_PORT   vite port   (default 5173)
#   MARGINS_E2E_GATEWAY_IP  host IP reachable from the agent-browser sandbox
#                           (default 172.18.0.1 — the docker bridge gateway)
#   CARGO_TARGET_DIR        cargo target (default repository-wide shared cache)
#   MARGINS_E2E_SERVER_FEATURES cargo features (default hosted-web: server,
#                           recall, and dynamically loaded Parakeet ASR)
# ---------------------------------------------------------------------------
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP="$(cd "$HERE/../.." && pwd)"
REPO="$(cd "$DESKTOP/.." && pwd)"
TAURI="$DESKTOP/src-tauri"
REPO_COMMON_ROOT="$(dirname "$(git -C "$REPO" rev-parse --path-format=absolute --git-common-dir)")"
SHARED_TARGET="$(dirname "$REPO_COMMON_ROOT")/margins-cargo-target"

PORT="${MARGINS_E2E_PORT:-8787}"
VITE_PORT="${MARGINS_E2E_VITE_PORT:-5173}"
GATEWAY_IP="${MARGINS_E2E_GATEWAY_IP:-172.18.0.1}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$SHARED_TARGET}"

STATE_ROOT="${MARGINS_E2E_STATE:-/tmp/margins-e2e}"
HOME_DIR="$STATE_ROOT/home"
DATA_DIR="$STATE_ROOT/data"
VAULT_DIR="$HOME_DIR/Documents/margins"
SERVER_BIN="$CARGO_TARGET_DIR/debug/margins-server"
SERVER_FEATURES="${MARGINS_E2E_SERVER_FEATURES:-hosted-web}"

KEEP_STATE=0
REBUILD=0
HERMETIC_BIN=0
for arg in "$@"; do
  case "$arg" in
    --keep-state)   KEEP_STATE=1 ;;
    --rebuild)      REBUILD=1 ;;
    --hermetic-bin) HERMETIC_BIN=1 ;;
  esac
done

# Leak guard: `ensure_cli_tools` installs enzyme/margins into $HOME/.local/bin.
# Snapshot the *real* bin dir up front so cleanup can prove the run did not
# install into or mutate it (it should only ever touch the isolated temp HOME).
REAL_BIN_DIR="${HOME:-/nonexistent}/.local/bin"
REAL_BIN_BEFORE="$(ls -la "$REAL_BIN_DIR" 2>/dev/null | sha256sum | cut -d' ' -f1)"

pids=()
cleanup() {
  echo "--- shutting down harness ---"
  for pid in "${pids[@]:-}"; do kill "$pid" 2>/dev/null || true; done
  local after
  after="$(ls -la "$REAL_BIN_DIR" 2>/dev/null | sha256sum | cut -d' ' -f1)"
  if [[ "$after" != "$REAL_BIN_BEFORE" ]]; then
    echo "!! WARNING: real bin dir $REAL_BIN_DIR changed during this run —"
    echo "!! the harness leaked a binary outside the temp HOME. Inspect and clean:"
    echo "!!   ls -la $REAL_BIN_DIR   (look for fresh enzyme/margins + .*.margins-managed)"
  fi
  if [[ "$KEEP_STATE" -eq 0 ]]; then rm -rf "$STATE_ROOT"; fi
}
trap cleanup EXIT INT TERM

echo "==> building margins-server (headless, Linux)"
if [[ "$REBUILD" -eq 1 || ! -x "$SERVER_BIN" ]]; then
  ( cd "$TAURI" && cargo build --no-default-features --features "$SERVER_FEATURES" --bin margins-server )
fi

if [[ "$KEEP_STATE" -eq 1 && -f "$DATA_DIR/token" ]]; then
  echo "==> reusing existing workspace at $STATE_ROOT (--keep-state)"
else
  echo "==> seeding fresh isolated workspace at $STATE_ROOT"
  rm -rf "$STATE_ROOT"
  mkdir -p "$DATA_DIR" "$VAULT_DIR/.margins/recordings" "$VAULT_DIR/meetings" "$VAULT_DIR/people"
  cp -r "$HERE/seed-vault/." "$VAULT_DIR/"
fi

# Hermetic bin mode: strip the real ~/.local/bin from the server's PATH so
# `ensure_cli_tools` cannot discover/reuse a real enzyme via `which` and instead
# downloads + installs into the isolated $HOME_DIR/.local/bin. This makes the
# CLI-install/project-setup flow a true, self-cleaning test (removed with the
# temp state) instead of silently depending on whatever is already installed.
SERVER_PATH="$PATH"
if [[ "$HERMETIC_BIN" -eq 1 ]]; then
  SERVER_PATH="$HOME_DIR/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
  echo "==> hermetic-bin: server PATH excludes real ~/.local/bin (installs land in $HOME_DIR/.local/bin)"
fi

echo "==> starting margins-server on 127.0.0.1:$PORT"
HOME="$HOME_DIR" PATH="$SERVER_PATH" \
MARGINS_HOST=127.0.0.1 MARGINS_PORT="$PORT" \
MARGINS_DATA_DIR="$DATA_DIR" MARGINS_PROFILE=e2e \
MARGINS_DISABLE_KEYCHAIN=1 \
  "$SERVER_BIN" &
pids+=("$!")

echo -n "==> waiting for /health "
for _ in $(seq 1 60); do
  if curl -sf "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then echo "ok"; break; fi
  echo -n "."; sleep 1
done
curl -sf "http://127.0.0.1:$PORT/health" >/dev/null || { echo "server failed to start"; exit 1; }

TOKEN="$(cat "$DATA_DIR/token")"

echo "==> starting Vite (0.0.0.0:$VITE_PORT) with proxy -> server"
( cd "$DESKTOP" && \
  MARGINS_DEV_HTTP_TARGET="http://127.0.0.1:$PORT" \
  MARGINS_DEV_HTTP_TOKEN="$TOKEN" \
  npm run dev -- --host 0.0.0.0 --port "$VITE_PORT" --strictPort ) &
pids+=("$!")

echo -n "==> waiting for Vite "
for _ in $(seq 1 60); do
  if curl -sf "http://127.0.0.1:$VITE_PORT/" >/dev/null 2>&1; then echo "ok"; break; fi
  echo -n "."; sleep 1
done

cat <<EOF

============================================================
 Margins local e2e harness is UP
------------------------------------------------------------
 Real backend : http://127.0.0.1:$PORT   (token: ${TOKEN:0:8}…)
 Frontend     : http://127.0.0.1:$VITE_PORT   (host curl)
 Drive it with agent-browser via the sandbox-reachable IP:

   agent-browser open "http://$GATEWAY_IP:$VITE_PORT/"
   agent-browser snapshot -i -c
   agent-browser screenshot /tmp/margins.png

 Seeded vault : $VAULT_DIR
 Distill skill: $SKILL_PATH
 State dir    : $STATE_ROOT  (--keep-state to preserve)
============================================================

Press Ctrl-C to tear everything down.
EOF

wait
