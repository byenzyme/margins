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
  local other_vault="$RUN_ROOT/$name/other-vault"
  local margins_home="$RUN_ROOT/$name/margins-home"
  local state="$margins_home/workspaces/fixture"
  local configs="$margins_home/configs"
  mkdir -p "$vault/.margins" "$other_vault" "$state/captures" "$configs" "$RUN_ROOT/$name/run"
  printf '# One\n\nA note.\n' > "$RUN_ROOT/$name/vault/one.md"
  printf 'legacy session state\n' > "$vault/.margins/session.md"
  printf 'old index\n' > "$state/index.db"
  # The Workspace declaration is one Enzyme program; workspaces/<id>/ is state only.
  printf '// fixture\nworkspace "fixture" {\n  source markdown "home" { path "%s" }\n  source margins-captures "captures" { path "%s" }\n  remember in folder "." in source "home" create note\n}\n' \
    "$vault" "$state/captures" > "$configs/fixture.enzyme"
  printf 'workspace "unrelated" {\n  source markdown "home" { path "%s" }\n  remember in folder "." in source "home" create note\n}\n' \
    "$other_vault" > "$configs/unrelated.enzyme"
  printf 'settings {\n  generation hosted\n  updates disabled\n}\n' > "$configs/settings.enzyme"
  cp "$configs/settings.enzyme" "$RUN_ROOT/$name/settings.original"
  cp "$configs/fixture.enzyme" "$RUN_ROOT/$name/fixture-program.original"
  printf '[llm]\nmode = "hosted"\n\n[defaults]\nminimum_tags = 2\n\n[cli]\nnote_agent = "codex"\n\n[vaults."%s"]\nentities = ["folder:people"]\n\n[vaults."%s"]\nentities = ["#unrelated"]\n\n[workspaces.target]\nentities = ["folder:people"]\n\n[workspaces.target.sources.notes]\npath = "%s"\nwritable = true\n\n[workspaces.unrelated]\nentities = ["#unrelated"]\n\n[workspaces.unrelated.sources.notes]\npath = "%s"\nwritable = true\n' \
    "$vault" "$other_vault" "$vault" "$other_vault" > "$margins_home/config.toml"
  cp "$margins_home/config.toml" "$RUN_ROOT/$name/global-config.original"
  chmod 0640 "$margins_home/config.toml"
  "$HARNESS" prepare \
    --vault "$vault" \
    --margins-home "$margins_home" \
    --run-dir "$RUN_ROOT/$name/run" >/dev/null
  test ! -e "$vault/.margins"
  test ! -e "$state"
  # Only the machine settings program stays active; Workspace programs are isolated.
  test "$(ls "$configs")" = settings.enzyme
  cmp "$RUN_ROOT/$name/settings.original" "$configs/settings.enzyme"
  test -f "$RUN_ROOT/$name/run/backup/vault-dot-margins/session.md"
  test -f "$RUN_ROOT/$name/run/backup/workspaces/fixture/index.db"
  test ! -e "$RUN_ROOT/$name/run/backup/workspaces/fixture/config.toml"
  cmp "$RUN_ROOT/$name/fixture-program.original" "$RUN_ROOT/$name/run/backup/configs/fixture.enzyme"
  test -f "$RUN_ROOT/$name/run/backup/configs/unrelated.enzyme"
  python3 - "$RUN_ROOT/$name/run/preexisting-workspaces.json" "$RUN_ROOT/$name/run/run.json" "$margins_home" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
assert [row["id"] for row in rows] == ["fixture"], rows
assert rows[0]["config_format"] == "enzyme", rows
assert rows[0]["config"] == sys.argv[3] + "/configs/fixture.enzyme", rows
manifest = json.load(open(sys.argv[2]))
assert manifest["workspace_ids_before"] == ["fixture", "unrelated"], manifest
assert manifest["config_registry_backed_up"] is True, manifest
PY
  test -f "$RUN_ROOT/$name/run/backup/machine-config/config.toml"
  cmp "$RUN_ROOT/$name/global-config.original" "$RUN_ROOT/$name/run/backup/machine-config/config.toml"
  python3 - "$margins_home/config.toml" "$vault" "$other_vault" <<'PY'
