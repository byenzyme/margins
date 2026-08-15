---
name: margins-workspace-setup
description: Set up a Margins workspace from the user's chosen base folder. Run Margins' filesystem scan, review its folder, tag, link, and running-log evidence, update only the selected workspace section in ~/.margins/config.toml, run margins init, then prove margins recall. Never edits note bodies.
allowed-tools: Bash, Read, Glob, Grep
---

# Margins Workspace Setup

Set up Margins in the folder named by the user. Margins records meetings, saves notes in this workspace, and uses `.margins/recall/index.db` for the vault recall index. Its machine-owned recall policy lives at `~/.margins/config.toml`; local model files live at `~/.margins/models/`. Any target projection artifacts are vault-specific under `.margins/recall/targets/` and never modify target directories.

Keep the workflow bounded. Do not move, rename, delete, summarize, or rewrite notes. Do not edit credentials, provider settings, model files, or unrelated workspace sections. The only planned mutations are the selected workspace section in `~/.margins/config.toml` and the files created by `margins init`.

## 1. Confirm The Workspace

Work from the provided base folder:

```bash
cd "<workspace path>"
pwd
```

Use that path as the Margins base unless it is empty, missing, scratch space, or clearly not where the user's notes live. If the path is wrong, stop and ask for the right folder before writing anything.

## 2. Scan And Inspect Read-Only

Run Margins' filesystem-only discovery before choosing policy:

```bash
margins scan
```

This does not open or create the recall database. Read its JSON evidence, especially `entities`, `top_tags`, `top_links`, `top_folders`, `folder_stats`, `folder_page_entities`, recognized `log:` entries, `frontmatter_samples`, `entity_samples`, `current_config`, and `excluded_folders`. It is the source of truth for what Margins can extract; do not replace it with folder counts or filename guesses.

Use read-only shell commands only to validate and contextualize the scan. Sample the folder tree and representative Markdown from likely note areas:

```bash
find . -maxdepth 3 -type d | sort | sed 's#^\./##' | head -120
find . -type f -name '*.md' \
  -not -path './.margins/*' -not -path './.git/*' -not -path './.obsidian/*' \
  | sort | head -80
```

Open a small, representative set behind the scan's strongest and weakest candidates: meeting notes, daily notes, people pages, project pages, tagged notes, frequently linked pages, recognized running logs, and obvious templates/archive/noise. Diagnose what Margins can use:

- Useful entities: meaningful base folders such as `people` or `projects`, recurring tags such as `#customer`, frequently linked notes like `[[Acme Pilot]]`, and recognized single-file timelines such as `log:journal`.
- Exclude from recall: templates, archive/trash, imports, raw transcripts, attachments, generated files, app/runtime folders, and noisy reference dumps.
- Weak signals: very short notes, inconsistent names, important non-Markdown material, or many notes directly at the root.

Prefer candidates supported by multiple real, recent notes. Prefer a base folder over its descendants when it already covers them. Treat page links already covered by an expandable folder as evidence about that folder, not duplicate configured entities. Never invent tags, links, or logs that the scan did not extract.

Use the vault's actual names. Do not propose restructuring unless the user asks later.

## 3. Write Recall Policy

Create or update a concise policy section for this exact workspace in `~/.margins/config.toml`. Scope edits only to `[vaults."<canonical workspace path>"]`; do not change sections for other vaults. Use only meaningful choices supported by scan plus inspection. Prefer 3-8 mixed entity anchors and 2-8 excluded folders when the vault supports that; a tiny vault may need fewer. Do not default to folder-only entities when the scan surfaced stronger tags, links, or logs.

For a workspace with no existing policy, use Margins to write the initial suggestion, then review and minimally tune only that new section:

```bash
margins scan --write-config
```

If `current_config.has_curated_entities` is already true, `--write-config` deliberately refuses to overwrite it. Compare the existing section with the fresh scan evidence and update only that section when the setup request authorizes it. Explain evidence before replacing a materially different existing policy.

```bash
python3 - <<'PY'
from pathlib import Path
vault = Path.cwd().resolve()
home = Path.home() / ".margins"
config = home / "config.toml"
home.mkdir(parents=True, exist_ok=True)
existing = config.read_text() if config.exists() else "# Margins configuration\n# Edit only the section for the workspace you are setting up.\n"
header = f'[vaults."{vault}"]'
if header in existing:
    raise SystemExit(f"{header} already exists in {config}; update only that section with the inspected policy.")
with config.open("a") as f:
    if existing and not existing.endswith("\n"):
        f.write("\n")
    if config.stat().st_size == 0:
        f.write(existing)
    f.write(f'''

{header}
entities = [
  "folder:people",
  "folder:projects",
  "#customer",
  "[[Acme Pilot]]",
  "log:journal",
]
excluded_folders = [
  "templates",
  "archive",
  "attachments",
]
''')
print(config)
PY
```

The Python block is a fallback for environments where `scan --write-config` cannot establish a new section; normally prefer the command. Edit examples before writing: preserve unrelated sections, keep only candidates found by the scan, and use exact entity syntax (`folder:path`, `#tag`, `[[link]]`, `log:name`). For a flat vault, use `folder:.` instead of inventing folders.

If `.margins/recall/index.db` or an older policy section already exists and appears stale or wrong, explain the evidence and ask before deleting or replacing derived state. Updating only this workspace's policy section is allowed; deleting an index is not.

## 4. Initialize Margins

Run setup from the confirmed base folder:

```bash
margins init
test -f .margins/recall/index.db && echo "recall index exists"
```

The command should print one XML line:

```xml
<margins_init path="<absolute workspace path>" status="ok" config_path="<home>/.margins/config.toml" />
```

The `path` and `config_path` attributes must match the confirmed workspace and `~/.margins/config.toml`. `status="thin"` means there is not enough included note material yet. If `status="no_policy"` or the command fails with a missing-policy message, fix the selected `[vaults."..."]` section and rerun `margins init`.

## 5. Prove Retrieval

Choose one exact phrase and one related semantic query from included notes. Run each twice to check stable behavior:

```bash
margins recall "<exact phrase from an included note>"
margins recall "<related person, project, or decision>"
```

Then query a distinctive phrase from an excluded folder, if one exists. It should not return that excluded file.

Read the XML `status`:

- `ok`: recall retrieval is live. It may be served by bridge retrieval or direct local lookup.
- `thin`: setup completed, but the included corpus is too small.
- `no_policy`: `~/.margins/config.toml` is missing the selected workspace section.
- `no_vault`: you are not inside the initialized workspace.
- `unavailable`: the installed CLI is not the official recall-capable composition.

`margins recall` is read-only. It must report the same effective `config_path`, and it must not initialize, refresh, edit notes, or contact a model provider.

## Report Back

Briefly report the confirmed path; the scan's strongest folder, tag, link, and log candidates; the entities/exclusions selected and why; the `margins init` XML line; whether `.margins/recall/index.db` exists; the recall queries/statuses; and any excluded-content check. Mention small capture habits only when the scan showed a clear gap.
