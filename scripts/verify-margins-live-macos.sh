#!/usr/bin/env bash
set -euo pipefail

if [[ $# -lt 1 || $# -gt 2 || ( $# -eq 2 && "$2" != "--signed" ) ]]; then
  echo "usage: $0 /path/to/margins-live [--signed]" >&2
  exit 2
fi

binary="$1"
if [[ ! -x "$binary" ]]; then
  echo "margins-live is missing or not executable: $binary" >&2
  exit 1
fi

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "margins-live Mach-O metadata can only be verified on macOS" >&2
  exit 1
fi

file "$binary" | grep -Fq 'Mach-O'

for key in NSMicrophoneUsageDescription NSAudioCaptureUsageDescription; do
  value="$(plutil -extract "$key" raw -o - "$binary")"
  if [[ -z "$value" ]]; then
    echo "margins-live has an empty $key" >&2
    exit 1
  fi
done

identifier="$(plutil -extract CFBundleIdentifier raw -o - "$binary")"
if [[ "$identifier" != "com.byenzyme.margins.live" ]]; then
  echo "margins-live has an unexpected CFBundleIdentifier: $identifier" >&2
  exit 1
fi

if [[ "${2:-}" == "--signed" ]]; then
  entitlements="$(codesign --display --entitlements :- "$binary" 2>/dev/null | tr -d '[:space:]')"
  if [[ "$entitlements" != *'<key>com.apple.security.device.audio-input</key><true/>'* ]]; then
    echo "margins-live is signed without the Hardened Runtime audio-input entitlement" >&2
    exit 1
  fi
fi

echo "margins-live contains its macOS audio permission descriptions"
