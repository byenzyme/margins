# margins-workflows

Portable Margins application workflows shared by CLI builds.

For setup, the crate owns Workspace and Source declarations, deterministic
plan/apply mutations, initialization and synchronization, and a fully open local
recall path. `local_recall` walks declared Markdown Sources read-only at
query time, honors Workspace exclusions, and returns typed evidence paths.

Setup declares the folders that define the practice's read/write boundary, then
starts the Workspace program from the Margins meetings preset
(`workspace_preset`): the engine fills it, folder readings for folders the notes
do not have are dropped, and the result is compiled into a plan; only the final
reviewed plan is applied before `init`, `sync`, and recall proof. This does not
introduce a second config format.

Connected-note distillation is a separate agent workflow. It may consume this
crate's live local recall or another source-backed recall implementation through
the same `margins.recall.v1` contract.

The crate also owns session and artifact workflows, integration contracts,
imports, publishing, alignment, and transcript views.
