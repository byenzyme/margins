# margins-workflows

Portable Margins application workflows shared by CLI builds.

For setup, the crate owns Workspace and Source declarations, deterministic
plan/apply mutations, initialization and synchronization, and a fully open local
recall path. `local_recall` walks declared Markdown Sources read-only at
query time, honors Workspace exclusions, and returns typed evidence paths.

Setup starts from the practice the user wants Margins to remember, then declares
the folders that define its read/write boundary. A CLI build may add a read-only
`scan.v2` evidence surface. The setup skill consumes that complete evidence to
form a grounded interpretation and invite plain-language corrections. Any desired
settings are compiled into a plan; only the final reviewed plan is applied before
`init`, `sync`, and recall proof. This does not introduce a second config format.

Connected-note distillation is a separate agent workflow. It may consume this
crate's live local recall or another source-backed recall implementation through
the same `margins.recall.v1` contract.

The crate also owns session and artifact workflows, integration contracts,
imports, publishing, alignment, and transcript views.
