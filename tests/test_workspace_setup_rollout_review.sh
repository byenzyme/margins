#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HARNESS="$REPO_ROOT/scripts/workspace-setup-rollout-review.py"
RUN_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/margins-rollout-review-test.XXXXXX")"
RUN_ROOT="$(cd "$RUN_ROOT" && pwd -P)"
trap 'rm -rf "$RUN_ROOT"' EXIT

make_fixture() {
  local name="$1"
  local vault="$RUN_ROOT/$name/vault"
  local margins_home="$RUN_ROOT/$name/margins-home"
  local state="$margins_home/workspaces/fixture"
  mkdir -p "$vault/.margins" "$state/captures" "$RUN_ROOT/$name/run"
  printf '# One\n\nA note.\n' > "$RUN_ROOT/$name/vault/one.md"
  printf 'legacy session state\n' > "$vault/.margins/session.md"
  printf 'old index\n' > "$state/index.db"
  printf 'id = "fixture"\n\n[policy]\nentities = []\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n\n[bindings.captures]\nkind = "captures"\npath = "%s"\n' \
    "$vault" "$state/captures" > "$state/config.toml"
  "$HARNESS" prepare \
    --vault "$vault" \
    --margins-home "$margins_home" \
    --run-dir "$RUN_ROOT/$name/run" >/dev/null
  test ! -e "$vault/.margins"
  test ! -e "$state"
  test -f "$RUN_ROOT/$name/run/backup/vault-dot-margins/session.md"
  test -f "$RUN_ROOT/$name/run/backup/workspaces/fixture/config.toml"
  grep -Fx 'Help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.' \
    "$RUN_ROOT/$name/run/user-prompt.txt" >/dev/null
  grep -Fx "export MARGINS_HOME=$margins_home" \
    "$RUN_ROOT/$name/run/rollout-environment.sh" >/dev/null
  grep -Fx 'unset MARGINS_WORKSPACE' \
    "$RUN_ROOT/$name/run/rollout-environment.sh" >/dev/null
}

make_fixture safe
if "$HARNESS" workspace-id --run-dir "$RUN_ROOT/safe/run" >/dev/null 2>&1; then
  echo "expected observer lookup to refuse when no Workspace was generated" >&2
  exit 1
fi
printf 'Setup completed. No credential values were inspected.\n' > "$RUN_ROOT/safe/transcript.txt"
printf '{"status":"ok"}\n' > "$RUN_ROOT/safe/run/final-status.json"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/safe/run" \
  --transcript "$RUN_ROOT/safe/transcript.txt" \
  --final-status "$RUN_ROOT/safe/run/final-status.json" >/dev/null
test -f "$RUN_ROOT/safe/vault/.margins/session.md"
test -f "$RUN_ROOT/safe/margins-home/workspaces/fixture/index.db"
grep -Fx 'legacy session state' "$RUN_ROOT/safe/vault/.margins/session.md" >/dev/null
grep -Fx 'old index' "$RUN_ROOT/safe/margins-home/workspaces/fixture/index.db" >/dev/null
python3 - "$RUN_ROOT/safe/run/hard-gates.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
assert value["passed"] is True
assert value["vault_markdown_immutability"]["passed"] is True
assert value["secret_material_in_transcript"]["passed"] is True
assert value["preexisting_setup_restoration"]["passed"] is True
PY

make_fixture secret
printf 'debug api_key="sk-or-v1-abcdefghijklmnopqrstuv"\n' > "$RUN_ROOT/secret/transcript.txt"
printf '{"api_key":"sk-abcdefghijklmnopqrstuvwxyz123456"}\n' > "$RUN_ROOT/secret/status-input.json"
if "$HARNESS" finalize \
  --run-dir "$RUN_ROOT/secret/run" \
  --transcript "$RUN_ROOT/secret/transcript.txt" \
  --final-status "$RUN_ROOT/secret/status-input.json" >/dev/null; then
  echo "expected secret-bearing transcript to fail" >&2
  exit 1
fi
test -f "$RUN_ROOT/secret/vault/.margins/session.md"
test -f "$RUN_ROOT/secret/margins-home/workspaces/fixture/index.db"
grep -F '"kind": "openrouter_key"' "$RUN_ROOT/secret/run/hard-gates.json" >/dev/null
grep -F '"source": "final-status.json"' "$RUN_ROOT/secret/run/hard-gates.json" >/dev/null
grep -F '[REDACTED_CREDENTIAL:openrouter_key]' "$RUN_ROOT/secret/run/transcript.txt" >/dev/null
grep -F '[REDACTED_CREDENTIAL:json_api_key]' "$RUN_ROOT/secret/run/final-status.json" >/dev/null
if grep -F 'sk-or-v1-abcdefghijklmnopqrstuv' "$RUN_ROOT/secret/run/transcript.txt" >/dev/null; then
  echo "captured transcript retained credential material" >&2
  exit 1
fi
if grep -F 'sk-abcdefghijklmnopqrstuvwxyz123456' "$RUN_ROOT/secret/run/final-status.json" >/dev/null; then
  echo "captured final status retained credential material" >&2
  exit 1
fi

make_fixture modified
printf '\nUnexpected setup edit.\n' >> "$RUN_ROOT/modified/vault/one.md"
printf 'Setup completed.\n' > "$RUN_ROOT/modified/transcript.txt"
if "$HARNESS" finalize \
  --run-dir "$RUN_ROOT/modified/run" \
  --transcript "$RUN_ROOT/modified/transcript.txt" >/dev/null; then
  echo "expected Markdown mutation to fail" >&2
  exit 1
