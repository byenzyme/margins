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
if [[ ! -d "$capture" ]]; then
  echo "missing bundled recorder: $capture" >&2
  exit 1
fi

embedded_plist="$(otool -P "$capture/Contents/MacOS/margins" | sed -n '/^<?xml /,$p')"
embedded_id="$(plutil -extract CFBundleIdentifier raw -o - - <<<"$embedded_plist")"
bundle_id="$(plutil -extract CFBundleIdentifier raw -o - "$capture/Contents/Info.plist")"
if [[ "$embedded_id" != "$bundle_id" ]]; then
  echo "capture executable and app bundle IDs differ: $embedded_id != $bundle_id" >&2
  exit 1
fi

# Hardened Runtime silently prevents microphone authorization without this
# entitlement. Sign the nested apps first, then reseal the parent bundle.
signing_args=(--force --options runtime --timestamp --sign "$identity")
if [[ -n "${MARGINS_SIGN_KEYCHAIN:-}" ]]; then
  signing_args+=(--keychain "$MARGINS_SIGN_KEYCHAIN")
fi
codesign "${signing_args[@]}" \
  --entitlements "$repo_dir/src/cli/MarginsNativeBridge.entitlements" "$capture"
codesign "${signing_args[@]}" \
  --entitlements "$repo_dir/src/cli/MarginsNativeBridge.entitlements" "$app"

entitlements="$(codesign --display --entitlements :- "$capture" 2>/dev/null | tr -d '[:space:]')"
if [[ "$entitlements" != *'<key>com.apple.security.device.audio-input</key><true/>'* ]]; then
  echo "microphone entitlement missing after signing: $capture" >&2
  exit 1
fi
entitlements="$(codesign --display --entitlements :- "$app" 2>/dev/null | tr -d '[:space:]')"
if [[ "$entitlements" != *'<key>com.apple.security.device.audio-input</key><true/>'* ]]; then
  echo "microphone entitlement missing after signing: $app" >&2
  exit 1
fi
codesign --verify --deep --strict --verbose=2 "$app"
echo "Bundled capture helper signed with microphone access: $app"
