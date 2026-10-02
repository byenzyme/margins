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

## Official hosted composition

This lane spends hosted model resources. Build the official CLI on the shared
lane, then provide an already brokered credential bundle by absolute path:

```bash
scripts/with-private-recall scripts/cargo-lane shared -- cargo build --bin margins-private \
  --no-default-features --features recall
MARGINS_E2E_BIN="$(scripts/cargo-lane target-dir)/debug/margins-private" \
MARGINS_E2E_HOSTED_BUNDLE_SOURCE=/absolute/path/to/llm-config-cache.json \
  scripts/e2e-official-hosted-workspace-setup.sh
```

The script never prints the bundle. It copies it to an isolated
`MARGINS_HOME` as mode `0600`, removes ambient provider credentials, and
deletes all temporary state on exit. It proves that official `workspace
propose --json` presents its human-readable understanding on stderr while its
exact stdout plan is directly accepted by revision-guarded `workspace apply`,
with the reveal grounded in the fixture's projects and Atlas Program terms.
It also proves that a stale apply fails and that private init/sync perform
hosted generation whose recall results retain native Markdown provenance and
inline catalyst receipts. The source bundle is hash-checked before and after
the run; the isolated destination may be refreshed by the broker contract.

`tests/test_setup_e2e_lanes.sh` is the fast, credential-free fake-shell
regression used by CI. It checks command ordering, isolation, plan pass-through,
bundle permissions, stale revision handling, and cleanup; it is not a
substitute for either real lane.

## Ecological pre-release rollout review

The deterministic lanes prove known contracts. Before a release, also run the
agent experience from the single ordinary user prompt and judge the complete
rollout without feeding the agent a component checklist or maintained transition
brief. The local capture and hard-gate procedure lives in
`docs/workspace-setup-rollout-review.md`.
