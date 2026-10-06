# margins-cli

The standalone Margins command parser and dispatcher.

The source-checkout binary is named `margins-public`; released product builds
may install it as `margins`. Substitute `margins-public` in the examples below
when running directly from this workspace.

The public binary reports and runs useful local workflows:

```bash
cd /absolute/path/to/notes
margins init
margins sync --json
margins recall "a phrase already in these notes"
```

`init` resolves or creates a Workspace. `sync` confirms declared Sources are
ready. `recall` reads declared Markdown at query time and returns source-backed
`margins.recall.v1` results, with no separate indexing command or wait.

For multiple folders:

```bash
margins workspace new practice --home /absolute/path/to/notes
margins --workspace practice source add notes \
  --name research --role reference --path /absolute/path/to/research
```

Choosing folders directly is the standard setup path. `workspace plan` and
`workspace apply` provide optional deterministic automation. Official builds
report `workspace.preset: true`; setup then starts the program from the meetings
preset with `workspace plan --preset margins-meetings`. Any other desired
settings are compiled with `workspace plan --desired`; only the final plan you
reviewed is applied, unchanged, before `init`/`sync`.

Connected-note distillation is a separate skill workflow. It starts with the
latest Margins session unless the user selects another session or supplies a
transcript, memo, or text. Builds with a speech adapter list `audio` in
`distillation.inputs`. The skill uses the `margins.recall.v1` results returned by
the CLI.
