#!/usr/bin/env bash
set -euo pipefail

# Launch Margins Desktop in dev mode using the same default profile as the
# installed /Applications app. This keeps settings, backchannel Keychain items,
# and Margins-owned Pi/Codex auth shared while still allowing isolated profiles via
# MARGINS_PROFILE=<name> or an explicit MARGINS_PI_AGENT_DIR.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"

profile_slug() {
  local profile="${MARGINS_PROFILE:-default}"
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

PROFILE_SLUG="$(profile_slug)"
MARGINS_PI_AGENT_DIR="${MARGINS_PI_AGENT_DIR:-$HOME/Library/Application Support/$PROFILE_SLUG/pi-agent}"
GLOBAL_PI_AGENT_DIR="${PI_GLOBAL_AGENT_DIR:-$HOME/.pi/agent}"

mkdir -p "$MARGINS_PI_AGENT_DIR"

# Preserve non-secret model/settings preferences for convenience, but never copy
# auth.json unless the developer explicitly opts in.
for file in settings.json models.json; do
  if [[ ! -f "$MARGINS_PI_AGENT_DIR/$file" && -f "$GLOBAL_PI_AGENT_DIR/$file" ]]; then
    cp "$GLOBAL_PI_AGENT_DIR/$file" "$MARGINS_PI_AGENT_DIR/$file"
  fi
done

if [[ "${MARGINS_COPY_GLOBAL_PI_AUTH:-0}" == "1" && ! -f "$MARGINS_PI_AGENT_DIR/auth.json" && -f "$GLOBAL_PI_AGENT_DIR/auth.json" ]]; then
  cp "$GLOBAL_PI_AGENT_DIR/auth.json" "$MARGINS_PI_AGENT_DIR/auth.json"
  chmod 600 "$MARGINS_PI_AGENT_DIR/auth.json" 2>/dev/null || true
  echo "Seeded Margins Pi auth from: $GLOBAL_PI_AGENT_DIR/auth.json"
fi

export MARGINS_PI_AGENT_DIR
export PI_CODING_AGENT_DIR="$MARGINS_PI_AGENT_DIR"

if [[ "$PROFILE_SLUG" == "margins" ]]; then
  echo "Launching Margins dev with shared default profile"
else
  echo "Launching Margins dev with isolated profile: ${MARGINS_PROFILE:-default}"
fi
echo "  profile data: $HOME/Library/Application Support/$PROFILE_SLUG"
echo "  MARGINS_PI_AGENT_DIR=$MARGINS_PI_AGENT_DIR"
echo "  PI_CODING_AGENT_DIR=$PI_CODING_AGENT_DIR"
echo "  auth file: $PI_CODING_AGENT_DIR/auth.json"
echo "  global Pi auth remains: $GLOBAL_PI_AGENT_DIR/auth.json"
echo ""

if [[ "${MARGINS_DEV_AUDIO_PERMS:-0}" == "1" ]]; then
  "$DESKTOP_DIR/scripts/dev-audio-permissions.sh" --grant || true
  echo ""
fi

cd "$DESKTOP_DIR"
npm run tauri -- dev "$@"
