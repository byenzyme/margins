#!/usr/bin/env bash
# Prove that Margins drives the shipped `enzyme` CLI inside the Margins home
# and nowhere else.
#
# Runs the real `margins` and `enzyme` binaries with HOME, TMPDIR, XDG_*, and
# MARGINS_HOME all inside one temporary root, a poisoned ENZYME_HOME exported
# by the parent shell, and a canary `~/.enzyme` (program, unreadable
# auth.json, index, models). A local OpenAI-compatible fixture is the only
# network. It proves:
#   1. setup -> init -> recall works end to end through `enzyme`;
#   2. `ENZYME_HOME=$MARGINS_HOME enzyme --workspace <id> search` returns the
#      hits `margins recall` returns, from the same index;
#   3. the canary ~/.enzyme is byte-identical (contents, modes, mtimes);
#   4. the only writes are under MARGINS_HOME, the declared create-note
#      folder, and TMPDIR scratch that is gone afterwards (no
#      /tmp/enzyme-api-cache, no .enzyme in the notes folder);
#   5. auth.json is never read (mode 000) and the parent ENZYME_HOME is
#      overridden;
#   6. a background catalyst worker spawned by `enzyme refresh` keeps 3-4;
#   7. an index built by the in-process Margins (#13) is reused by the CLI
#      with every document row kept (no re-embedding), and recall still works.
#
# Usage: scripts/e2e-enzyme-home-isolation.sh
#   MARGINS_E2E_BIN  margins binary built with recall (required)
#   MARGINS_ENZYME_BIN  enzyme binary (default: scripts/enzyme-bin)
#   MARGINS_LEGACY_INPROCESS_BIN  optional #13-era margins binary; builds the
#     legacy index live instead of using tests/fixtures/inprocess-index.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "$(id -u)" == 0 ]]; then
  # Mode 000 does not stop root, so the auth.json canary (item 5) would prove
  # nothing. Run the gate as an ordinary user.
  echo "isolation: FAIL: run as a non-root user; root can read the mode-000 auth.json canary" >&2
  exit 1
fi
: "${MARGINS_E2E_BIN:?set MARGINS_E2E_BIN to a margins binary built with recall}"
MARGINS_BIN="$(cd "$(dirname "$MARGINS_E2E_BIN")" && pwd)/$(basename "$MARGINS_E2E_BIN")"
ENZYME_BIN="$("$REPO_ROOT/scripts/enzyme-bin")"
PHRASE="The quartz harbor ledger records every crossing tonight"

ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-isolation.XXXXXX")"
ROOT="$(cd "$ROOT" && pwd -P)"
MOCK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/margins-isolation-mock.XXXXXX")"
MOCK_PID=""
cleanup() {
  if [[ -n "$MOCK_PID" ]]; then kill "$MOCK_PID" 2>/dev/null || true; fi
  chmod -R u+rwX "$ROOT" 2>/dev/null || true
  rm -rf "$ROOT" "$MOCK_DIR"
}
trap cleanup EXIT

fail() { echo "isolation: FAIL: $*" >&2; exit 1; }
pass() { echo "isolation: ok: $*"; }

# A host-wide hosted lease cache must not appear (or change) during the run.
API_CACHE=/tmp/enzyme-api-cache
api_cache_state() { stat -c '%Y %s' "$API_CACHE" 2>/dev/null || echo absent; }
API_CACHE_BEFORE="$(api_cache_state)"

python3 "$REPO_ROOT/tests/fixture_openai_server.py" \
  --port-file "$MOCK_DIR/port" --count-file "$MOCK_DIR/count" >"$MOCK_DIR/log" 2>&1 &
MOCK_PID=$!
for _ in $(seq 1 600); do
  [[ -s "$MOCK_DIR/port" ]] && break
  kill -0 "$MOCK_PID" 2>/dev/null || { cat "$MOCK_DIR/log" >&2; fail "fixture generator exited"; }
  sleep 0.025
done
PORT="$(cat "$MOCK_DIR/port")"

