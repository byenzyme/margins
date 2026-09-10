# Workspace Diagnostic Test Cases

These synthetic workspaces exercise first-project diagnosis and import guidance.
They are intentionally spotty: mixed naming, weak metadata, stale transcripts,
partial cache folders, code/docs folders, and imported material.

Use `../LOOP_SPEC.md` for the self-improvement loop that runs diagnostics,
judges outputs, patches the skill, and reruns these cases.

Each case contains:

- `manifest.json` — structured fixture metadata and expected safe defaults;
- `expected-diagnostic.md` — desired user-facing diagnostic behavior;
- `workspace/` — the folder tree the diagnostic should inspect.

Start with these high-signal cases:

- `fresh-empty`
- `sparse-random-notes`
- `append-only-dated-log`
- `frontmatter-obsidian`
- `daily-notes-vault`
- `codebase-plans-folder`
- `rfc-decision-log`
- `google-drive-export`
- `zoom-transcript-dump`
- `mixed-messy-power-vault`
- `foreign-domain-vault`
- `partial-enzyme-or-margins`
- `fully-configured-vault`

## Control fixtures

Two cases serve as promotion gates rather than exercising failure scenarios:

**`append-only-dated-log`** — the ready-now control. The skill should produce
~zero failure-mode findings. If it manufactures issues on this vault, the
run is over-diagnosing.

**`fully-configured-vault`** — the healthy-vault control. Margins and Enzyme
are already set up, notes are consistently structured, and the private search
cache is initialized. The skill must say no restructuring is needed, show the
four-tier map with all content in "already usable", and still produce a proof
note. Any suggestion of mutation here is an over-diagnosis regression.

Both control cases must pass before a skill revision can be promoted. See
`../LOOP_SPEC.md §Promotion Path` and the design brief `§Verification plan`.

## Mutation guard

Before and after any diagnostic run against a fixture workspace, use the
mutation guard to confirm no files were added, removed, or changed:

```bash
# snapshot before the run
bash ../scripts/mutation-guard.sh snapshot test-cases/<case>/workspace /tmp/snap.txt

# run the diagnostic ...

# verify after
bash ../scripts/mutation-guard.sh verify test-cases/<case>/workspace /tmp/snap.txt
```

The guard is strict — any filesystem residue (including `.enzyme/enzyme.log`
or anything written to `.margins/`) is a failure. The script exits non-zero and
prints a diff of the changed files.

Run the self-test to confirm the guard works in your environment:

```bash
bash ../scripts/mutation-guard.sh selftest
```

## Promotion gate (summary)

A skill revision is ready for promotion only when:

1. A **live rerun** of the real skill against the fixtures produces actual
   `enzyme scan` output — not artifact regeneration (artifact-only passes are
   tautological, as documented in the failure-mode report §3).
2. **Both control cases pass**: `append-only-dated-log` attracts ~zero findings;
   `fully-configured-vault` returns a no-action-needed verdict with a proof note.
3. The mutation guard returns CLEAN for every fixture run in read-only mode.

See the design brief `§Verification plan` for the full promotion checklist.

---

The core pass condition is stable across every case:

> Margins can start now, identifies the least surprising place for new meeting
> notes, and does not suggest changing existing files.
