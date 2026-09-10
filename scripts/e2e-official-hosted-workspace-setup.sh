#!/usr/bin/env bash
set -euo pipefail

: "${MARGINS_E2E_BIN:?set MARGINS_E2E_BIN to the official Margins binary}"
: "${MARGINS_E2E_HOSTED_BUNDLE_SOURCE:?set MARGINS_E2E_HOSTED_BUNDLE_SOURCE to an absolute brokered bundle path}"

case "$MARGINS_E2E_HOSTED_BUNDLE_SOURCE" in
  /*) ;;
  *) printf 'official hosted e2e: bundle source must be an absolute path\n' >&2; exit 64 ;;
esac
test -f "$MARGINS_E2E_HOSTED_BUNDLE_SOURCE" || {
  printf 'official hosted e2e: bundle source is not a regular file\n' >&2
  exit 66
}
test -x "$MARGINS_E2E_BIN" || {
  printf 'official hosted e2e: binary is not executable: %s\n' "$MARGINS_E2E_BIN" >&2
  exit 66
}

RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-official-hosted-e2e.XXXXXX")"
cleanup() { rm -rf -- "$RUN_ROOT"; }
trap cleanup EXIT
umask 077

export HOME="$RUN_ROOT/home"
export MARGINS_HOME="$RUN_ROOT/margins-home"
export XDG_CONFIG_HOME="$RUN_ROOT/xdg-config"
export XDG_DATA_HOME="$RUN_ROOT/xdg-data"
export XDG_CACHE_HOME="$RUN_ROOT/xdg-cache"
unset OPENAI_API_KEY OPENAI_BASE_URL OPENAI_MODEL
unset OPENROUTER_API_KEY OPENROUTER_BASE_URL OPENROUTER_MODEL
unset ENZYME_FREE_CONFIG_URL ENZYME_API_CACHE_PATH ENZYME_HOME

WORKSPACE_ID="${MARGINS_E2E_WORKSPACE:-official-hosted-e2e}"
WORKSPACE_STATE="$MARGINS_HOME/workspaces/$WORKSPACE_ID"
NOTES="$RUN_ROOT/notes"
mkdir -p "$HOME" "$MARGINS_HOME" "$NOTES/projects" "$NOTES/people" "$NOTES/templates"

BUNDLE="$MARGINS_HOME/llm-config-cache.json"
SOURCE_BUNDLE_SHA256="$(python3 - "$MARGINS_E2E_HOSTED_BUNDLE_SOURCE" <<'PY'
import hashlib, sys
print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())
PY
)"
cp -- "$MARGINS_E2E_HOSTED_BUNDLE_SOURCE" "$BUNDLE"
chmod 0600 "$BUNDLE"
python3 - "$BUNDLE" <<'PY'
import os, stat, sys
mode = stat.S_IMODE(os.stat(sys.argv[1]).st_mode)
if mode != 0o600:
    raise SystemExit(f"hosted bundle mode is {mode:o}, expected 600")
PY
printf '[llm]\nmode = "hosted"\n' > "$MARGINS_HOME/config.toml"

printf '# Atlas\n\n[[Atlas Program]] tracks the phosphorescent handoff, accountable owner, review timing, delivery risk, and decision boundary. This substantive record preserves source-backed planning evidence and follow-up commitments.\n' \
  > "$NOTES/projects/Atlas.md"
for number in 2 3 4 5 6; do
  printf '# Atlas evidence %s\n\n[[Atlas Program]] tracks the phosphorescent handoff, accountable owner, review timing, delivery risk, and decision boundary for checkpoint %s. This substantive record preserves source-backed planning evidence and follow-up commitments.\n' \
    "$number" "$number" > "$NOTES/projects/evidence-$number.md"
done
for name in "Rui Tan" "Mina Shah" "Owen Lee" "Priya Das"; do
  printf '# %s\n\n%s relationship context tracks the phosphorescent handoff, accountable owner, review timing, delivery risk, and decision boundary. This substantive people record preserves source-backed relationship evidence, follow-up commitments, trust cadence, priorities, constraints, and collaboration history.\n' \
    "$name" "$name" > "$NOTES/people/$name.md"
done
printf '# Template\n\nDO_NOT_RECALL_OFFICIAL_HOSTED_TEMPLATE\n' > "$NOTES/templates/meeting.md"

snapshot_markdown() {
  python3 - "$1" "$2" <<'PY'
import hashlib
from pathlib import Path
import sys
root = Path(sys.argv[1])
rows = [f"{path.relative_to(root)} {hashlib.sha256(path.read_bytes()).hexdigest()}"
        for path in sorted(root.rglob("*.md"))]
Path(sys.argv[2]).write_text("\n".join(rows) + "\n")
PY
}

run_workspace() { "$MARGINS_E2E_BIN" --workspace "$WORKSPACE_ID" "$@"; }

BEFORE="$RUN_ROOT/notes-before.sha256"
AFTER="$RUN_ROOT/notes-after.sha256"
STATUS="$RUN_ROOT/status.json"
SCAN="$RUN_ROOT/scan.json"
DESIRED="$RUN_ROOT/desired.toml"
PLAN="$RUN_ROOT/plan.json"
STALE_PLAN="$RUN_ROOT/stale-plan.json"
APPLY="$RUN_ROOT/apply.json"
REPLAY="$RUN_ROOT/replay.json"
STALE_ERROR="$RUN_ROOT/stale-error.json"
snapshot_markdown "$NOTES" "$BEFORE"

"$MARGINS_E2E_BIN" workspace new "$WORKSPACE_ID" --home "$NOTES" > "$RUN_ROOT/workspace-new.txt"
run_workspace workspace status --json > "$STATUS"
REVISION="$(python3 - "$STATUS" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
revision = value.get("revision") or value.get("config_revision")
if not isinstance(revision, str) or not revision:
    raise SystemExit("workspace status did not provide a revision")
print(revision)
PY
)"

run_workspace scan > "$SCAN"
python3 - "$SCAN" <<'PY'
import json, sys
scan = json.load(open(sys.argv[1]))
assert scan["schema_version"] == "scan.v2", scan
for field in (
    "instructions", "summary", "coverage_entities", "entity_curation_candidates",
    "top_entities", "top_tags", "top_links", "top_folders", "entity_samples",
    "sample_files", "folder_stats", "folder_page_entities", "folder_children",
    "tag_children", "frontmatter_samples", "current_config", "available_profiles",
):
    assert field in scan, (field, scan)
assert "templates" in scan["excluded_folders"], scan
assert any(item["spec"] == "folder:projects" for item in scan["coverage_entities"]), scan
assert any(item["spec"] == "folder:projects" for item in scan["entity_curation_candidates"]), scan
assert any(item["spec"] == "folder:people" for item in scan["coverage_entities"]), scan
assert any(item["spec"] == "folder:people" for item in scan["entity_curation_candidates"]), scan
PY

cp "$WORKSPACE_STATE/config.toml" "$DESIRED"
python3 - "$DESIRED" <<'PY'
from pathlib import Path
import re, sys
path = Path(sys.argv[1])
text = path.read_text()
text, count = re.subn(
    r'excluded_folders\s*=\s*\[[^]]*\]',
    'excluded_folders = [".git", "node_modules", "templates"]',
    text,
    count=1,
    flags=re.S,
)
assert count == 1, text
text, count = re.subn(
    r'entities\s*=\s*\[\]',
    'entities = [{ "folder:people" = { profile = "relational" } }]',
    text,
    count=1,
)
assert count == 1, text
path.write_text(text)
PY
run_workspace workspace plan --desired "$WORKSPACE_STATE/config.toml" --json > "$STALE_PLAN"
run_workspace workspace plan --desired "$DESIRED" --json > "$PLAN"
python3 - "$PLAN" "$WORKSPACE_ID" "$REVISION" <<'PY'
import json, sys
plan = json.load(open(sys.argv[1]))
assert plan["schema_version"] == "margins.workspace.plan.v1", plan
assert plan["workspace_id"] == sys.argv[2], plan
assert plan["base_revision"] == sys.argv[3], plan
assert isinstance(plan.get("actions"), list) and plan["actions"], plan
assert plan["desired"]["policy"]["excluded_folders"] == [".git", "node_modules", "templates"], plan
assert plan["desired"]["policy"]["entities"] == [{"folder:people": {"profile": "relational", "expandable": False}}], plan
assert plan["desired"]["policy"]["excluded_entities"] == [], plan
PY
run_workspace workspace apply --plan "$PLAN" --json > "$APPLY"
python3 - "$PLAN" "$APPLY" "$REVISION" <<'PY'
import json, sys
plan = json.load(open(sys.argv[1]))
receipt = json.load(open(sys.argv[2]))
assert receipt["schema_version"] == "margins.workspace.apply.v1", receipt
assert receipt["ok"] is True and receipt["replayed"] is False, receipt
assert receipt["plan_id"] == plan["plan_id"], receipt
assert receipt["before_revision"] == sys.argv[3], receipt
assert receipt["after_revision"] != receipt["before_revision"], receipt
assert receipt["request_id"] == "workspace-apply-" + receipt["request_hash"], receipt
PY

run_workspace workspace apply --plan "$PLAN" --json > "$REPLAY"
python3 - "$APPLY" "$REPLAY" <<'PY'
import json, sys
first = json.load(open(sys.argv[1]))
replay = json.load(open(sys.argv[2]))
assert replay["replayed"] is True, replay
assert replay["request_id"] == first["request_id"], (first, replay)
PY

set +e
run_workspace workspace apply --plan "$STALE_PLAN" --json > /dev/null 2> "$STALE_ERROR"
STALE_STATUS=$?
set -e
test "$STALE_STATUS" -ne 0
python3 - "$STALE_ERROR" <<'PY'
import json, sys
error = json.load(open(sys.argv[1]))
assert error["schema_version"] == "margins.error.v1", error
assert error["error"]["code"] == "workspace_revision_conflict", error
PY

run_workspace init > "$RUN_ROOT/init.xml"
run_workspace sync --json > "$RUN_ROOT/sync.json"
run_workspace recall 'phosphorescent handoff decision boundary' > "$RUN_ROOT/recall.json"
grep -q 'status="ok"' "$RUN_ROOT/init.xml"
python3 - "$RUN_ROOT/sync.json" "$RUN_ROOT/recall.json" "$NOTES" <<'PY'
import json
from pathlib import Path
import sys
sync = json.load(open(sys.argv[1]))
recall = json.load(open(sys.argv[2]))
notes = Path(sys.argv[3]).resolve()
assert sync["schema_version"] == "margins.sync.v1", sync
assert sync["ok"] is True and sync["recall"]["status"] == "ok", sync
assert recall["schema_version"] == "margins.recall.v1", recall
assert recall["status"] == "ok" and recall["total_results"] > 0, recall
assert recall["search_strategy"] == "catalyze", recall
native = [r for r in recall["results"] if r.get("evidence", {}).get("kind") == "native_markdown"]
assert native, recall
for result in native:
    assert Path(result["evidence"]["path"]).resolve().is_relative_to(notes), result
assert any("```receipts" in (r.get("via_catalyst_text") or "") for r in native), recall
assert any("```receipts" in (c.get("text") or "") for c in recall["top_contributing_catalysts"]), recall
PY

snapshot_markdown "$NOTES" "$AFTER"
cmp "$BEFORE" "$AFTER"
test -f "$WORKSPACE_STATE/config.toml"
test -f "$WORKSPACE_STATE/index.db"
test ! -e "$NOTES/.margins"
test ! -e "$NOTES/.enzyme"
test "$SOURCE_BUNDLE_SHA256" = "$(python3 - "$MARGINS_E2E_HOSTED_BUNDLE_SOURCE" <<'PY'
import hashlib, sys
print(hashlib.sha256(open(sys.argv[1], "rb").read()).hexdigest())
PY
)"

printf 'official_hosted_workspace_setup_e2e=ok\n'