# --- The temp root -----------------------------------------------------------
export HOME="$ROOT/home"
export TMPDIR="$ROOT/tmp"
export XDG_CONFIG_HOME="$ROOT/xdg/config" XDG_CACHE_HOME="$ROOT/xdg/cache"
export XDG_DATA_HOME="$ROOT/xdg/data" XDG_STATE_HOME="$ROOT/xdg/state"
export XDG_RUNTIME_DIR="$ROOT/xdg/run"
export MARGINS_HOME="$ROOT/margins-home"
export MARGINS_ENZYME_BIN="$ENZYME_BIN"
export MARGINS_RECALL_DEBUG=1
unset MARGINS_WORKSPACE OPENAI_API_KEY OPENAI_BASE_URL OPENAI_MODEL || true
mkdir -p "$HOME" "$TMPDIR" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME" "$XDG_DATA_HOME" \
  "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR" "$MARGINS_HOME"
chmod 700 "$XDG_RUNTIME_DIR"

# A poisoned ENZYME_HOME from the parent shell: any read fails to parse.
POISON="$ROOT/poisoned-enzyme-home"
mkdir -p "$POISON/configs"
printf 'workspace {{ unparseable\n' > "$POISON/configs/poison.enzyme"
printf 'this is = = not toml\n' > "$POISON/config.toml"
printf '{"token":"poisoned"}\n' > "$POISON/auth.json"
export ENZYME_HOME="$POISON"

# The user's real Enzyme home stand-in.
CANARY="$HOME/.enzyme"
mkdir -p "$CANARY/configs" "$CANARY/workspaces/notes" "$CANARY/models"
printf 'workspace "notes" {\n  source markdown "notes" { path "~/notes" }\n}\n' > "$CANARY/configs/notes.enzyme"
printf 'canary index bytes\n' > "$CANARY/workspaces/notes/enzyme.db"
printf 'canary model bytes\n' > "$CANARY/models/canary.gguf"
printf '{"access_token":"canary-must-not-be-read"}\n' > "$CANARY/auth.json"
chmod 000 "$CANARY/auth.json"

# Notes with a planted phrase and enough material for catalysts.
NOTES="$ROOT/notes"
mkdir -p "$NOTES/projects" "$NOTES/Meetings" "$NOTES/People"
printf '# Harbor\n\n%s. [[Ada Chen]] keeps it for the #harbor project.\n' "$PHRASE" > "$NOTES/projects/harbor.md"
for i in 1 2 3 4 5; do
  printf '# Harbor review %s\n\n[[Ada Chen]] and the #harbor crew review crossing %s: schedule, tides, cargo, crew rotation, risks, decisions, and follow-ups for the season.\n' "$i" "$i" > "$NOTES/projects/review-$i.md"
done
printf '# Ada Chen\n\nAda runs the harbor ledger and the crossing reviews.\n' > "$NOTES/People/Ada Chen.md"
# Notes the in-process index of step 7 was built over.
FIXTURE="$REPO_ROOT/tests/fixtures/inprocess-index"
LEGACY_NOTES="$ROOT/legacy-notes"
mkdir -p "$LEGACY_NOTES"
cp -R "$FIXTURE/notes/." "$LEGACY_NOTES/"

snapshot() {
  python3 - "$ROOT" "$1" <<'PY'
import hashlib, json, os, stat, sys
root, out = sys.argv[1], sys.argv[2]
entries = {}
for dirpath, dirnames, filenames in os.walk(root):
    for name in dirnames + filenames:
        path = os.path.join(dirpath, name)
        st = os.lstat(path)
        digest = None
        if stat.S_ISREG(st.st_mode) and st.st_mode & 0o444:
            try:
                with open(path, "rb") as handle:
                    digest = hashlib.sha256(handle.read()).hexdigest()
            except PermissionError:
                digest = "unreadable"
        entries[os.path.relpath(path, root)] = [
            stat.S_IFMT(st.st_mode), stat.S_IMODE(st.st_mode), st.st_size,
            st.st_mtime_ns if not stat.S_ISDIR(st.st_mode) else None, digest,
        ]
with open(out, "w") as handle:
    json.dump(entries, handle)
PY
}

