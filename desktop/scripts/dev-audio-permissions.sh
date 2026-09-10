#!/usr/bin/env bash
set -euo pipefail

# Best-effort helper for local Tauri dev audio permissions.
# macOS TCC permissions are owned by the OS and cannot be bypassed; this script
# only tries the same user-TCC grant pattern used by install.sh for the dev
# runner, then prints manual fallback instructions if TCC is not writable from
# the current terminal.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DESKTOP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_DIR="$(cd "$DESKTOP_DIR/.." && pwd)"
TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"
GRANT=0

for arg in "$@"; do
  case "$arg" in
    --grant) GRANT=1 ;;
  esac
done

if [[ "${MARGINS_DEV_GRANT_AUDIO:-0}" == "1" ]]; then
  GRANT=1
fi

clients=()
add_client() {
  local kind="$1"
  local client="$2"
  [[ -z "$client" ]] && return 0
  clients+=("$kind|$client")
}

# Bundle ID of the terminal/app launching Tauri dev. In cmux this is commonly
# com.cmuxterm.app; in a normal terminal it may be Terminal, iTerm, Ghostty, etc.
if [[ -n "${__CFBundleIdentifier:-}" ]]; then
  add_client 0 "$__CFBundleIdentifier"
fi

case "${TERM_PROGRAM:-}" in
  ghostty) add_client 0 "com.mitchellh.ghostty" ;;
  iTerm.app) add_client 0 "com.googlecode.iterm2" ;;
  Apple_Terminal) add_client 0 "com.apple.Terminal" ;;
esac

# Some TCC prompts key non-bundled dev binaries by absolute path. Add the debug
# binary path when it exists so a rebuilt/restarted dev app is more likely to be
# recognized consistently.
DEBUG_EXE="$DESKTOP_DIR/src-tauri/target/debug/margins-desktop"
if [[ -x "$DEBUG_EXE" ]]; then
  add_client 1 "$DEBUG_EXE"
fi

# De-duplicate while preserving simple output.
unique_clients=()
seen=""
for item in "${clients[@]}"; do
  if [[ "$seen" != *"//$item//"* ]]; then
    unique_clients+=("$item")
    seen="$seen//$item//"
  fi
done

if [[ ${#unique_clients[@]} -eq 0 ]]; then
  echo "Could not detect a dev runner for TCC audio permissions."
  echo "Open System Settings > Privacy & Security > Screen & System Audio Recording and enable your terminal/dev app."
  exit 0
fi

can_query=1
sqlite3 "$TCC_DB" "select count(*) from access limit 1;" >/dev/null 2>&1 || can_query=0

if [[ "$can_query" != "1" ]]; then
  echo "Cannot read macOS TCC database from this terminal."
  echo "Grant Full Disk Access to the terminal/dev runner if you want automatic dev grants, or enable manually:"
  echo "  System Settings > Privacy & Security > Screen & System Audio Recording"
  printf 'Detected dev runners:\n'
  for item in "${unique_clients[@]}"; do
    IFS='|' read -r kind client <<< "$item"
    echo "  - $client"
  done
  exit 0
fi

for item in "${unique_clients[@]}"; do
  IFS='|' read -r client_type client <<< "$item"
  has_perm=$(sqlite3 "$TCC_DB" "SELECT COUNT(*) FROM access WHERE service='kTCCServiceAudioCapture' AND client='$client' AND auth_value=2;" 2>/dev/null || echo "0")
  if [[ "$has_perm" != "0" ]]; then
    echo "Dev system-audio permission already granted: $client"
    continue
  fi

  if [[ "$GRANT" == "1" ]]; then
    echo "Granting dev system-audio permission: $client"
    if sqlite3 "$TCC_DB" "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, flags) VALUES ('kTCCServiceAudioCapture', '$client', $client_type, 2, 0, 1, 0);" 2>/dev/null; then
      echo "  granted"
    else
      echo "  failed; enable manually in Screen & System Audio Recording"
    fi
  else
    echo "Dev system-audio permission missing: $client"
  fi
done

if [[ "$GRANT" == "1" ]]; then
  echo "If a permission was newly granted, restart the terminal/dev app before trusting system-audio capture."
else
  echo "Run with --grant to attempt automatic dev grants."
fi
