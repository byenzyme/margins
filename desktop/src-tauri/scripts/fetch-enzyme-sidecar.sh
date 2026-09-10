#!/usr/bin/env bash
# Fetch the pinned enzyme engine release into src-tauri/binaries/ as a Tauri
# sidecar (externalBin). Keep the tag in sync with ENZYME_RELEASE_TAG in
# src/lib.rs. Usage: fetch-enzyme-sidecar.sh [target-triple]
set -euo pipefail

TAG="${ENZYME_RELEASE_TAG:-v0.6.2}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$SCRIPT_DIR/../binaries"

TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"

case "$TRIPLE" in
  aarch64-apple-darwin) ASSET="enzyme-macos-arm64.tar.gz" ;;
  x86_64-apple-darwin) ASSET="enzyme-macos-x86_64.tar.gz" ;;
  aarch64-unknown-linux-gnu) ASSET="enzyme-linux-arm64.tar.gz" ;;
  x86_64-unknown-linux-gnu) ASSET="enzyme-linux-x86_64.tar.gz" ;;
  *) echo "fetch-enzyme-sidecar: no enzyme release asset for target $TRIPLE" >&2; exit 1 ;;
esac

DEST="$BIN_DIR/enzyme-$TRIPLE"
if [ -x "$DEST" ] && [ "${FORCE:-0}" != "1" ]; then
  echo "fetch-enzyme-sidecar: $DEST already present (FORCE=1 to refetch)"
  exit 0
fi

URL="https://github.com/useenzyme/enzyme/releases/download/$TAG/$ASSET"
STAGING="$(mktemp -d)"
trap 'rm -rf "$STAGING"' EXIT

echo "fetch-enzyme-sidecar: downloading $URL"
curl -fsSL "$URL" -o "$STAGING/$ASSET"
curl -fsSL "$URL.sha256" -o "$STAGING/$ASSET.sha256"

EXPECTED="$(awk '{print $1}' "$STAGING/$ASSET.sha256")"
if command -v shasum >/dev/null; then
  ACTUAL="$(shasum -a 256 "$STAGING/$ASSET" | awk '{print $1}')"
else
  ACTUAL="$(sha256sum "$STAGING/$ASSET" | awk '{print $1}')"
fi
if [ "$EXPECTED" != "$ACTUAL" ]; then
  echo "fetch-enzyme-sidecar: checksum mismatch (expected $EXPECTED, got $ACTUAL)" >&2
  exit 1
fi

tar -xzf "$STAGING/$ASSET" -C "$STAGING"
[ -f "$STAGING/enzyme" ] || { echo "fetch-enzyme-sidecar: archive did not contain an enzyme binary" >&2; exit 1; }

mkdir -p "$BIN_DIR"
chmod 0755 "$STAGING/enzyme"
mv "$STAGING/enzyme" "$DEST"
echo "fetch-enzyme-sidecar: installed $DEST ($TAG)"
