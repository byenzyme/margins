#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$("$REPO_ROOT/scripts/cargo-lane" target-dir)}"
export CARGO_TARGET_DIR

BIN_DIR="${MARGINS_BIN_DIR:-${HOME:?HOME is required}/.local/bin}"
DEST="$BIN_DIR/margins"

# Build profile selects the compiled feature set:
#   recall (default) — portable lookup only (`--features recall`), no native
#     capture/ASR/inference toolchain. The honest maximum on Linux/CI.
#   full — the default feature set (audio-capture + recall + recall-local-model
#     + coreml-asr + polyvoice-coreml). macOS source installs use this so a
#     single `margins` records, transcribes, and generates catalyst bridges.
PROFILE="${MARGINS_CLI_PROFILE:-recall}"
BUILD_ARGS=(build --release --locked)
case "$PROFILE" in
  recall) BUILD_ARGS+=(--no-default-features --features recall) ;;
  full) ;;
  *) echo "Unknown MARGINS_CLI_PROFILE '$PROFILE' (expected 'recall' or 'full')" >&2; exit 1 ;;
esac
BUILD_ARGS+=(--bin margins-private)

echo "Building official Margins CLI (profile: $PROFILE)..."
(
  cd "$REPO_ROOT"
  scripts/with-private-recall scripts/cargo-lane shared -- cargo "${BUILD_ARGS[@]}"
)

SOURCE="$CARGO_TARGET_DIR/release/margins-private"
if [ ! -x "$SOURCE" ]; then
  echo "Built CLI was not found at $SOURCE" >&2
  exit 1
fi

capability_kind() {
  local candidate="$1"
  if [ ! -x "$candidate" ]; then
    echo "missing"
    return
  fi
  local capabilities
  if ! capabilities="$("$candidate" capabilities 2>/dev/null)"; then
    echo "unknown"
    return
  fi
  python3 - "$capabilities" <<'PY'
import json
import sys

try:
    report = json.loads(sys.argv[1])
except Exception:
    print("unknown")
    raise SystemExit

recall = report.get("recall") or {}
if (
    report.get("schema") == 1
    and report.get("product") == "margins"
    and report.get("official") is True
    and recall.get("indexing") is True
    and recall.get("lookup") is True
):
    print("official-recall")
elif (
    report.get("schema") == 1
    and report.get("product") == "margins"
    and report.get("official") is False
    and recall.get("indexing") is False
    and recall.get("lookup") is False
):
    print("public-portable")
else:
    print("unknown")
PY
}

source_kind="$(capability_kind "$SOURCE")"
if [ "$source_kind" != "official-recall" ]; then
  echo "Built CLI does not report official recall capabilities: $source_kind" >&2
  exit 1
fi

mkdir -p "$BIN_DIR"
tmp="$BIN_DIR/.margins-install-$$"
cp "$SOURCE" "$tmp"
chmod 0755 "$tmp"

if [ -e "$DEST" ] || [ -L "$DEST" ]; then
  existing_kind="$(capability_kind "$DEST")"
  case "$existing_kind" in
    official-recall)
      ;;
    public-portable)
      preserved="$BIN_DIR/.margins.public-portable.$(date +%s).$$"
      if [ -e "$preserved" ] || [ -L "$preserved" ]; then
        rm -f "$tmp"
        echo "Preserved public CLI path already exists unexpectedly: $preserved" >&2
        exit 1
      fi
      mv "$DEST" "$preserved"
      echo "Preserved public portable Margins CLI at $preserved"
      ;;
    *)
      rm -f "$tmp"
      echo "$DEST already exists and is not a known Margins public/official CLI; refusing to replace it." >&2
      exit 1
      ;;
  esac
fi

mv "$tmp" "$DEST"

capabilities="$("$DEST" capabilities)"
python3 - "$capabilities" <<'PY'
import json
import sys

report = json.loads(sys.argv[1])
if report.get("schema") != 1 or report.get("product") != "margins":
    raise SystemExit("installed binary did not report Margins capabilities")
if report.get("official") is not True:
    raise SystemExit("installed binary is not the official composition")
recall = report.get("recall") or {}
if recall.get("scan") is not True or recall.get("indexing") is not True or recall.get("lookup") is not True:
    raise SystemExit("installed binary does not include recall scan, indexing, and lookup")
PY

echo "Installed official margins command at $DEST"
echo "Add $BIN_DIR to PATH if your shell cannot find margins."
