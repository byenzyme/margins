#!/usr/bin/env bash
# Prove that an installed margins finds and version-checks the enzyme engine
# shipped with it, with no MARGINS_ENZYME_BIN set.
#
# Everything runs inside one temporary root (HOME, TMPDIR, XDG_*, MARGINS_HOME);
# a local OpenAI-compatible fixture is the only generator. It proves:
#   1. archive layout: margins and enzyme packed and extracted the way the
#      release action does; setup -> init -> recall runs the sibling enzyme;
#   2. install.sh layout: scripts/install-official-cli.sh under a temp HOME
#      puts margins in ~/.local/bin and the engine in
#      ~/.local/libexec/margins/enzyme (never ~/.local/bin/enzyme), taking it
#      through scripts/enzyme-pin fetch with its sha256 and version checks;
#      init and recall run that engine;
#   3. an enzyme in ~/.local/bin (a user's own install, any version) is never
#      used by the installed margins, even when the libexec engine is missing
#      and the user's reports the pinned version;
#   4. an engine reporting another version is refused with an error naming
#      both versions, and nothing is indexed.
#
# The engine is scripts/enzyme-bin's build of the pinned rev, packed as a local
# release asset and pinned through a temporary copy of the pin (same rev and
# version, a sha256 for this host), so no published enzyme release is needed.
#
# Usage: scripts/e2e-bundled-engine.sh
#   MARGINS_E2E_BIN     margins binary built with recall (required)
#   MARGINS_ENZYME_BIN  enzyme binary to bundle (default: scripts/enzyme-bin);
#                       never set for the installed margins
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
: "${MARGINS_E2E_BIN:?set MARGINS_E2E_BIN to a margins binary built with recall}"
ENZYME_BIN="$("$REPO_ROOT/scripts/enzyme-bin")"
PIN_VERSION="$("$REPO_ROOT/scripts/enzyme-pin" get version)"
HOST_TARGET="$("$REPO_ROOT/scripts/enzyme-pin" host-target)"
PHRASE="The quartz harbor ledger records every crossing tonight"

ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-bundled-engine.XXXXXX")"
ROOT="$(cd "$ROOT" && pwd -P)"
MOCK_PID=""
cleanup() {
  if [[ -n "$MOCK_PID" ]]; then kill "$MOCK_PID" 2>/dev/null || true; fi
  rm -rf "$ROOT"
}
trap cleanup EXIT

fail() { echo "bundled-engine: FAIL: $*" >&2; exit 1; }
pass() { echo "bundled-engine: ok: $*"; }

FIXTURE="$ROOT/fixture"
mkdir -p "$FIXTURE"
python3 "$REPO_ROOT/tests/fixture_openai_server.py" \
  --port-file "$FIXTURE/port" --count-file "$FIXTURE/count" >"$FIXTURE/log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 600); do
  [[ -s "$FIXTURE/port" ]] && break
  kill -0 "$MOCK_PID" 2>/dev/null || { cat "$FIXTURE/log" >&2; fail "fixture generator exited"; }
  sleep 0.025
done
PORT="$(cat "$FIXTURE/port")"

export HOME="$ROOT/home"
export TMPDIR="$ROOT/tmp"
export XDG_CONFIG_HOME="$ROOT/xdg/config" XDG_CACHE_HOME="$ROOT/xdg/cache"
export XDG_DATA_HOME="$ROOT/xdg/data" XDG_STATE_HOME="$ROOT/xdg/state"
export XDG_RUNTIME_DIR="$ROOT/xdg/run"
export MARGINS_RECALL_DEBUG=1
unset MARGINS_ENZYME_BIN MARGINS_WORKSPACE ENZYME_HOME OPENAI_API_KEY OPENAI_BASE_URL OPENAI_MODEL || true
mkdir -p "$HOME" "$TMPDIR" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" \
  "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

