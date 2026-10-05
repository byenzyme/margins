#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"

APP_NAME="${MARGINS_APP_NAME:-Margins}"
BUILT_APP_NAME="${MARGINS_BUILT_APP_NAME:-Margins}"
INSTALL_PATH="${MARGINS_INSTALL_PATH:-/Applications/${APP_NAME}.app}"
APP_EXE="${INSTALL_PATH}/Contents/MacOS/margins-desktop"
# Pre-rename install path: the product was once bundled lowercase. On
# case-insensitive volumes this is the same inode and the rm below covers it;
# on case-sensitive volumes it would otherwise linger as a stale copy.
LEGACY_INSTALL_PATH="/Applications/margins.app"
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${HOME}/.cache/margins-cargo-target}"
BUILT_APP="${CARGO_TARGET_DIR}/release/bundle/macos/${BUILT_APP_NAME}.app"

export CARGO_TARGET_DIR

echo "Building ${BUILT_APP_NAME}.app..."
(
  cd "$DESKTOP_DIR"
  TAURI_BUILD_ARGS=(build)
  if [[ -n "${MARGINS_TAURI_FEATURES:-}" ]]; then
    echo "  using Tauri features: ${MARGINS_TAURI_FEATURES}"
    TAURI_BUILD_ARGS+=(--features "$MARGINS_TAURI_FEATURES")
  fi
  if [[ -n "${MARGINS_TAURI_CONFIG_JSON:-}" ]]; then
    TAURI_BUILD_ARGS+=(--config "$MARGINS_TAURI_CONFIG_JSON")
  fi
  # The release config enables auto-updater artifacts (bundle.createUpdaterArtifacts),
  # whose signed .tar.gz requires TAURI_SIGNING_PRIVATE_KEY. A local reinstall only
  # needs the .app and has no signing key, so skip updater-artifact generation when
  # the key is absent — the installed app is identical (updater plugin still built in).
  # Releases (CI) set the key and produce the signed artifact as normal.
  if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    echo "  no TAURI_SIGNING_PRIVATE_KEY; skipping updater artifacts for local build"
    TAURI_BUILD_ARGS+=(--config '{"bundle":{"createUpdaterArtifacts":false}}')
  fi
  npm run tauri -- "${TAURI_BUILD_ARGS[@]}"
)

if [[ ! -d "$BUILT_APP" ]]; then
  echo "Expected built app not found: $BUILT_APP" >&2
  exit 1
fi

echo "Stopping installed ${APP_NAME}.app if it is running..."
osascript -e "tell application \"${APP_NAME}\" to quit" >/dev/null 2>&1 || true
sleep "${MARGINS_REINSTALL_QUIT_WAIT_SECONDS:-2}"

if pgrep -fl "${INSTALL_PATH}/Contents/MacOS|margins-desktop" >/dev/null 2>&1; then
  echo "Installed app is still running; terminating remaining margins-desktop processes..."
  pkill -f "${INSTALL_PATH}/Contents/MacOS|margins-desktop" || true
  sleep 1
fi

echo "Installing to ${INSTALL_PATH}..."
rm -rf "$INSTALL_PATH"
[[ "$LEGACY_INSTALL_PATH" != "$INSTALL_PATH" ]] && rm -rf "$LEGACY_INSTALL_PATH"
ditto "$BUILT_APP" "$INSTALL_PATH"

echo "Opening ${INSTALL_PATH}..."
if [[ "${MARGINS_OPEN_DEVTOOLS:-0}" == "1" || "${MARGINS_LAUNCH_DIRECT:-0}" == "1" ]]; then
  LOG_SUFFIX="$(printf '%s' "$APP_NAME" | tr '[:upper:] ' '[:lower:]-' | sed -E 's/[^a-z0-9_-]+/-/g')"
  if [[ "${MARGINS_SANITIZE_LAUNCH_ENV:-0}" == "1" ]]; then
    echo "  launching executable directly with first-run AI environment scrubbed."
    (
      cd /
      env \
        -u OPENAI_API_KEY \
        -u OPENAI_BASE_URL \
        -u OPENAI_MODEL \
        -u MARGINS_API_KEY \
        -u MARGINS_OPENAI_API_KEY \
        -u MARGINS_OPENROUTER_MGMT_KEY \
        -u MARGINS_INCLUDED_AI_CONFIG_URL \
        "$APP_EXE" >/tmp/margins-desktop-${LOG_SUFFIX}.log 2>&1
    ) &
  else
    echo "  launching executable directly so the app inherits the environment."
    "$APP_EXE" >/tmp/margins-desktop-${LOG_SUFFIX}.log 2>&1 &
  fi
else
  open -a "$INSTALL_PATH"
fi
sleep "${MARGINS_REINSTALL_OPEN_WAIT_SECONDS:-2}"

if pgrep -fl "${INSTALL_PATH}/Contents/MacOS|margins-desktop" >/dev/null 2>&1; then
  echo "Running installed app:"
  pgrep -fl "${INSTALL_PATH}/Contents/MacOS|margins-desktop"
else
  echo "Install finished, but no running process was detected yet." >&2
fi

echo "Done."
