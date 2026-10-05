# Official CLI release pipeline

The official `margins` executable is built from a tag in the public
`byenzyme/margins` source tree. The private `byenzyme/margins-desktop` repository
remains an archive after cutover. Only the Enzyme recall engine is fetched from
a private repository, at the exact revision in
`scripts/private_recall_dependency.toml` and `Cargo.private-recall.lock`.

## Release topology

`.github/workflows/cli-release.yml` checks out an existing public source tag
and builds the root `margins` package's internal `margins-private` target with
`scripts/with-private-recall`, then stages it in each archive as
the user-facing `margins` executable alongside `margins-server`, for:

- `aarch64-apple-darwin` on the `macos-15` Apple Silicon runner with
  `audio-capture,coreml-asr,polyvoice-coreml,recall,recall-local-model`;
- `x86_64-unknown-linux-gnu` on Ubuntu with
  `audio-capture,parakeet-asr,polyvoice-diarization,recall`.

The public manifest defaults to portable capture only, so a source build never
needs private repository access or a platform ASR/diarization backend. Official
builds fail closed by explicitly enabling the target's full media and recall
feature set through the private composition and asserting its capabilities in
the packaged-binary smoke test.

The Apple Silicon archive no longer includes `margins-live`; the desktop app
and its live runtime were retired on 2026-10-02 (PR #100).

Each native runner creates its archive, extracts it, and executes that exact
packaged binary's `__release-smoke` contract. A separate publish job verifies
that both archives and checksums are present, then creates the release in
`byenzyme/margins` and updates `byenzyme/homebrew-margins`. Build jobs receive
no publishing credential. The build and publish jobs run in the
`official-cli-release` GitHub Environment, which holds every release secret
(see below). The build steps live in the composite action
`.github/actions/build-official-cli`, shared with the no-publish dry run.

The Rust cache in that action contains private `enzyme-rust` source and build
artifacts. Only a tag-push release run saves it, because tag-scoped caches are
unreadable from other refs, and it uses a distinct `v0-official-private` key
prefix. A cache saved on `main` would be restorable by any run, including fork
pull requests in `public-ci.yml`. So a `workflow_dispatch` of `cli-release.yml`
and the dry run never restore or save a cache.

Re-dispatching `cli-release.yml` for a tag created before the
environment-secrets change (v0.4.15 and earlier) does not work. The workflow
checks out the tag, and those tags do not contain
`.github/actions/build-official-cli`. Cut a new patch tag instead.

The publish job uploads archives to the release for that same public source
tag. It does not transform, rsync, commit, or tag a second source tree. Tags
and releases are not created by setup or validation work.

## Release order and BB plugin runtime pairing

The BB plugin pins its runtime exactly (`RUNTIME_RELEASE_VERSION` in
`integrations/bb-plugin-margins/src/runtime-manager.ts`) and installs from the
`dist/` committed on `main`. Plugin and server upgrade as a pair; there is no
back-compat layer. A remote or overridden server with a mismatched protocol
fails with an explicit "upgrade both" error.

Release in this order:

1. The full local Linux gate passes on `main` (`scripts/with-private-recall scripts/local-gate linux`).
2. The macOS gate and the Mac smoke checklist pass on the attached Mac bb host.
3. Merge the PR that bumps `RUNTIME_RELEASE_VERSION` and its rebuilt `dist/`
   **immediately** before tagging. Between that merge and the published release,
   fresh plugin installs point at a runtime that does not exist yet.
4. Bump and tag:
   1. Run the **Version Bump** workflow (`version-bump.yml`) from `main` with
      `X.Y.Z`. It runs in the `official-cli-release` environment, bumps the
      package versions and both lockfiles, commits `Release vX.Y.Z` and pushes
      it to `main`. It does **not** create a tag or dispatch `cli-release.yml`;
      the tag ruleset forbids it.
   2. The workflow's notice and job summary print the release commit SHA and
      the exact tag commands. A repository admin (Joshua) runs them locally:

      ```bash
      git fetch origin main
      git tag -a vX.Y.Z <sha> -m "Margins X.Y.Z" && git push origin vX.Y.Z
      ```

   3. That tag push triggers `cli-release.yml`, which builds, signs, notarizes
      and publishes on GitHub. This is the only validation-adjacent work that
      runs on GitHub runners besides the optional dry run below.
5. Verify the published archives and a fresh BB plugin install against the new
   release.

## Required secrets and permissions

All release secrets are **environment secrets on the `official-cli-release`
GitHub Environment** of `byenzyme/margins`, not repository secrets. Only jobs
that declare `environment: official-cli-release` can read them: `build` and
`publish` in `cli-release.yml` (`validate` reads no secrets and stays outside
the environment), `bump` in `version-bump.yml`, and
`build` in `cli-release-validation.yml`.

| Secret | Used by | Purpose |
| --- | --- | --- |
| `ENZYME_RUST_DEPLOY_KEY` | build, bump, dry run | Private key of a **read-only deploy key** on private `byenzyme/enzyme-rust` (org deploy keys are enabled). `.github/actions/private-recall-ssh` loads it into an `ssh-agent`, pins GitHub's published host keys, and rewrites `https://github.com/byenzyme/enzyme-rust` to `ssh://git@github.com/byenzyme/enzyme-rust` with `url.insteadOf`. Manifests and lockfiles keep the https URL. With `CARGO_NET_GIT_FETCH_WITH_CLI=true`, Cargo fetches through that SSH path. The agent, key file and rewrite are removed right after the private fetch, even on failure. |
| `MARGINS_GOOGLE_OAUTH_CLIENT_JSON` | build, dry run | The downloaded Desktop OAuth client JSON. The CLI build embeds it through `option_env!`; source and lockfiles contain no client credential. A public source build may instead set a runtime file or JSON environment variable. |
| `APPLE_CERTIFICATE_BASE64` | macOS build, dry run | Base64 Developer ID Application `.p12`. |
| `APPLE_CERTIFICATE_PASSWORD` | macOS build, dry run | Password for that `.p12`. |
| `APPLE_TEAM_ID` | macOS build, dry run | Team for `notarytool`. |
| `APPLE_ID` | macOS build, dry run | Apple ID for `notarytool`. |
| `APPLE_PASSWORD` | macOS build, dry run | App-specific password for `notarytool`. |
| `HOMEBREW_TAP_TOKEN` | publish | Separate fine-grained PAT limited to `byenzyme/homebrew-margins`, with **Contents: read and write**. It needs no access to releases or private source. |

`ENZYME_RUST_READ_TOKEN`, `MARGINS_RELEASE_TOKEN` and the `TAURI_*` secrets are
no longer used and can be deleted.

To rotate `ENZYME_RUST_DEPLOY_KEY`:

1. Generate a new key pair, for example
   `ssh-keygen -t ed25519 -N '' -C margins-release -f enzyme-rust-deploy`.
2. Add `enzyme-rust-deploy.pub` as a deploy key on `byenzyme/enzyme-rust` with
   **Allow write access** left unchecked.
3. Replace the `ENZYME_RUST_DEPLOY_KEY` environment secret on
   `official-cli-release` with the private key file's contents, then delete the
   old deploy key and the local key files.

If GitHub rotates its SSH host keys, update the pinned `known_hosts` entries
and fingerprints in `.github/actions/private-recall-ssh/action.yml` from
<https://api.github.com/meta> and GitHub's published fingerprints.

Local developers do not use the deploy key. `scripts/with-private-recall` keeps
using their own git credentials (SSH key, credential helper or token) with
read access to `byenzyme/enzyme-rust`. A missing secret fails its step with an `::error::` naming it.

Environment and tag policy:

- The `official-cli-release` deployment policy allows only `v*` tags and the
  `main` branch, so a workflow dispatched from any other branch cannot read the
  secrets.
- The active tag ruleset "release tags v*" lets only repository admins create,
  update or delete `v*` tags. GitHub Actions cannot be exempted, which is why
  the Version Bump workflow prints tag commands instead of tagging.
- If required reviewers are configured on the environment, every job that
  declares it (including `build`) waits for approval; approve
  `publish` only after the real-Mac verification below.
- Prefer short-lived GitHub App tokens where available.

The workflow's built-in `GITHUB_TOKEN` has `contents: read` except for the
publish job, where it receives `contents: write` to publish on the current
repository, and the bump job, where it pushes the release commit to `main`.
Checkout does not persist credentials in release builds. The Homebrew token is
scoped to its publish step.

## No-publish dry run

`.github/workflows/cli-release-validation.yml` (**Validate official CLI
packages**) is a manual rehearsal of the release build. It uses the same
`.github/actions/build-official-cli` composite action as `cli-release.yml`: it
fetches the private engine over SSH with `ENZYME_RUST_DEPLOY_KEY` and the official
feature composition, embeds `MARGINS_GOOGLE_OAUTH_CLIENT_JSON`, and on macOS
imports the Apple certificate, codesigns and submits to `notarytool --wait`.
It then smokes the packaged archive (`oauth_client` valid, recall present) and
uploads the archives as short-lived workflow artifacts. It never creates a
release or tag and never touches the Homebrew tap.

Run it from `main` (the environment rejects other branches), only when a
rehearsal is worth a real notarization submission:

```bash
gh workflow run cli-release-validation.yml --repo byenzyme/margins --ref main
```

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
  "oauth_client": "valid",
  "capture_available": true,
  "capture_provider": "<private provider identity>",
  "tui_available": true
}
```

`scripts/smoke-official-cli.sh` rejects non-JSON output, an invalid embedded
OAuth client, a missing provider, false capability flags, `UnavailableCaptureProvider`, or
`capture_unavailable`. The contract tests composition and linkage, not hardware
behavior. It is a required integration point from sibling thread
`thr_dny6tqhx9m`; the release workflow must not be enabled until that command
and its unit/integration test land.

## Core product smoke runbook

Before approving a macOS CLI release candidate, run the automated product smoke
from a real terminal app with Microphone permission:

```bash
MARGINS_FLUID_COREML_MODEL_DIR="$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2" \
  scripts/with-private-recall scripts/core-product-smoke.sh
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
