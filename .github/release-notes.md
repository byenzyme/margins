Margins 0.4.20 is a small follow-up to 0.4.19. The release archives contain
`margins`, `margins-server`, and the bundled `enzyme` 0.12.2.

- Margins looks for older per-folder `.margins/` stores only up to your home
  folder, and never mistakes a Margins home (such as `~/.margins`) for one. It
  no longer tells you that earlier recordings in `~/.margins` stay readable.
- `margins init` lists a folder the engine skipped once, under **Skipped** with
  its reason, instead of also under **Reads**. The JSON output is unchanged.
- Suggested commands are plain `margins …` when the current folder already
  selects that Workspace, and add `--workspace <id>` only when it does not.

Upgrade the CLI and the bb plugin (with its `margins-server`) together. The
plugin installs `margins`, `margins-server`, and `enzyme` from the 0.4.20
archive as one runtime.

Known limits: Linux live transcription is not available.