# Setup -> init -> recall with one installed margins and its own Margins home.
# Asserts every engine call went to EXPECTED_ENGINE.
exercise() {
  local label="$1" margins="$2" expected_engine="$3" home="$ROOT/margins-home-$1"
  local notes="$ROOT/notes-$1" work="$ROOT/work-$1"
  mkdir -p "$notes/projects" "$work"
  printf '# Harbor\n\n%s.\n' "$PHRASE" > "$notes/projects/harbor.md"
  printf '# Garden\n\nTomatoes need afternoon shade in July.\n' > "$notes/garden.md"
  export MARGINS_HOME="$home"
  "$margins" workspace new practice --home "$notes" --json >/dev/null \
    || fail "$label: workspace new"
  cat > "$work/desired.enzyme" <<EOF
workspace "practice" {
  source markdown "notes" { path "$notes" }
  remember in folder "Meetings" create note
}
EOF
  "$margins" --workspace practice workspace plan --desired "$work/desired.enzyme" --json \
    > "$work/plan.json" || fail "$label: workspace plan"
  "$margins" --workspace practice workspace apply --plan "$work/plan.json" --json >/dev/null \
    || fail "$label: workspace apply"
  python3 - "$home" "$PORT" <<'PY'
import json, os, pathlib, sys
home, port = pathlib.Path(sys.argv[1]), int(sys.argv[2])
bundle = home / "llm-config-cache.json"
bundle.write_text(json.dumps({
    "api_key": "fixture-key", "base_url": f"http://127.0.0.1:{port}/v1",
    "model": "fixture-catalyst-model", "expires_at": 4102444800, "cached_at": 1,
    "profile": "fixture-profile",
}) + "\n")
os.chmod(bundle, 0o600)
(home / "configs" / "settings.enzyme").write_text("settings {\n  generation hosted\n  updates disabled\n}\n")
PY
  "$margins" --workspace practice init > "$work/init.out" 2> "$work/init.err" \
    || { cat "$work/init.err" >&2; fail "$label: margins init"; }
  [[ -f "$home/workspaces/practice/enzyme.db" ]] || fail "$label: no index at MARGINS_HOME/workspaces/practice"
  "$margins" --workspace practice recall --json "$PHRASE" > "$work/recall.json" 2> "$work/recall.err" \
    || { cat "$work/recall.err" >&2; fail "$label: margins recall"; }
  python3 - "$work/recall.json" <<'PY' || fail "$label: recall did not return the planted note"
import json, sys
recall = json.load(open(sys.argv[1]))
assert recall["status"] == "ok", recall
assert any(hit["document_ref"] == "projects/harbor.md" for hit in recall["results"]), recall
PY
  # MARGINS_RECALL_DEBUG prints `recall: enzyme <engine> <args>` for every call.
  python3 - "$expected_engine" "$work/init.err" "$work/recall.err" <<'PY' \
    || fail "$label: an engine call did not use $expected_engine"
import sys
expected, logs = sys.argv[1], sys.argv[2:]
prefix = "recall: enzyme /"
calls = [line[len(prefix) - 1:].split(" ", 1)[0]
         for log in logs for line in open(log) if line.startswith(prefix)]
assert calls, "no engine calls were logged"
others = sorted({call for call in calls if call != expected})
assert not others, f"engine calls went to {others}"
print(f"  {len(calls)} engine calls, all to {expected}")
PY
}

# --- 1. Archive layout ----------------------------------------------------------
PACK="$ROOT/pack"
mkdir -p "$PACK/archive" "$ROOT/archive"
install -m 0755 "$MARGINS_E2E_BIN" "$PACK/archive/margins"
install -m 0755 "$ENZYME_BIN" "$PACK/archive/enzyme"
tar -czf "$PACK/margins.tar.gz" -C "$PACK/archive" .
tar -xzf "$PACK/margins.tar.gz" -C "$ROOT/archive"
exercise archive "$ROOT/archive/margins" "$ROOT/archive/enzyme"
pass "1 archive layout: setup -> init -> recall through the sibling enzyme"

# --- 2. install.sh layout ----------------------------------------------------------
# The bundled build as a local release asset, pinned by a temporary pin with
# the real rev and version and this host's sha256.
mkdir -p "$PACK/asset"
install -m 0755 "$ENZYME_BIN" "$PACK/asset/enzyme"
ASSET="$PACK/$("$REPO_ROOT/scripts/enzyme-pin" asset "$HOST_TARGET")"
tar -czf "$ASSET" -C "$PACK/asset" enzyme
# sha256sum is GNU coreutils; macOS ships shasum.
ASSET_SHA="$(python3 -c 'import hashlib, sys; print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())' "$ASSET")"
[[ "$ASSET_SHA" =~ ^[0-9a-f]{64}$ ]] || fail "cannot hash $ASSET"
sed -e "s/^sha256\.$HOST_TARGET = .*/sha256.$HOST_TARGET = \"$ASSET_SHA\"/" \
  "$REPO_ROOT/scripts/enzyme-cli.pin" > "$PACK/enzyme-cli.pin"
