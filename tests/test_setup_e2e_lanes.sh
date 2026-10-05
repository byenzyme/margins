#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
PUBLIC_RUNTIME="$REPO_ROOT/scripts/e2e-fresh-workspace-setup.sh"
OFFICIAL_HOSTED="$REPO_ROOT/scripts/e2e-official-hosted-workspace-setup.sh"
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
      plan)
        [ "${1:-}" = "--desired" ] || error usage; desired="$2"; shift 2
        [ "${1:-}" = "--json" ] || error usage
        case "$desired" in *.enzyme) ;; *) error usage ;; esac
        if grep -Fq 'leave out folders ["templates"]' "$desired"; then
          plan_id=fixture-plan
        else
          plan_id=fixture-stale
        fi
        log "$workspace workspace plan id=$plan_id revision=$(revision)"
        python3 - "$desired" "$workspace" "$(revision)" "$plan_id" "$(program_path "$workspace")" <<'PY'
import difflib, hashlib, json, sys
desired, workspace, base, plan_id, current = sys.argv[1:]
program = open(desired).read()
before = open(current).read()
excluded = ["templates"] if 'leave out folders ["templates"]' in program else []
entities = (
    [{"folder:people": {"profile": "relational", "expandable": False}}]
    if 'learn questions from folder "people"' in program
    else []
)
empty = {"excluded_folders": [], "excluded_tags": [], "entities": [], "excluded_entities": []}
after = {**empty, "excluded_folders": excluded, "entities": entities}
actions = (
    [{"action": "set_policy", "summary": "Attention policy: fixture", "before": empty, "after": after}]
    if program != before
    else []
)
print(json.dumps({
    "schema_version": "margins.workspace.plan.v2",
    "workspace_id": workspace,
    "base_revision": base,
    "plan_id": plan_id,
    "actions": actions,
    "desired_program": program,
    "desired_sha256": hashlib.sha256(program.encode()).hexdigest(),
    "diff": "".join(difflib.unified_diff(
        before.splitlines(True), program.splitlines(True),
        f"a/configs/{workspace}.enzyme", f"b/configs/{workspace}.enzyme",
    )),
}))
PY
        ;;
      apply)
        [ "${1:-}" = "--plan" ] || error usage; plan="$2"; shift 2
        [ "${1:-}" = "--json" ] || error usage
        readarray -t plan_fields < <(python3 - "$plan" <<'PY'
import json, sys
plan = json.load(open(sys.argv[1]))
print(plan["plan_id"])
print(plan["base_revision"])
PY
)
        plan_id="${plan_fields[0]}"; expected="${plan_fields[1]}"
        [ "$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1]))["schema_version"])' "$plan")" = margins.workspace.plan.v2 ] \
          || error workspace_plan_invalid
        request_hash="fixturehash-${plan_id}"
        request="workspace-apply-${request_hash}"
        receipt="$(state_dir "$workspace")/receipt-${plan_id}"
        if [ -f "$receipt" ]; then
          log "$workspace workspace apply replay plan=$plan_id request=$request"
          printf '{"schema_version":"margins.workspace.apply.v2","ok":true,"workspace_id":"%s","request_id":"%s","request_hash":"%s","plan_id":"%s","before_revision":"rev1","after_revision":"rev2","replayed":true,"actions":[{"position":0,"status":"applied","action":"set_policy"}]}\n' \
            "$workspace" "$request" "$request_hash" "$plan_id"
          exit 0
        fi
        actual="$(revision)"
        if [ "$expected" != "$actual" ]; then
          log "$workspace workspace apply stale plan=$plan_id expected=$expected actual=$actual"
          error workspace_revision_conflict
        fi
        printf 'rev2\n' > "$(state_dir "$workspace")/revision"
        python3 -c 'import json, sys; open(sys.argv[2], "w").write(json.load(open(sys.argv[1]))["desired_program"])' \
          "$plan" "$(program_path "$workspace")"
        touch "$receipt"
        log "$workspace workspace apply direct-plan=$(basename "$plan") request=$request revision=$expected"
        printf '{"schema_version":"margins.workspace.apply.v2","ok":true,"workspace_id":"%s","request_id":"%s","request_hash":"%s","plan_id":"%s","before_revision":"%s","after_revision":"rev2","replayed":false,"actions":[{"position":0,"status":"applied","action":"set_policy"}]}\n' \
          "$workspace" "$request" "$request_hash" "$plan_id" "$expected"
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
  scan)
    log "$workspace scan full-evidence=scan.v2"
    printf '{"schema_version":"scan.v2","instructions":[],"summary":{"file_count":7},"coverage_entities":[{"spec":"folder:projects"},{"spec":"folder:people"}],"entity_curation_candidates":[{"spec":"folder:projects"},{"spec":"folder:people"}],"top_entities":[],"top_tags":[],"top_links":[],"top_folders":[],"entity_samples":[],"sample_files":[],"folder_stats":[],"folder_page_entities":[],"folder_children":[],"tag_children":[],"frontmatter_samples":[],"current_config":{},"available_profiles":[],"entities":["folder:projects","folder:people"],"excluded_folders":[".git","node_modules","templates"]}\n'
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
grep -Fq 'fresh-workspace recall phosphorescent handoff decision boundary' "$PUBLIC_LOG"

