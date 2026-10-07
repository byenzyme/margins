#!/usr/bin/env bash
set -euo pipefail

directory=${1:?usage: verify-release-artifacts.sh DIRECTORY VERSION}
version=${2:?usage: verify-release-artifacts.sh DIRECTORY VERSION}

for target in aarch64-apple-darwin x86_64-unknown-linux-gnu; do
  archive="$directory/margins-${version}-${target}.tar.gz"
  checksum="${archive}.sha256"
  test -s "$archive"
  test -s "$checksum"
  expected=$(cut -d ' ' -f1 "$checksum")
  actual=$(sha256sum "$archive" | cut -d ' ' -f1)
  test "$expected" = "$actual" || {
    echo "checksum mismatch: $archive" >&2
    exit 1
  }
  expected=$'enzyme\nmargins\nmargins-server'
  actual_entries=$(tar -tzf "$archive" | sed -e '/^\.$/d' -e '/^\.\/$/d' -e 's#^\./##' | LC_ALL=C sort)
  expected_entries=$(printf '%s\n' "$expected" | LC_ALL=C sort)
  test "$actual_entries" = "$expected_entries" || {
    echo "unexpected executables in $archive" >&2
    exit 1
  }
done

app_archive="$directory/Margins-${version}-macos-arm64.zip"
app_checksum="${app_archive}.sha256"
test -s "$app_archive"
test -s "$app_checksum"
expected=$(cut -d ' ' -f1 "$app_checksum")
actual=$(sha256sum "$app_archive" | cut -d ' ' -f1)
test "$expected" = "$actual" || {
  echo "checksum mismatch: $app_archive" >&2
  exit 1
}
app_entries=$(python3 - "$app_archive" <<'PY'
import sys
import zipfile

with zipfile.ZipFile(sys.argv[1]) as archive:
    print("\n".join(archive.namelist()))
PY
)
for entry in \
  'Margins.app/Contents/Info.plist' \
  'Margins.app/Contents/MacOS/MarginsMenu' \
  'Margins.app/Contents/Helpers/Margins Capture.app/Contents/Info.plist' \
  'Margins.app/Contents/Helpers/Margins Capture.app/Contents/MacOS/margins'; do
  grep -Fxq "$entry" <<< "$app_entries" || {
    echo "missing $entry in $app_archive" >&2
    exit 1
  }
done

test "$(find "$directory" -maxdepth 1 -type f | wc -l | tr -d ' ')" = 6