fi
test -f "$RUN_ROOT/modified/vault/.margins/session.md"
test -f "$RUN_ROOT/modified/margins-home/workspaces/fixture/index.db"
grep -F '"one.md"' "$RUN_ROOT/modified/run/vault-changes.json" >/dev/null

make_fixture partial_finalize
mkdir -p "$RUN_ROOT/partial_finalize/margins-home/workspaces/partial"
printf 'partial setup\n' > "$RUN_ROOT/partial_finalize/margins-home/workspaces/partial/index.db"
printf 'Setup failed partway.\n' > "$RUN_ROOT/partial_finalize/transcript.txt"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/partial_finalize/run" \
  --transcript "$RUN_ROOT/partial_finalize/transcript.txt" >/dev/null
test -f "$RUN_ROOT/partial_finalize/vault/.margins/session.md"
test -f "$RUN_ROOT/partial_finalize/margins-home/workspaces/fixture/index.db"
test -f "$RUN_ROOT/partial_finalize/run/generated/workspaces/partial/index.db"

mkdir -p "$RUN_ROOT/fresh/vault" "$RUN_ROOT/fresh/margins-home" "$RUN_ROOT/fresh/run"
printf '# Fresh\n' > "$RUN_ROOT/fresh/vault/fresh.md"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/fresh/vault" \
  --margins-home "$RUN_ROOT/fresh/margins-home" \
  --run-dir "$RUN_ROOT/fresh/run" >/dev/null
mkdir -p "$RUN_ROOT/fresh/margins-home/workspaces/fresh/captures"
printf 'generated index\n' > "$RUN_ROOT/fresh/margins-home/workspaces/fresh/index.db"
printf 'id = "fresh"\n\n[policy]\nentities = []\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n\n[bindings.captures]\nkind = "captures"\npath = "%s"\n' \
  "$RUN_ROOT/fresh/vault" "$RUN_ROOT/fresh/margins-home/workspaces/fresh/captures" \
  > "$RUN_ROOT/fresh/margins-home/workspaces/fresh/config.toml"
test "$("$HARNESS" workspace-id --run-dir "$RUN_ROOT/fresh/run")" = fresh
printf 'Setup completed.\n' > "$RUN_ROOT/fresh/transcript.txt"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/fresh/run" \
  --transcript "$RUN_ROOT/fresh/transcript.txt" >/dev/null
test ! -e "$RUN_ROOT/fresh/margins-home/workspaces/fresh"
test -f "$RUN_ROOT/fresh/run/generated/workspaces/fresh/index.db"
test -f "$RUN_ROOT/fresh/run/workspace-configs-after/fresh.toml"

mkdir -p "$RUN_ROOT/recovery/vault/.margins" "$RUN_ROOT/recovery/margins-home/workspaces/fixture" "$RUN_ROOT/recovery/run"
printf '# Recovery\n' > "$RUN_ROOT/recovery/vault/recovery.md"
printf 'old local\n' > "$RUN_ROOT/recovery/vault/.margins/old.md"
printf 'id = "fixture"\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n' \
  "$RUN_ROOT/recovery/vault" > "$RUN_ROOT/recovery/margins-home/workspaces/fixture/config.toml"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/recovery/vault" \
  --margins-home "$RUN_ROOT/recovery/margins-home" \
  --run-dir "$RUN_ROOT/recovery/run" >/dev/null
mkdir -p "$RUN_ROOT/recovery/margins-home/workspaces/partial"
printf 'partial setup\n' > "$RUN_ROOT/recovery/margins-home/workspaces/partial/index.db"
"$HARNESS" restore --run-dir "$RUN_ROOT/recovery/run" >/dev/null
test -f "$RUN_ROOT/recovery/vault/.margins/old.md"
test -f "$RUN_ROOT/recovery/margins-home/workspaces/fixture/config.toml"
grep -Fx 'old local' "$RUN_ROOT/recovery/vault/.margins/old.md" >/dev/null
test ! -e "$RUN_ROOT/recovery/margins-home/workspaces/partial"
test -f "$RUN_ROOT/recovery/run/generated/workspaces/partial/index.db"

mkdir -p "$RUN_ROOT/symlink/vault" "$RUN_ROOT/symlink/margins-home/workspaces" "$RUN_ROOT/symlink/external-state" "$RUN_ROOT/symlink/run"
printf '# Symlink\n' > "$RUN_ROOT/symlink/vault/symlink.md"
printf 'id = "linked"\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n' \
  "$RUN_ROOT/symlink/vault" > "$RUN_ROOT/symlink/external-state/config.toml"
ln -s "$RUN_ROOT/symlink/external-state" "$RUN_ROOT/symlink/margins-home/workspaces/linked"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/symlink/vault" \
  --margins-home "$RUN_ROOT/symlink/margins-home" \
  --run-dir "$RUN_ROOT/symlink/run" >/dev/null
test -L "$RUN_ROOT/symlink/run/backup/workspaces/linked"
test -f "$RUN_ROOT/symlink/external-state/config.toml"
"$HARNESS" restore --run-dir "$RUN_ROOT/symlink/run" >/dev/null
test -L "$RUN_ROOT/symlink/margins-home/workspaces/linked"
test -f "$RUN_ROOT/symlink/margins-home/workspaces/linked/config.toml"

echo "workspace setup rollout review harness: ok"
