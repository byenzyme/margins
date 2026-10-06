#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$REPO_ROOT/scripts/e2e-fresh-onboarding.sh"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-onboarding-ci.XXXXXX")"
FIXTURE_PID=""
GOOGLE_FIXTURE_PID=""
cleanup() {
  if [ -n "$FIXTURE_PID" ]; then kill "$FIXTURE_PID" 2>/dev/null || true; fi
  if [ -n "$GOOGLE_FIXTURE_PID" ]; then kill "$GOOGLE_FIXTURE_PID" 2>/dev/null || true; fi
  rm -rf -- "$RUN_ROOT"
}
trap cleanup EXIT

: "${MARGINS_E2E_BIN:?set MARGINS_E2E_BIN to the margins CLI under test}"
# This CI fixture supplies Google HTTP responses. A live Granola MCP session
# belongs to the separate authenticated lane, not this isolated Google run.
export MARGINS_E2E_GOOGLE_ONLY=1

OBSERVED_BUILD_COMMIT="$("$MARGINS_E2E_BIN" capabilities | python3 -c \
  'import json, sys; print(json.load(sys.stdin)["build"]["commit"])')"
MISMATCHED_BUILD_COMMIT=0000000000000000000000000000000000000000
if [ "$OBSERVED_BUILD_COMMIT" = "$MISMATCHED_BUILD_COMMIT" ]; then
  MISMATCHED_BUILD_COMMIT=1111111111111111111111111111111111111111
fi
set +e
MARGINS_E2E_EXPECTED_COMMIT="$MISMATCHED_BUILD_COMMIT" \
  "$HARNESS" init --sandbox "$RUN_ROOT/mismatch-sandbox" \
  > "$RUN_ROOT/mismatch.stdout" 2> "$RUN_ROOT/mismatch.stderr"
MISMATCH_STATUS=$?
set -e
[ "$MISMATCH_STATUS" -ne 0 ]
grep -Fxq \
  "fresh-onboarding e2e: ERROR: binary build commit mismatch: observed=$OBSERVED_BUILD_COMMIT expected=$MISMATCHED_BUILD_COMMIT" \
  "$RUN_ROOT/mismatch.stderr"
test ! -e "$RUN_ROOT/mismatch-sandbox"

export MARGINS_E2E_EXPECTED_COMMIT
MARGINS_E2E_EXPECTED_COMMIT="$(git -C "$REPO_ROOT" rev-parse HEAD)"

"$HARNESS" init --sandbox "$RUN_ROOT/sandbox" > "$RUN_ROOT/init.txt"
# shellcheck disable=SC1091
source "$RUN_ROOT/sandbox/env.sh"

GOOGLE_FIXTURE_PORT_FILE="$RUN_ROOT/google-fixture.port"
python3 - "$GOOGLE_FIXTURE_PORT_FILE" >"$RUN_ROOT/google-fixture.log" 2>&1 <<'PY' &
import base64
import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlparse

def b64url(text):
    return base64.urlsafe_b64encode(text.encode()).decode().rstrip("=")

THREADS = [
    {
        "id": "thread-e2e-1",
        "history": "9001",
        "message": "msg-e2e-1",
        "date_ms": "1787245200000",
        "date_header": "Thu, 20 Aug 2026 17:00:00 +0000",
        "subject": "Fixture kickoff checkpoint",
        "body": (
            "Alice Client asked for the Tuesday pilot kickoff checkpoint and revised pilot plan. "
            "The fixture notes the owner follow up, launch readiness, rollout risk, and next "
            "decision review so catalyst generation has substantive generic evidence."
        ),
    },
    {
        "id": "thread-e2e-2",
        "history": "9002",
        "message": "msg-e2e-2",
        "date_ms": "1787331600000",
        "date_header": "Fri, 21 Aug 2026 17:00:00 +0000",
        "subject": "Fixture kickoff follow-up",
        "body": (
            "Alice Client followed up with a second confirmation for the Tuesday pilot kickoff "
            "checkpoint and Friday plan. The fixture covers support expectations, accountable "
            "owners, timeline confidence, and documented acceptance criteria."
        ),
    },
    {
        "id": "thread-e2e-3",
        "history": "9003",
        "message": "msg-e2e-3",
        "date_ms": "1787418000000",
        "date_header": "Sat, 22 Aug 2026 17:00:00 +0000",
        "subject": "Fixture rollout notes",
        "body": (
            "Alice Client summarized rollout notes for the pilot checkpoint. The fixture includes "
            "implementation questions, review timing, communication cadence, and the expected "
            "summary that should remain source agnostic after materialization."
        ),
    },
    {
        "id": "thread-e2e-4",
        "history": "9004",
        "message": "msg-e2e-4",
        "date_ms": "1787504400000",
        "date_header": "Sun, 23 Aug 2026 17:00:00 +0000",
        "subject": "Fixture final readiness",
        "body": (
            "Alice Client confirmed final readiness for the pilot checkpoint after reviewing "
            "the revised plan. The fixture records follow through, open questions, next action "
            "ownership, and enough plain text evidence for one eligible catalyst era."
        ),
    },
]
EVENT_ID = "evt-client-kickoff"
DOC_ID = "doc-meet-e2e-1"

