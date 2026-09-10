# Official CLI release pipeline

The official `margins` executable is built only from the private
`useenzyme/margins-desktop` source-of-truth. The public `useenzyme/margins`
repository is an audited open-core export and validates its source graph, but
has no release-writing workflow. Never add a tag-triggered binary publisher or
a cross-repository write token to the public export.

## Release topology

`.github/workflows/cli-release.yml` checks out an existing private tag and
builds the root `margins` package's internal `margins-private` target with an
explicit, fail-closed private composition, then stages it in each archive as
the user-facing `margins` executable, for:

- `aarch64-apple-darwin` on the `macos-15` Apple Silicon runner with
  `audio-capture,coreml-asr`;
- `x86_64-apple-darwin` on the `macos-15-intel` runner with
  `audio-capture,coreml-asr`;
- `x86_64-unknown-linux-gnu` on Ubuntu with
  `audio-capture,parakeet-asr`.

Each native runner creates its archive, extracts it, and executes that exact
packaged binary's `__release-smoke` contract. A separate publish job verifies
that all three archives and checksums are present, then creates the release in
`useenzyme/margins` and updates `useenzyme/homebrew-margins`. Build jobs receive
no publishing credential. The publish job is protected by the
`official-cli-release` GitHub Environment so reviewers can inspect all artifacts
before secrets become available.

The public repository must already contain the matching tag. `--verify-tag`
prevents the private workflow from silently inventing a public tag at an
unreviewed commit. Tags and releases are intentionally not created by setup or
validation work.

## Required secrets and permissions

Configure these as secrets on the protected `official-cli-release` Environment
in `useenzyme/margins-desktop`:

- `MARGINS_RELEASE_TOKEN`: a fine-grained PAT (or equivalent installation
  token) limited to `useenzyme/margins`, with repository **Contents: read and
  write**. It needs no access to the private source repository or Homebrew tap.
- `HOMEBREW_TAP_TOKEN`: a separate fine-grained PAT limited to
  `useenzyme/homebrew-margins`, with repository **Contents: read and write**. It
  needs no access to releases or private source.

Protect the Environment with required reviewers and restrict it to `v*` tags.
Protect matching tags in both source and public repositories. If organization
policy permits GitHub Apps, two single-repository installations with short-lived
tokens are preferable to user PATs; keep the same split authority.

The workflow's built-in `GITHUB_TOKEN` has only `contents: read`. Checkout does
not persist credentials. No token is available until the publish job, and each
cross-repository operation receives only its own token.

## Packaged-binary smoke contract

The private root composition must provide:

```text
margins __release-smoke
```

It must not enumerate or open audio devices, request TCC permissions, create a
session, write user data, or start recording. On success it exits zero and emits
one JSON object:

```json
{
  "schema": 1,
  "capture_available": true,
  "capture_provider": "<private provider identity>",
  "tui_available": true
}
```

`scripts/smoke-official-cli.sh` rejects non-JSON output, a missing provider,
false capability flags, `UnavailableCaptureProvider`, or
`capture_unavailable`. The contract tests composition and linkage, not hardware
behavior. It is a required integration point from sibling thread
`thr_dny6tqhx9m`; the release workflow must not be enabled until that command
and its unit/integration test land.

## Core product smoke runbook

Before approving a macOS CLI release candidate, run the automated product smoke
from a real terminal app with Microphone permission:

```bash
MARGINS_FLUID_COREML_MODEL_DIR="$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2" \
  scripts/core-product-smoke.sh
```

The full gate compiles the focused tests once in one disposable Cargo lane,
builds the canonical release binary once, checks its composition, exercises a
hermetic workspace and live capture, and inspects WAV/SQLite durability and
shutdown health. Logs are preserved under `/tmp/margins-core-product-smoke.*`.

For repeated audio iteration after building the release binary from the same
checkout, avoid recompilation with:

```bash
scripts/core-product-smoke.sh --skip-build --skip-tests --skip-workspace
```

Use `--skip-live` from bb or another process without terminal TCC permission.
`--workspace-recall` adds generator-backed recall coverage and may consume model
or API resources.

## Real-Mac release candidate verification

CI deliberately does not perform destructive or privacy-sensitive live
recording. Before approving the protected publish job, run the ecological
Workspace setup review in `docs/workspace-setup-rollout-review.md`, then install
the exact Apple-Silicon archive on a real Mac and verify:

1. Gatekeeper/signing/notarization expectations for the CLI distribution.
2. First-run Screen & System Audio Recording and microphone permission prompts,
   including denial and later recovery in System Settings.
3. `margins new` opens the real TUI, captures mic and system lanes, permits the
   intended device switch, and exits cleanly without losing the memo.
4. The resulting WAV/session metadata have non-empty expected lanes and correct
   duration/timeline behavior; playback confirms actual audio rather than
   silence.
5. A second attach/record segment and a normal processing command work from the
   installed archive.
6. `brew install`/upgrade from a staged formula selects the correct architecture
   and `brew test margins` passes.

Repeat the basic installed-binary/capture check on an Intel Mac when Intel is a
supported release tier. Linux CI can validate the linked provider and TUI
composition, but a release that claims Linux live capture should additionally
be exercised on a representative PipeWire/PulseAudio desktop with real devices.