grep -q "^sha256\.$HOST_TARGET = \"$ASSET_SHA\"" "$PACK/enzyme-cli.pin" \
  || fail "the pin has no sha256.$HOST_TARGET line to override"

MARGINS_CLI_SOURCE="$MARGINS_E2E_BIN" MARGINS_ENZYME_ASSET="$ASSET" \
  MARGINS_ENZYME_PIN="$PACK/enzyme-cli.pin" \
  "$REPO_ROOT/scripts/install-official-cli.sh" > "$PACK/install.out" 2>&1 \
  || { cat "$PACK/install.out" >&2; fail "install-official-cli.sh"; }
INSTALLED="$HOME/.local/bin/margins"
ENGINE="$HOME/.local/libexec/margins/enzyme"
[[ -x "$INSTALLED" ]] || fail "install.sh did not install $INSTALLED"
[[ -x "$ENGINE" ]] || fail "install.sh did not install $ENGINE"
[[ ! -e "$HOME/.local/bin/enzyme" ]] || fail "install.sh put enzyme on PATH"
[[ "$("$ENGINE" --version)" == "enzyme $PIN_VERSION" ]] || fail "installed engine is not enzyme $PIN_VERSION"
exercise local "$INSTALLED" "$ENGINE"
pass "2 install.sh layout: ~/.local/bin/margins runs ~/.local/libexec/margins/enzyme"

# --- 3. A user's own enzyme in ~/.local/bin is never used -------------------------
printf '#!/bin/sh\necho "enzyme 99.0.0"\n' > "$HOME/.local/bin/enzyme"
chmod 0755 "$HOME/.local/bin/enzyme"
exercise user-enzyme "$INSTALLED" "$ENGINE"
# Even one reporting the pinned version, with the installed engine gone.
printf '#!/bin/sh\necho "enzyme %s"\n' "$PIN_VERSION" > "$HOME/.local/bin/enzyme"
mv "$ENGINE" "$PACK/engine.keep"
export MARGINS_HOME="$ROOT/margins-home-local"
if "$INSTALLED" --workspace practice init > "$ROOT/missing.out" 2> "$ROOT/missing.err"; then
  fail "init ran without the installed engine"
fi
grep -qF "$ENGINE (missing)" "$ROOT/missing.err" \
  || { cat "$ROOT/missing.err" >&2; fail "the refusal does not name the missing libexec engine"; }
! grep -qF "$HOME/.local/bin/enzyme" "$ROOT/missing.err" \
  || { cat "$ROOT/missing.err" >&2; fail "margins considered the enzyme beside it"; }
mv "$PACK/engine.keep" "$ENGINE"
rm "$HOME/.local/bin/enzyme"
pass "3 an enzyme beside the installed margins is never used, even at the pinned version"

# --- 4. A mismatched engine is refused ----------------------------------------------
STALE="$ROOT/stale"
mkdir -p "$STALE/bin" "$STALE/libexec/margins"
install -m 0755 "$MARGINS_E2E_BIN" "$STALE/bin/margins"
printf '#!/bin/sh\necho "enzyme 0.0.1"\n' > "$STALE/libexec/margins/enzyme"
chmod 0755 "$STALE/libexec/margins/enzyme"
export MARGINS_HOME="$ROOT/margins-home-stale"
mkdir -p "$ROOT/notes-stale"
printf '# Harbor\n\n%s.\n' "$PHRASE" > "$ROOT/notes-stale/harbor.md"
"$STALE/bin/margins" workspace new practice --home "$ROOT/notes-stale" --json >/dev/null \
  || fail "stale: workspace new"
if "$STALE/bin/margins" --workspace practice init > "$ROOT/stale.out" 2> "$ROOT/stale.err"; then
  fail "init ran with an engine reporting enzyme 0.0.1"
fi
if ! grep -q "0.0.1" "$ROOT/stale.err" || ! grep -qF "$PIN_VERSION" "$ROOT/stale.err"; then
  cat "$ROOT/stale.err" >&2
  fail "the refusal does not name both versions"
fi
[[ ! -e "$MARGINS_HOME/workspaces/practice/enzyme.db" ]] || fail "a refused engine left an index"
pass "4 enzyme 0.0.1 refused: $(grep -m1 -F "$PIN_VERSION" "$ROOT/stale.err")"