# Every change between two snapshots must fall under an allowed prefix.
assert_confined() {
  python3 - "$1" "$2" "$3" <<'PY'
import json, sys
before, after = (json.load(open(p)) for p in sys.argv[1:3])
label = sys.argv[3]
allowed = ("margins-home/", "notes/Meetings/", "tmp/")
changed = sorted(
    path for path in set(before) | set(after)
    if before.get(path) != after.get(path)
)
outside = [p for p in changed if not p.startswith(allowed) and p not in ("margins-home", "tmp", "notes/Meetings")]
if outside:
    sys.exit(f"isolation: FAIL ({label}): writes outside MARGINS_HOME:\n  " + "\n  ".join(outside[:40]))
canary = [p for p in changed if p.startswith("home/.enzyme")]
if canary:
    sys.exit(f"isolation: FAIL ({label}): ~/.enzyme changed: {canary}")
for path in after:
    if path.endswith("/.enzyme") and path.startswith("notes"):
        sys.exit(f"isolation: FAIL ({label}): {path} created in the notes folder")
print(f"isolation: ok: {label}: {len(changed)} changed paths, all under MARGINS_HOME, the create-note folder, or TMPDIR")
PY
}

assert_tmp_clean() {
  local leftover
  leftover="$(find "$TMPDIR" -mindepth 1 -print -quit)"
  [[ -z "$leftover" ]] || fail "$1: TMPDIR scratch left behind: $(find "$TMPDIR" -mindepth 1 | head -5)"
  [[ "$(api_cache_state)" == "$API_CACHE_BEFORE" ]] || fail "$1: $API_CACHE was created or changed"
}

margins() { "$MARGINS_BIN" "$@"; }

BEFORE="$MOCK_DIR/before.json"
snapshot "$BEFORE"

# --- 1. Setup -> init -> recall ---------------------------------------------
margins workspace new practice --home "$NOTES" --json >/dev/null
cat > "$MOCK_DIR/desired.enzyme" <<EOF
workspace "practice" {
  source markdown "notes" { path "$NOTES" }
  remember in folder "Meetings" create note
}
EOF
margins --workspace practice workspace plan --desired "$MOCK_DIR/desired.enzyme" --json > "$MOCK_DIR/plan.json"
margins --workspace practice workspace apply --plan "$MOCK_DIR/plan.json" --json > /dev/null
# Hosted setup: Margins' cached bundle points at the fixture generator.
python3 - "$MARGINS_HOME" "$PORT" <<'PY'
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
margins --workspace practice init > "$MOCK_DIR/init.out" 2> "$MOCK_DIR/init.err" \
  || { cat "$MOCK_DIR/init.err" >&2; fail "margins init"; }
grep -q 'engine index first_build' "$MOCK_DIR/init.err" || fail "first init did not report first_build"
[[ -f "$MARGINS_HOME/workspaces/practice/enzyme.db" ]] || fail "index not at MARGINS_HOME/workspaces/practice/enzyme.db"
margins --workspace practice recall "$PHRASE" > "$MOCK_DIR/recall.json"
python3 - "$MOCK_DIR/recall.json" <<'PY' || fail "recall did not return the planted note"
import json, sys
recall = json.load(open(sys.argv[1]))
assert recall["status"] == "ok", recall
assert any(hit["document_ref"] == "projects/harbor.md" for hit in recall["results"]), recall
PY
[[ "$(cat "$MOCK_DIR/count")" -gt 0 ]] || fail "init generated nothing through the fixture endpoint"
pass "1 setup -> init -> recall through enzyme ($(cat "$MOCK_DIR/count") generator requests, all to the fixture)"

# --- 2. Plain enzyme reads the same programs and index -----------------------
ENZYME_HOME="$MARGINS_HOME" "$ENZYME_BIN" --workspace practice search "$PHRASE" --phrase "$PHRASE" -n 8 --json \
  > "$MOCK_DIR/enzyme-search.json"
