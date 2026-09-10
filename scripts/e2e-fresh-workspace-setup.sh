#!/usr/bin/env bash
set -euo pipefail

MARGINS_BIN="${MARGINS_BIN:-margins-public}"
MODE="${1:-synthetic}"
LIVE_NOTES="${2:-/workspace/obsidian}"
WORKSPACE_ID="${MARGINS_E2E_WORKSPACE:-fresh-workspace}"

RUNROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-workspace-setup.XXXXXX")"
cleanup() {
  if [ "${KEEP_MARGINS_E2E:-0}" = "1" ]; then
    printf 'keeping RUNROOT=%s\n' "$RUNROOT"
  else
    rm -rf -- "$RUNROOT"
  fi
}
trap cleanup EXIT

export HOME="$RUNROOT/home"
export MARGINS_HOME="$RUNROOT/margins-home"
export XDG_CONFIG_HOME="$RUNROOT/xdg-config"
export XDG_DATA_HOME="$RUNROOT/xdg-data"
export XDG_CACHE_HOME="$RUNROOT/xdg-cache"
mkdir -p "$HOME" "$MARGINS_HOME"
WORKSPACE_STATE="$MARGINS_HOME/workspaces/$WORKSPACE_ID"

run_workspace() {
  "$MARGINS_BIN" --workspace "$WORKSPACE_ID" "$@"
}

markdown_snapshot() {
  python3 - "$1" "$2" <<'PY'
import hashlib
from pathlib import Path
import sys

root = Path(sys.argv[1])
rows = []
for path in sorted(root.rglob("*.md")):
    rows.append(f"{path.relative_to(root)} {hashlib.sha256(path.read_bytes()).hexdigest()}")
Path(sys.argv[2]).write_text("\n".join(rows) + "\n")
PY
}

assert_state_layout() {
  test -f "$WORKSPACE_STATE/config.toml"
  test ! -e "$NOTES/.margins"
  test ! -e "$MARGINS_HOME/integrations"
  test ! -e "$MARGINS_HOME/recall"
  if find "$WORKSPACE_STATE" -type f -name '*.md' -print -quit | grep -q .; then
    printf 'workspace state contains a Markdown body\n' >&2
    exit 1
  fi
}

setup_and_query() {
  NOTES="$1"
  local query="$2"
  local before="$RUNROOT/notes-before.sha256"
  local after="$RUNROOT/notes-after.sha256"
  markdown_snapshot "$NOTES" "$before"

  "$MARGINS_BIN" workspace new "$WORKSPACE_ID" --home "$NOTES" > "$RUNROOT/workspace-new.txt"
  (
    cd "$HOME"
    run_workspace init > "$RUNROOT/init.xml"
    run_workspace sync --json > "$RUNROOT/sync.json"
    run_workspace recall "$query" > "$RUNROOT/recall.json"
    run_workspace workspace status --json > "$RUNROOT/status.json"
    run_workspace source list --json > "$RUNROOT/sources.json"
  )

  grep -q 'status="ok"' "$RUNROOT/init.xml"
  python3 - "$RUNROOT/recall.json" "$RUNROOT/sync.json" "$RUNROOT/status.json" "$RUNROOT/sources.json" "$NOTES" "$WORKSPACE_ID" <<'PY'
import json
from pathlib import Path
import sys

recall = json.load(open(sys.argv[1]))
sync = json.load(open(sys.argv[2]))
status = json.load(open(sys.argv[3]))
sources = json.load(open(sys.argv[4]))
notes = str(Path(sys.argv[5]).resolve())
assert recall["schema_version"] == "margins.recall.v1", recall
assert recall["search_strategy"] == "live_local_markdown", recall
assert recall["total_results"] > 0, recall
assert sync["schema_version"] == "margins.sync.v1", sync
assert sync["ok"] is True, sync
assert all(source["status"] == "ready" for source in sync["sources"]), sync
assert status["id"] == sys.argv[6], status
assert status["home"] == notes, status
assert status["recall"]["available"] is True, status
home = next(source for source in sources if source.get("role") == "home")
assert home["kind"] == "notes", home
assert home["path"] == notes, home
PY

  markdown_snapshot "$NOTES" "$after"
  cmp "$before" "$after"
  assert_state_layout
  printf '%s_e2e=ok\n' "$MODE"
  printf 'workspace_state=%s\n' "$WORKSPACE_STATE"
}

synthetic() {
  local notes="$RUNROOT/notes"
  mkdir -p "$notes/people" "$notes/projects" "$notes/templates"
  printf '# Rui Tan\nRui owns the Atlas continuity scorecard.\n' > "$notes/people/Rui Tan.md"
  printf '# Atlas\nThe phosphorescent handoff preserves the decision boundary after forty-eight hours. [[Rui Tan]] #customer\n' > "$notes/projects/Atlas.md"
  printf '# Template\nDO_NOT_INDEX_WORKSPACE_TEMPLATE\n' > "$notes/templates/meeting.md"
  setup_and_query "$notes" 'phosphorescent handoff decision boundary'
  if grep -q 'DO_NOT_INDEX_WORKSPACE_TEMPLATE' "$RUNROOT/recall.json"; then
    printf 'unrelated template entered recall\n' >&2
    exit 1
  fi
}

copied_notes() {
  test -d "$LIVE_NOTES" || { printf 'notes folder not found: %s\n' "$LIVE_NOTES" >&2; exit 2; }
  local notes="$RUNROOT/notes-copy"
  mkdir -p "$notes"
  cp -R "$LIVE_NOTES/." "$notes/"
  local query
  query="$(python3 - "$notes" <<'PY'
from pathlib import Path
import re, sys
for path in sorted(Path(sys.argv[1]).rglob("*.md")):
    words = re.findall(r"[A-Za-z][A-Za-z0-9_-]+", path.read_text(errors="ignore"))
    if len(words) >= 3:
        print(" ".join(words[:5]))
        break
else:
    raise SystemExit("no substantive Markdown file to query")
PY
)"
  setup_and_query "$notes" "$query"
}

case "$MODE" in
  synthetic) synthetic ;;
  copied-notes) copied_notes ;;
  *) printf 'usage: %s [synthetic|copied-notes] [notes-folder]\n' "$0" >&2; exit 64 ;;
esac
