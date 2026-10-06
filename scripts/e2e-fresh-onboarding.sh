#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)"

die() {
  printf 'fresh-onboarding e2e: ERROR: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Usage:
  scripts/e2e-fresh-onboarding.sh init [--sandbox DIR]
  scripts/e2e-fresh-onboarding.sh phase1
  scripts/e2e-fresh-onboarding.sh auth-instructions
  scripts/e2e-fresh-onboarding.sh phase2
  scripts/e2e-fresh-onboarding.sh assert-source-remove-preservation
  scripts/e2e-fresh-onboarding.sh contract-lifecycle
  scripts/e2e-fresh-onboarding.sh contract-greps
  scripts/e2e-fresh-onboarding.sh contract-phase8
  scripts/e2e-fresh-onboarding.sh contract-phase9
  scripts/e2e-fresh-onboarding.sh assert-machine-forget-preservation [--granola-only]
  scripts/e2e-fresh-onboarding.sh verify-isolation
  scripts/e2e-fresh-onboarding.sh report

Run init first, then source the env file it prints before invoking another phase.
Set MARGINS_E2E_BIN before init to test a specific margins executable.
Before phase2, verify-isolation/report require only the Workspace program
($MARGINS_HOME/configs/<id>.enzyme); phase2 writes a
sandbox marker after completing and later checks also require ledger.db/enzyme.db.
EOF
}

shell_quote() {
  python3 - "$1" <<'PY'
import shlex
import sys
print(shlex.quote(sys.argv[1]))
PY
}

inspect_binary_provenance() {
  local binary="$1"
  local capabilities
  capabilities="$("$binary" capabilities)" || die "could not read binary capabilities: $binary"
  python3 - "$binary" "$capabilities" <<'PY'
import hashlib
import json
from pathlib import Path
import sys

binary = Path(sys.argv[1])
capabilities = json.loads(sys.argv[2])
build = capabilities.get("build")
required = {"commit", "short", "dirty", "built_at", "profile"}
if not isinstance(build, dict) or not required <= set(build):
    raise SystemExit("binary capabilities omitted the required build object")
digest = hashlib.sha256()
with binary.open("rb") as handle:
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        digest.update(block)
print(build["commit"])
print(digest.hexdigest())
print(json.dumps(build, sort_keys=True, separators=(",", ":")))
PY
}

assert_expected_commit() {
  local observed="$1"
  if [ -n "${MARGINS_E2E_EXPECTED_COMMIT:-}" ] \
      && [ "$observed" != "$MARGINS_E2E_EXPECTED_COMMIT" ]; then
    die "binary build commit mismatch: observed=$observed expected=$MARGINS_E2E_EXPECTED_COMMIT"
  fi
}

assert_binary_provenance() {
  : "${MARGINS_E2E_BIN:?sandbox env is missing MARGINS_E2E_BIN}"
  : "${MARGINS_E2E_BIN_SHA256:?sandbox env is missing MARGINS_E2E_BIN_SHA256}"
  local inspected observed_commit observed_sha
  inspected="$(inspect_binary_provenance "$MARGINS_E2E_BIN")" || \
    die "could not inspect binary provenance: $MARGINS_E2E_BIN"
  observed_commit="$(printf '%s\n' "$inspected" | sed -n '1p')"
  observed_sha="$(printf '%s\n' "$inspected" | sed -n '2p')"
  assert_expected_commit "$observed_commit"
  [ "$observed_sha" = "$MARGINS_E2E_BIN_SHA256" ] || \
    die "binary sha256 mismatch: observed=$observed_sha initialized=$MARGINS_E2E_BIN_SHA256"
}

