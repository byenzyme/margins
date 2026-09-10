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
  test "$(tar -tzf "$archive")" = margins
done

test "$(find "$directory" -maxdepth 1 -type f | wc -l | tr -d ' ')" = 4
