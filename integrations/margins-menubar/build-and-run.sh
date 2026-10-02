#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
APP_PATH=${MARGINS_MENU_APP_PATH:-"$SCRIPT_DIR/.build/Margins Menu Test.app"}
EXECUTABLE="$APP_PATH/Contents/MacOS/MarginsMenu"
BUILD_ONLY=${1:-}
mkdir -p "$APP_PATH/Contents/MacOS"
if [ "${MARGINS_MENU_TEST_WINDOW:-0}" = 1 ]; then
  UI_ELEMENT=false
  set -- -D MARGINS_MENU_TEST_WINDOW
else
  UI_ELEMENT=true
  set --
fi
swiftc -parse-as-library -O -framework AppKit -framework SwiftUI "$@" \
  "$SCRIPT_DIR/MarginsMenu.swift" "$SCRIPT_DIR/MenuPairingServer.swift" -o "$EXECUTABLE"
cat > "$APP_PATH/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.byenzyme.margins.menu-test</string>
  <key>CFBundleName</key><string>Margins Menu Test</string>
  <key>CFBundleExecutable</key><string>MarginsMenu</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSUIElement</key><$UI_ELEMENT/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - "$APP_PATH"
if [ "$BUILD_ONLY" = "--build-only" ]; then
  printf '%s\n' "$APP_PATH"
  exit 0
fi
set -- -n
if [ "${MARGINS_MENU_REMOTE+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_REMOTE=$MARGINS_MENU_REMOTE"; fi
if [ "${MARGINS_MENU_WORKSPACE+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_WORKSPACE=$MARGINS_MENU_WORKSPACE"; fi
if [ "${MARGINS_MENU_BRIDGE_APP+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_BRIDGE_APP=$MARGINS_MENU_BRIDGE_APP"; fi
if [ "${MARGINS_MENU_SETTINGS_DOMAIN+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_SETTINGS_DOMAIN=$MARGINS_MENU_SETTINGS_DOMAIN"; fi
if [ "${MARGINS_MENU_SSH_REMOTE_BINARY+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_SSH_REMOTE_BINARY=$MARGINS_MENU_SSH_REMOTE_BINARY"; fi
if [ "${MARGINS_MENU_SSH_REMOTE_DATA_DIR+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_SSH_REMOTE_DATA_DIR=$MARGINS_MENU_SSH_REMOTE_DATA_DIR"; fi
if [ "${MARGINS_MENU_HOME+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_HOME=$MARGINS_MENU_HOME"; fi
if [ "${MARGINS_MENU_TRANSFER_DIR+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_TRANSFER_DIR=$MARGINS_MENU_TRANSFER_DIR"; fi
if [ "${MARGINS_MENU_MIC_DEVICE+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_MIC_DEVICE=$MARGINS_MENU_MIC_DEVICE"; fi
if [ "${MARGINS_MENU_TEST_WINDOW+x}" = x ]; then set -- "$@" --env "MARGINS_MENU_TEST_WINDOW=$MARGINS_MENU_TEST_WINDOW"; fi
open "$@" -a "$APP_PATH"
printf 'Started %s\n' "$APP_PATH"
