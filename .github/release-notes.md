Margins 0.4.17 makes each Workspace one editable program and ships its own
recall engine. The release archives contain `margins`, `margins-server`, and
`enzyme`.

- Each Workspace is now one `.enzyme` program that says which notes Margins
  reads, where it writes, and what it pays attention to. `margins workspace
  show` prints it and `margins workspace edit` opens it in your editor and
  reviews the change before applying it. In bb, the Meetings panel has a
  highlighted editor for the same program.
- Setup starts from the `margins-meetings` preset. It fills in readings for
  your notes folders and turns on automatic selection, so a new Workspace can
  recall from its notes without a scan or a review step.
- Margins runs the bundled `enzyme` 0.12.1 that ships beside it. It keeps its
  index, models, and settings in the Margins home and never reads or changes
  `~/.enzyme` or an `enzyme` you installed yourself.
- Existing setups migrate on first run. Each Workspace's `config.toml` becomes
  its program, and machine settings move from `~/.margins/config.toml` to
  `~/.margins/margins.toml`; each original is kept as `config.toml.migrated`.
  Each Workspace's `index.db` is renamed to `enzyme.db` without reindexing.
- Margins now builds entirely from public source.
- The `margins scan` and `margins workspace compile` commands are removed. Use
  `margins workspace edit`, or `margins workspace plan --preset` followed by
  `margins workspace apply`.

Upgrade the CLI and the bb plugin (with its `margins-server`) together. The
plugin installs `margins`, `margins-server`, and `enzyme` from the 0.4.17
archive as one runtime and refuses an archive without `enzyme`. Stop any older
`margins` or `margins-server` before the first run: one that still has the
index open keeps writing to it after the rename.

Known limits: Linux live transcription is not available.