ENZYME_HOME="$MARGINS_HOME" "$ENZYME_BIN" --workspace practice status --json > "$MOCK_DIR/enzyme-status.json"
python3 - "$MOCK_DIR/recall.json" "$MOCK_DIR/enzyme-search.json" "$MOCK_DIR/enzyme-status.json" "$MARGINS_HOME" <<'PY' \
  || fail "enzyme and margins disagree"
import json, sys
recall, search, status = (json.load(open(p)) for p in sys.argv[1:4])
home = sys.argv[4]
assert status["database"] == f"{home}/workspaces/practice/enzyme.db", status["database"]
# Margins' merge: exact hits first (taking a catalyst hit's provenance when
# the same document has one), then the remaining catalyst hits, eight at most.
catalyst = search["catalyst_hits"]
merged, seen = [], set()
for exact in search["exact_hits"]:
    if exact["path"] in seen:
        continue
    seen.add(exact["path"])
    merged.append(next((hit for hit in catalyst if hit["path"] == exact["path"]), exact))
merged += [hit for hit in catalyst if hit["path"] not in seen]
expected = [hit["path"] for hit in merged[:8]]
actual = [hit["document_ref"] for hit in recall["results"]]
assert actual == expected, (actual, expected)
assert [h.get("via_catalyst_id") for h in recall["results"]] == [h["via_catalyst_id"] for h in merged[:8]]
print(f"isolation: ok: 2 enzyme search == margins recall ({len(actual)} hits, {status['documents']} documents)")
PY

AFTER="$MOCK_DIR/after-init.json"
snapshot "$AFTER"
assert_confined "$BEFORE" "$AFTER" "3-4 after setup, init, recall"
assert_tmp_clean "3-4 after setup, init, recall"
[[ ! -e "$NOTES/.enzyme" ]] || fail "notes/.enzyme exists"
[[ ! -e "$HOME/.enzyme/api-cache" ]] || fail "~/.enzyme/api-cache exists"
pass "3-4 confinement"
# Item 5 is covered two ways: auth.json is mode 000 for this non-root user
# (any read would fail the command), and the parent ENZYME_HOME holds an
# unparseable program (any use of it would fail to resolve). Every command
# above succeeded and every generator request reached the fixture.
pass "5 auth.json unreadable (mode 000, non-root) and poisoned parent ENZYME_HOME overridden; nothing failed or fell back"

# --- 6. Background worker ----------------------------------------------------
EPOCH_BEFORE="$(ENZYME_HOME="$MARGINS_HOME" "$ENZYME_BIN" --workspace practice status --json \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["index"]["epoch"]["active_epoch_id"])')"
# A program change makes a catalyst epoch due; `margins sync` refreshes, and
# enzyme builds the epoch in a detached worker.
sed 's|  remember in folder "Meetings" create note|  remember in folder "Meetings" create note\n  leave out tags ["private"]|' \
  "$MOCK_DIR/desired.enzyme" > "$MOCK_DIR/desired-2.enzyme"
margins --workspace practice workspace plan --desired "$MOCK_DIR/desired-2.enzyme" --json > "$MOCK_DIR/plan-2.json"
margins --workspace practice workspace apply --plan "$MOCK_DIR/plan-2.json" --json > /dev/null
margins --workspace practice sync --json > "$MOCK_DIR/sync.json" 2> "$MOCK_DIR/sync.err" \
  || { cat "$MOCK_DIR/sync.err" >&2; fail "margins sync"; }
grep -q 'engine refresh background_spawned=true' "$MOCK_DIR/sync.err" \
  || { cat "$MOCK_DIR/sync.err" >&2; fail "sync did not spawn a background catalyst worker"; }
