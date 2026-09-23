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
open -n -a "$APP_PATH"
printf 'Started %s\n' "$APP_PATH"
