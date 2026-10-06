#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$("$REPO_ROOT/scripts/cargo-lane" target-dir)}"
export CARGO_TARGET_DIR

BIN_DIR="${MARGINS_BIN_DIR:-${HOME:?HOME is required}/.local/bin}"
DEST="$BIN_DIR/margins"
# margins finds its pinned engine at <prefix>/libexec/margins/enzyme, off PATH,
# so it never replaces or shadows an `enzyme` the user installed.
ENGINE_DIR="$(dirname "$BIN_DIR")/libexec/margins"

# Build profile selects the compiled feature set:
#   recall (default) — portable lookup only (`--features recall`), no native
#     capture/ASR/inference toolchain. The honest maximum on Linux/CI.
#   full — audio-capture + recall + recall-local-model + coreml-asr +
#     polyvoice-coreml. macOS source installs use this so a
#     single `margins` records, transcribes, and generates catalyst bridges.
PROFILE="${MARGINS_CLI_PROFILE:-recall}"
BUILD_ARGS=(build --release --locked)
case "$PROFILE" in
  recall) BUILD_ARGS+=(--no-default-features --features recall) ;;
  full)
    BUILD_ARGS+=(
      --no-default-features
      --features audio-capture,coreml-asr,polyvoice-coreml,recall,recall-local-model
    )
    ;;
  *) echo "Unknown MARGINS_CLI_PROFILE '$PROFILE' (expected 'recall' or 'full')" >&2; exit 1 ;;
esac
BUILD_ARGS+=(--bin margins-private)

# The engine margins runs: the official enzyme release asset in
# scripts/enzyme-cli.pin, or an explicit MARGINS_ENZYME_BIN (for example
# "$(scripts/enzyme-bin)" before that release exists). Either must report the
# pinned version.
engine_stage="$(mktemp -d "${TMPDIR:-/tmp}/margins-engine.XXXXXX")"
trap 'rm -rf "$engine_stage"' EXIT
if [ -n "${MARGINS_ENZYME_BIN:-}" ]; then
  "$REPO_ROOT/scripts/enzyme-pin" check "$MARGINS_ENZYME_BIN"
  install -m 0755 "$MARGINS_ENZYME_BIN" "$engine_stage/enzyme"
else
  "$REPO_ROOT/scripts/enzyme-pin" fetch "$("$REPO_ROOT/scripts/enzyme-pin" host-target)" "$engine_stage" || {
    echo "Could not install the pinned enzyme engine. To use a local build, rerun with" >&2
    echo "  MARGINS_ENZYME_BIN=\"\$(scripts/cargo-lane shared -- scripts/enzyme-bin)\"" >&2
    exit 1
  }
fi

# MARGINS_CLI_SOURCE installs an already-built margins-private instead of
# building one (the bundled-engine e2e uses the gate's build).
if [ -n "${MARGINS_CLI_SOURCE:-}" ]; then
  SOURCE="$MARGINS_CLI_SOURCE"
else
  echo "Building official Margins CLI (profile: $PROFILE)..."
  (
    cd "$REPO_ROOT"
    scripts/cargo-lane shared -- cargo "${BUILD_ARGS[@]}"
  )
  SOURCE="$CARGO_TARGET_DIR/release/margins-private"
fi
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
mkdir -p "$ENGINE_DIR"
install -m 0755 "$engine_stage/enzyme" "$ENGINE_DIR/.enzyme-install-$$"
mv "$ENGINE_DIR/.enzyme-install-$$" "$ENGINE_DIR/enzyme"

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
if recall.get("indexing") is not True or recall.get("lookup") is not True:
    raise SystemExit("installed binary does not include recall indexing and lookup")
PY

echo "Installed official margins command at $DEST"
echo "Installed its enzyme engine at $ENGINE_DIR/enzyme"
echo "Add $BIN_DIR to PATH if your shell cannot find margins."