def payload(path, query):
    decoded = unquote(path)
    if decoded.endswith("/gmail/v1/users/me/profile"):
        return {"emailAddress": "owner@example.com"}
    if decoded.endswith("/threads") and "/gmail/v1/users/" in decoded:
        return {"threads": [{"id": thread["id"]} for thread in THREADS]}
    for thread in THREADS:
        if decoded.endswith(f"/threads/{thread['id']}"):
            return {
                "id": thread["id"],
                "historyId": thread["history"],
                "messages": [{
                    "id": thread["message"],
                    "threadId": thread["id"],
                    "internalDate": thread["date_ms"],
                    "payload": {
                        "mimeType": "text/plain",
                        "headers": [
                            {"name": "From", "value": "Alice Client <alice@acme.test>"},
                            {"name": "To", "value": "Owner <owner@example.com>"},
                            {"name": "Subject", "value": thread["subject"]},
                            {"name": "Date", "value": thread["date_header"]},
                        ],
                        "body": {"data": b64url(thread["body"])},
                    },
                }],
            }
    if decoded.endswith("/v3/users/me/calendarList"):
        return {"items": [{"id": "primary", "summary": "Primary"}]}
    if "/v3/calendars/primary/events" in decoded:
        return {
            "items": [{
                "id": EVENT_ID,
                "calendarId": "primary",
                "status": "confirmed",
                "summary": "Tuesday pilot kickoff checkpoint",
                "description": "Review the revised pilot plan and follow-up owner.",
                "htmlLink": "https://calendar.google.com/event?eid=fixture",
                "start": {"dateTime": "2026-08-20T17:00:00Z"},
                "end": {"dateTime": "2026-08-20T17:30:00Z"},
                "attendees": [
                    {"displayName": "Alice Client", "email": "alice@acme.test", "responseStatus": "accepted"},
                    {"displayName": "Owner", "email": "owner@example.com", "self": True, "responseStatus": "accepted"},
                ],
                "organizer": {"displayName": "Owner", "email": "owner@example.com"},
            }],
            "nextSyncToken": "sync-token-1",
        }
    if decoded.endswith("/v3/files"):
        q = (query.get("q") or [""])[0]
        if "Meet Recordings" in q:
            return {"files": [{"id": "folder-meet-recordings", "name": "Meet Recordings"}]}
        return {"files": [{
            "id": DOC_ID,
            "name": "Tuesday pilot kickoff transcript",
            "createdTime": "2026-08-20T17:31:00Z",
            "modifiedTime": "2026-08-20T17:45:00Z",
            "webViewLink": "https://docs.google.com/document/d/fixture/edit",
        }]}
    if decoded.endswith(f"/v1/documents/{DOC_ID}"):
        return {"body": {"content": [
            {"paragraph": {"elements": [{"textRun": {"content": "Alice Client asked for the revised pilot plan.\n"}}]}},
            {"paragraph": {"elements": [{"textRun": {"content": "Owner confirmed the Friday follow-up.\n"}}]}},
        ]}}
    if decoded.endswith("/v2/conferenceRecords"):
        return {"conferenceRecords": [{
            "name": "conferenceRecords/conf-e2e",
            "startTime": "2026-08-20T17:00:00Z",
            "endTime": "2026-08-20T17:30:00Z",
        }]}
    if decoded.endswith("/v2/conferenceRecords/conf-e2e/transcripts"):
        return {"transcripts": [{
            "name": "conferenceRecords/conf-e2e/transcripts/transcript-e2e",
            "docsDestination": {"document": f"documents/{DOC_ID}"},
            "startTime": "2026-08-20T17:00:00Z",
            "endTime": "2026-08-20T17:30:00Z",
        }]}
    if decoded.endswith("/v2/conferenceRecords/conf-e2e/participants"):
        return {"participants": [{
            "name": "conferenceRecords/conf-e2e/participants/participant-e2e",
            "signedinUser": {"displayName": "Alice Client", "user": "users/alice"},
        }]}
    raise KeyError(decoded)