import sys, tomllib
value = tomllib.load(open(sys.argv[1], "rb"))
assert value["llm"]["mode"] == "hosted"
assert value["defaults"]["minimum_tags"] == 2
assert value["cli"]["note_agent"] == "codex"
assert sys.argv[2] not in value.get("vaults", {})
assert "target" not in value.get("workspaces", {})
assert value["vaults"][sys.argv[3]]["entities"] == ["#unrelated"]
assert value["workspaces"]["unrelated"]["sources"]["notes"]["path"] == sys.argv[3]
PY
  grep -Fx 'Help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.' \
    "$RUN_ROOT/$name/run/user-prompt.txt" >/dev/null
  grep -Fx "export MARGINS_HOME=$margins_home" \
    "$RUN_ROOT/$name/run/rollout-environment.sh" >/dev/null
  grep -Fx 'unset MARGINS_WORKSPACE' \
    "$RUN_ROOT/$name/run/rollout-environment.sh" >/dev/null
}

make_fixture safe
# The rollout's binary migrates the legacy machine config inside the isolation.
mv "$RUN_ROOT/safe/margins-home/config.toml" "$RUN_ROOT/safe/margins-home/config.toml.migrated"
printf '[cli]\nnote_agent = "codex"\n' > "$RUN_ROOT/safe/margins-home/margins.toml"
printf 'settings {\n  generation local\n  updates disabled\n}\n' > "$RUN_ROOT/safe/margins-home/configs/settings.enzyme"
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
cmp "$RUN_ROOT/safe/fixture-program.original" "$RUN_ROOT/safe/margins-home/configs/fixture.enzyme"
cmp "$RUN_ROOT/safe/global-config.original" "$RUN_ROOT/safe/margins-home/config.toml"
cmp "$RUN_ROOT/safe/settings.original" "$RUN_ROOT/safe/margins-home/configs/settings.enzyme"
test ! -e "$RUN_ROOT/safe/margins-home/margins.toml"
test ! -e "$RUN_ROOT/safe/margins-home/config.toml.migrated"
test -f "$RUN_ROOT/safe/run/generated/machine-config/margins.toml"
test -f "$RUN_ROOT/safe/run/generated/machine-config/config.toml.migrated"
grep -Fx 'legacy session state' "$RUN_ROOT/safe/vault/.margins/session.md" >/dev/null
grep -Fx 'old index' "$RUN_ROOT/safe/margins-home/workspaces/fixture/index.db" >/dev/null
python3 - "$RUN_ROOT/safe/run/hard-gates.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
assert value["passed"] is True
assert value["vault_markdown_immutability"]["passed"] is True
assert value["secret_material_in_transcript"]["passed"] is True
assert value["preexisting_setup_restoration"]["passed"] is True
assert value["preexisting_setup_restoration"]["details"]["global_config_exact"] is True
assert value["preexisting_setup_restoration"]["details"]["config_registry_exact"] is True
assert value["preexisting_setup_restoration"]["details"]["config_registry_tracked"] is True
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
mkdir -p "$RUN_ROOT/partial_finalize/margins-home/workspaces/partial" "$RUN_ROOT/partial_finalize/margins-home/configs"
printf 'partial setup\n' > "$RUN_ROOT/partial_finalize/margins-home/workspaces/partial/index.db"
printf 'workspace "partial" {\n  source markdown "home" { path "%s" }\n}\n' \
  "$RUN_ROOT/partial_finalize/vault" > "$RUN_ROOT/partial_finalize/margins-home/configs/partial.enzyme"
printf 'Setup failed partway.\n' > "$RUN_ROOT/partial_finalize/transcript.txt"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/partial_finalize/run" \
  --transcript "$RUN_ROOT/partial_finalize/transcript.txt" >/dev/null
