#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

PROFILE="${MARGINS_FIRST_RUN_PROFILE:-first-run-test}"
APP_NAME="${MARGINS_FIRST_RUN_APP_NAME:-Margins First Run}"
INSTALL_PATH="${MARGINS_FIRST_RUN_INSTALL_PATH:-/Applications/${APP_NAME}.app}"
BUNDLE_ID="${MARGINS_FIRST_RUN_BUNDLE_ID:-com.margins.desktop.first-run}"

profile_slug() {
  local profile="$1"
  if [[ -z "$profile" || "$profile" == "default" ]]; then
    printf 'margins'
    return
  fi
  # Keep this in sync with desktop/src-tauri/src/settings.rs.
  local slug
  slug="$(printf '%s' "$profile" | tr '[:upper:]' '[:lower:]' | sed -E 's/[^a-z0-9_-]+/-/g; s/^-+//; s/-+$//')"
  [[ -n "$slug" ]] || slug="profile"
  printf 'margins-%s' "$slug"
}

PROFILE_SLUG="$(profile_slug "$PROFILE")"
# By default first-run uses the normal verified FluidAudio cache. Set this to
# an empty temporary directory only when the E2E needs a fully isolated model
# download (it is roughly 464 MB).
FIRST_RUN_MODEL_DIR="${MARGINS_FIRST_RUN_MODEL_DIR:-}"
TAURI_CONFIG_PATH="$(cd "$SCRIPT_DIR/.." && pwd)/src-tauri/tauri.conf.json"
FIRST_RUN_TAURI_CONFIG_JSON="$(
  APP_NAME="$APP_NAME" BUNDLE_ID="$BUNDLE_ID" TAURI_CONFIG_PATH="$TAURI_CONFIG_PATH" node <<'NODE'
const fs = require("fs");

const appName = process.env.APP_NAME;
const bundleId = process.env.BUNDLE_ID;
const configPath = process.env.TAURI_CONFIG_PATH;
const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
const mainWindow = config.app?.windows?.[0] || {};

process.stdout.write(JSON.stringify({
  productName: appName,
  identifier: bundleId,
  app: {
    windows: [
      {
        ...mainWindow,
        title: appName,
      },
    ],
  },
}));
NODE
)"

if [[ "$PROFILE_SLUG" == "margins" ]]; then
  echo "Refusing to clear the default Margins profile. Use a non-default MARGINS_FIRST_RUN_PROFILE." >&2
  exit 1
fi

echo "Preparing clean first-run test profile:"
echo "  app: ${INSTALL_PATH}"
echo "  bundle id: ${BUNDLE_ID}"
echo "  profile: ${PROFILE}"
echo "  profile slug: ${PROFILE_SLUG}"
if [[ -n "$FIRST_RUN_MODEL_DIR" ]]; then
  echo "  isolated model dir: ${FIRST_RUN_MODEL_DIR}"
else
  echo "  model cache: shared ~/Library/Application Support/FluidAudio/Models (default)"
fi

osascript -e "tell application \"${APP_NAME}\" to quit" >/dev/null 2>&1 || true
pkill -f "${INSTALL_PATH}/Contents/MacOS|MARGINS_PROFILE=${PROFILE}|${PROFILE_SLUG}" >/dev/null 2>&1 || true

for base in \
  "$HOME/Library/Application Support" \
  "$HOME/Library/Preferences" \
  "$HOME/Library/Caches"; do
  target="${base}/${PROFILE_SLUG}"
  if [[ -e "$target" ]]; then
    echo "  removing ${target}"
    rm -rf "$target"
  fi
done

for target in \
  "$HOME/Library/WebKit/${BUNDLE_ID}" \
  "$HOME/Library/Saved Application State/${BUNDLE_ID}.savedState" \
  "$HOME/Library/HTTPStorages/${BUNDLE_ID}" \
  "$HOME/Library/Caches/${BUNDLE_ID}" \
  "$HOME/Library/Preferences/${BUNDLE_ID}.plist"; do
  if [[ -e "$target" ]]; then
    echo "  removing ${target}"
    rm -rf "$target"
  fi
done

for scope in note-ai backchannel included-openrouter; do
  service="${PROFILE_SLUG}.${scope}"
  echo "  clearing Keychain service ${service}"
  security delete-generic-password -s "$service" -a api-key >/dev/null 2>&1 || true
  security delete-generic-password -s "$service" -a expires-at >/dev/null 2>&1 || true
  security delete-generic-password -s "$service" -a bootstrap-id >/dev/null 2>&1 || true
done

FIRST_RUN_TCC_SERVICES=(Microphone AudioCapture)
for service in "${FIRST_RUN_TCC_SERVICES[@]}"; do
  echo "  resetting ${service} permission for ${BUNDLE_ID}"
  tccutil reset "$service" "$BUNDLE_ID" >/dev/null 2>&1 || true
done

export MARGINS_PROFILE="$PROFILE"
export MARGINS_APP_NAME="$APP_NAME"
export MARGINS_BUILT_APP_NAME="$APP_NAME"
export MARGINS_INSTALL_PATH="$INSTALL_PATH"
export MARGINS_LAUNCH_DIRECT=1
export MARGINS_SANITIZE_LAUNCH_ENV=1
if [[ -n "$FIRST_RUN_MODEL_DIR" ]]; then
  export MARGINS_FLUID_COREML_MODEL_DIR="$FIRST_RUN_MODEL_DIR"
fi
export MARGINS_TAURI_CONFIG_JSON="$FIRST_RUN_TAURI_CONFIG_JSON"

"$SCRIPT_DIR/reinstall-app.sh"
