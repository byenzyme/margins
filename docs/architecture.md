# Architecture

The workspace contains two separate workflows:

```text
SETUP
Workspace + Source declarations
  -> init + sync
  -> live read-only lexical recall proof

DISTILLATION
latest or selected Margins session / supplied evidence
  -> source-backed recall through margins.recall.v1
  -> connected-note skill
  -> reviewed note in the Workspace home
```

`workspace plan` and `workspace apply` provide optional deterministic automation
over the same declarations. A CLI build may report `recall.scan: true`; the setup
skill then consumes the complete read-only `scan.v2` evidence after declaration.
The skill—not a second CLI renderer—forms the grounded interpretation, leads with
that understanding, and invites plain-language corrections. Any desired settings
are compiled with `workspace plan --desired`; a correction is recompiled into a
fresh plan, and only the final reviewed plan is applied unchanged before `init` and
`sync`. Declared Sources still define the full recall boundary.

Distillation starts only after setup is ready. It resolves `transcript latest`
by default, while a session id or supplied transcript, memo, text, or supported
audio can select other evidence. The skill consumes the same `margins.recall.v1`
result contract whether recall reads local Markdown at query time or a CLI build
provides another source-backed retrieval mode.

These extension points reuse the existing Workspace, plan, recall, and note
contracts rather than introducing a second setup protocol. The export boundary
remains literal and fail-closed.