test -f "$RUN_ROOT/partial_finalize/vault/.margins/session.md"
test -f "$RUN_ROOT/partial_finalize/margins-home/workspaces/fixture/index.db"
test -f "$RUN_ROOT/partial_finalize/run/generated/workspaces/partial/index.db"
test -f "$RUN_ROOT/partial_finalize/run/generated/configs/partial.enzyme"
test ! -e "$RUN_ROOT/partial_finalize/margins-home/configs/partial.enzyme"
cmp "$RUN_ROOT/partial_finalize/fixture-program.original" "$RUN_ROOT/partial_finalize/margins-home/configs/fixture.enzyme"
test -f "$RUN_ROOT/partial_finalize/run/workspace-configs-after/partial.enzyme"

mkdir -p "$RUN_ROOT/fresh/vault" "$RUN_ROOT/fresh/margins-home" "$RUN_ROOT/fresh/run"
printf '# Fresh\n' > "$RUN_ROOT/fresh/vault/fresh.md"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/fresh/vault" \
  --margins-home "$RUN_ROOT/fresh/margins-home" \
  --run-dir "$RUN_ROOT/fresh/run" >/dev/null
mkdir -p "$RUN_ROOT/fresh/margins-home/workspaces/fresh/captures" "$RUN_ROOT/fresh/margins-home/configs"
printf '[vaults."%s"]\nentities = ["#generated"]\n' \
  "$RUN_ROOT/fresh/vault" > "$RUN_ROOT/fresh/margins-home/config.toml"
printf 'generated index\n' > "$RUN_ROOT/fresh/margins-home/workspaces/fresh/index.db"
printf 'workspace "fresh" {\n  source margins-captures "captures" {\n    path "%s"\n  }\n\n  source markdown "home" { path "%s" }\n  // a comment with path "/elsewhere"\n  remember in folder "inbox" in source "home" create note\n}\n' \
  "$RUN_ROOT/fresh/margins-home/workspaces/fresh/captures" "$RUN_ROOT/fresh/vault" \
  > "$RUN_ROOT/fresh/margins-home/configs/fresh.enzyme"
test "$("$HARNESS" workspace-id --run-dir "$RUN_ROOT/fresh/run")" = fresh
printf 'Setup completed.\n' > "$RUN_ROOT/fresh/transcript.txt"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/fresh/run" \
  --transcript "$RUN_ROOT/fresh/transcript.txt" >/dev/null
test ! -e "$RUN_ROOT/fresh/margins-home/workspaces/fresh"
test ! -e "$RUN_ROOT/fresh/margins-home/configs"
test ! -e "$RUN_ROOT/fresh/margins-home/config.toml"
test -f "$RUN_ROOT/fresh/run/generated/workspaces/fresh/index.db"
test -f "$RUN_ROOT/fresh/run/generated/configs/fresh.enzyme"
test -f "$RUN_ROOT/fresh/run/generated/machine-config/config.toml"
cmp "$RUN_ROOT/fresh/run/generated/configs/fresh.enzyme" "$RUN_ROOT/fresh/run/workspace-configs-after/fresh.enzyme"
test -f "$RUN_ROOT/fresh/run/workspace-configs-after/fresh.enzyme.sha256"
test ! -e "$RUN_ROOT/fresh/run/workspace-configs-after/fresh.toml"
grep -F 'fresh.enzyme' "$RUN_ROOT/fresh/run/generated-config-state.json" >/dev/null
python3 - "$RUN_ROOT/fresh/run/generated-workspaces.json" "$RUN_ROOT/fresh/vault" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
assert [row["id"] for row in rows] == ["fresh"], rows
assert rows[0]["config_format"] == "enzyme", rows
assert rows[0]["overlapping_bindings"] == [sys.argv[2]], rows
PY