class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        parsed = urlparse(self.path)
        try:
            body = json.dumps(payload(parsed.path, parse_qs(parsed.query))).encode()
            self.send_response(200)
        except KeyError as error:
            body = json.dumps({"error": str(error)}).encode()
            self.send_response(404)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, _format, *_args):
        return

server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
Path(sys.argv[1]).write_text(str(server.server_address[1]) + "\n")
server.serve_forever()
PY
GOOGLE_FIXTURE_PID=$!
for _ in $(seq 1 100); do
  [ -s "$GOOGLE_FIXTURE_PORT_FILE" ] && break
  kill -0 "$GOOGLE_FIXTURE_PID" 2>/dev/null || { cat "$RUN_ROOT/google-fixture.log" >&2; exit 1; }
  sleep 0.01
done
[ -s "$GOOGLE_FIXTURE_PORT_FILE" ]
export MARGINS_GOOGLE_NATIVE_E2E_BASE_URL="http://127.0.0.1:$(cat "$GOOGLE_FIXTURE_PORT_FILE")"
python3 - "$MARGINS_HOME" <<'PY'
import json
import os
import sys
from pathlib import Path

home = Path(sys.argv[1])
account = "owner@example.com"
scopes = [
    "https://www.googleapis.com/auth/gmail.readonly",
    "https://www.googleapis.com/auth/calendar.readonly",
    "https://www.googleapis.com/auth/drive.readonly",
    "https://www.googleapis.com/auth/documents.readonly",
    "https://www.googleapis.com/auth/meetings.space.readonly",
]
account_home = home / "google" / account
account_home.mkdir(parents=True, exist_ok=True)
(account_home / "account.json").write_text(json.dumps({
    "schema_version": "margins.google-account.v1",
    "account": account,
    "storage": "file_0600",
    "scopes": scopes,
    "connected_at": "2026-08-20T00:00:00Z",
}, indent=2, sort_keys=True) + "\n")
token_path = account_home / "token-cache.json"
token_path.write_text(json.dumps({
    "schema_version": "margins.google-token-cache.v1",
    "account": account,
    "tokens": [{
        "scopes": scopes,
        "token": {
            "access" + "_token": "fixture-access-token",
            "refresh" + "_token": "fixture-refresh-token",
            "expires_at": "2099-01-01T00:00:00Z",
        },
    }],
}, indent=2, sort_keys=True) + "\n")
os.chmod(account_home / "account.json", 0o600)
os.chmod(token_path, 0o600)
PY

RECALL_AVAILABLE="$("$MARGINS_E2E_BIN" capabilities | python3 -c \
  'import json, sys; print(1 if json.load(sys.stdin).get("recall", {}).get("indexing") else 0)')"
if [ "$RECALL_AVAILABLE" = 1 ]; then
  FIXTURE_PORT_FILE="$RUN_ROOT/fixture-generator.port"
  python3 "$REPO_ROOT/tests/fixture_openai_server.py" \
    --port-file "$FIXTURE_PORT_FILE" >"$RUN_ROOT/fixture-generator.log" 2>&1 &
  FIXTURE_PID=$!
  for _ in $(seq 1 100); do
    [ -s "$FIXTURE_PORT_FILE" ] && break
    kill -0 "$FIXTURE_PID" 2>/dev/null || { cat "$RUN_ROOT/fixture-generator.log" >&2; exit 1; }
    sleep 0.01
  done
  [ -s "$FIXTURE_PORT_FILE" ]
  FIXTURE_PORT="$(cat "$FIXTURE_PORT_FILE")"
  cat > "$MARGINS_HOME/config.toml" <<'EOF'
