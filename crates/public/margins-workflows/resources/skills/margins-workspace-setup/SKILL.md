---
name: margins-workspace-setup
description: Run Margins Desktop workspace setup by using the generic Enzyme workspace setup skill with Margins-provided project, destination, and binary context.
allowed-tools: Read, Glob, Grep, Bash, Write, Edit
---

# Margins Workspace Setup

<!-- Canonical copy owned by the public margins-workflows crate. -->

Use this skill when the user starts setup from Margins Desktop. This is a wrapper around the generic `enzyme-workspace-setup` skill. Keep Margins product language here; keep Enzyme's general vault diagnosis and repair principles in `enzyme-workspace-setup`.

## Inputs From Margins

The Margins setup prompt should provide:

- Project folder path.
- Current note destination, usually the configured inbox or meeting-note folder.
- `ENZYME_BIN`, an absolute path to the Enzyme binary Margins installed or resolved.
- The local installed skill paths under `.margins/skills/`.

Treat those inputs as authoritative. Do not search for another project registry through Enzyme. Enzyme owns vault indexing; Margins owns project selection and note destination.

## Binary Boundary

Use the Margins-provided binary for every Enzyme call:

```bash
"$ENZYME_BIN" --version
```

If `ENZYME_BIN` is unset or fails, do not hunt for a global binary and do not install anything yourself. Tell the user to return to Margins Desktop and click **Setup with agent** again; Margins will reinstall or re-resolve the binary before copying a fresh prompt.

Never delete `~/.enzyme/auth.json` or run `"$ENZYME_BIN" logout` without explicit confirmation.

## Project Boundary

The configured project folder may not be where the user's notes actually live. Diagnosis is still read-only until the user confirms otherwise.

If the folder looks empty, scratch, or wrong, stop and ask the user to switch or add the correct project in Margins Desktop, then click **Setup with agent** again. Do not run any `projects add` command through Enzyme; the engine CLI does not own Margins' project registry.

Treat `.margins/` as app-owned state. Treat `.enzyme/` as Enzyme-owned state. Do not move or summarize raw Margins artifacts such as transcripts, JSON, logs, recordings, or cache files during workspace setup.

## How To Use The Generic Skill

First read and follow the installed generic skill:

```text
<project>/.margins/skills/enzyme-workspace-setup/SKILL.md
```

Apply these Margins-specific bindings while using it:

- The vault path is the Margins project folder.
- The preferred destination for any proof note is the configured Margins note destination.
- Normal init is:

```bash
"$ENZYME_BIN" -p "<project folder>" init --quiet
```

- Use `--use-env-llm` only if the user explicitly asks to use their own provider key. Do not inspect, print, unset, or rely on API-key variables during normal setup.
- Skip `scan --write-config`: Margins handles vault configuration. Do not run `scan --write-config`; go directly to `"$ENZYME_BIN" -p "<project folder>" init --quiet`.
- If a stage that needs an LLM is blocked by quota, auth, or rate limit, disclose it as a blocker scoped to that stage and do not invent structural work to compensate.

## Margins Product Promise

The user should leave setup knowing:

- They can start capturing now.
- Existing files are not changed without one explicit consent moment.
- A proof note, if created, is additive.
- Any repair is small, deterministic, body-preserving, backed up, and mechanically reverted before it is claimed safe.
- Margins is not replacing their vault schema; it is using Enzyme to make their existing Markdown more legible to future agents.

When presenting findings, say "Margins can use this" or "Margins cannot use this yet" only after Enzyme evidence supports it. Translate engine facts into user-visible language.