mkdir -p "$RUN_ROOT/malformed/vault/.margins" "$RUN_ROOT/malformed/margins-home/workspaces/fixture" "$RUN_ROOT/malformed/run"
printf '# Malformed\n' > "$RUN_ROOT/malformed/vault/note.md"
printf 'old local\n' > "$RUN_ROOT/malformed/vault/.margins/old.md"
printf 'old index\n' > "$RUN_ROOT/malformed/margins-home/workspaces/fixture/index.db"
printf '[vaults."unterminated"\n' > "$RUN_ROOT/malformed/margins-home/config.toml"
if "$HARNESS" prepare \
  --vault "$RUN_ROOT/malformed/vault" \
  --margins-home "$RUN_ROOT/malformed/margins-home" \
  --run-dir "$RUN_ROOT/malformed/run" >/dev/null 2>&1; then
  echo "expected malformed global config to fail closed" >&2
  exit 1
fi
test -f "$RUN_ROOT/malformed/vault/.margins/old.md"
test -f "$RUN_ROOT/malformed/margins-home/workspaces/fixture/index.db"
grep -Fx '[vaults."unterminated"' "$RUN_ROOT/malformed/margins-home/config.toml" >/dev/null

mkdir -p "$RUN_ROOT/ambiguous/vault/.margins" "$RUN_ROOT/ambiguous/margins-home/workspaces/fixture" "$RUN_ROOT/ambiguous/run"
printf '# Ambiguous\n' > "$RUN_ROOT/ambiguous/vault/note.md"
printf 'old local\n' > "$RUN_ROOT/ambiguous/vault/.margins/old.md"
printf 'old index\n' > "$RUN_ROOT/ambiguous/margins-home/workspaces/fixture/index.db"
printf '[defaults]\ndescription = """\n[vaults."%s"]\n[update]\n"""\n\n[vaults."%s"]\nentities = ["#old"]\n\n[update]\nauto = false\n' \
  "$RUN_ROOT/ambiguous/vault" "$RUN_ROOT/ambiguous/vault" \
  > "$RUN_ROOT/ambiguous/margins-home/config.toml"
if "$HARNESS" prepare \
  --vault "$RUN_ROOT/ambiguous/vault" \
  --margins-home "$RUN_ROOT/ambiguous/margins-home" \
  --run-dir "$RUN_ROOT/ambiguous/run" >/dev/null 2>&1; then
  echo "expected ambiguous global config to fail semantic preservation" >&2
  exit 1
fi
test -f "$RUN_ROOT/ambiguous/vault/.margins/old.md"
test -f "$RUN_ROOT/ambiguous/margins-home/workspaces/fixture/index.db"
grep -F 'description = """' "$RUN_ROOT/ambiguous/margins-home/config.toml" >/dev/null

mkdir -p "$RUN_ROOT/recovery/vault/.margins" "$RUN_ROOT/recovery/margins-home/workspaces/fixture" "$RUN_ROOT/recovery/run"
printf '# Recovery\n' > "$RUN_ROOT/recovery/vault/recovery.md"
printf 'old local\n' > "$RUN_ROOT/recovery/vault/.margins/old.md"
printf 'id = "fixture"\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n' \
  "$RUN_ROOT/recovery/vault" > "$RUN_ROOT/recovery/margins-home/workspaces/fixture/config.toml"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/recovery/vault" \
  --margins-home "$RUN_ROOT/recovery/margins-home" \
  --run-dir "$RUN_ROOT/recovery/run" >/dev/null
# A legacy workspaces/<id>/config.toml with no program is still a recognized input.
python3 - "$RUN_ROOT/recovery/run/preexisting-workspaces.json" <<'PY'
import json, sys
rows = json.load(open(sys.argv[1]))
assert [(row["id"], row["config_format"]) for row in rows] == [("fixture", "legacy-toml")], rows
PY
# Setup migrated the legacy declaration into the Workspace language.
mkdir -p "$RUN_ROOT/recovery/margins-home/configs"
printf 'workspace "fixture" {\n  source markdown "home" { path "%s" }\n}\n' \
  "$RUN_ROOT/recovery/vault" > "$RUN_ROOT/recovery/margins-home/configs/fixture.enzyme"
