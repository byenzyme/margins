#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 2 ]]; then
  echo "usage: $0 '/path/to/Margins Menu.app' 'Developer ID Application: Name (TEAM)'" >&2
  exit 2
fi

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "$script_dir/../.." && pwd)"
app="$1"
identity="$2"
capture="$app/Contents/Helpers/Margins Capture.app"
live="$app/Contents/Helpers/Margins Live.app"

for helper in "$capture" "$live"; do
  if [[ ! -d "$helper" ]]; then
    echo "missing bundled recorder: $helper" >&2
    exit 1
  fi
done

embedded_plist="$(otool -P "$capture/Contents/MacOS/margins" | sed -n '/^<?xml /,$p')"
embedded_id="$(plutil -extract CFBundleIdentifier raw -o - - <<<"$embedded_plist")"
bundle_id="$(plutil -extract CFBundleIdentifier raw -o - "$capture/Contents/Info.plist")"
if [[ "$embedded_id" != "$bundle_id" ]]; then
  echo "capture executable and app bundle IDs differ: $embedded_id != $bundle_id" >&2
  exit 1
fi

# Hardened Runtime silently prevents microphone authorization without this
# entitlement. Sign the nested apps first, then reseal the parent bundle.
codesign --force --options runtime --sign "$identity" \
  --entitlements "$repo_dir/src/cli/MarginsNativeBridge.entitlements" "$capture"
codesign --force --options runtime --sign "$identity" \
  --entitlements "$repo_dir/desktop/src-tauri/MarginsLive.entitlements" "$live"
codesign --force --options runtime --sign "$identity" \
  --entitlements "$repo_dir/src/cli/MarginsNativeBridge.entitlements" "$app"

for helper in "$capture" "$live"; do
  entitlements="$(codesign --display --entitlements :- "$helper" 2>/dev/null | tr -d '[:space:]')"
  if [[ "$entitlements" != *'<key>com.apple.security.device.audio-input</key><true/>'* ]]; then
    echo "microphone entitlement missing after signing: $helper" >&2
    exit 1
  fi
done
codesign --verify --deep --strict --verbose=2 "$app"
echo "Bundled recorders signed with microphone access: $app"
