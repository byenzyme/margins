<!-- Draft lines for the next release (CLI-1, PR #33); fold into its notes. -->
- **`margins sync --json` without a catalyst generator now succeeds.** It
  reports `ok: true`, `recall.status: "index_only"`, and exits 0; 0.4.18
  reported `ok: false` and exited 1. Scripts that treated that failure as
  "catalysts are not set up" should read `recall.status` instead.
- **`margins recall` without catalysts returns direct matches** instead of
  failing: the JSON has `reason: "catalysts_not_set_up"` and
  `search_strategy: "direct"`, and readable output says catalysts are off and
  how to turn them on.
- The bundled engine is `enzyme` 0.12.2.
<!-- End of draft lines. -->

Margins 0.4.18 makes the CLI easier to start with on its own and fixes the bb
program editor. The release archives contain `margins`, `margins-server`, and
the bundled `enzyme` 0.12.1.

- **`margins recall` now prints readable text by default.** Scripts and
  agents that parse its output must pass `--json` to get the
  `margins.recall.v1` envelope.
- **`margins enzyme <args…>`** runs the bundled engine's read-only commands
  (`status`, `search`, `catalyze`, `scan`, `spec`, `model list`) against your
  Margins home and the selected Workspace. It never uses `~/.enzyme`, and it
  refuses every other command.
- `margins workspace plan` and `margins workspace apply` read in plain
  language without `--json`. A plan is saved under `~/.margins/plans`, and
  `plan` prints the exact `apply --plan` command to run. After `setup`,
  `init`, and `apply`, Margins says where your Workspace program lives and how
  to read and change it.
- `workspace plan` and `workspace edit` colour their diffs on a terminal
  (`--color auto|always|never`). Saved plans are pruned to the newest 40 from
  the last day. `margins enzyme` refuses `spec plan --prompts`.
- Only `margins init` creates a Workspace. `recall`, `workspace status`,
  `source list`, `sync`, integrations, and Granola import no longer create one
  for the current folder. Without a Workspace they fail with
  `workspace_required` (exit 1) and say how to pick or create one.
- In bb, **Workspace program** opens the program editor again. Switching
  meetings no longer depends on saving the memo first. A memo that cannot be
  saved stays in the tab with a Copy action, and it comes back when you reopen
  that meeting.
- Discarding a session also removes its live-transcript files.

Upgrade the CLI and the bb plugin (with its `margins-server`) together. The
plugin installs `margins`, `margins-server`, and `enzyme` from the 0.4.18
archive as one runtime.

Known limits: Linux live transcription is not available.