mkdir -p "$RUN_ROOT/recovery/margins-home/workspaces/partial"
printf 'partial setup\n' > "$RUN_ROOT/recovery/margins-home/workspaces/partial/index.db"
"$HARNESS" restore --run-dir "$RUN_ROOT/recovery/run" >/dev/null
test -f "$RUN_ROOT/recovery/vault/.margins/old.md"
test -f "$RUN_ROOT/recovery/margins-home/workspaces/fixture/config.toml"
grep -Fx 'old local' "$RUN_ROOT/recovery/vault/.margins/old.md" >/dev/null
test ! -e "$RUN_ROOT/recovery/margins-home/workspaces/partial"
test ! -e "$RUN_ROOT/recovery/margins-home/configs"
test -f "$RUN_ROOT/recovery/run/generated/workspaces/partial/index.db"
test -f "$RUN_ROOT/recovery/run/generated/configs/fixture.enzyme"

mkdir -p "$RUN_ROOT/legacy-v2/vault" "$RUN_ROOT/legacy-v2/margins-home" "$RUN_ROOT/legacy-v2/run/backup"
printf '[llm]\nmode = "hosted"\n' > "$RUN_ROOT/legacy-v2/margins-home/config.toml"
printf '{}\n' > "$RUN_ROOT/legacy-v2/run/preexisting-workspace-state.json"
printf '{}\n' > "$RUN_ROOT/legacy-v2/run/vault-local-state-before.json"
python3 - "$RUN_ROOT/legacy-v2/run/run.json" "$RUN_ROOT/legacy-v2/vault" "$RUN_ROOT/legacy-v2/margins-home" <<'PY'
import json, sys
json.dump({
    "schema": "margins.workspace-setup-rollout-review.v2",
    "vault": sys.argv[2],
    "margins_home": sys.argv[3],
    "workspace_ids_before": [],
    "preexisting_workspaces": [],
    "workspace_registry_backed_up": False,
    "vault_local_state_backed_up": False,
}, open(sys.argv[1], "w"))
PY
"$HARNESS" restore --run-dir "$RUN_ROOT/legacy-v2/run" >/dev/null
grep -Fx 'mode = "hosted"' "$RUN_ROOT/legacy-v2/margins-home/config.toml" >/dev/null
grep -F '"global_config_tracked": false' "$RUN_ROOT/legacy-v2/run/restoration.json" >/dev/null
grep -F '"config_registry_tracked": false' "$RUN_ROOT/legacy-v2/run/restoration.json" >/dev/null

mkdir -p "$RUN_ROOT/symlink/vault" "$RUN_ROOT/symlink/margins-home/workspaces" "$RUN_ROOT/symlink/external-state" "$RUN_ROOT/symlink/run"
printf '# Symlink\n' > "$RUN_ROOT/symlink/vault/symlink.md"
printf 'id = "linked"\n\n[bindings.home]\nkind = "notes"\npath = "%s"\nrole = "home"\n' \
  "$RUN_ROOT/symlink/vault" > "$RUN_ROOT/symlink/external-state/config.toml"
ln -s "$RUN_ROOT/symlink/external-state" "$RUN_ROOT/symlink/margins-home/workspaces/linked"
printf '[llm]\nmode = "auto"\n\n[vaults."%s"]\nentities = ["#old"]\n' \
  "$RUN_ROOT/symlink/vault" > "$RUN_ROOT/symlink/external-global-config.toml"
ln -s "$RUN_ROOT/symlink/external-global-config.toml" "$RUN_ROOT/symlink/margins-home/config.toml"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/symlink/vault" \
  --margins-home "$RUN_ROOT/symlink/margins-home" \
  --run-dir "$RUN_ROOT/symlink/run" >/dev/null
test -L "$RUN_ROOT/symlink/run/backup/workspaces/linked"
test -L "$RUN_ROOT/symlink/run/backup/machine-config/config.toml"
test ! -L "$RUN_ROOT/symlink/margins-home/config.toml"
test -f "$RUN_ROOT/symlink/external-state/config.toml"
"$HARNESS" restore --run-dir "$RUN_ROOT/symlink/run" >/dev/null
test -L "$RUN_ROOT/symlink/margins-home/workspaces/linked"
test -L "$RUN_ROOT/symlink/margins-home/config.toml"
test -f "$RUN_ROOT/symlink/margins-home/workspaces/linked/config.toml"
grep -F '#old' "$RUN_ROOT/symlink/margins-home/config.toml" >/dev/null

