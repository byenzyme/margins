#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 4 ]]; then
  echo "usage: $0 VERSION SIGNED_MARGINS_BINARY DEVELOPER_ID KEYCHAIN" >&2
  exit 2
fi

version=$1
binary=$2
identity=$3
keychain=$4
script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repo_dir=$(cd "$script_dir/../.." && pwd)
if [[ ! -f "$binary" ]]; then
  echo "missing signed Margins binary: $binary" >&2
  exit 1
fi
binary=$(cd "$(dirname "$binary")" && pwd)/$(basename "$binary")
archive="$PWD/Margins-${version}-macos-arm64.zip"
work=$(mktemp -d)
cleanup() {
  status=$?
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
app="$work/Margins.app"
helper="$app/Contents/Helpers/Margins Capture.app"
mkdir -p "$app/Contents/MacOS" "$helper/Contents/MacOS"

swiftc -parse-as-library -O -target arm64-apple-macos14.0 \
  -framework AppKit -framework SwiftUI \
  "$script_dir/MarginsMenu.swift" "$script_dir/MenuPairingServer.swift" \
  -o "$app/Contents/MacOS/MarginsMenu"

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>com.byenzyme.margins</string>
  <key>CFBundleName</key><string>Margins</string>
  <key>CFBundleDisplayName</key><string>Margins</string>
  <key>CFBundleExecutable</key><string>MarginsMenu</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>LSMinimumSystemVersion</key><string>14.0</string>
  <key>LSUIElement</key><true/>
  <key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST

cp "$repo_dir/src/cli/MarginsNativeBridge-Info.plist" "$helper/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Add :CFBundleExecutable string margins' "$helper/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Add :CFBundlePackageType string APPL' "$helper/Contents/Info.plist"
/usr/libexec/PlistBuddy -c 'Add :LSMinimumSystemVersion string 14.0' "$helper/Contents/Info.plist"
install -m 0755 "$binary" "$helper/Contents/MacOS/margins"

MARGINS_SIGN_KEYCHAIN="$keychain" \
  "$script_dir/sign-bundled-recorders.sh" "$app" "$identity"

for name in APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID; do
  if [[ -z "${!name:-}" ]]; then
    echo "missing notarization credential: $name" >&2
    exit 1
  fi
done
ditto -c -k --keepParent "$app" "$work/notarization.zip"
if ! xcrun notarytool submit "$work/notarization.zip" \
  --apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" \
  --team-id "$APPLE_TEAM_ID" --wait --output-format json \
  > "$work/notarization.json"; then
  cat "$work/notarization.json"
  echo "Apple notarization submission failed" >&2
  exit 1
fi
cat "$work/notarization.json"
notarization_status=$(plutil -extract status raw -o - "$work/notarization.json")
if [[ "$notarization_status" != Accepted ]]; then
  echo "Apple notarization status was $notarization_status, expected Accepted" >&2
  exit 1
fi
xcrun stapler staple "$app"
xcrun stapler validate "$app"
codesign --verify --deep --strict --verbose=2 "$app"
spctl --assess --type execute --verbose=4 "$app"

ditto -c -k --keepParent "$app" "$archive"
(cd "$(dirname "$archive")" && shasum -a 256 "$(basename "$archive")") > "${archive}.sha256"
echo "Packaged $archive"