EPOCH_AFTER=""
for _ in $(seq 1 600); do
  EPOCH_AFTER="$(ENZYME_HOME="$MARGINS_HOME" "$ENZYME_BIN" --workspace practice status --json \
    | python3 -c 'import json,sys; e=json.load(sys.stdin)["index"]["epoch"]; print("" if e["build_epoch_id"] else e["active_epoch_id"])')"
  [[ -n "$EPOCH_AFTER" && "$EPOCH_AFTER" != "$EPOCH_BEFORE" ]] && break
  sleep 0.1
done
[[ -n "$EPOCH_AFTER" && "$EPOCH_AFTER" != "$EPOCH_BEFORE" ]] \
  || fail "the background worker did not complete an epoch within 60s"
# The worker holds the workspace's refresh.lock while it builds; wait until
# it has let go, so the snapshot cannot race a live worker.
python3 - "$MARGINS_HOME/workspaces/practice/refresh.lock" <<'PY' \
  || fail "the background worker still holds refresh.lock after 30s"
import fcntl, sys, time
deadline = time.time() + 30
with open(sys.argv[1], "a") as handle:
    while True:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            fcntl.flock(handle, fcntl.LOCK_UN)
            break
        except BlockingIOError:
            if time.time() > deadline:
                sys.exit(1)
            time.sleep(0.1)
PY
AFTER_WORKER="$MOCK_DIR/after-worker.json"
snapshot "$AFTER_WORKER"
assert_confined "$BEFORE" "$AFTER_WORKER" "6 after the background worker"
assert_tmp_clean "6 after the background worker"
pass "6 background worker $EPOCH_AFTER inherited the isolation"

# --- 7. An index built by the in-process Margins -----------------------------
# Markdown notes plus a Gmail source (tests/fixtures/inprocess-index). The
# expected outcome is pinned: the CLI reuses the index, and every document,
# Markdown and ledger alike, keeps its row (no re-embedding). The google-mail
# kind emits the in-process `sqlite:mail/<hex(id)>` refs through {source}.
LEGACY_STATE="$MARGINS_HOME/workspaces/legacy"
sed "s|@NOTES@|$LEGACY_NOTES|" "$FIXTURE/program.enzyme.in" > "$MOCK_DIR/legacy.enzyme"
if [[ -n "${MARGINS_LEGACY_INPROCESS_BIN:-}" ]]; then
  LEGACY="$MARGINS_LEGACY_INPROCESS_BIN"
  # The #13-era binary predates margins-sources.enzyme; it lowers in process.
  mv "$MARGINS_HOME/configs/margins-sources.enzyme" "$MOCK_DIR/sources.keep"
  "$LEGACY" workspace new legacy --home "$LEGACY_NOTES" --json >/dev/null
  "$LEGACY" --workspace legacy workspace plan --desired "$MOCK_DIR/legacy.enzyme" --json > "$MOCK_DIR/legacy-plan.json"
  "$LEGACY" --workspace legacy workspace apply --plan "$MOCK_DIR/legacy-plan.json" --json >/dev/null
  "$LEGACY" --workspace legacy init >/dev/null 2>&1 || true   # creates the ledger
  python3 "$FIXTURE/seed_ledger.py" "$LEGACY_STATE/ledger.db"
  "$LEGACY" --workspace legacy init >/dev/null 2>&1 || fail "legacy in-process init"
  mv "$MOCK_DIR/sources.keep" "$MARGINS_HOME/configs/margins-sources.enzyme"
  LEGACY_SOURCE="live #13-era binary"
else
  margins workspace new legacy --home "$LEGACY_NOTES" --json >/dev/null
  margins --workspace legacy workspace plan --desired "$MOCK_DIR/legacy.enzyme" --json > "$MOCK_DIR/legacy-plan.json"
  margins --workspace legacy workspace apply --plan "$MOCK_DIR/legacy-plan.json" --json >/dev/null
  gzip -dc "$FIXTURE/enzyme.db.gz" > "$LEGACY_STATE/enzyme.db"
  gzip -dc "$FIXTURE/ledger.db.gz" > "$LEGACY_STATE/ledger.db"
  cp "$FIXTURE/index.identity" "$LEGACY_STATE/index.identity"
  LEGACY_SOURCE="tests/fixtures/inprocess-index"