mkdir -p "$RUN_ROOT/symlink-drift/vault" "$RUN_ROOT/symlink-drift/margins-home" "$RUN_ROOT/symlink-drift/run"
printf '# Symlink drift\n' > "$RUN_ROOT/symlink-drift/vault/note.md"
printf '[llm]\nmode = "auto"\n\n[vaults."%s"]\nentities = ["#old"]\n' \
  "$RUN_ROOT/symlink-drift/vault" > "$RUN_ROOT/symlink-drift/external-config.toml"
ln -s "$RUN_ROOT/symlink-drift/external-config.toml" "$RUN_ROOT/symlink-drift/margins-home/config.toml"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/symlink-drift/vault" \
  --margins-home "$RUN_ROOT/symlink-drift/margins-home" \
  --run-dir "$RUN_ROOT/symlink-drift/run" >/dev/null
printf '# external mutation\n' >> "$RUN_ROOT/symlink-drift/external-config.toml"
if "$HARNESS" restore --run-dir "$RUN_ROOT/symlink-drift/run" >/dev/null; then
  echo "expected external symlink-target drift to fail restoration" >&2
  exit 1
fi
test -L "$RUN_ROOT/symlink-drift/margins-home/config.toml"
grep -F '"global_config_exact": false' "$RUN_ROOT/symlink-drift/run/restoration.json" >/dev/null

# An upgraded home: margins.toml (which may still carry legacy setup for the
# practice), retired originals, and the engine settings program.
mkdir -p "$RUN_ROOT/upgraded/vault" "$RUN_ROOT/upgraded/other-vault" "$RUN_ROOT/upgraded/margins-home/configs" "$RUN_ROOT/upgraded/margins-home/workspaces/fixture" "$RUN_ROOT/upgraded/run"
printf '# Upgraded\n' > "$RUN_ROOT/upgraded/vault/note.md"
UPGRADED_HOME="$RUN_ROOT/upgraded/margins-home"
printf '[workspace]\ndefault = "fixture"\n\n[cli]\nnote_agent = "codex"\n\n[vaults."%s"]\nentities = ["folder:people"]\n\n[vaults."%s"]\nentities = ["#unrelated"]\n' \
  "$RUN_ROOT/upgraded/vault" "$RUN_ROOT/upgraded/other-vault" > "$UPGRADED_HOME/margins.toml"
chmod 0600 "$UPGRADED_HOME/margins.toml"
printf '[llm]\nmode = "hosted"\n' > "$UPGRADED_HOME/config.toml.migrated"
printf 'old\n' > "$UPGRADED_HOME/config.toml.migrated.1"
printf 'settings {\n  generation hosted\n  updates disabled\n}\n' > "$UPGRADED_HOME/configs/settings.enzyme"
printf 'workspace "fixture" {\n  source markdown "home" { path "%s" }\n  remember in folder "." in source "home" create note\n}\n' \
  "$RUN_ROOT/upgraded/vault" > "$UPGRADED_HOME/configs/fixture.enzyme"
printf 'index\n' > "$UPGRADED_HOME/workspaces/fixture/enzyme.db"
for file in margins.toml config.toml.migrated config.toml.migrated.1 configs/settings.enzyme; do
  mkdir -p "$(dirname "$RUN_ROOT/upgraded/original/$file")"
  cp -p "$UPGRADED_HOME/$file" "$RUN_ROOT/upgraded/original/$file"
done
"$HARNESS" prepare \
  --vault "$RUN_ROOT/upgraded/vault" \
  --margins-home "$UPGRADED_HOME" \
  --run-dir "$RUN_ROOT/upgraded/run" >/dev/null
