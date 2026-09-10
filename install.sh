#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ "$(uname -s)" != "Darwin" ]; then
  # Linux/CI: portable recall-only CLI (no native capture/ASR toolchain).
  "$SCRIPT_DIR/scripts/install-official-cli.sh"
  exit 0
fi

# macOS: build the full-featured CLI so one `margins` records, transcribes, and
# generates catalyst bridges — matching the audio-capture permission granted below.
MARGINS_CLI_PROFILE=full "$SCRIPT_DIR/scripts/install-official-cli.sh"

TCC_DB="$HOME/Library/Application Support/com.apple.TCC/TCC.db"

# Detect terminal bundle ID.
TERMINAL_ID=""
if [[ -n "${__CFBundleIdentifier:-}" ]]; then
    TERMINAL_ID="$__CFBundleIdentifier"
elif [[ "${TERM_PROGRAM:-}" == "ghostty" ]]; then
    TERMINAL_ID="com.mitchellh.ghostty"
elif [[ "${TERM_PROGRAM:-}" == "iTerm.app" ]]; then
    TERMINAL_ID="com.googlecode.iterm2"
elif [[ "${TERM_PROGRAM:-}" == "Apple_Terminal" ]]; then
    TERMINAL_ID="com.apple.Terminal"
fi

# Grant audio capture permission to the terminal where possible. The Core Audio
# process tap inherits TCC permissions from the parent terminal.
if [[ -n "$TERMINAL_ID" ]]; then
    HAS_PERM=$(sqlite3 "$TCC_DB" \
        "SELECT COUNT(*) FROM access WHERE service='kTCCServiceAudioCapture' AND client='$TERMINAL_ID' AND auth_value=2;" 2>/dev/null || echo "0")
    if [[ "$HAS_PERM" == "0" ]]; then
        echo "Granting audio capture permission to $TERMINAL_ID..."
        sqlite3 "$TCC_DB" \
            "INSERT OR REPLACE INTO access (service, client, client_type, auth_value, auth_reason, auth_version, flags) VALUES ('kTCCServiceAudioCapture', '$TERMINAL_ID', 0, 2, 0, 1, 0);" 2>/dev/null \
            && echo "Done. Restart your terminal for system audio capture to work." \
            || echo "Failed. Grant manually in System Settings > Privacy & Security > Screen & System Audio Recording."
    else
        echo "$TERMINAL_ID already has audio capture permission."
    fi
else
    echo "Could not detect terminal. Grant system audio capture manually in System Settings if needed."
fi