now_ms() {
  python3 - <<'PY'
import time
print(time.time_ns() // 1_000_000)
PY
}

require_sandbox_env() {
  : "${MARGINS_E2E_SANDBOX:?source the sandbox env file printed by init first}"
  : "${MARGINS_HOME:?sandbox env is missing MARGINS_HOME}"
  : "${MARGINS_WORKSPACE:?sandbox env is missing MARGINS_WORKSPACE}"
  : "${MARGINS_WORKSPACE_STATE:?sandbox env is missing MARGINS_WORKSPACE_STATE}"
  : "${NOTES_HOME:?sandbox env is missing NOTES_HOME}"
  : "${REFERENCE_NOTES:?sandbox env is missing REFERENCE_NOTES}"
  : "${MARGINS_E2E_BIN:?sandbox env is missing MARGINS_E2E_BIN}"
  : "${MARGINS_GOOGLE_CREDENTIAL_BACKEND:?sandbox env is missing MARGINS_GOOGLE_CREDENTIAL_BACKEND}"
  : "${MARGINS_GRANOLA_CREDENTIAL_BACKEND:?sandbox env is missing MARGINS_GRANOLA_CREDENTIAL_BACKEND}"
  [ -d "$MARGINS_E2E_SANDBOX" ] || die "sandbox no longer exists: $MARGINS_E2E_SANDBOX"
  [ "$MARGINS_HOME" = "$MARGINS_E2E_SANDBOX/margins-home" ] || die "MARGINS_HOME escaped the sandbox: $MARGINS_HOME"
  [ "$NOTES_HOME" = "$MARGINS_E2E_SANDBOX/notes-home" ] || die "NOTES_HOME escaped the sandbox: $NOTES_HOME"
  [ "$REFERENCE_NOTES" = "$MARGINS_E2E_SANDBOX/reference-notes" ] || die "REFERENCE_NOTES escaped the sandbox: $REFERENCE_NOTES"
  [ "$MARGINS_GOOGLE_CREDENTIAL_BACKEND" = "file" ] || die "Google credential backend is not sandbox-safe: $MARGINS_GOOGLE_CREDENTIAL_BACKEND"
  [ "$MARGINS_GRANOLA_CREDENTIAL_BACKEND" = "file" ] || die "Granola credential backend is not sandbox-safe: $MARGINS_GRANOLA_CREDENTIAL_BACKEND"
  local resolved_sandbox resolved_notes_home resolved_reference_notes
  resolved_sandbox="$(CDPATH= cd -- "$MARGINS_E2E_SANDBOX" && pwd -P)"
  resolved_notes_home="$(CDPATH= cd -- "$NOTES_HOME" && pwd -P)"
  resolved_reference_notes="$(CDPATH= cd -- "$REFERENCE_NOTES" && pwd -P)"
  for resolved_notes in "$resolved_notes_home" "$resolved_reference_notes"; do
    case "$resolved_notes/" in
      "$resolved_sandbox"/*) ;;
      *) die "resolved notes escaped the sandbox: $resolved_notes" ;;
    esac
  done
}

workspace_program_path() {
  printf '%s\n' "$MARGINS_HOME/configs/$MARGINS_WORKSPACE.enzyme"
}

require_workspace_declaration() {
  local program
  program="$(workspace_program_path)"
  [ -f "$program" ] || die "workspace program is missing: $program"
  # workspaces/<id>/ holds state only; a live legacy config.toml means the
  # declaration was not migrated into the Workspace language.
  [ ! -e "$MARGINS_WORKSPACE_STATE/config.toml" ] || \
    die "legacy workspace config.toml still present in state dir: $MARGINS_WORKSPACE_STATE/config.toml"
}

assert_workspace_layout() {
  require_workspace_declaration
  if [ -f "$MARGINS_E2E_SANDBOX/.phase2-complete" ]; then
    for filename in ledger.db enzyme.db; do
      [ -f "$MARGINS_WORKSPACE_STATE/$filename" ] || \
        die "workspace state is missing $filename after phase2: $MARGINS_WORKSPACE_STATE/$filename"
    done
  fi
  if find "$NOTES_HOME" -type d -name .margins -print -quit | grep -q .; then
    die "workspace state leaked inside the notes home: $NOTES_HOME"
  fi
  if find "$REFERENCE_NOTES" -type d -name .margins -print -quit | grep -q .; then
    die "workspace state leaked inside the reference notes: $REFERENCE_NOTES"
  fi
}

run_margins() {
  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" "$@"
}

read_workspace_revision() {
  local status_path="$1"
  python3 - "$status_path" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
for key in ("revision", "config_revision"):
    if isinstance(data.get(key), str) and data[key]:
        print(data[key])
        raise SystemExit(0)
raise SystemExit("workspace status missing revision")
PY
}

write_desired_fixture() {
  cp "$(workspace_program_path)" "$1"
}

ledger_run_count() {
  python3 - "$MARGINS_WORKSPACE_STATE/ledger.db" <<'PY'
import sqlite3
import sys

ledger = sys.argv[1]
db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
if "runs" not in tables:
    print(0)
else:
    print(db.execute("SELECT COUNT(*) FROM runs").fetchone()[0])
PY
}

assert_json_schema() {
  local path="$1"
  local expected_schema="$2"
  python3 - "$path" "$expected_schema" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data.get("schema_version") != sys.argv[2]:
    raise SystemExit(f"expected schema {sys.argv[2]!r}, got {data.get('schema_version')!r}")
PY
}

assert_margins_error_code() {
  local path="$1"
  local expected_code="$2"
  python3 - "$path" "$expected_code" <<'PY'
import json
import sys

raw = open(sys.argv[1]).read().strip()
if not raw:
    raise SystemExit("empty error payload")
try:
    data = json.loads(raw)
except json.JSONDecodeError:
    raise SystemExit("expected JSON margins.error.v1 payload")
if data.get("schema_version") != "margins.error.v1":
    raise SystemExit(f"unexpected error schema: {data.get('schema_version')!r}")
code = (data.get("error") or {}).get("code")
if code != sys.argv[2]:
    raise SystemExit(f"expected code {sys.argv[2]!r}, got {code!r}")
PY
}

run_workspace_plan_apply() {
  local label_prefix="$1"
  local desired="$MARGINS_E2E_ARTIFACTS/${label_prefix}-desired.enzyme"
  local plan="$MARGINS_E2E_ARTIFACTS/${label_prefix}-plan.json"
  local apply="$MARGINS_E2E_ARTIFACTS/${label_prefix}-apply.json"
  local status="$MARGINS_E2E_ARTIFACTS/${label_prefix}-workspace-status.json"
  timed_run "${label_prefix}.workspace-status" "$status" run_margins workspace status --json
  write_desired_fixture "$desired"
  timed_run "${label_prefix}.workspace-plan" "$plan" run_margins workspace plan --desired "$desired" --json
  assert_json_schema "$plan" "margins.workspace.plan.v2"
  timed_run "${label_prefix}.workspace-apply" "$apply" \
    run_margins workspace apply --plan "$plan" --json
  assert_json_schema "$apply" "margins.workspace.apply.v2"
}

run_integrations_reconcile() {
  local label_prefix="$1"
  local request_id="$2"
  local reconcile="$MARGINS_E2E_ARTIFACTS/${label_prefix}-reconcile.json"
  local status="$MARGINS_E2E_ARTIFACTS/${label_prefix}-workspace-status.json"
  timed_run "${label_prefix}.workspace-status" "$status" run_margins workspace status --json
  local revision
  revision="$(read_workspace_revision "$status")"
  timed_run "${label_prefix}.integrations-reconcile" "$reconcile" \
    run_margins integrations reconcile --if-revision "$revision" --request-id "$request_id" --json
  assert_json_schema "$reconcile" "margins.integrations.reconcile.v1"
  printf '%s\n' "$reconcile"
}

assert_calendar_materialization_contract() {
  local ledger="$MARGINS_WORKSPACE_STATE/ledger.db"
  [ -f "$ledger" ] || die "ledger.db is required for calendar materialization checks: $ledger"
  python3 - "$ledger" <<'PY' || die "calendar Phase 5 materialization contract failed"
import sqlite3
import sys

ledger = sys.argv[1]
db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")}
for table in ("calendar_event_evidence", "calendar_event_attendees"):
    if table not in tables:
        raise SystemExit(f"missing authoritative table: {table}")
event_count = db.execute(
    "SELECT COUNT(*) FROM calendar_event_evidence WHERE connector_id = 'gcal'"
).fetchone()[0]
attendee_count = db.execute("SELECT COUNT(*) FROM calendar_event_attendees").fetchone()[0]
if event_count == 0:
    raise SystemExit("calendar_event_evidence has no gcal rows")
if attendee_count == 0:
    raise SystemExit("calendar_event_attendees has no rows")
for forbidden in ("episodes", "recall_items"):
    if forbidden in tables:
        raise SystemExit(f"generic Episode framework object must be absent: {forbidden}")
print(f"calendar_events={event_count}; calendar_attendees={attendee_count}; episodes_absent=1; recall_items_absent=1")
PY
}

assert_phase6_external_document_contract() {
  local ledger="$MARGINS_WORKSPACE_STATE/ledger.db"
  [ -f "$ledger" ] || die "ledger.db is required for Phase 6 external-document checks: $ledger"
  local phase6_sources="$MARGINS_E2E_ARTIFACTS/phase6-source-list.json"
  run_margins source list --json > "$phase6_sources" || die "Phase 6 source list failed"
  python3 - "$ledger" "$(workspace_program_path)" "$NOTES_HOME" "${MARGINS_E2E_GOOGLE_ONLY:-0}" "$phase6_sources" <<'PY' || die "Phase 6 external-document contract failed"
import json
import re
import sqlite3
import sys
from pathlib import Path

ledger = sys.argv[1]
program_path = Path(sys.argv[2])
notes_home = Path(sys.argv[3])
google_only = sys.argv[4] == "1"
sources = json.load(open(sys.argv[5]))
db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type IN ('table', 'view')")}
for table in ("external_document_evidence", "external_document_participants"):
    if table not in tables:
        raise SystemExit(f"missing authoritative table: {table}")
for connector in ("google_meet", "granola"):
    if google_only and connector == "granola":
        continue
    document_count = db.execute(
        "SELECT COUNT(*) FROM external_document_evidence WHERE connector_id = ? AND tombstoned_at IS NULL",
        (connector,),
    ).fetchone()[0]
    participant_count = db.execute(
        "SELECT COUNT(*) FROM external_document_participants WHERE connector_id = ?",
        (connector,),
    ).fetchone()[0]
    if document_count == 0:
        raise SystemExit(f"{connector} has no authoritative external documents")
    if connector == "granola" and participant_count == 0:
        raise SystemExit("Granola fixture has no role-blind participant associations")
    if connector == "granola":
        raw_count = db.execute(
            "SELECT COUNT(*) FROM raw_items WHERE connector_id = ?",
            (connector,),
        ).fetchone()[0]
        if raw_count == 0:
            raise SystemExit("live Granola transport cache has no source-native rows")
        print(f"granola_raw_transport_rows={raw_count}")
    linked_body_count = db.execute(
        "SELECT COUNT(*) FROM external_document_evidence WHERE connector_id = ? AND body_text LIKE '%[[%'",
        (connector,),
    ).fetchone()[0]
    if linked_body_count != 0:
        raise SystemExit(f"{connector} authoritative bodies must not contain native wikilinks")
    print(f"{connector}_documents={document_count}; {connector}_participants={participant_count}")
for forbidden in ("episodes", "recall_items"):
    if forbidden in tables:
        raise SystemExit(f"generic Episode framework object must be absent: {forbidden}")
program = program_path.read_text()

def program_source_block(kind):
    match = re.search(r'source\s+' + re.escape(kind) + r'\s+"[^"]*"\s*\{([^}]*)\}', program)
    return match.group(1) if match else None

meet = next((row for row in sources if row.get("kind") == "google-meet"), None)
granola = next((row for row in sources if row.get("kind") == "granola"), None)
meet_block = program_source_block("google-meet")
granola_block = program_source_block("granola")
if meet is None or not meet.get("account") or meet_block is None or not re.search(r'\baccount\s+"[^"]+"', meet_block):
    raise SystemExit("google-meet source must declare account identity in the Workspace program")
if not google_only:
    if granola is None or not granola.get("account") or granola_block is None \
            or not re.search(r'\baccount\s+"[^"]+"', granola_block):
        raise SystemExit("granola source must declare account identity in the Workspace program")
    if "path" in granola or re.search(r'\bpath\b', granola_block):
        raise SystemExit("granola binding must not declare export path")
    if "projection" in granola:
        raise SystemExit("granola binding must not declare retired projection settings")
    collection = granola.get("collection") or {}
    if collection.get("time_range") != "last_30_days":
        raise SystemExit("granola binding must declare the bounded last_30_days collection")
    if collection.get("workspace_only") is not False:
        raise SystemExit("granola binding must explicitly declare workspace_only=false")
if list((notes_home / "meetings").glob("*.md")):
    raise SystemExit("Granola ledger evidence must not project notes into the home")
if (notes_home / "people" / "Alice Client.md").exists():
    raise SystemExit("Granola must not auto-create native person stubs")
if (notes_home / "organizations" / "Acme.md").exists():
    raise SystemExit("Granola must not auto-create native organization stubs")
print(f"external_document_tables=2; meet_documents>0; episodes_absent=1; recall_items_absent=1; account_bindings={1 if google_only else 2}")
PY
}

recall_composition_available() {
  run_margins capabilities | python3 -c \
    'import json, sys; raise SystemExit(0 if json.load(sys.stdin).get("recall", {}).get("indexing") else 1)'
}

record_observation() {
  printf '[%s] %s\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$*" >> "$MARGINS_E2E_OBSERVATIONS"
}

timed_run() {
  local label="$1"
  local stdout_path="$2"
  shift 2
  local stderr_path="$MARGINS_E2E_ARTIFACTS/${label//[^A-Za-z0-9_.-]/_}.stderr"
  local started ended status
  started="$(now_ms)"
  set +e
  "$@" >"$stdout_path" 2>"$stderr_path"
  status=$?
  set -e
  ended="$(now_ms)"
  printf '%s\t%s\t%s\n' "$label" "$((ended - started))" "$status" >> "$MARGINS_E2E_TIMINGS"
  if [ -s "$stderr_path" ] && [ "$label" = "phase2.index-refresh" ] \
      && [ "$(head -n 1 "$stderr_path")" = "Building or refreshing recall index and catalysts…" ] \
      && ! grep -qv -e '^Building or refreshing recall index and catalysts…$' \
        -e '^Your Workspace is the program at ' -e '^  Read it: ' -e '^  Change it: ' "$stderr_path"; then
    record_observation "$label progress: Building or refreshing recall index and catalysts…"
  elif [ -s "$stderr_path" ]; then
    {
      printf '[%s] stderr/warnings from %s:\n' "$(date -u '+%Y-%m-%dT%H:%M:%SZ')" "$label"
      sed -n '1,120p' "$stderr_path"
    } >> "$MARGINS_E2E_OBSERVATIONS"
  fi
  if [ "$status" -ne 0 ]; then
    sed -n '1,160p' "$stderr_path" >&2
    die "$label failed with exit status $status (artifacts: $MARGINS_E2E_ARTIFACTS)"
  fi
}

snapshot_paths() {
  local output="$1"
  shift
  python3 - "$output" "$@" <<'PY'
import hashlib
import json
import os
from pathlib import Path
import stat
import sys

output = Path(sys.argv[1])
roots = list(dict.fromkeys(os.path.abspath(p) for p in sys.argv[2:] if p))

def item(path: Path):
    try:
        info = path.lstat()
    except FileNotFoundError:
        return {"type": "missing"}
    base = {
        "mode": stat.S_IMODE(info.st_mode),
        "mtime_ns": info.st_mtime_ns,
        "size": info.st_size,
    }
    if path.is_symlink():
        base.update(type="symlink", target=os.readlink(path))
    elif path.is_file():
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for block in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(block)
        base.update(type="file", sha256=digest.hexdigest())
    elif path.is_dir():
        base.update(type="directory")
    else:
        base.update(type="other")
    return base

snapshot = {"roots": roots, "entries": {}}
for raw_root in roots:
    root = Path(raw_root)
    snapshot["entries"][raw_root] = item(root)
    if root.is_dir() and not root.is_symlink():
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames.sort()
            filenames.sort()
            parent = Path(dirpath)
            for name in dirnames + filenames:
                path = parent / name
                snapshot["entries"][str(path)] = item(path)
output.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
PY
}

seed_notes_home() {
  mkdir -p "$1/.obsidian" "$2"
  printf '%s\n' '# Fresh onboarding home' > "$1/home.md"
  python3 - "$2" <<'PY'
from pathlib import Path
import sys
root = Path(sys.argv[1])
fixtures = {
    "2026-04-15-client-call-transcript.md": """# Client call transcript

[[Ada Lovelace]]: We should ship the pilot next week.

Sam: I will send the revised scope.
""",
    "2026-05-02-customer-planning-review.md": """---
title: Customer planning review
kind: meeting_note
occurred_at: 2026-05-02T14:30:00Z
attendees:
  - name: Grace Hopper
    email: grace@example.com
  - "[[People/Ada Lovelace]]"
---

# Planning review

The customer approved the next milestone.
""",
    "2026-06-03-project-checkpoint.md": """---
title: Mutable source document
date: 2026-06-03
---

# Mutable source document

Version one.
""",
    "2026-06-18-identity-follow-up.md": """---
title: Ambiguous identity evidence
people:
  - Kevin
---

# Follow-up

Taylor mentioned a deadline, but Taylor is prose and must not be guessed as a person.
""",
}
for name, content in fixtures.items():
    (root / name).write_text(content)
path = root / "2026-06-18-identity-follow-up.md"
text = path.read_text()
text = text.replace("title: Ambiguous identity evidence", "title: Identity follow-up\noccurred_at: 2026-06-18T16:00:00Z\nkind: meeting_note")
text = text.replace("Taylor mentioned a deadline, but Taylor is prose and must not be guessed as a person.", "The team reviewed the renewal deadline. Taylor was mentioned in prose and must not be guessed as an attendee.\n\n- [ ] Confirm the account owner before the next review.")
path.write_text(text)

# Keep the fixture above Enzyme's four-document/128-token grounding guard for
# one explicitly declared identity. The guard remains fail-closed; the fixture
# supplies enough real document evidence instead of lowering engine policy.
paths = sorted(root.glob("*.md"))
for path in paths:
    text = path.read_text()
    if not text.startswith("---\n"):
        text = "---\npeople:\n  - \"[[alice@acme.test]]\"\n---\n\n" + text
    elif "\npeople:\n" in text:
        text = text.replace("\npeople:\n", "\npeople:\n  - \"[[alice@acme.test]]\"\n", 1)
    else:
        text = text.replace("---\n", "---\npeople:\n  - \"[[alice@acme.test]]\"\n", 1)
    text += (
        "\n\nTuesday pilot kickoff checkpoint evidence: Alice reviewed the rollout "
        "sequence, confirmed the customer milestones, recorded the open "
        "dependency, assigned the follow-up owner, and compared the delivery "
        "risk against the acceptance criteria. The next review will verify the "
        "revised scope, implementation result, and customer response.\n"
    )
    path.write_text(text)
PY
}

init_sandbox() {
  local sandbox=""
  while [ "$#" -gt 0 ]; do
    case "$1" in
      --sandbox) [ "$#" -ge 2 ] || die "--sandbox requires a directory"; sandbox="$2"; shift 2 ;;
      *) die "unknown init option: $1" ;;
    esac
  done

  local margins_bin="${MARGINS_E2E_BIN:-}"
  if [ -z "$margins_bin" ]; then
    margins_bin="$(command -v margins || true)"
  fi
  [ -n "$margins_bin" ] || margins_bin="margins"
  case "$margins_bin" in
    /*) ;;
    *)
      local resolved_bin
      resolved_bin="$(command -v "$margins_bin" || true)"
      if [ -n "$resolved_bin" ]; then margins_bin="$resolved_bin"; fi
      ;;
  esac
  margins_bin="$(python3 - "$margins_bin" <<'PY'
from pathlib import Path
import sys
print(Path(sys.argv[1]).expanduser().resolve())
PY
)"
  [ -x "$margins_bin" ] || die "binary is not executable: $margins_bin"
  local inspected build_commit binary_sha256 build_json
  inspected="$(inspect_binary_provenance "$margins_bin")" || \
    die "could not inspect binary provenance: $margins_bin"
  build_commit="$(printf '%s\n' "$inspected" | sed -n '1p')"
  binary_sha256="$(printf '%s\n' "$inspected" | sed -n '2p')"
  build_json="$(printf '%s\n' "$inspected" | sed -n '3p')"
  assert_expected_commit "$build_commit"

  if [ -z "$sandbox" ]; then
    sandbox="$(mktemp -d "${TMPDIR:-/tmp}/margins-fresh-onboarding.XXXXXX")"
    # Canonicalize so macOS /var -> /private/var symlinks never cause textual
    # path mismatches between the harness and the CLI's canonical notes paths.
    sandbox="$(cd "$sandbox" && pwd -P)"
  else
    sandbox="$(python3 - "$sandbox" <<'PY'
from pathlib import Path
import sys
print(Path(sys.argv[1]).expanduser().resolve())
PY
)"
    mkdir -p "$sandbox"
  fi

  local real_home="${HOME:-}"
  [ -n "$real_home" ] || die "HOME is not set"
  local baseline="$sandbox/isolation-baseline.json"
  if [ ! -f "$baseline" ]; then
    local -a protected_paths
    protected_paths=(
      "$real_home/.margins"
      "$real_home/.config/margins"
      "$real_home/.config/Margins"
      "$real_home/.local/share/margins"
      "$real_home/Library/Application Support/Margins"
    )
    if [ -n "${MARGINS_HOME:-}" ]; then protected_paths+=("$MARGINS_HOME"); fi
    snapshot_paths "$baseline" "${protected_paths[@]}"
  fi

  mkdir -p "$sandbox/margins-home" "$sandbox/home" \
    "$sandbox/home/.config" "$sandbox/home/.local/share" "$sandbox/artifacts" "$sandbox/bin"
  seed_notes_home "$sandbox/notes-home" "$sandbox/reference-notes"
  python3 - "$sandbox/granola-export.json" <<'PY'
from pathlib import Path
import json
import sys

Path(sys.argv[1]).write_text(json.dumps({"documents": [{
    "id": "granola-e2e-1",
    "title": "Granola customer follow-up",
    "created_at": "2026-08-20T17:00:00Z",
    "notes": "Alice Client asked for the revised pilot plan.",
    "transcript": "Alice Client: Please send the revised pilot plan. Owner: I will send it Friday.",
    "attendees": [{"name": "Alice Client", "email": "alice@acme.test"}],
}]} , indent=2) + "\n")
PY
  : > "$sandbox/timings.tsv"
  : > "$sandbox/trust-friction-observations.txt"

  # macOS-only evidence: fingerprint each item's stable identity and full
  # metadata separately. Verification rejects creations while reporting
  # unrelated system updates to existing items as informational.
  if [ "$(uname -s)" = "Darwin" ]; then
    python3 - "$sandbox/keychain-items-baseline.json" <<'PY'
import hashlib, json, re, subprocess, sys
result = subprocess.run(["security", "dump-keychain"], capture_output=True, text=True)
if result.returncode:
    raise SystemExit("could not snapshot the macOS user keychain before the harness run")
blocks = re.split(r'(?=^keychain: )', result.stdout, flags=re.MULTILINE)
def fingerprint(block):
    attributes = {}
    for line in block.splitlines():
        match = re.match(r'^\s*"(svce|acct|cdat)"<[^>]+>=(.*)$', line)
        if match:
            attributes[match.group(1)] = match.group(2).strip()
    identity = json.dumps([attributes.get(key, "") for key in ("svce", "acct", "cdat")])
    return hashlib.sha256(identity.encode()).hexdigest(), hashlib.sha256(block.encode()).hexdigest()
items = dict(fingerprint(block) for block in blocks if block.startswith("keychain: "))
open(sys.argv[1], "w").write(json.dumps({"items": items}, sort_keys=True) + "\n")
PY
    chmod 600 "$sandbox/keychain-items-baseline.json"
  fi

  # Always expose the CLI under test as `margins` on PATH so the human steps
  # ("margins connect google ...") run the official build, not whatever
  # older `margins` the user's shell would otherwise resolve.
  mkdir -p "$sandbox/bin"
  printf '#!/bin/sh\nexec %s "$@"\n' "$(shell_quote "$margins_bin")" > "$sandbox/bin/margins"
  chmod 755 "$sandbox/bin/margins"
  local path_value="$sandbox/bin:$PATH"

  local env_file="$sandbox/env.sh"
  python3 - "$env_file" "$sandbox" "$real_home" "$margins_bin" "$path_value" "$build_commit" "$binary_sha256" "$build_json" "${MARGINS_E2E_EXPECTED_COMMIT:-}" <<'PY'
from pathlib import Path
import shlex
import sys

env_file = Path(sys.argv[1])
sandbox = Path(sys.argv[2])
values = {
    "MARGINS_E2E_SANDBOX": str(sandbox),
    "MARGINS_E2E_REAL_HOME": sys.argv[3],
    "MARGINS_E2E_BIN": sys.argv[4],
    "MARGINS_E2E_BUILD_COMMIT": sys.argv[6],
    "MARGINS_E2E_BIN_SHA256": sys.argv[7],
    "MARGINS_E2E_BUILD_JSON": sys.argv[8],
    "MARGINS_E2E_EXPECTED_COMMIT": sys.argv[9],
    "MARGINS_GOOGLE_CREDENTIAL_BACKEND": "file",
    "MARGINS_GRANOLA_CREDENTIAL_BACKEND": "file",
    "MARGINS_HOME": str(sandbox / "margins-home"),
    "MARGINS_WORKSPACE": "fresh-onboarding",
    "MARGINS_WORKSPACE_STATE": str(sandbox / "margins-home" / "workspaces" / "fresh-onboarding"),
    "NOTES_HOME": str(sandbox / "notes-home"),
    "REFERENCE_NOTES": str(sandbox / "reference-notes"),
    "HOME": str(sandbox / "home"),
    "XDG_CONFIG_HOME": str(sandbox / "home" / ".config"),
    "XDG_DATA_HOME": str(sandbox / "home" / ".local" / "share"),
    "MARGINS_E2E_ARTIFACTS": str(sandbox / "artifacts"),
    "MARGINS_E2E_TIMINGS": str(sandbox / "timings.tsv"),
    "MARGINS_E2E_OBSERVATIONS": str(sandbox / "trust-friction-observations.txt"),
    "PATH": sys.argv[5],
}
exports = "\n".join(f"export {key}={shlex.quote(value)}" for key, value in values.items())
env_file.write_text(exports + "\nunset OPENAI_API_KEY OPENROUTER_API_KEY OPENAI_BASE_URL OPENROUTER_BASE_URL OPENAI_MODEL OPENROUTER_MODEL\n")
env_file.chmod(0o600)
PY

  python3 - "$sandbox/artifacts/binary-provenance.json" "$margins_bin" "$binary_sha256" "$build_json" <<'PY'
import json
from pathlib import Path
import sys

Path(sys.argv[1]).write_text(json.dumps({
    "build": json.loads(sys.argv[4]),
    "binary": {"path": sys.argv[2], "sha256": sys.argv[3]},
}, indent=2, sort_keys=True) + "\n")
PY

  (
    # shellcheck disable=SC1090
    source "$env_file"
    "$MARGINS_E2E_BIN" workspace new "$MARGINS_WORKSPACE" --home "$NOTES_HOME" \
      > "$MARGINS_E2E_ARTIFACTS/workspace-new.txt"
    "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add notes \
      --name meeting-notes --role reference --path "$REFERENCE_NOTES" \
      > "$MARGINS_E2E_ARTIFACTS/source-meeting-notes.txt"
    for source_kind in google-mail google-calendar google-meet; do
      case "$source_kind" in
        google-mail) source_name=mail ;;
        google-calendar) source_name=calendar ;;
        google-meet) source_name=meet ;;
      esac
      if [ "$source_kind" = google-mail ]; then
        "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add "$source_kind" \
          --name "$source_name" --account owner@example.com \
          --query "-in:spam -in:trash" --backfill-days 365 \
          > "$MARGINS_E2E_ARTIFACTS/source-$source_name.txt"
      elif [ "$source_kind" = google-calendar ]; then
        "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add "$source_kind" \
          --name "$source_name" --account owner@example.com \
          --lookback-days 365 --lookahead-days 180 \
          > "$MARGINS_E2E_ARTIFACTS/source-$source_name.txt"
      else
        "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add "$source_kind" \
          --name "$source_name" --account owner@example.com \
          > "$MARGINS_E2E_ARTIFACTS/source-$source_name.txt"
      fi
    done
    if [ "${MARGINS_E2E_GOOGLE_ONLY:-0}" != 1 ]; then
      "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add granola \
        --name granola --account owner@example.com --time-range last_30_days \
        > "$MARGINS_E2E_ARTIFACTS/source-granola.txt"
    fi
  )

  printf 'Sandbox initialized: %s\n' "$sandbox"
  printf 'Seeded meeting notes: 4\n'
  printf 'Isolation baseline: %s\n' "$baseline"
  printf 'Binary provenance: commit=%s sha256=%s\n' "$build_commit" "$binary_sha256"
  printf 'Next, run:\n  source %s\n' "$(shell_quote "$env_file")"
}

phase1() {
  [ "$#" -eq 0 ] || die "phase1 takes no options"
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  rm -f -- "$MARGINS_E2E_SANDBOX/isolation-verdict.txt"
  local status="$MARGINS_E2E_ARTIFACTS/phase1-status.json"
  timed_run "phase1.status" "$status" run_margins integrations status --json
  local summary
  summary="$(python3 - "$status" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
results = data.get("results") or data.get("connectors") or []
if not isinstance(results, list):
    raise SystemExit("integrations status missing connector rows")
google_rows = [
    row for row in results
    if row.get("connector_id") in {"email", "gcal", "google_meet", "google"}
    or row.get("source") in {"email", "gcal", "google_meet", "google"}
]
if not google_rows:
    raise SystemExit("status omitted Google connector rows")
for row in google_rows:
    auth = row.get("auth_state") or (row.get("value") or {}).get("auth_state")
    if auth == "ok":
        raise SystemExit(f"ISOLATION LEAK: fresh sandbox reported auth_state=ok for {row!r}")
print(f"google_rows={len(google_rows)}; pre_auth=1")
PY
)" || die "phase1 assertions failed; inspect $status"
  record_observation "phase1: $summary"
  local granola_status="$MARGINS_E2E_ARTIFACTS/phase1-granola-machine-status.json"
  timed_run "phase1.granola-machine-status" "$granola_status" \
    "$MARGINS_E2E_BIN" connect status --service granola --json
  python3 - "$granola_status" <<'PY' || die "fresh Granola binding produced a false-positive connection"
import json, sys
data = json.load(open(sys.argv[1]))
if data.get("connections") != []:
    raise SystemExit(f"fresh workspace binding leaked into machine connection status: {data.get('connections')}")
print("granola_connections=0; binding_is_not_machine_auth=1")
PY
  printf 'phase1 PASS: %s\n' "$summary"
  printf 'Status artifact: %s\n' "$status"
}

auth_instructions() {
  [ "$#" -eq 0 ] || die "auth-instructions takes no options"
  require_sandbox_env
  require_workspace_declaration
  cat <<'EOF'
desktop: run margins connect google [--account <email>] and complete the browser consent screen
headless/ssh: run margins connect google --headless [--account <email>] --json
headless mode prints the consent URL to stderr before consent; the localhost cannot-connect page is expected, and you must copy the entire address-bar URL, not only the code, into the hidden prompt in the same terminal; never paste callback URLs, codes, or tokens into chat
desktop: run margins connect granola [--account <email>] --json and complete the browser consent screen
headless/ssh: run margins connect granola --headless [--account <email>] --json; after local browser consent, copy the entire localhost address-bar URL into that command's hidden prompt; never paste callback URLs, codes, or tokens into chat
EOF
}

create_fake_recall_index() {
  local sources="$MARGINS_E2E_ARTIFACTS/fake-index-source-list.json"
  run_margins source list --json > "$sources"
  python3 - "$REFERENCE_NOTES" "$MARGINS_WORKSPACE_STATE" "$sources" <<'PY'
from pathlib import Path
import json
import sqlite3
import sys
import time

notes = Path(sys.argv[1])
workspace = Path(sys.argv[2])
markdown = [row for row in json.load(open(sys.argv[3])) if row.get("kind") == "notes"]
db = workspace / "enzyme.db"
db.parent.mkdir(parents=True, exist_ok=True)
if db.exists():
    db.unlink()
conn = sqlite3.connect(db)
conn.executescript("""
CREATE TABLE docs (
  id INTEGER PRIMARY KEY,
  source_ref TEXT NOT NULL,
  title TEXT,
  content TEXT,
  created_at INTEGER NOT NULL,
  modified_at INTEGER NOT NULL,
  indexed_at INTEGER NOT NULL,
  metadata TEXT
);
CREATE TABLE doc_links (doc_id INTEGER NOT NULL, link TEXT NOT NULL);
""")
now = time.time_ns() // 1_000_000
# Workspace-language identity: one Markdown source -> root-relative refs;
# several -> "<source name>/<relative>".
source_name = next(row["name"] for row in markdown if Path(row["path"]) == notes)
prefix = "" if len(markdown) == 1 else f"{source_name}/"
for index, path in enumerate(sorted(notes.glob("*.md")), 1):
    relative = str(path.relative_to(notes))
    source_ref = f"{prefix}{relative}"
    content = path.read_text()
    mtime = path.stat().st_mtime_ns // 1_000_000
    title = next((line.removeprefix("title:").strip() for line in content.splitlines() if line.startswith("title:")), path.stem)
    conn.execute("INSERT INTO docs VALUES (?, ?, ?, ?, ?, ?, ?, '{}')", (index, source_ref, title, content, mtime, mtime, now))
conn.commit()
conn.close()
PY
}

phase2() {
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  rm -f -- "$MARGINS_E2E_SANDBOX/isolation-verdict.txt"
  [ "$#" -eq 0 ] || die "phase2 takes no options"

  local auth="$MARGINS_E2E_ARTIFACTS/phase2-google-status.json"
  timed_run "phase2.google-status" "$auth" run_margins connect status --json
  python3 - "$auth" <<'PY' || die "Google status did not show a connected sandbox account"
import json, sys
data = json.load(open(sys.argv[1]))
connections = data.get("connections", [])
if not any(row.get("account") == "owner@example.com" and row.get("connected") is True for row in connections):
    raise SystemExit("owner@example.com is not connected")
PY

  if [ "${MARGINS_E2E_GOOGLE_ONLY:-0}" != 1 ]; then
  local granola_auth="$MARGINS_E2E_ARTIFACTS/phase2-granola-status.json"
  timed_run "phase2.granola-status" "$granola_auth" \
    "$MARGINS_E2E_BIN" connect status --service granola --json
  python3 - "$granola_auth" "$MARGINS_HOME/granola/owner@example.com" <<'PY' || \
    die "Granola status or private credential metadata was incoherent"
import json, stat, sys
from pathlib import Path

status = json.load(open(sys.argv[1]))
account_dir = Path(sys.argv[2])
row = next((row for row in status.get("connections", []) if row.get("account") == "owner@example.com"), None)
if row is None or row.get("connected") is not True or row.get("status") != "connected":
    raise SystemExit(f"owner@example.com is not coherently connected: {row}")
if row.get("storage") != "file_0600":
    raise SystemExit(f"unexpected headless storage: {row.get('storage')!r}")
if sorted(row.get("access") or []) != ["meetings", "notes", "participants", "transcripts"]:
    raise SystemExit("connected Granola status omitted typed access capabilities")
for path, expected in ((account_dir.parent, 0o700), (account_dir, 0o700), (account_dir / "account.json", 0o600), (account_dir / "token-cache.json", 0o600)):
    observed = stat.S_IMODE(path.stat().st_mode)
    if observed != expected:
        raise SystemExit(f"private storage mode mismatch for {path.name}: {observed:o}")
metadata = json.load(open(account_dir / "account.json"))
if metadata.get("account") != "owner@example.com" or not metadata.get("client_id"):
    raise SystemExit("durable Granola account/client identity is incomplete")
required = {"openid", "profile", "email", "offline_access"}
if not required <= set(metadata.get("scopes") or []):
    raise SystemExit("durable Granola scope metadata is incomplete")
print("granola_connected=1; storage_dirs=0700; metadata=0600; token_cache=0600; identity=verified; scopes=4")
PY
  fi

  run_workspace_plan_apply phase2
  local apply_replay="$MARGINS_E2E_ARTIFACTS/phase2-apply-replay.json"
  timed_run "phase2.workspace-apply-replay" "$apply_replay" \
    run_margins workspace apply --plan "$MARGINS_E2E_ARTIFACTS/phase2-plan.json" --json
  python3 - "$apply_replay" "$MARGINS_E2E_ARTIFACTS/phase2-apply.json" <<'PY' || die "workspace apply replay contract failed"
import json, sys
replay = json.load(open(sys.argv[1]))
first = json.load(open(sys.argv[2]))
if replay.get("replayed") is not True:
    raise SystemExit("expected replayed=true on apply replay")
if replay.get("after_revision") != first.get("after_revision"):
    raise SystemExit("apply replay changed revision")
print("apply_replayed=1")
PY

  local reconcile_path
  reconcile_path="$(run_integrations_reconcile phase2 e2e-reconcile-1)"
  local runs_before runs_after
  runs_before="$(ledger_run_count)"
  local reconcile_replay="$MARGINS_E2E_ARTIFACTS/phase2-reconcile-replay.json"
  timed_run "phase2.integrations-reconcile-replay" "$reconcile_replay" \
    run_margins integrations reconcile --if-revision "$(read_workspace_revision "$MARGINS_E2E_ARTIFACTS/phase2-workspace-status.json")" \
      --request-id e2e-reconcile-1 --json
  runs_after="$(ledger_run_count)"
  python3 - "$reconcile_replay" "$reconcile_path" "$runs_before" "$runs_after" <<'PY' || die "integrations reconcile replay contract failed"
import json, sys
replay = json.load(open(sys.argv[1]))
first = json.load(open(sys.argv[2]))
runs_before = int(sys.argv[3])
runs_after = int(sys.argv[4])
if replay.get("replayed") is not True:
    raise SystemExit("expected replayed=true on reconcile replay")
if first.get("schema_version") != "margins.integrations.reconcile.v1":
    raise SystemExit("unexpected reconcile schema on first receipt")
if runs_after != runs_before:
    raise SystemExit(f"reconcile replay created additional ledger runs: before={runs_before} after={runs_after}")
results = replay.get("results") or []
auto = [row for row in results if row.get("connector_id") in {"email", "gcal", "google_meet"}]
if not auto:
    raise SystemExit("reconcile missing automatic connector results")
granola = next((row for row in results if row.get("connector_id") == "granola"), None)
if granola is not None and granola.get("status") not in {"manual", "skipped", "not_applicable"}:
    raise SystemExit(f"expected Granola manual/skipped status, got {granola.get('status')!r}")
print(f"reconcile_replayed=1; automatic_connectors={len(auto)}; ledger_runs={runs_after}")
PY
  local reconcile_summary="$(
    python3 - "$reconcile_path" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
results = data.get("results") or []
email = next((row for row in results if row.get("connector_id") == "email"), {})
for key in ("records_written", "records_updated", "records_unchanged"):
    if key not in email and key not in (email.get("value") or {}):
        pass
print(f"reconcile_results={len(results)}")
PY
)"
  record_observation "phase2: $reconcile_summary"

  if [ "${MARGINS_E2E_GOOGLE_ONLY:-0}" != 1 ]; then
  local granola_live_status="$MARGINS_E2E_ARTIFACTS/phase2-granola-live-workspace-status.json"
  timed_run "phase2.granola-live-workspace-status" "$granola_live_status" \
    run_margins workspace status --json
  local granola_live_revision
  granola_live_revision="$(read_workspace_revision "$granola_live_status")"
  local granola_live="$MARGINS_E2E_ARTIFACTS/phase2-granola-live-reconcile.json"
  timed_run "phase2.granola-live-reconcile" "$granola_live" \
    run_margins integrations reconcile --connector granola --account owner@example.com \
      --if-revision "$granola_live_revision" --request-id e2e-granola-live-1 --json
  local granola_runs_before granola_runs_after
  granola_runs_before="$(ledger_run_count)"
  local granola_replay="$MARGINS_E2E_ARTIFACTS/phase2-granola-live-replay.json"
  timed_run "phase2.granola-live-replay" "$granola_replay" \
    run_margins integrations reconcile --connector granola --account owner@example.com \
      --if-revision "$granola_live_revision" --request-id e2e-granola-live-1 --json
  granola_runs_after="$(ledger_run_count)"
  python3 - "$granola_live" "$granola_replay" "$granola_runs_before" "$granola_runs_after" <<'PY' || \
    die "live Granola MCP reconcile/replay contract failed"
import json, sys
first = json.load(open(sys.argv[1]))
replay = json.load(open(sys.argv[2]))
before, after = map(int, sys.argv[3:])
if first.get("schema_version") != "margins.integrations.reconcile.v1" or first.get("ok") is not True:
    raise SystemExit("live Granola reconcile did not return a successful typed result")
rows = first.get("results") or []
if len(rows) != 1 or rows[0].get("connector_id") != "granola" or rows[0].get("status") != "applied":
    raise SystemExit(f"explicit Granola reconcile returned unexpected rows: {rows}")
counts = sum(int(rows[0].get(key, 0)) for key in ("records_written", "records_updated", "records_unchanged"))
if replay.get("replayed") is not True or before != after:
    raise SystemExit("Granola reconcile replay changed the ledger")
upstream_shape = "authenticated_empty_upstream" if counts == 0 else "authenticated_nonempty_upstream"
print(f"granola_mcp_authenticated=1; upstream_shape={upstream_shape}; records={counts}; replayed=1; ledger_runs={after}")
PY
  record_observation "phase2: authenticated bounded Granola MCP reconcile and idempotent replay verified; an empty upstream set is valid and does not prove nonempty parsing"

  local import_granola="$MARGINS_E2E_ARTIFACTS/phase2-import-granola.xml"
  timed_run "phase2.import-granola" "$import_granola" run_margins import granola \
    "$MARGINS_E2E_SANDBOX/granola-export.json" --account owner@example.com
  record_observation "phase2: supplemental Granola export import verified shared external-document materialization separately from live MCP parsing"
  fi
  assert_calendar_materialization_contract
  assert_phase6_external_document_contract
  python3 - "$reconcile_path" <<'PY' || die "integrations reconcile failed"
import json
from pathlib import Path
import sys

data = json.loads(Path(sys.argv[1]).read_text())
if data.get("schema_version") != "margins.integrations.reconcile.v1":
    raise SystemExit("unexpected reconcile schema")
results = data.get("results") or []
required = {"email", "gcal", "google_meet"}
seen = {row.get("connector_id") for row in results}
if not required <= seen:
    raise SystemExit(f"missing automatic reconcile results: {sorted(required - seen)}")
for row in results:
    if row.get("connector_id") not in required:
        continue
    status = row.get("status")
    if status in {"error", "failed"}:
        raise SystemExit(f"reconcile failed for {row.get('connector_id')}: {row}")
print("email_calendar_and_meet_reconciled=3")
PY

  local init_output="$MARGINS_E2E_ARTIFACTS/phase2-index-init.txt"
  if recall_composition_available; then
    timed_run "phase2.index-refresh" "$init_output" run_margins init
  else
    local started ended
    started="$(now_ms)"
    create_fake_recall_index >"$init_output" 2>&1
    ended="$(now_ms)"
    printf '%s\t%s\t0\n' "phase2.fixture-index" "$((ended - started))" >> "$MARGINS_E2E_TIMINGS"
    printf 'sandbox-local fixture recall index created for CLI without recall composition\n' > "$init_output"
  fi

  record_observation "phase2 complete: $reconcile_summary"
  printf 'complete\n' > "$MARGINS_E2E_SANDBOX/.phase2-complete"
  printf 'phase2 PASS: %s\n' "$reconcile_summary"
  printf 'Artifacts: %s\n' "$MARGINS_E2E_ARTIFACTS"
}

assert_source_remove_preservation() {
  [ "$#" -eq 0 ] || die "assert-source-remove-preservation takes no options"
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  [ -f "$MARGINS_WORKSPACE_STATE/ledger.db" ] || \
    die "ledger.db is required before source-remove preservation checks"
  local ledger_digest sources_before sources_after
  ledger_digest="$(python3 - "$MARGINS_WORKSPACE_STATE/ledger.db" <<'PY'
import hashlib, sys
path = sys.argv[1]
digest = hashlib.sha256()
with open(path, "rb") as handle:
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        digest.update(block)
print(digest.hexdigest())
PY
)"
  sources_before="$MARGINS_E2E_ARTIFACTS/source-remove-before.json"
  timed_run "lifecycle.source-list-before" "$sources_before" run_margins source list --json
  python3 - "$sources_before" <<'PY' || die "mail source missing before removal"
import json, sys
names = {row["name"] for row in json.load(open(sys.argv[1]))}
if "mail" not in names:
    raise SystemExit("mail")
PY
  local removed="$MARGINS_E2E_ARTIFACTS/source-remove-mail.json"
  timed_run "lifecycle.source-remove-mail" "$removed" run_margins source remove mail --json
  sources_after="$MARGINS_E2E_ARTIFACTS/source-remove-after.json"
  timed_run "lifecycle.source-list-after" "$sources_after" run_margins source list --json
  python3 - "$sources_before" "$sources_after" "$MARGINS_WORKSPACE_STATE/ledger.db" "$ledger_digest" <<'PY' || \
    die "source remove did not preserve ledger evidence"
import hashlib, json, sys

before = {row["name"] for row in json.load(open(sys.argv[1]))}
after = {row["name"] for row in json.load(open(sys.argv[2]))}
if "mail" in after:
    raise SystemExit("mail binding still declared")
if before - after != {"mail"}:
    raise SystemExit(f"unexpected binding delta: before={sorted(before)} after={sorted(after)}")
ledger = sys.argv[3]
expected = sys.argv[4]
digest = hashlib.sha256()
with open(ledger, "rb") as handle:
    for block in iter(lambda: handle.read(1024 * 1024), b""):
        digest.update(block)
if digest.hexdigest() != expected:
    raise SystemExit("ledger.db changed after source remove")
print("mail_removed=1; ledger_preserved=1")
PY
  record_observation "lifecycle: source remove mail preserved ledger.db"
  printf 'assert-source-remove-preservation PASS\n'
}

contract_lifecycle() {
  [ "$#" -eq 0 ] || die "contract-lifecycle takes no options"
  assert_binary_provenance
  require_sandbox_env
  local help_path="$MARGINS_E2E_ARTIFACTS/disconnect-google-help.txt"
  "$MARGINS_E2E_BIN" disconnect google --help >"$help_path" 2>&1 || true
  if grep -Eq '(^|[[:space:]])--forget([[:space:]]|$)|(^|[[:space:]])--force([[:space:]]|$)' "$help_path"; then
    die "disconnect google help still documents legacy --forget/--force"
  fi
  grep -Fq -- '--account' "$help_path" || die "disconnect google help missing required --account flag"
  record_observation "lifecycle contract: disconnect google help accepts machine-only --account grammar"
  printf 'contract-lifecycle PASS: machine-only disconnect help grammar\n'
}

contract_phase8() {
  [ "$#" -eq 0 ] || die "contract-phase8 takes no options"
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  [ -f "$MARGINS_WORKSPACE_STATE/ledger.db" ] || \
    die "contract-phase8 requires ledger.db from phase2"
  local status="$MARGINS_E2E_ARTIFACTS/contract-phase8-workspace-status.json"
  timed_run "contract-phase8.workspace-status" "$status" run_margins workspace status --json
  local revision plan apply stale_err
  revision="$(read_workspace_revision "$status")"
  write_desired_fixture "$MARGINS_E2E_ARTIFACTS/contract-phase8-desired.enzyme"
  # Use a new plan identity. Replaying phase2's already-applied no-op plan is
  # valid even after another mutation and cannot test stale-plan rejection.
  python3 - "$MARGINS_E2E_ARTIFACTS/contract-phase8-desired.enzyme" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
# Retention moved to machine config; mutate a program statement instead.
assert "contract-phase8-" not in text
end = text.rstrip().rfind("}")
assert end > 0, "desired program has no closing workspace brace"
path.write_text(text[:end] + '  leave out folders ["contract-phase8-a"]\n' + text[end:])
PY
  plan="$MARGINS_E2E_ARTIFACTS/contract-phase8-plan.json"
  timed_run "contract-phase8.workspace-plan" "$plan" \
    run_margins workspace plan --desired "$MARGINS_E2E_ARTIFACTS/contract-phase8-desired.enzyme" --json
  local revision_marker="$MARGINS_E2E_SANDBOX/contract-phase8-revision-marker"
  mkdir -p "$revision_marker"
  printf '# Phase 8 revision marker\n' > "$revision_marker/marker.md"
  run_margins source add notes --name contract-phase8-revision-marker \
    --role reference --path "$revision_marker" \
    > "$MARGINS_E2E_ARTIFACTS/contract-phase8-revision-marker-add.txt"
  stale_err="$MARGINS_E2E_ARTIFACTS/contract-phase8-stale-apply.json"
  set +e
  run_margins workspace apply --plan "$plan" --json >/dev/null 2>"$stale_err"
  local stale_status=$?
  set -e
  [ "$stale_status" -ne 0 ] || die "stale revision apply unexpectedly succeeded"
  assert_margins_error_code "$stale_err" "workspace_revision_conflict"
  run_margins source remove contract-phase8-revision-marker \
    > "$MARGINS_E2E_ARTIFACTS/contract-phase8-revision-marker-remove.txt"
  apply="$MARGINS_E2E_ARTIFACTS/contract-phase8-apply.json"
  timed_run "contract-phase8.workspace-apply" "$apply" \
    run_margins workspace apply --plan "$plan" --json
  python3 - "$MARGINS_E2E_ARTIFACTS/contract-phase8-desired.enzyme" <<'PY'
from pathlib import Path
import sys
path = Path(sys.argv[1])
text = path.read_text()
assert '"contract-phase8-a"' in text
path.write_text(text.replace('"contract-phase8-a"', '"contract-phase8-b"', 1))
PY
  local followup_plan="$MARGINS_E2E_ARTIFACTS/contract-phase8-followup-plan.json"
  timed_run "contract-phase8.workspace-plan-followup" "$followup_plan" \
    run_margins workspace plan --desired "$MARGINS_E2E_ARTIFACTS/contract-phase8-desired.enzyme" --json
  local followup_apply="$MARGINS_E2E_ARTIFACTS/contract-phase8-followup-apply.json"
  timed_run "contract-phase8.workspace-apply-followup" "$followup_apply" \
    run_margins workspace apply --plan "$followup_plan" --json
  python3 - "$apply" "$followup_apply" <<'PY'
import json, sys
first = json.load(open(sys.argv[1]))
second = json.load(open(sys.argv[2]))
assert first["request_id"] != second["request_id"], (first, second)
assert second["request_id"] == "workspace-apply-" + second["request_hash"], second
PY
  record_observation "contract-phase8: stale revision and derived workspace apply identities verified"
  printf 'contract-phase8 PASS\n'
}

contract_greps() {
  [ "$#" -eq 0 ] || die "contract-greps takes no options"
  local contract="$REPO_ROOT/desktop/INTEGRATIONS_CONNECTOR_CONTRACT.md"
  local recall="$REPO_ROOT/desktop/INTEGRATIONS_RECALL_SQLITE_SOURCE.md"
  local setup_skill="$REPO_ROOT/crates/public/margins-workflows/resources/skills/margins-workspace-setup/SKILL.md"
  local onboarding_skill="$REPO_ROOT/crates/public/margins-workflows/resources/skills/margins-guided-onboarding/SKILL.md"
  for file in "$contract" "$recall" "$setup_skill" "$onboarding_skill"; do
    [ -f "$file" ] || die "contract file missing: $file"
  done
  grep -Fq 'margins.workspace.plan.v2' "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing workspace plan schema"
  grep -Fq 'margins.workspace.apply.v2' "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing workspace apply schema"
  grep -Fq 'margins.integrations.reconcile.v1' "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing integrations reconcile schema"
  grep -Fq 'margins.error.v1' "$contract" \
    || die "contract grep missing margins.error.v1 failure envelope"
  grep -Fq 'workspace plan' "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing workspace plan command"
  grep -Fq 'integrations reconcile' "$contract" "$recall" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing integrations reconcile command"
  if grep -Eiq \
    '`margins[^`]* integrations (survey|approve|pull)|integrations (survey|approve|pull) --' \
    "$contract" "$recall" "$setup_skill" "$onboarding_skill"; then
    die "contract grep found forbidden integrations survey/approve/pull instructions"
  fi
  for pattern in \
    'external_document_evidence' \
    'external_document_participants' \
    'meet_<sha256>' \
    'granola_<sha256>'; do
    grep -Fq "$pattern" "$contract" "$recall" || die "contract grep missing required phrase: $pattern"
  done
  grep -Fq 'margins-managed-projection' "$contract" "$recall" \
    || die "contract grep missing managed projection exclusion"
  grep -Fq 'never creates a capture/session identity' \
    "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing projection/session separation"
  grep -Fq 'margins import granola PATH --account EMAIL' \
    "$contract" "$recall" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing explicit Granola import account"
  grep -Fq 'broad reconcile skips Granola' "$contract" "$recall" \
    || die "contract grep missing Granola broad-reconcile skip semantics"
  grep -Fq 'transport/auth only' "$contract" "$recall" \
    || die "contract grep missing Granola transport/auth-only status"
  grep -Fq 'last-sync authority' "$contract" "$recall" \
    || die "contract grep missing absence of global last-sync authority"
  grep -Fq 'deletion semantics comparable to Meet tombstones' \
    "$contract" "$recall" \
    || die "contract must clarify Granola does not claim Meet-style tombstone deletion"
  grep -Fq 'margins.retention.preview.v1' "$contract" "$recall" "$setup_skill" \
    || die "contract grep missing retention preview schema"
  grep -Fq 'margins.retention.apply.v1' "$contract" "$recall" "$setup_skill" \
    || die "contract grep missing retention apply schema"
  grep -Fq 'retention preview' "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing retention preview command"
  grep -Fq 'retention apply' "$contract" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing retention apply command"
  grep -Fq 'raw_cache_max_age_days' "$contract" \
    || die "contract grep missing RetentionPolicy raw_cache_max_age_days"
  grep -Fq 'tombstone_max_age_days' "$contract" \
    || die "contract grep missing RetentionPolicy tombstone_max_age_days"
  grep -Eiq 'never automatic|never runs automatically|no automatic purge' \
    "$contract" "$recall" "$setup_skill" "$onboarding_skill" \
    || die "contract grep missing explicit never-automatic retention doctrine"
  grep -Fq 'no provider deletion' "$contract" "$recall" \
    || die "contract grep missing no provider deletion boundary"
  grep -Fq 'no native Markdown deletion' "$contract" "$recall" "$setup_skill" \
    || die "contract grep missing no native Markdown deletion boundary"
  grep -Fq 'no Granola projection-file deletion' "$contract" "$recall" "$setup_skill" \
    || die "contract grep missing no Granola projection-file deletion boundary"
  grep -Fq 'no message-level Gmail target' "$contract" \
    || die "contract grep missing no message-level Gmail target boundary"
  grep -Fq 'index_refresh_required' "$contract" \
    || die "contract grep missing retention preview index_refresh_required field"
  python3 - "$contract" "$recall" "$setup_skill" "$onboarding_skill" <<'PY' || \
    die "contract grep found forbidden implicit-purge/provider-delete/native/projection-delete claims"
import re
import sys

patterns = [
    (r'\bautomatic purge\b', r'\bno automatic purge\b'),
    (r'\bauto purge\b', None),
    (r'\bimplicit purge\b', None),
    (r'\bimplicitly purge\b', None),
    (r'\bprovider deletion\b', r'\bno provider deletion\b'),
    (r'\bnative markdown deletion\b', r'\bno native markdown deletion\b'),
    (r'\bprojection-file deletion\b', r'\bno granola projection-file deletion\b'),
    (r'\bmessage-level gmail target\b', r'\bno message-level gmail target\b'),
    (r'\bdelete native markdown\b', None),
    (r'\bdelete.*provider\b', None),
]
for path in sys.argv[1:]:
    for line_no, line in enumerate(open(path), 1):
        lowered = line.lower()
        for pattern, allowed in patterns:
            if re.search(pattern, lowered):
                if allowed and re.search(allowed, lowered):
                    continue
                raise SystemExit(f"{path}:{line_no}: forbidden claim: {line.strip()}")
PY
  if grep -Eiq \
    'granola.*absolute `path`|already-native note|required home transcript|Meet projects to `<home>/Margins/Transcripts`' \
    "$contract" "$recall" "$setup_skill" "$onboarding_skill"; then
    die "contract grep found forbidden legacy Meet/Granola Episode authority phrasing"
  fi
  if [ -n "${MARGINS_E2E_OBSERVATIONS:-}" ]; then
    record_observation "contract-greps: Phase 8/9 public contract phrases present; legacy survey/approve/pull and forbidden purge-delete claims absent"
  fi
  printf 'contract-greps PASS\n'
}

assert_retention_preview_schema() {
  local path="$1"
  python3 - "$path" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data.get("schema_version") != "margins.retention.preview.v1":
    raise SystemExit(f"unexpected preview schema: {data.get('schema_version')!r}")
required = (
    "workspace_id",
    "revision",
    "plan_id",
    "target",
    "scope",
    "counts",
    "ledger_fingerprint",
    "destructive",
    "index_refresh_required",
)
for key in required:
    if key not in data:
        raise SystemExit(f"preview missing required field: {key}")
target = data["target"]
for key in ("connector_id", "source_account"):
    if not target.get(key):
        raise SystemExit(f"preview target missing {key}")
if data.get("destructive") is not True:
    raise SystemExit("preview must mark destructive=true")
print(f"plan_id={data['plan_id']}; scope={data['scope']}")
PY
}

assert_retention_apply_schema() {
  local path="$1"
  python3 - "$path" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data.get("schema_version") != "margins.retention.apply.v1":
    raise SystemExit(f"unexpected apply schema: {data.get('schema_version')!r}")
required = (
    "workspace_id",
    "revision",
    "request_id",
    "request_hash",
    "plan_id",
    "target",
    "scope",
    "deleted",
    "replayed",
    "index_refresh_required",
)
for key in required:
    if key not in data:
        raise SystemExit(f"apply missing required field: {key}")
print(f"plan_id={data['plan_id']}; replayed={data['replayed']}")
PY
}

ledger_table_count() {
  local table="$1"
  python3 - "$MARGINS_WORKSPACE_STATE/ledger.db" "$table" <<'PY'
import sqlite3
import sys

ledger, table = sys.argv[1], sys.argv[2]
db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
if table not in tables:
    print(0)
else:
    print(db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0])
PY
}

contract_phase9() {
  [ "$#" -eq 0 ] || die "contract-phase9 takes no options"
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  [ -f "$MARGINS_WORKSPACE_STATE/ledger.db" ] || \
    die "contract-phase9 requires ledger.db from phase2"
  local preview_help="$MARGINS_E2E_ARTIFACTS/contract-phase9-retention-preview-help.txt"
  local apply_help="$MARGINS_E2E_ARTIFACTS/contract-phase9-retention-apply-help.txt"
  "$MARGINS_E2E_BIN" retention preview --help >"$preview_help"
  "$MARGINS_E2E_BIN" retention apply --help >"$apply_help"
  grep -Fq -- '--connector' "$preview_help" || die "retention preview help missing --connector"
  grep -Fq -- '--account' "$preview_help" || die "retention preview help missing --account"
  grep -Fq -- '--scope' "$preview_help" || die "retention preview help missing --scope"
  grep -Fq -- '--plan' "$apply_help" || die "retention apply help missing --plan"
  grep -Fq -- '--if-revision' "$apply_help" || die "retention apply help missing --if-revision"
  grep -Fq -- '--request-id' "$apply_help" || die "retention apply help missing --request-id"
  local raw_before thread_before connectors_before
  raw_before="$(ledger_table_count raw_items)"
  thread_before="$(ledger_table_count thread_evidence)"
  connectors_before="$(ledger_table_count connectors)"
  [ "$raw_before" -gt 0 ] || die "contract-phase9 requires raw_items rows from native Google reconcile"
  [ "$thread_before" -gt 0 ] || die "contract-phase9 requires thread_evidence rows from native Google reconcile"
  local status="$MARGINS_E2E_ARTIFACTS/contract-phase9-workspace-status.json"
  timed_run "contract-phase9.workspace-status" "$status" run_margins workspace status --json
  local revision preview apply replay stale_err idempotency_err
  revision="$(read_workspace_revision "$status")"
  preview="$MARGINS_E2E_ARTIFACTS/contract-phase9-preview-raw-cache.json"
  timed_run "contract-phase9.retention-preview" "$preview" \
    run_margins retention preview \
      --connector email --account owner@example.com --scope raw-cache --json
  assert_retention_preview_schema "$preview"
  python3 - "$preview" <<'PY' || die "raw-cache preview contract failed"
import json, sys
data = json.load(open(sys.argv[1]))
if data.get("scope") != "raw_cache":
    raise SystemExit(f"expected raw_cache scope, got {data.get('scope')!r}")
counts = data.get("counts") or {}
if int(counts.get("raw", counts.get("raw_items", 0)) or 0) <= 0:
    raise SystemExit("raw-cache preview must report raw rows to delete")
if data.get("index_refresh_required") is True:
    raise SystemExit("raw-cache preview must not require index refresh")
print(f"raw_rows={counts.get('raw', counts.get('raw_items'))}")
PY
  local preview_repeat="$MARGINS_E2E_ARTIFACTS/contract-phase9-preview-repeat.json"
  timed_run "contract-phase9.retention-preview-repeat" "$preview_repeat" \
    run_margins retention preview \
      --connector email --account owner@example.com --scope raw-cache --json
  python3 - "$preview" "$preview_repeat" <<'PY' || die "retention preview is not deterministic"
import json, sys
first = json.load(open(sys.argv[1]))
second = json.load(open(sys.argv[2]))
for key in ("revision", "plan_id", "ledger_fingerprint"):
    if first.get(key) != second.get(key):
        raise SystemExit(f"preview drift on {key}: {first.get(key)!r} vs {second.get(key)!r}")
print("preview_deterministic=1")
PY
  local materialization_preview="$MARGINS_E2E_ARTIFACTS/contract-phase9-preview-materialization.json"
  timed_run "contract-phase9.retention-preview-materialization" "$materialization_preview" \
    run_margins retention preview \
      --connector email --account owner@example.com --scope materialization --json
  assert_retention_preview_schema "$materialization_preview"
  python3 - "$materialization_preview" <<'PY' || die "materialization preview contract failed"
import json, sys
data = json.load(open(sys.argv[1]))
if data.get("scope") != "materialization":
    raise SystemExit(f"expected materialization scope, got {data.get('scope')!r}")
if data.get("index_refresh_required") is not True:
    raise SystemExit("materialization preview must require index refresh")
counts = data.get("counts") or {}
active = int(counts.get("active", counts.get("active_evidence", 0)) or 0)
if active <= 0:
    raise SystemExit("materialization preview must report active evidence rows")
print(f"active_rows={active}; index_refresh_required=1")
PY
  apply="$MARGINS_E2E_ARTIFACTS/contract-phase9-apply-raw-cache.json"
  timed_run "contract-phase9.retention-apply" "$apply" \
    run_margins retention apply --plan "$preview" \
      --if-revision "$revision" --request-id contract-phase9-raw-cache --json
  assert_retention_apply_schema "$apply"
  python3 - "$apply" <<'PY' || die "raw-cache apply replayed flag wrong on first apply"
import json, sys
data = json.load(open(sys.argv[1]))
if data.get("replayed") is True:
    raise SystemExit("first retention apply must not replay")
print("first_apply=1")
PY
  replay="$MARGINS_E2E_ARTIFACTS/contract-phase9-apply-replay.json"
  timed_run "contract-phase9.retention-apply-replay" "$replay" \
    run_margins retention apply --plan "$preview" \
      --if-revision "$revision" --request-id contract-phase9-raw-cache --json
  assert_retention_apply_schema "$replay"
  python3 - "$apply" "$replay" <<'PY' || die "retention apply replay contract failed"
import json, sys
first = json.load(open(sys.argv[1]))
replay = json.load(open(sys.argv[2]))
if replay.get("replayed") is not True:
    raise SystemExit("expected replayed=true on retention apply replay")
if replay.get("plan_id") != first.get("plan_id"):
    raise SystemExit("apply replay changed plan_id")
print("apply_replayed=1")
PY
  local revision_marker="$MARGINS_E2E_SANDBOX/contract-phase9-revision-marker"
  mkdir -p "$revision_marker"
  printf '# Phase 9 revision marker\n' > "$revision_marker/marker.md"
  local config_drift_out="$MARGINS_E2E_ARTIFACTS/contract-phase9-revision-marker-add.txt"
  timed_run "contract-phase9.config-drift" "$config_drift_out" \
    run_margins source add notes --name contract-phase9-revision-marker \
      --role reference --path "$revision_marker"
  stale_err="$MARGINS_E2E_ARTIFACTS/contract-phase9-stale-apply.json"
  set +e
  run_margins retention apply --plan "$preview" \
    --if-revision "$revision" --request-id contract-phase9-stale --json \
    >/dev/null 2>"$stale_err"
  local stale_status=$?
  set -e
  [ "$stale_status" -ne 0 ] || die "stale revision retention apply unexpectedly succeeded"
  assert_margins_error_code "$stale_err" "workspace_revision_conflict"
  local current_status="$MARGINS_E2E_ARTIFACTS/contract-phase9-current-workspace-status.json"
  timed_run "contract-phase9.current-workspace-status" "$current_status" \
    run_margins workspace status --json
  local current_revision
  current_revision="$(read_workspace_revision "$current_status")"
  local preview_conflict="$MARGINS_E2E_ARTIFACTS/contract-phase9-preview-after-raw-cache.json"
  timed_run "contract-phase9.retention-preview-after-raw-cache" "$preview_conflict" \
    run_margins retention preview \
      --connector email --account owner@example.com --scope raw-cache --json
  idempotency_err="$MARGINS_E2E_ARTIFACTS/contract-phase9-idempotency.json"
  set +e
  run_margins retention apply --plan "$preview_conflict" \
    --if-revision "$current_revision" --request-id contract-phase9-raw-cache --json \
    >/dev/null 2>"$idempotency_err"
  local idempotency_status=$?
  set -e
  [ "$idempotency_status" -ne 0 ] || die "retention idempotency conflict apply unexpectedly succeeded"
  assert_margins_error_code "$idempotency_err" "idempotency_conflict"
  local reconcile_mutation="$MARGINS_E2E_ARTIFACTS/contract-phase9-reconcile-ledger-mutation.json"
  timed_run "contract-phase9.reconcile-ledger-mutation" "$reconcile_mutation" \
    run_margins integrations reconcile --if-revision "$current_revision" \
      --request-id contract-phase9-stale-plan-ledger-mutation --json
  stale_plan_err="$MARGINS_E2E_ARTIFACTS/contract-phase9-stale-plan.json"
  set +e
  run_margins retention apply --plan "$preview_conflict" \
    --if-revision "$current_revision" --request-id contract-phase9-stale-plan --json \
    >/dev/null 2>"$stale_plan_err"
  local stale_plan_status=$?
  set -e
  [ "$stale_plan_status" -ne 0 ] || die "stale retention plan apply unexpectedly succeeded"
  python3 - "$stale_plan_err" <<'PY' || die "stale retention plan did not return margins.error.v1"
import json, sys
raw = open(sys.argv[1]).read().strip()
if not raw:
    raise SystemExit("empty stale-plan error payload")
data = json.loads(raw)
if data.get("schema_version") != "margins.error.v1":
    raise SystemExit(f"unexpected error schema: {data.get('schema_version')!r}")
code = str((data.get("error") or {}).get("code", ""))
if code != "retention_plan_stale":
    raise SystemExit(f"expected retention_plan_stale, got {code!r}")
print(f"stale_plan_code={code}")
PY
  local final_preview="$MARGINS_E2E_ARTIFACTS/contract-phase9-preview-final-raw-cache.json"
  local final_apply="$MARGINS_E2E_ARTIFACTS/contract-phase9-apply-final-raw-cache.json"
  timed_run "contract-phase9.retention-preview-final" "$final_preview" \
    run_margins retention preview \
      --connector email --account owner@example.com --scope raw-cache --json
  timed_run "contract-phase9.retention-apply-final" "$final_apply" \
    run_margins retention apply --plan "$final_preview" \
      --if-revision "$current_revision" --request-id contract-phase9-final-raw-cache --json
  assert_retention_apply_schema "$final_apply"
  local raw_after thread_after connectors_after
  raw_after="$(ledger_table_count raw_items)"
  thread_after="$(ledger_table_count thread_evidence)"
  connectors_after="$(ledger_table_count connectors)"
  [ "$raw_after" -lt "$raw_before" ] || die "raw-cache retention apply did not delete raw_items"
  [ "$thread_after" = "$thread_before" ] || \
    die "raw-cache retention apply changed thread_evidence: before=$thread_before after=$thread_after"
  [ "$connectors_after" = "$connectors_before" ] || \
    die "raw-cache retention apply changed connectors: before=$connectors_before after=$connectors_after"
  record_observation "contract-phase9: raw-cache retention preview/apply/replay and conflict envelopes verified; evidence preserved"
  printf 'contract-phase9 PASS\n'
}

assert_machine_forget_preservation() {
  local granola_only=0
  if [ "${1:-}" = "--granola-only" ]; then
    granola_only=1
    shift
  fi
  [ "$#" -eq 0 ] || die "assert-machine-forget-preservation takes no options"
  assert_binary_provenance
  require_sandbox_env
  require_workspace_declaration
  local before="$MARGINS_E2E_ARTIFACTS/machine-forget-before.json"
  local after="$MARGINS_E2E_ARTIFACTS/machine-forget-after.json"
  local result="$MARGINS_E2E_ARTIFACTS/machine-forget.json"
  python3 - "$MARGINS_HOME" "$before" <<'PY'
import hashlib, json, sqlite3, sys
from pathlib import Path

home, output = map(Path, sys.argv[1:])
snapshot = {"workspaces": {}}
programs = sorted(path for path in (home / "configs").glob("*.enzyme") if path.name != "profiles.enzyme")
if not programs:
    raise SystemExit("no Workspace programs under configs/")
for config in programs:
    state = home / "workspaces" / config.stem
    row = {"config_sha256": hashlib.sha256(config.read_bytes()).hexdigest()}
    index = state / "enzyme.db"
    row["index_sha256"] = hashlib.sha256(index.read_bytes()).hexdigest() if index.exists() else None
    ledger = state / "ledger.db"
    if ledger.exists():
        db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
        row["counts"] = {
            table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
            for table in (
                "connectors",
                "reconcile_receipts",
                "raw_items",
                "thread_evidence",
                "participant_threads",
                "calendar_event_evidence",
                "calendar_event_attendees",
                "external_document_evidence",
                "external_document_participants",
            )
        }
        schema_objects = {row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
        )}
        for forbidden in ("episodes", "recall_items"):
            if forbidden in schema_objects:
                raise SystemExit(f"generic Episode framework object must be absent: {forbidden}")
        db.close()
    snapshot["workspaces"][config.stem] = row
output.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
PY
  if [ "$granola_only" -eq 0 ]; then
  "$MARGINS_E2E_BIN" disconnect google --account owner@example.com --json > "$result"
  "$MARGINS_E2E_BIN" connect status --json > "$MARGINS_E2E_ARTIFACTS/machine-status-after-forget.json"
  python3 - "$MARGINS_HOME" "$before" "$after" "$result" "$MARGINS_E2E_ARTIFACTS/machine-status-after-forget.json" <<'PY' || \
    die "machine forget changed Workspace desired state or retained evidence"
import hashlib, json, sqlite3, sys
from pathlib import Path

home, before_path, after_path, result_path, status_path = map(Path, sys.argv[1:])
before = json.load(open(before_path))
after = {"workspaces": {}}
programs = sorted(path for path in (home / "configs").glob("*.enzyme") if path.name != "profiles.enzyme")
if not programs:
    raise SystemExit("no Workspace programs under configs/")
for config in programs:
    state = home / "workspaces" / config.stem
    row = {"config_sha256": hashlib.sha256(config.read_bytes()).hexdigest()}
    index = state / "enzyme.db"
    row["index_sha256"] = hashlib.sha256(index.read_bytes()).hexdigest() if index.exists() else None
    ledger = state / "ledger.db"
    if ledger.exists():
        db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
        row["counts"] = {
            table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
            for table in (
                "connectors",
                "reconcile_receipts",
                "raw_items",
                "thread_evidence",
                "participant_threads",
                "calendar_event_evidence",
                "calendar_event_attendees",
                "external_document_evidence",
                "external_document_participants",
            )
        }
        schema_objects = {row[0] for row in db.execute(
            "SELECT name FROM sqlite_master WHERE type IN ('table', 'view')"
        )}
        for forbidden in ("episodes", "recall_items"):
            if forbidden in schema_objects:
                raise SystemExit(f"generic Episode framework object must be absent: {forbidden}")
        google = db.execute(
            "SELECT health_status, last_sync_at FROM connectors WHERE connector_id IN ('email','gcal','google_meet')"
        ).fetchall()
        if google and any(status != "needs-auth" or last_sync is None for status, last_sync in google):
            raise SystemExit(f"Google health did not preserve snapshot as needs-auth: {google}")
        db.close()
    after["workspaces"][config.stem] = row
after_path.write_text(json.dumps(after, indent=2, sort_keys=True) + "\n")
if after != before:
    raise SystemExit(f"retained state changed: before={before} after={after}")
result = json.load(open(result_path))
if result.get("scope") != "machine" or result.get("forgotten") is not True:
    raise SystemExit(f"unexpected forget result: {result}")
if result.get("retained_workspaces") != ["fresh-onboarding", "second-practice"]:
    raise SystemExit(f"unexpected retained workspaces: {result}")
status = json.load(open(status_path))
connection = next(row for row in status["connections"] if row["account"] == "owner@example.com")
if connection.get("connected") is not False or connection.get("workspaces") != ["fresh-onboarding", "second-practice"]:
    raise SystemExit(f"unexpected machine status: {connection}")
if (home / "google" / "owner@example.com").exists():
    raise SystemExit("machine Google home survived forget")
print("machine_forgotten=1; bindings_preserved=1; evidence_preserved=1; health=needs-auth")
PY
  fi
  if [ "${MARGINS_E2E_GOOGLE_ONLY:-0}" != 1 ]; then
  local granola_result="$MARGINS_E2E_ARTIFACTS/granola-machine-forget.json"
  local granola_status="$MARGINS_E2E_ARTIFACTS/granola-machine-status-after-forget.json"
  "$MARGINS_E2E_BIN" disconnect granola --account owner@example.com --json > "$granola_result"
  "$MARGINS_E2E_BIN" connect status --service granola --json > "$granola_status"
  python3 - "$MARGINS_HOME" "$before" "$granola_result" "$granola_status" <<'PY' || \
    die "Granola disconnect changed Workspace desired state or retained evidence"
import hashlib, json, sqlite3, sys
from pathlib import Path

home, before_path, result_path, status_path = map(Path, sys.argv[1:])
before = json.load(open(before_path))
after = {"workspaces": {}}
programs = sorted(path for path in (home / "configs").glob("*.enzyme") if path.name != "profiles.enzyme")
if not programs:
    raise SystemExit("no Workspace programs under configs/")
for config in programs:
    state = home / "workspaces" / config.stem
    row = {"config_sha256": hashlib.sha256(config.read_bytes()).hexdigest()}
    index = state / "enzyme.db"
    row["index_sha256"] = hashlib.sha256(index.read_bytes()).hexdigest() if index.exists() else None
    ledger = state / "ledger.db"
    if ledger.exists():
        db = sqlite3.connect(f"file:{ledger}?mode=ro", uri=True)
        row["counts"] = {
            table: db.execute(f"SELECT COUNT(*) FROM {table}").fetchone()[0]
            for table in (
                "connectors", "reconcile_receipts", "raw_items", "thread_evidence",
                "participant_threads", "calendar_event_evidence", "calendar_event_attendees",
                "external_document_evidence", "external_document_participants",
            )
        }
        granola = db.execute(
            "SELECT health_status, last_sync_at FROM connectors WHERE connector_id = 'granola'"
        ).fetchall()
        if granola and any(status != "needs-auth" or last_sync is None for status, last_sync in granola):
            raise SystemExit(f"Granola health did not preserve snapshot as needs-auth: {granola}")
        db.close()
    after["workspaces"][config.stem] = row
if after != before:
    raise SystemExit("Granola disconnect changed retained config/index/evidence shapes")
result = json.load(open(result_path))
if result.get("scope") != "machine" or result.get("forgotten") is not True:
    raise SystemExit(f"unexpected Granola forget result: {result}")
if result.get("retained_workspaces") != ["fresh-onboarding"]:
    raise SystemExit(f"unexpected Granola Workspace associations: {result}")
status = json.load(open(status_path))
if status.get("connections") != []:
    raise SystemExit(f"Granola machine status retained credentials after disconnect: {status.get('connections')}")
if (home / "granola" / "owner@example.com").exists():
    raise SystemExit("machine Granola credential home survived disconnect")
print("granola_forgotten=1; binding_preserved=1; external_evidence_preserved=1; health=needs-auth")
PY
  fi
  record_observation "lifecycle: machine forget preserved bindings/evidence and marked snapshots needs-auth"
  printf 'assert-machine-forget-preservation PASS\n'
}

verify_isolation() {
  [ "$#" -eq 0 ] || die "verify-isolation takes no options"
  assert_binary_provenance
  require_sandbox_env
  assert_workspace_layout
  local baseline="$MARGINS_E2E_SANDBOX/isolation-baseline.json"
  local current="$MARGINS_E2E_SANDBOX/isolation-current.json"
  [ -f "$baseline" ] || die "isolation baseline is missing: $baseline"
  python3 - "$MARGINS_E2E_SANDBOX" "$MARGINS_HOME" <<'PY'
from pathlib import Path
import sys

sandbox, margins_home = (Path(value).resolve() for value in sys.argv[1:])
google_home = margins_home / "google" / "owner@example.com"
granola_home = margins_home / "granola" / "owner@example.com"
for label, path in (("MARGINS_HOME", margins_home), ("Margins-managed Google home", google_home), ("Margins-managed Granola home", granola_home)):
    if path != sandbox and sandbox not in path.parents:
        raise SystemExit(f"ISOLATION FAILURE: {label} is outside sandbox: {path}")
PY
  if [ "$(uname -s)" = "Darwin" ]; then
    local keychain_baseline="$MARGINS_E2E_SANDBOX/keychain-items-baseline.json"
    [ -f "$keychain_baseline" ] || die "macOS keychain baseline is missing: $keychain_baseline"
    python3 - "$keychain_baseline" <<'PY'
import hashlib, json, re, subprocess, sys
baseline = json.load(open(sys.argv[1]))["items"]
result = subprocess.run(["security", "dump-keychain"], capture_output=True, text=True)
if result.returncode:
    raise SystemExit("ISOLATION FAILURE: could not inspect the macOS user keychain")
blocks = re.split(r'(?=^keychain: )', result.stdout, flags=re.MULTILINE)
def fingerprint(block):
    attributes = {}
    for line in block.splitlines():
        match = re.match(r'^\s*"(svce|acct|cdat)"<[^>]+>=(.*)$', line)
        if match:
            attributes[match.group(1)] = match.group(2).strip()
    identity = json.dumps([attributes.get(key, "") for key in ("svce", "acct", "cdat")])
    return hashlib.sha256(identity.encode()).hexdigest(), hashlib.sha256(block.encode()).hexdigest()
current = dict(fingerprint(block) for block in blocks if block.startswith("keychain: "))
added = set(current) - set(baseline)
if added:
    raise SystemExit(f"ISOLATION FAILURE: {len(added)} macOS keychain item(s) were created during the harness run")
modified = sum(current[identity] != baseline[identity] for identity in set(current) & set(baseline))
print(f"macOS keychain check: no new items; {modified} pre-existing item(s) modified (informational)")
PY
  else
    printf 'macOS keychain check: skipped on %s\n' "$(uname -s)"
  fi
  python3 - "$baseline" "$current" <<'PY'
import json
from pathlib import Path
import sys

baseline_path = Path(sys.argv[1])
current_path = Path(sys.argv[2])
baseline = json.loads(baseline_path.read_text())

# Reproduce the Bash snapshot algorithm directly to avoid trusting current env paths.
import hashlib, os, stat
def item(path):
    try: info = path.lstat()
    except FileNotFoundError: return {"type": "missing"}
    base = {"mode": stat.S_IMODE(info.st_mode), "mtime_ns": info.st_mtime_ns, "size": info.st_size}
    if path.is_symlink(): base.update(type="symlink", target=os.readlink(path))
    elif path.is_file():
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for block in iter(lambda: handle.read(1024 * 1024), b""): digest.update(block)
        base.update(type="file", sha256=digest.hexdigest())
    elif path.is_dir(): base.update(type="directory")
    else: base.update(type="other")
    return base
snapshot = {"roots": baseline["roots"], "entries": {}}
for raw_root in baseline["roots"]:
    root = Path(raw_root)
    snapshot["entries"][raw_root] = item(root)
    if root.is_dir() and not root.is_symlink():
        for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
            dirnames.sort(); filenames.sort()
            parent = Path(dirpath)
            for name in dirnames + filenames:
                path = parent / name
                snapshot["entries"][str(path)] = item(path)
current_path.write_text(json.dumps(snapshot, indent=2, sort_keys=True) + "\n")
if baseline != snapshot:
    before = baseline["entries"]
    after = snapshot["entries"]
    changed = sorted(path for path in set(before) | set(after) if before.get(path) != after.get(path))
    print("ISOLATION FAILURE: protected real auth/config paths drifted:", file=sys.stderr)
    for path in changed[:30]: print(f"  {path}", file=sys.stderr)
    raise SystemExit(1)
PY
  local kib
  kib="$(python3 - "$MARGINS_E2E_SANDBOX" <<'PY'
from pathlib import Path
import sys
total = sum(p.lstat().st_size for p in Path(sys.argv[1]).rglob('*') if p.is_file() or p.is_symlink())
print((total + 1023) // 1024)
PY
)"
  printf 'verify-isolation PASS: protected real paths are byte/mtime-identical\n'
  printf 'Sandbox size: %s KiB\n' "$kib"
  printf 'Margins-managed Google home: %s (inside sandbox)\n' "$MARGINS_HOME/google/owner@example.com"
  printf 'Margins-managed Granola home: %s (inside sandbox)\n' "$MARGINS_HOME/granola/owner@example.com"
  printf 'Cleanup when finished (never run automatically):\n  rm -rf -- %s\n' "$(shell_quote "$MARGINS_E2E_SANDBOX")"
  printf 'pass\n' > "$MARGINS_E2E_SANDBOX/isolation-verdict.txt"
}

report_scorecard() {
  [ "$#" -eq 0 ] || die "report takes no options"
  assert_binary_provenance
  require_sandbox_env
  rm -f -- "$MARGINS_E2E_SANDBOX/isolation-verdict.txt"
  verify_isolation
  local workspace_status="$MARGINS_E2E_ARTIFACTS/report-workspace-status.json"
  timed_run "report.workspace-status" "$workspace_status" run_margins workspace status --json
  python3 - "$MARGINS_E2E_SANDBOX" "$workspace_status" <<'PY'
from pathlib import Path
import json
import sys

root = Path(sys.argv[1])
workspace_status = json.loads(Path(sys.argv[2]).read_text())
latest = {}
timings = root / "timings.tsv"
if timings.exists():
    for line in timings.read_text().splitlines():
        if not line.strip(): continue
        label, elapsed, status = line.split("\t")
        latest[label] = (int(elapsed), int(status))

def survey_counts(name):
    path = root / "artifacts" / name
    if not path.exists(): return {}
    data = json.loads(path.read_text())
    return {
        "sources": len(data.get("sources", [])),
        "proposals": sum(len(row.get("proposals", [])) for row in data.get("sources", [])),
    }

def reconcile_counts(name):
    path = root / "artifacts" / name
    if not path.exists(): return {}
    data = json.loads(path.read_text())
    results = data.get("results") or []
    records = 0
    for row in results:
        value = row.get("value") or row
        records += sum(int(value.get(key, 0) or 0) for key in ("records_written", "records_updated", "records_unchanged"))
    return {"results": len(results), "records": records}

records = reconcile_counts("phase2-reconcile.json").get("records", 0)

print("Fresh Margins onboarding scorecard")
for label, (elapsed, status) in latest.items():
    print(f"  {label}: {elapsed} ms ({'PASS' if status == 0 else 'FAIL'})")
print(f"  phase1 counts: status-only")
print(f"  phase2 counts: {reconcile_counts('phase2-reconcile.json')}, records={records}")
verdict = (root / "isolation-verdict.txt").read_text().strip().upper() if (root / "isolation-verdict.txt").exists() else "NOT VERIFIED"
print(f"  isolation: {verdict}")
catalyst = workspace_status.get("catalyst", {})
if catalyst:
    print(f"  catalyst mode: {catalyst.get('mode', 'unknown')} ({catalyst.get('reason', 'unknown')})")
else:
    print(f"  local recall mode: {workspace_status['recall']['mode']}")
provenance = json.loads((root / "artifacts" / "binary-provenance.json").read_text())
build = provenance["build"]
binary = provenance["binary"]
branch = f" branch={build['branch']}" if build.get("branch") else ""
print(f"  binary provenance: commit={build['commit']} short={build['short']} dirty={str(build['dirty']).lower()}{branch} built_at={build['built_at']} profile={build['profile']} sha256={binary['sha256']}")
print("\nTrust-friction observations")
observations = root / "trust-friction-observations.txt"
print(observations.read_text().rstrip() if observations.exists() and observations.stat().st_size else "(none recorded)")
PY
}

command="${1:-}"
if [ -z "$command" ]; then usage; exit 2; fi
shift
case "$command" in
  init) init_sandbox "$@" ;;
  phase1) phase1 "$@" ;;
  auth-instructions) auth_instructions "$@" ;;
  phase2) phase2 "$@" ;;
  assert-source-remove-preservation) assert_source_remove_preservation "$@" ;;
  contract-lifecycle) contract_lifecycle "$@" ;;
  contract-greps) contract_greps "$@" ;;
  contract-phase8) contract_phase8 "$@" ;;
  contract-phase9) contract_phase9 "$@" ;;
  assert-machine-forget-preservation) assert_machine_forget_preservation "$@" ;;
  verify-isolation) verify_isolation "$@" ;;
  report) report_scorecard "$@" ;;
  -h|--help|help) usage ;;
  *) usage >&2; die "unknown subcommand: $command" ;;
esac
