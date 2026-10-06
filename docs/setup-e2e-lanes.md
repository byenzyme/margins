# Setup E2E lanes

Margins has two intentionally separate setup end-to-end lanes.

## Public source

The public lane builds and tests the root workspace with recall off and no
enzyme-rust access. The credential-free setup harness remains available for
the public CLI binary built from this tree:

```bash
scripts/local-gate public
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public --no-default-features --locked
MARGINS_BIN="$(scripts/cargo-lane target-dir)/debug/margins-public" \
  scripts/e2e-fresh-workspace-setup.sh synthetic
```

The setup harness exercises `workspace new`, `init`, `sync`, Workspace/Source
reporting, and Markdown Source immutability in temporary HOME and MARGINS_HOME.
Recall lookup and generation are covered by the separate official lane.

## Preset setup through the real binaries

Setup is preset-only: `workspace new`, `workspace plan --preset
margins-meetings` (the engine fills the managed preset; readings for folders the
notes do not have are dropped), the reviewed `workspace apply`, `init`, and an
exact-phrase recall. The official lane runs that flow through the real `margins`
and `enzyme` binaries in temporary homes with a local fixture generator; it spends
no hosted model resources:

```bash
MARGINS_ENZYME_BIN="$(scripts/enzyme-bin)" scripts/with-private-recall \
  scripts/cargo-lane disposable -- cargo test --no-default-features --features recall \
  --test workspace_language_e2e preset_setup
```

It proves that a notes folder with `Meetings` and `People` but no `Projects`
gets only the existing readings, that the planted phrase is recalled, that new
notes go to `Meetings`, and that running setup again plans no change and writes no
second program. `scripts/local-gate linux` runs it with the rest of the suite.

### Retired: official hosted grounded-review lane

`scripts/e2e-official-hosted-workspace-setup.sh` proved the scan-grounded
`workspace propose` review with a brokered hosted credential. Setup no longer
scans or reviews, so the script was removed with `margins scan` and `workspace
compile` (2026-10-06). Its earlier evidence stays valid for the builds it ran on.

`tests/test_setup_e2e_lanes.sh` is the fast, credential-free fake-shell
regression used by CI. It checks the public lane's command ordering, isolation,
and cleanup; it is not a substitute for either real lane.

## Ecological pre-release rollout review

The deterministic lanes prove known contracts. Before a release, also run the
agent experience from the single ordinary user prompt and judge the complete
rollout without feeding the agent a component checklist or maintained transition
brief. The local capture and hard-gate procedure lives in
`docs/workspace-setup-rollout-review.md`.