BUNDLE_SOURCE="$RUN_ROOT/brokered-bundle.json"
printf '%s\n' '{"api_key":"fixture-secret","base_url":"https://fixture.invalid/v1","model":"fixture-model","expires_at":null,"cached_at":1,"profile":"fixture"}' > "$BUNDLE_SOURCE"
OFFICIAL_LOG="$RUN_ROOT/official.log"
TMPDIR="$RUN_ROOT/tmp" MARGINS_E2E_BIN="$FAKE_BIN" \
  MARGINS_E2E_HOSTED_BUNDLE_SOURCE="$BUNDLE_SOURCE" \
  FAKE_MARGINS_LOG="$OFFICIAL_LOG" \
  "$OFFICIAL_HOSTED" > "$RUN_ROOT/official.out"
grep -Fxq 'official_hosted_workspace_setup_e2e=ok' "$RUN_ROOT/official.out"
OFFICIAL_HOME="$(sed -n 's/.* MARGINS_HOME=\([^ ]*\).*/\1/p' "$OFFICIAL_LOG" | head -1)"
test -n "$OFFICIAL_HOME" && test ! -e "$OFFICIAL_HOME"
grep -Fq 'bundle_mode=600' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e scan full-evidence=scan.v2' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e workspace apply direct-plan=plan.json request=workspace-apply-fixturehash-fixture-plan revision=rev1' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e workspace apply replay plan=fixture-plan request=workspace-apply-fixturehash-fixture-plan' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e workspace apply stale plan=fixture-stale expected=rev1 actual=rev2' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e init' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e sync' "$OFFICIAL_LOG"
grep -Fq 'official-hosted-e2e recall phosphorescent handoff decision boundary' "$OFFICIAL_LOG"
if grep -Fq 'fixture-secret' "$RUN_ROOT/official.out" "$OFFICIAL_LOG"; then
  printf 'hosted bundle contents leaked to output or command log\n' >&2
  exit 1
fi
for script in \
  "$REPO_ROOT/scripts/e2e-fresh-workspace-setup.sh" \
  "$REPO_ROOT/scripts/e2e-official-hosted-workspace-setup.sh"; do
  if grep -Eiq 'anchor[-_ ]?schema' "$script"; then
    printf 'setup E2E script depends on a forbidden schema: %s\n' "$script" >&2
    exit 1
  fi
done
if grep -Eiq 'anchor[-_ ]?schema' "$RUN_ROOT/public.out" "$RUN_ROOT/official.out"; then
  printf 'setup E2E output depends on a forbidden schema\n' >&2
  exit 1
fi

printf 'setup E2E lane shell fixtures PASS\n'