test ! -e "$UPGRADED_HOME/config.toml.migrated"
test ! -e "$UPGRADED_HOME/config.toml.migrated.1"
test -f "$RUN_ROOT/upgraded/run/backup/machine-config/config.toml.migrated.1"
cmp "$RUN_ROOT/upgraded/original/margins.toml" "$RUN_ROOT/upgraded/run/backup/machine-config/margins.toml"
cmp "$RUN_ROOT/upgraded/original/configs/settings.enzyme" "$UPGRADED_HOME/configs/settings.enzyme"
test ! -e "$UPGRADED_HOME/configs/fixture.enzyme"
python3 - "$UPGRADED_HOME/margins.toml" "$RUN_ROOT/upgraded/vault" "$RUN_ROOT/upgraded/other-vault" "$RUN_ROOT/upgraded/run/run.json" <<'PY'
import json, sys, tomllib
value = tomllib.load(open(sys.argv[1], "rb"))
assert value["cli"]["note_agent"] == "codex", value
assert sys.argv[2] not in value.get("vaults", {}), value
assert value["vaults"][sys.argv[3]]["entities"] == ["#unrelated"], value
manifest = json.load(open(sys.argv[4]))
assert manifest["machine_config_backed_up"] == [
    "config.toml.migrated", "config.toml.migrated.1", "margins.toml"
], manifest
assert manifest["workspace_ids_before"] == ["fixture"], manifest
PY
# The rollout changes machine config and settings, and an older binary writes config.toml.
printf '[workspace]\ndefault = "generated"\n' > "$UPGRADED_HOME/margins.toml"
printf '[llm]\nmode = "local"\n' > "$UPGRADED_HOME/config.toml"
printf 'settings {\n  generation local\n}\n' > "$UPGRADED_HOME/configs/settings.enzyme"
printf 'Setup completed.\n' > "$RUN_ROOT/upgraded/transcript.txt"
"$HARNESS" finalize \
  --run-dir "$RUN_ROOT/upgraded/run" \
  --transcript "$RUN_ROOT/upgraded/transcript.txt" >/dev/null
for file in margins.toml config.toml.migrated config.toml.migrated.1 configs/settings.enzyme; do
  cmp "$RUN_ROOT/upgraded/original/$file" "$UPGRADED_HOME/$file"
done
test "$(stat -c %a "$UPGRADED_HOME/margins.toml" 2>/dev/null || stat -f %Lp "$UPGRADED_HOME/margins.toml")" = 600
test ! -e "$UPGRADED_HOME/config.toml"
test -f "$RUN_ROOT/upgraded/run/generated/machine-config/config.toml"
test -f "$RUN_ROOT/upgraded/run/generated/machine-config/margins.toml"
python3 - "$RUN_ROOT/upgraded/run/hard-gates.json" <<'PY'
import json, sys
value = json.load(open(sys.argv[1]))
details = value["preexisting_setup_restoration"]["details"]
assert value["preexisting_setup_restoration"]["passed"] is True, value
assert details["global_config_exact"] is True, details
assert details["config_registry_exact"] is True, details
PY

# Restoration refuses when a retired original drifted without a backup.
mkdir -p "$RUN_ROOT/drift/vault" "$RUN_ROOT/drift/margins-home" "$RUN_ROOT/drift/run"
printf '# Drift\n' > "$RUN_ROOT/drift/vault/note.md"
printf '[cli]\nnote_agent = "codex"\n' > "$RUN_ROOT/drift/margins-home/margins.toml"
"$HARNESS" prepare \
  --vault "$RUN_ROOT/drift/vault" \
  --margins-home "$RUN_ROOT/drift/margins-home" \
  --run-dir "$RUN_ROOT/drift/run" >/dev/null
rm "$RUN_ROOT/drift/run/backup/machine-config/margins.toml"
printf '[cli]\nnote_agent = "cursor"\n' > "$RUN_ROOT/drift/margins-home/margins.toml"
if "$HARNESS" restore --run-dir "$RUN_ROOT/drift/run" >/dev/null 2>&1; then
  echo "expected a drifted margins.toml without its backup to fail restoration" >&2
  exit 1
fi

echo "workspace setup rollout review harness: ok"
