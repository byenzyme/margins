# Workspace setup rollout review

This is the ecological pre-release review for the agent-driven Workspace setup
experience. It complements the deterministic setup E2E lanes; it does not turn
the rollout into a component checklist.

The harness requires Python 3.11 or newer. Its generated environment file is
for a POSIX-compatible shell. Use the executable script from the checkout; file
attachments may lose executable mode and should be invoked with `python3` when
validating a transferred copy.

## The one prompt

The agent under review receives only:

> Help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.

Do not add hints about capabilities, scan fields, credentials, command order,
consent, or expected settings. Discovering and applying the embedded guide is
part of the product being reviewed.

Use a realistic notes practice and a release-candidate binary. The default lane
is a genuinely fresh setup for that practice: prior folder-local, current
Workspace, and legacy path-addressed setup state must not influence what the
agent discovers. Keep the leading prompt fixed, record the exact model/provider,
and preserve the raw transcript.

## Capture without prescribing

Quit Margins and stop every other Margins process before this review. `prepare`
temporarily isolates three kinds of existing state in the private run directory:

- the practice's `.margins/` folder, if present; and
- the complete `$MARGINS_HOME/workspaces/` registry; and
- `$MARGINS_HOME/config.toml`, with only legacy `[vaults.*]` and
  `[workspaces.*]` entries whose note roots overlap this practice removed from
  the temporary active copy.

The whole registry is isolated—not only the expected Workspace—because unrelated
Workspace ids affect implicit naming, and a faulty rollout must not be able to
mutate an unbacked entry. Existing Workspaces are temporarily unavailable until
finalization. Moving the registry whole ensures old config, indexes, catalysts,
captures, receipts, and partial transactions cannot leak into the rollout. The
harness records which prior bindings overlapped the practice for review. Global
`[llm]`, `[defaults]`, and `[update]` settings and setup entries for other note
roots remain active, so the rollout retains realistic machine capabilities
without inheriting the practice's decisions. Unsupported or malformed global
config fails preparation before any state is moved. The harness does not touch
`.obsidian/` or machine-level catalyst credentials, models, or account
connections; those are installation capabilities rather than setup for this
folder.

Prepare a local evidence directory outside both the practice and
`MARGINS_HOME`, but on the same filesystem as both. The harness refuses a
cross-filesystem backup so every state transition can use an atomic rename:

```bash
HARNESS=/absolute/path/to/margins-checkout/scripts/workspace-setup-rollout-review.py
VAULT=/absolute/path/to/notes
RUN_DIR="/tmp/margins-workspace-setup-rollout-$(date +%Y%m%d-%H%M%S)"
"$HARNESS" prepare \
  --vault "$VAULT" \
  --margins-home "${MARGINS_HOME:-$HOME/.margins}" \
  --run-dir "$RUN_DIR"
```

The command writes `rollout-environment.sh`. Launch a fresh agent process from
the practice with that file sourced so it uses the same Margins home and cannot
inherit a `MARGINS_WORKSPACE` selector. Give it only the exact contents of
`user-prompt.txt`; the environment preparation is test-fixture isolation, not
extra prompt content.

After the agent finishes, export its complete transcript as text. Ask the
harness to identify the one Workspace the rollout actually created, then use
that explicit id to capture the product's redacted final status:

```bash
source "$RUN_DIR/rollout-environment.sh"
cd "$VAULT"
WORKSPACE_ID="$("$HARNESS" workspace-id \
  --run-dir "$RUN_DIR")"
margins --workspace "$WORKSPACE_ID" workspace status --json \
  > "$RUN_DIR/final-status.json"
"$HARNESS" finalize \
  --run-dir "$RUN_DIR" \
  --transcript /absolute/path/to/transcript.txt \
  --final-status "$RUN_DIR/final-status.json"
```

Never run observer-side `workspace status` without an explicit selector: its
normal CLI behavior may create an implicit Workspace, turning evidence capture
into part of the rollout. If `workspace-id` reports zero or multiple generated
Workspaces, preserve that diagnostic and run `finalize` without
`--final-status`; the generated directories and configs are still captured for
the independent reviewer.

`finalize` snapshots the generated Workspace state and complete configs under
the run directory, then restores the original Workspace directories,
folder-local `.margins/` state, and original root config byte-for-byte (including
a pre-existing symlink and its mode). Restoration is itself a hard gate. If the
run is interrupted after `prepare`, recover explicitly with:

```bash
"$HARNESS" restore --run-dir "$RUN_DIR"
```

`prepare` and `finalize` record raw Markdown hashes, Workspace-state hashes,
root-config entry metadata and hashes, generated config, and the transcript.
They do not copy note bodies, call a model, prescribe command transitions, or
infer consent. The hard gate fails if Markdown changed, recognizable credential
material appears in the transcript or observer-facing status/config views, or
pre-existing setup cannot be restored exactly. When a credential gate fires,
the captured view replaces matching values with labeled redactions and records
the original artifact's hash; it does not duplicate the leaked value into that
review view.

Keep this evidence and its backup local: filenames, configuration, indexes,
captures, and the raw conversation may still be private even when no note bodies
or credentials were deliberately copied into the report files. Delete the run
directory only after restoration is confirmed and review artifacts are no
longer needed.

## Open-ended independent judgment

Use an independent, trusted local reviewer with the unmodified
`judge-prompt.md`. The complete run directory contains backups of every
Workspace, generated indexes/captures, filenames, and configuration; do not
upload or share it wholesale. A remote reviewer should receive only the minimum
review artifacts needed—normally the redacted transcript, final status,
generated Workspace config views, hard-gate/restoration reports, and the
before/after hash reports—never `backup/` or `generated/`. The reviewer should
evaluate the rollout as one observational construction, beginning with whatever
it finds most consequential. It should not score whether every anticipated
component appeared, and it should not receive a maintained state-transition
brief.

The review is trying to discover whether a thoughtful user would experience the
setup as correct, minimal, safe, and intelligible. Useful findings include unknown
seam failures, unnecessary tool churn, weak causal stories, missed evidence,
consent drift, hidden fallback, leaked secrets, and a final answer that disagrees
with persisted state. These are examples for interpreting a completed rollout,
not instructions added to the agent's leading prompt.

The only automatic blockers are universal invariants:

- credential material appeared in the transcript or observer-facing artifacts;
- setup modified Markdown notes;
- an independent review finds an unconsented material setting change;
- recall was not usable even though the final answer claimed success; or
- the final account contradicts the resulting Workspace configuration.

Preserve the judge response verbatim beside the raw rollout. Because the whole
construction is allowed to vary, conclusions are observational unless a separate
controlled experiment establishes causality. Follow
`desktop/PROMPT_BEHAVIOR_EVALUATION.md` for any later ablation.
