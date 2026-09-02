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
over the same declarations. A CLI build may add `workspace.propose`; when its
capability report includes `workspace.propose: true`, the setup skill invokes
it after declaration. The command grounds an initial reading in the Workspace
home, writes an explanation of what Margins understands the practice to be to
stderr, and emits a draft plan to stdout; the declared Sources still define the
full recall boundary. The agent leads with that understanding and invites
plain-language corrections; a correction is recompiled into a fresh plan with
`workspace plan --desired`, and only the final reviewed plan is applied unchanged
before `init` and `sync`. The static skill does not encode or reproduce the
recommendation logic.

Distillation starts only after setup is ready. It resolves `transcript latest`
by default, while a session id or supplied transcript, memo, text, or supported
audio can select other evidence. The skill consumes the same `margins.recall.v1`
result contract whether recall reads local Markdown at query time or a CLI build
provides another source-backed retrieval mode.

These extension points reuse the existing Workspace, plan, recall, and note
contracts rather than introducing a second setup protocol. The export boundary
remains literal and fail-closed.
