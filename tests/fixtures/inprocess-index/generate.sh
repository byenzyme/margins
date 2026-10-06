#!/usr/bin/env bash
# Regenerate enzyme.db.gz and index.identity: the index the in-process Margins
# (#13, commit 71c73b5, private engine 624e539) built for workspace "legacy"
# over ./notes with the fixture generator.
#
#   MARGINS_LEGACY_INPROCESS_BIN=/path/to/margins-private \
#     tests/fixtures/inprocess-index/generate.sh
#
# Build that binary from 71c73b5 with
# `scripts/with-private-recall scripts/cargo-lane shared -- cargo build
#  --no-default-features --features recall --bin margins-private`.
set -euo pipefail
: "${MARGINS_LEGACY_INPROCESS_BIN:?set MARGINS_LEGACY_INPROCESS_BIN}"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
root="$(mktemp -d)"
trap 'kill "${mock:-0}" 2>/dev/null || true; rm -rf "$root"' EXIT
python3 "$repo/tests/fixture_openai_server.py" --port-file "$root/port" >/dev/null 2>&1 &
mock=$!
for _ in $(seq 1 600); do [[ -s "$root/port" ]] && break; sleep 0.025; done
export HOME="$root/home" MARGINS_HOME="$root/margins-home" ENZYME_HOME="$root/no-enzyme-home"
mkdir -p "$HOME" "$MARGINS_HOME/configs"
cp -R "$here/notes" "$root/notes"
"$MARGINS_LEGACY_INPROCESS_BIN" workspace new legacy --home "$root/notes" --json >/dev/null
printf '{"api_key":"fixture-key","base_url":"http://127.0.0.1:%s/v1","model":"fixture-catalyst-model","expires_at":4102444800,"cached_at":1,"profile":"fixture-profile"}\n' \
  "$(cat "$root/port")" > "$MARGINS_HOME/llm-config-cache.json"
chmod 600 "$MARGINS_HOME/llm-config-cache.json"
printf 'settings {\n  generation hosted\n  updates disabled\n}\n' > "$MARGINS_HOME/configs/settings.enzyme"
"$MARGINS_LEGACY_INPROCESS_BIN" --workspace legacy init >/dev/null
state="$MARGINS_HOME/workspaces/legacy"
sqlite3 "$state/enzyme.db" "VACUUM INTO '$root/enzyme.db'"
gzip -9 -n -c "$root/enzyme.db" > "$here/enzyme.db.gz"
cp "$state/index.identity" "$here/index.identity"
echo "wrote $here/enzyme.db.gz and $here/index.identity"