[llm]
mode = "hosted"
EOF
  python3 - "$MARGINS_HOME/llm-config-cache.json" "$FIXTURE_PORT" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1])
port = int(sys.argv[2])
path.write_text(json.dumps({
    "api_key": "fixture-key",
    "base_url": f"http://127.0.0.1:{port}/v1",
    "model": "fixture-catalyst-model",
    "expires_at": 4102444800,
    "cached_at": 1,
    "profile": "fixture-profile",
}, sort_keys=True) + "\n")
PY
fi

# The Workspace is one Enzyme program; workspaces/<id>/ holds state only.
test -f "$MARGINS_HOME/configs/$MARGINS_WORKSPACE.enzyme"
test ! -e "$MARGINS_WORKSPACE_STATE/config.toml"
"$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source list --json \
  > "$RUN_ROOT/sources-after-init.json"
python3 - "$RUN_ROOT/sources-after-init.json" <<'PY'
import json, sys
sources = json.load(open(sys.argv[1]))
names = {source["name"] for source in sources}
assert {"home", "meeting-notes", "mail", "calendar", "meet"} <= names, names
assert "granola" not in names, names
home = next(source for source in sources if source["name"] == "home")
assert home["kind"] == "notes"
assert home["role"] == "home"
assert isinstance(home["path"], str)
assert "account" not in home
assert "gmail" not in home
reference = next(source for source in sources if source["name"] == "meeting-notes")
assert reference["kind"] == "notes"
assert reference["role"] == "reference"
assert isinstance(reference["path"], str)
assert reference["path"] != home["path"]
assert not reference["path"].startswith(home["path"] + "/")
assert "account" not in reference
assert "gmail" not in reference
mail = next(source for source in sources if source["name"] == "mail")
assert mail["kind"] == "google-mail"
assert mail["account"] == "owner@example.com"
assert mail["gmail"] == {"query": "-in:spam -in:trash", "backfill_days": 365}, mail
assert "path" not in mail
assert "role" not in mail
calendar = next(source for source in sources if source["name"] == "calendar")
assert calendar["kind"] == "google-calendar"
assert calendar["account"] == "owner@example.com"
assert "path" not in calendar
assert "role" not in calendar
assert "gmail" not in calendar
assert calendar["calendar"] == {"lookback_days": 365, "lookahead_days": 180}, calendar
allowed_calendar_keys = {
    "name", "kind", "account", "calendar", "cache_raw_payload", "project_to_home", "indexed_how",
}
assert set(calendar) <= allowed_calendar_keys, calendar
PY
"$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" workspace status --json \
  > "$RUN_ROOT/workspace-status.json"
python3 - "$RUN_ROOT/workspace-status.json" "$RECALL_AVAILABLE" <<'PY'
import json, sys
status = json.load(open(sys.argv[1]))
expected = ({"mode": "hosted", "reason": "hosted_bundle_ready"}
            if sys.argv[2] == "1" else
            None)
if expected:
    assert status["catalyst"] == expected, status
else:
    assert "catalyst" not in status, status
    assert status["recall"]["schema_version"] == "margins.local-recall.v1", status
    assert status["recall"]["mode"] == "live_lexical", status
    assert status["recall"]["available"] is True, status
    assert status["recall"]["documents"] > 0, status
PY

[ "$(stat -c '%a' "$MARGINS_HOME/google/owner@example.com/token-cache.json" 2>/dev/null || stat -f '%Lp' "$MARGINS_HOME/google/owner@example.com/token-cache.json")" = 600 ]