fi
[[ -f "$LEGACY_STATE/index.identity" ]] || fail "legacy index has no in-process identity marker"
docs() {
  python3 - "$LEGACY_STATE/enzyme.db" "$1" <<'PY'
import json, sqlite3, sys
rows = sqlite3.connect(sys.argv[1]).execute("SELECT source_ref, id, content_hash FROM docs").fetchall()
json.dump({ref: [doc_id, digest] for ref, doc_id, digest in rows}, open(sys.argv[2], "w"))
PY
}
docs "$MOCK_DIR/legacy-docs-before.json"
REQUESTS_BEFORE="$(cat "$MOCK_DIR/count")"
margins --workspace legacy init > /dev/null 2> "$MOCK_DIR/legacy-1.err" \
  || { cat "$MOCK_DIR/legacy-1.err" >&2; fail "init over the in-process index"; }
grep -q 'engine index reuse reason=in_process_index' "$MOCK_DIR/legacy-1.err" \
  || { cat "$MOCK_DIR/legacy-1.err" >&2; fail "the CLI did not reuse the in-process index"; }
LEGACY_REQUESTS=$(( $(cat "$MOCK_DIR/count") - REQUESTS_BEFORE ))
[[ ! -e "$LEGACY_STATE/index.identity" ]] || fail "in-process identity marker left behind"
docs "$MOCK_DIR/legacy-docs-after.json"
python3 - "$MOCK_DIR/legacy-docs-before.json" "$MOCK_DIR/legacy-docs-after.json" <<'PY' \
  || fail "the transition did not keep the pinned document outcome"
import json, sys
before, after = (json.load(open(p)) for p in sys.argv[1:3])
markdown = {ref for ref in before if not ref.startswith("sqlite:")}
ids = [f"relay-thread-{index}" for index in range(4)]
hex_refs = {f"sqlite:mail/{record.encode().hex()}" for record in ids}
assert markdown, before.keys()
assert {ref for ref in before if ref.startswith("sqlite:")} == hex_refs, before.keys()
changed = sorted(ref for ref in before.keys() | after.keys() if before.get(ref) != after.get(ref))
assert not changed, f"documents re-identified or re-embedded: {changed}"
PY
margins --workspace legacy init > /dev/null 2> "$MOCK_DIR/legacy-2.err" || fail "second init"
! grep -q 'engine index rebuild\|in_process_index\|first_build' "$MOCK_DIR/legacy-2.err" \
  || fail "the transition repeated on the second init"
LEGACY_PHRASE="$(cat "$FIXTURE/phrase.txt")"
margins --workspace legacy recall "$LEGACY_PHRASE" > "$MOCK_DIR/legacy-recall.json"
margins --workspace legacy recall --source mail "$(cat "$FIXTURE/mail-phrase.txt")" > "$MOCK_DIR/legacy-mail.json"
python3 - "$MOCK_DIR/legacy-recall.json" "$(cat "$FIXTURE/phrase-ref.txt")" "$MOCK_DIR/legacy-mail.json" <<'PY' \
  || fail "recall over the transitioned index"
import json, sys
recall = json.load(open(sys.argv[1]))
assert recall["status"] == "ok", recall
assert any(hit["document_ref"] == sys.argv[2] for hit in recall["results"]), recall
mail = json.load(open(sys.argv[3]))
hits = mail["results"]
assert hits and all(hit["evidence"]["kind"] == "external_record" for hit in hits), mail
assert any(hit["evidence"]["source_id"].startswith("relay-thread-") for hit in hits), mail
PY
pass "7 in-process index ($LEGACY_SOURCE) reused by the CLI: every Markdown and ledger row kept ($LEGACY_REQUESTS generator requests); recall works"

FINAL="$MOCK_DIR/final.json"
snapshot "$FINAL"
assert_confined "$BEFORE" "$FINAL" "final"
assert_tmp_clean "final"
echo "isolation: PASS (mock generator requests: $(cat "$MOCK_DIR/count" 2>/dev/null || echo 0))"
