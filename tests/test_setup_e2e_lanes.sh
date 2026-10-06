#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PUBLIC_RUNTIME="$REPO_ROOT/scripts/e2e-fresh-workspace-setup.sh"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-setup-lanes-ci.XXXXXX")"
cleanup() { rm -rf -- "$RUN_ROOT"; }
trap cleanup EXIT
mkdir -p "$RUN_ROOT/tmp"

FAKE_BIN="$RUN_ROOT/margins-fake"
FAKE_LOG="$RUN_ROOT/fake.log"
cat > "$FAKE_BIN" <<'FAKE'
#!/usr/bin/env bash
set -euo pipefail

log() { printf '%s\n' "$*" >> "${FAKE_MARGINS_LOG:?}"; }
error() {
  printf '{"schema_version":"margins.error.v1","error":{"code":"%s","message":"fixture"},"retryable":false}\n' "$1" >&2
  exit 1
}
state_dir() { printf '%s/workspaces/%s' "$MARGINS_HOME" "$1"; }
program_path() { printf '%s/configs/%s.enzyme' "$MARGINS_HOME" "$1"; }
revision() { sed -n '1p' "$(state_dir "$workspace")/revision"; }
notes_home() {
  sed -n 's/^ *source markdown "home" { path "\(.*\)" }$/\1/p' "$(program_path "$workspace")" | head -1
}

workspace=""
if [ "${1:-}" = "--workspace" ]; then workspace="$2"; shift 2; fi
command="${1:-}"; shift || true

case "$command" in
  workspace)
    sub="${1:-}"; shift || true
    case "$sub" in
      new)
        workspace="$1"; shift
        [ "${1:-}" = "--home" ] || error usage
        notes="$2"
        dir="$(state_dir "$workspace")"
        mkdir -p "$dir/captures" "$MARGINS_HOME/configs"
        cat > "$(program_path "$workspace")" <<EOF
// Enzyme reading configuration

workspace "$workspace" {
  source margins-captures "captures" {
    path "$dir/captures"
  }

  source markdown "home" { path "$notes" }

  remember in folder "." create note
}
EOF
        printf 'rev1\n' > "$dir/revision"
        bundle_mode=none
        if [ -f "$MARGINS_HOME/llm-config-cache.json" ]; then
          bundle_mode="$(python3 - "$MARGINS_HOME/llm-config-cache.json" <<'PY'
import os, stat, sys
print(f"{stat.S_IMODE(os.stat(sys.argv[1]).st_mode):o}")
PY
)"
        fi
        log "workspace new $workspace home=$notes HOME=$HOME MARGINS_HOME=$MARGINS_HOME bundle_mode=$bundle_mode"
        printf 'Workspace: %s\n' "$workspace"
        ;;
      status)
        dir="$(state_dir "$workspace")"; notes="$(notes_home)"
        log "$workspace workspace status"
        printf '{"id":"%s","revision":"%s","home":"%s","config":"%s","index":"%s/index.db","recall":{"available":true}}\n' \
          "$workspace" "$(revision)" "$notes" "$(program_path "$workspace")" "$dir"
        ;;
      *) error usage ;;
    esac
    ;;
  init)
    touch "$(state_dir "$workspace")/index.db"
    log "$workspace init"
    printf '<margins_init status="ok" />\n'
    ;;
  sync)
    log "$workspace sync"
    printf '{"schema_version":"margins.sync.v1","ok":true,"sources":[{"name":"home","status":"ready"}],"recall":{"status":"ok"}}\n'
    ;;
  recall)
    notes="$(notes_home)"; log "$workspace recall $*"
    if [ -f "$MARGINS_HOME/config.toml" ]; then
      printf '{"schema_version":"margins.recall.v1","status":"ok","reason":"catalyst","search_strategy":"catalyze","total_results":1,"results":[{"document_ref":"projects/Atlas.md","evidence":{"kind":"native_markdown","path":"%s/projects/Atlas.md"},"via_catalyst_text":"bridge\\n```receipts\\n{receipt}\\n```"}],"top_contributing_catalysts":[{"text":"bridge\\n```receipts\\n{receipt}\\n```"}]}\n' "$notes"
    else
      printf '{"schema_version":"margins.recall.v1","search_strategy":"live_local_markdown","total_results":1,"results":[{"content":"phosphorescent handoff decision boundary"}]}\n'
    fi
    ;;
  source)
    [ "${1:-}" = "list" ] || error usage
    notes="$(notes_home)"; log "$workspace source list"
    printf '[{"name":"home","kind":"notes","role":"home","path":"%s"}]\n' "$notes"
    ;;
  *) error usage ;;
esac
FAKE
chmod +x "$FAKE_BIN"

PUBLIC_LOG="$RUN_ROOT/public.log"
TMPDIR="$RUN_ROOT/tmp" MARGINS_BIN="$FAKE_BIN" FAKE_MARGINS_LOG="$PUBLIC_LOG" \
  "$PUBLIC_RUNTIME" synthetic > "$RUN_ROOT/public.out"
grep -Fxq 'synthetic_e2e=ok' "$RUN_ROOT/public.out"
PUBLIC_HOME="$(sed -n 's/.* MARGINS_HOME=\([^ ]*\).*/\1/p' "$PUBLIC_LOG" | head -1)"
test -n "$PUBLIC_HOME" && test ! -e "$PUBLIC_HOME"
grep -Fq 'fresh-workspace init' "$PUBLIC_LOG"
grep -Fq 'fresh-workspace sync' "$PUBLIC_LOG"
grep -Fq 'fresh-workspace recall --json phosphorescent handoff decision boundary' "$PUBLIC_LOG"

for script in \
  "$REPO_ROOT/scripts/e2e-fresh-workspace-setup.sh"; do
  if grep -Eiq 'anchor[-_ ]?schema' "$script"; then
    printf 'setup E2E script depends on a forbidden schema: %s\n' "$script" >&2
    exit 1
  fi
done
if grep -Eiq 'anchor[-_ ]?schema' "$RUN_ROOT/public.out"; then
  printf 'setup E2E output depends on a forbidden schema\n' >&2
  exit 1
fi

printf 'setup E2E lane shell fixtures PASS\n'