"$HARNESS" phase1
"$HARNESS" phase1
# The consent boundary is a valid stopping point: no connector reconcile has yet
# created ledger.db, and the portable CLI has not created index.db.
test ! -e "$MARGINS_WORKSPACE_STATE/ledger.db"
test ! -e "$MARGINS_WORKSPACE_STATE/index.db"
"$HARNESS" verify-isolation > "$RUN_ROOT/pre-phase2-verify-isolation.txt"
"$HARNESS" report > "$RUN_ROOT/pre-phase2-report.txt"
grep -q 'isolation: PASS' "$RUN_ROOT/pre-phase2-report.txt"
"$HARNESS" auth-instructions > "$RUN_ROOT/auth-instructions.txt"
grep -Fq 'desktop: run margins connect google [--account <email>] and complete the browser consent screen' \
  "$RUN_ROOT/auth-instructions.txt"
grep -Fq 'headless/ssh: run margins connect google --headless [--account <email>] --json' \
  "$RUN_ROOT/auth-instructions.txt"
grep -Fq 'localhost cannot-connect page is expected' \
  "$RUN_ROOT/auth-instructions.txt"
grep -Fq 'copy the entire address-bar URL, not only the code' \
  "$RUN_ROOT/auth-instructions.txt"
grep -Fq 'never paste callback URLs, codes, or tokens into chat' \
  "$RUN_ROOT/auth-instructions.txt"
if grep -Eiq 'gog|GOG_HOME|Cloud project|client JSON|access_token|refresh_token' "$RUN_ROOT/auth-instructions.txt"; then
  printf 'auth instructions exposed internal Google transport details\n' >&2
  exit 1
fi
"$HARNESS" phase2
"$HARNESS" phase2
"$HARNESS" assert-source-remove-preservation
"$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" source add google-mail \
  --name mail --account owner@example.com \
  --query "-in:spam -in:trash" --backfill-days 365 >/dev/null
if [ "$RECALL_AVAILABLE" = 1 ]; then
  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" init >/dev/null
fi
"$HARNESS" contract-lifecycle
"$HARNESS" contract-phase8
"$HARNESS" contract-phase9
if [ "$RECALL_AVAILABLE" = 1 ]; then
  # Retention mutates the ledger after phase2's snapshot. Retrieval must be
  # refreshed explicitly before the final recall assertion.
  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" init \
    > "$RUN_ROOT/recall-after-retention-init.txt"
fi
"$HARNESS" verify-isolation > "$RUN_ROOT/verify-isolation-1.txt"
"$HARNESS" verify-isolation > "$RUN_ROOT/verify-isolation-2.txt"
grep -Eq "Margins-managed Google home: $RUN_ROOT/sandbox/margins-home/google/owner@example.com \(inside sandbox\)" \
  "$RUN_ROOT/verify-isolation-1.txt"

# A second Workspace declares the same machine account and immediately sees
# the one existing connection; no consent or credential mutation runs again.
mkdir -p "$MARGINS_E2E_SANDBOX/second-notes"
"$MARGINS_E2E_BIN" workspace new second-practice --home "$MARGINS_E2E_SANDBOX/second-notes" >/dev/null
for pair in google-mail:mail google-calendar:calendar google-meet:meet; do
  kind="${pair%%:*}"
  name="${pair##*:}"
  "$MARGINS_E2E_BIN" --workspace second-practice source add "$kind" \
    --name "$name" --account owner@example.com >/dev/null
done
test ! -e "$MARGINS_HOME/workspaces/second-practice/config.toml"
"$MARGINS_E2E_BIN" --workspace second-practice source list --json \
  > "$RUN_ROOT/second-practice-sources.json"
python3 - "$MARGINS_HOME/configs/second-practice.enzyme" "$RUN_ROOT/second-practice-sources.json" <<'PY'
import json, re, sys
program = open(sys.argv[1]).read()
assert re.search(
    r'source\s+google-mail\s+"mail"\s*\{[^}]*\baccount\s+"owner@example\.com"', program
), program
mail = next(row for row in json.load(open(sys.argv[2])) if row["name"] == "mail")
assert mail["gmail"] == {
    "query": "-in:spam -in:trash",
    "backfill_days": 365,
}, mail
PY
"$MARGINS_E2E_BIN" connect status --json > "$RUN_ROOT/machine-connect-status.json"
python3 - "$RUN_ROOT/machine-connect-status.json" <<'PY'
import json, sys
status = json.load(open(sys.argv[1]))
connection = next(row for row in status["connections"] if row["account"] == "owner@example.com")
assert connection["connected"] is True, connection
assert connection["workspaces"] == ["fresh-onboarding", "second-practice"], connection
PY
"$HARNESS" report > "$RUN_ROOT/report.txt"

