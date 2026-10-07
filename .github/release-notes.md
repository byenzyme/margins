Margins 0.4.19 reshapes the CLI around getting started, and recordings now go
to a Workspace. The release archives contain `margins`, `margins-server`, and
the bundled `enzyme` 0.12.2.

- **`margins init` sets up a Workspace in one step.** Run it in your notes
  folder, or use `margins init <folder>`. It applies the `margins-meetings`
  preset, creates the Workspace, makes it your default if you have none, and
  indexes your notes. Search works right away. At a terminal, it then offers
  catalysts once: hosted (no download, no sign-in; excerpts go to Margins'
  hosted service), local (about 88 MB), or not now. Running `init` again in a
  covered folder refreshes that Workspace and leaves its program alone.
- **`margins status`** shows the Workspace, its index and catalysts, Sources,
  connections, and recent captures. `margins status --explain` lists what the
  program reads, what the engine picked or skipped, and why.
- **`margins edit`** previews what a change to the program does before you
  confirm it. `margins sync` and `init` report what Margins pays attention to
  that changed since the last refresh.
- **`margins --help` shows 16 commands, grouped by task:** `init`, `status`,
  `edit`, `sync`, `recall`, `connect`, `disconnect`, `setup`, `guide`, `new`,
  `attach`, `current`, `ls`, `transcript`, `transcribe`, and `note`.
- **Recordings go to the Workspace.** `new`, `attach`, `transcribe`, and bare
  `margins` use `--workspace`, then the Workspace covering the current folder,
  then your default. They no longer create a `.margins/` folder where you run
  them.
- Older per-folder `.margins/` stores stay where they are and stay readable:
  `ls`, `transcript`, and the other reading commands still find a store at or
  above the current folder, and `margins --project <id|path> ls` opens one
  directly. Margins tells you when a Workspace now hides one.
- The first `new` or `transcribe` offers to download the speech model (about
  464 MB). `init` never downloads it.
- At a terminal, errors print as plain text instead of JSON.
- The bundled engine is `enzyme` 0.12.2.

## Changed behaviour

- **Removed:** `margins enzyme` (use `margins status --explain`) and `margins
  workspace migrate` (Margins migrates on any command that writes).
- **Hidden:** `workspace …`, `source`, `integrations`, `retention`,
  `capabilities`, `service`, `transfers`, `agents`, `recent`, `artifacts`,
  `artifacts-prune`, `audio-export`, `memo`, `note-association`,
  `processing-status`, `process`, `rename`, `import`, `archive`, and
  `--local` no longer appear in help. They take the same arguments and flags
  and keep the same exit codes and JSON.
- **Recording needs a Workspace.** Without one, recording fails with
  `workspace_required` and tells you to run `margins init` in your notes
  folder. `--project` stores are read-only, and recording into one fails with
  `legacy_store_read_only`. `margins-server` no longer creates a Workspace
  either. A bb project whose Workspace is missing shows a
  `workspace_not_found` message instead.
- **`margins recall` without catalysts returns direct matches** instead of
  failing. Its JSON has `reason: "catalysts_not_set_up"` and
  `search_strategy: "direct"`. Readable output says catalysts are off and how
  to turn them on.
- **`margins sync --json` without catalysts now succeeds.** It reports
  `ok: true` and `recall.status: "index_only"`, and exits 0. 0.4.18 reported
  `ok: false` and exited 1. Scripts that read that failure as "catalysts are
  not set up" should read `recall.status` instead. `margins.sync.v1` also gains
  an `attention` object.
- `margins init` prints readable output. Scripts should pass `--json` to get
  a `margins.init.v1` object instead of the `<margins_init/>` receipt.

Upgrade the CLI and the bb plugin (with its `margins-server`) together. The
plugin installs `margins`, `margins-server`, and `enzyme` from the 0.4.19
archive as one runtime.

Known limits: Linux live transcription is not available.
