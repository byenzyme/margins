#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_PATH=${MARGINS_MENU_APP_PATH:-"$SCRIPT_DIR/.build/Margins Menu Test.app"}
EXECUTABLE="$APP_PATH/Contents/MacOS/MarginsMenu"
mkdir -p "$APP_PATH/Contents/MacOS"
swiftc -parse-as-library -O -framework AppKit -framework SwiftUI \
  "$SCRIPT_DIR/MarginsMenu.swift" -o "$EXECUTABLE"
cat > "$APP_PATH/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.byenzyme.margins.menu-test</string>
  <key>CFBundleName</key><string>Margins Menu Test</string>
  <key>CFBundleExecutable</key><string>MarginsMenu</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP_PATH"
if [ "${1:-}" = "--build-only" ]; then
  printf '%s\n' "$APP_PATH"
  exit 0
fi
set -- -n
if [ "${MARGINS_MENU_REMOTE+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_REMOTE=$MARGINS_MENU_REMOTE"; fi
if [ "${MARGINS_MENU_WORKSPACE+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_WORKSPACE=$MARGINS_MENU_WORKSPACE"; fi
if [ "${MARGINS_MENU_BRIDGE_APP+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_BRIDGE_APP=$MARGINS_MENU_BRIDGE_APP"; fi
if [ "${MARGINS_MENU_LOCAL_DISCOVERY+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_LOCAL_DISCOVERY=$MARGINS_MENU_LOCAL_DISCOVERY"; fi
if [ "${MARGINS_MENU_TRANSCRIBE_BIN+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_TRANSCRIBE_BIN=$MARGINS_MENU_TRANSCRIBE_BIN"; fi
if [ "${MARGINS_MENU_TRANSCRIBE_VAULT+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_TRANSCRIBE_VAULT=$MARGINS_MENU_TRANSCRIBE_VAULT"; fi
if [ "${MARGINS_MENU_TRANSCRIBE_HOME+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_TRANSCRIBE_HOME=$MARGINS_MENU_TRANSCRIBE_HOME"; fi
if [ "${MARGINS_MENU_SSH_REMOTE_BINARY+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_SSH_REMOTE_BINARY=$MARGINS_MENU_SSH_REMOTE_BINARY"; fi
if [ "${MARGINS_MENU_SSH_REMOTE_DATA_DIR+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_SSH_REMOTE_DATA_DIR=$MARGINS_MENU_SSH_REMOTE_DATA_DIR"; fi
if [ "${MARGINS_MENU_HOME+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_HOME=$MARGINS_MENU_HOME"; fi
open "$@" -a "$APP_PATH"
printf 'Started %s\n' "$APP_PATH"