grep -q 'isolation: PASS' "$RUN_ROOT/report.txt"
grep -q 'records=' "$RUN_ROOT/report.txt"
grep -Fq "binary provenance: commit=$MARGINS_E2E_EXPECTED_COMMIT" "$RUN_ROOT/report.txt"
if [ "$RECALL_AVAILABLE" = 1 ]; then
  grep -Fq 'catalyst mode: hosted (hosted_bundle_ready)' "$RUN_ROOT/report.txt"

  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" recall \
    "Tuesday pilot kickoff checkpoint" > "$RUN_ROOT/recall.json"
  python3 - "$RUN_ROOT/recall.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["schema_version"] == "margins.recall.v1", result
assert result.get("status"), result
assert result.get("reason"), result
freshness = result.get("freshness", {})
assert isinstance(freshness, dict) and "status" in freshness and "stale" in freshness, result
assert "index" in freshness, result
assert isinstance(freshness.get("materialization"), list), result
assert result["total_results"] > 0, result
assert result["top_contributing_catalysts"], result
for row in result["results"]:
    assert row.get("document_ref"), row
    evidence = row.get("evidence", {})
    assert evidence.get("kind") in {"native_markdown", "external_record", "session_record"}, row
    assert row.get("file_path") is None, row
    assert row.get("via_catalyst_id"), row
PY

  kill "$FIXTURE_PID"
  wait "$FIXTURE_PID" 2>/dev/null || true
  FIXTURE_PID=""
  rm -f -- "$MARGINS_HOME/llm-config-cache.json" "$MARGINS_HOME/config.toml"
  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" recall \
    "Tuesday pilot kickoff checkpoint" > "$RUN_ROOT/recall-without-generator.json"
  python3 - "$RUN_ROOT/recall.json" "$RUN_ROOT/recall-without-generator.json" <<'PY'
import json, sys
before, after = (json.load(open(path)) for path in sys.argv[1:])
assert after["status"] == before["status"] == "ok", after
assert after["total_results"] == before["total_results"], (before, after)
assert after["results"] == before["results"], (before, after)
PY
else
  grep -Fq 'local recall mode: live_lexical' "$RUN_ROOT/report.txt"
  "$MARGINS_E2E_BIN" --workspace "$MARGINS_WORKSPACE" recall \
    "Tuesday pilot kickoff checkpoint" > "$RUN_ROOT/recall.json"
  python3 - "$RUN_ROOT/recall.json" <<'PY'
import json, sys
result = json.load(open(sys.argv[1]))
assert result["schema_version"] == "margins.recall.v1", result
assert result["status"] == "ok" and result["reason"] == "local_lexical", result
assert result["total_results"] > 0, result
assert all(row["evidence"]["kind"] == "native_markdown" for row in result["results"]), result
PY
fi

"$HARNESS" assert-machine-forget-preservation

# A stale PASS file must not survive a live report verification failure.
printf 'pass\n' > "$MARGINS_E2E_SANDBOX/isolation-verdict.txt"
python3 - "$MARGINS_E2E_SANDBOX/isolation-baseline.json" <<'PY'
import json, sys
path = sys.argv[1]
data = json.load(open(path))
data["entries"]["/definitely-not-a-real-isolation-path"] = {"type": "file"}
open(path, "w").write(json.dumps(data, indent=2, sort_keys=True) + "\n")
PY
if "$HARNESS" report > "$RUN_ROOT/stale-report.txt" 2>&1; then
  printf 'report trusted a stale isolation verdict\n' >&2
  exit 1
fi
! grep -q 'isolation: PASS' "$RUN_ROOT/stale-report.txt"
printf 'fresh-onboarding native fixture CI PASS\n'
