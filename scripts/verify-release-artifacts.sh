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
  expected=$'margins\nmargins-server'
  if [[ "$target" == aarch64-apple-darwin ]]; then
    expected+=$'\nmargins-live'
  fi
  actual_entries=$(tar -tzf "$archive" | sed -e '/^\.\/?$/d' -e 's#^\./##' | LC_ALL=C sort)
  expected_entries=$(printf '%s\n' "$expected" | LC_ALL=C sort)
  test "$actual_entries" = "$expected_entries" || {
    echo "unexpected executables in $archive" >&2
    exit 1
  }
done

test "$(find "$directory" -maxdepth 1 -type f | wc -l | tr -d ' ')" = 4
