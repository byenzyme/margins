# Official CLI release pipeline

The official `margins` executable is built from a tag in the public
`byenzyme/margins` source tree. The private `byenzyme/margins-desktop` repository
remains an archive after cutover. The build needs no private source: Margins
runs the `enzyme` CLI, and each archive ships the official enzyme release asset
pinned in `scripts/enzyme-cli.pin`.

## Release topology

`.github/workflows/cli-release.yml` checks out an existing public source tag
and builds the root `margins` package's internal `margins-private` target,
then stages it in each archive as
the user-facing `margins` executable alongside `margins-server` and the pinned
`enzyme` engine (see [The bundled enzyme engine](#the-bundled-enzyme-engine)),
for:

- `aarch64-apple-darwin` on the `macos-15` Apple Silicon runner with
  `audio-capture,coreml-asr,polyvoice-coreml,recall,recall-local-model`;
- `x86_64-unknown-linux-gnu` on Ubuntu with
  `audio-capture,parakeet-asr,polyvoice-diarization,recall`.

The manifest defaults to portable capture only, so a source build never needs
a platform ASR/diarization backend. Official builds fail closed by explicitly
enabling the target's full media and recall feature set and asserting its
capabilities in the packaged-binary smoke test.

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

The Rust cache in that action contains build artifacts that embed the Google
OAuth client. Only a tag-push release run saves it, because tag-scoped caches
are unreadable from other refs, and it uses a distinct `v1-official` key prefix
(`v0-official-private` caches also held private engine source). A cache saved
on `main` would be restorable by any run, including fork pull requests in
`public-ci.yml`. So a `workflow_dispatch` of `cli-release.yml` and the dry run
never restore or save a cache.

Re-dispatching `cli-release.yml` for a tag created before the
environment-secrets change (v0.4.15 and earlier) does not work. The workflow
checks out the tag, and those tags do not contain
`.github/actions/build-official-cli`. Cut a new patch tag instead.

The publish job uploads archives to the release for that same public source
tag. It does not transform, rsync, commit, or tag a second source tree. Tags
and releases are not created by setup or validation work.

## The bundled enzyme engine

Margins runs the `enzyme` CLI for indexing, recall, status and models, so every
archive ships one next to `margins`:

```text
margins-X.Y.Z-<target>.tar.gz
  margins
  margins-server
  enzyme
```

`margins-server` never runs `enzyme`; only `margins` does.

**Source.** The archive's `enzyme` is the official `byenzyme/enzyme` release
asset `enzyme-<platform>.tar.gz`, which enzyme-rust's release workflow builds
with `--features local-llm` for each target. Margins does not build enzyme
itself, because:

- it is the binary Enzyme users get (no Margins-only build);
- it is a public download, so the Margins release needs no enzyme-rust
  credential for it, which a fully public Margins build requires;
- a sha256 pins it exactly, and no Margins release rebuilds llama.cpp a second
  time.

**Pin.** `scripts/enzyme-cli.pin` is the one place that names the engine:
`rev` (the enzyme-rust commit that tests and gates build with
`scripts/enzyme-bin`), `version` (what `enzyme --version` prints for it; a new
enzyme release tagged `v<version>` at `rev`, never an already-published
version), and `sha256.<target>` (that release's asset for each Margins release
target). Until those sha256 values exist, `scripts/enzyme-bin` and the gates
build `rev`, so they never test an older release.
`scripts/enzyme-pin fetch <target> <dir>` first runs `verify-release`: the
pinned digest must be the one GitHub recorded for that asset of release
`v<version>` and must not be any other published release's asset, and, where
enzyme-rust is readable (a developer's credentials, not CI), its tag
`v<version>` must be `rev`. The public release tags point at plugin-sync
commits, not enzyme-rust revs, so CI cannot compare the tag itself. It then
downloads
`https://github.com/byenzyme/enzyme/releases/download/v<version>/<asset>`,
checks its sha256 against the pin, requires it to contain exactly one regular
file named `enzyme`, and on a native runner checks `enzyme --version`. A target
with no sha256 fails the build: Margins cannot release before the enzyme
release it pins exists. The packaged-archive smoke step runs
`scripts/enzyme-pin check` on the extracted `enzyme` again. On macOS the
release re-signs `enzyme` with the Margins Developer ID and hardened runtime
and notarizes it with the other two binaries, so the shipped file's bytes
differ from the asset; the asset's sha256 is checked before signing. Signing
passes `--preserve-metadata=entitlements` for `enzyme`; the v0.11.1 asset is
linker-signed ad hoc and carries no entitlements.

**Install layout.** In the archive and in the BB plugin's runtime directory,
`enzyme` sits next to `margins`. Installers that put `margins` in a shared
`bin` directory put the engine at `<prefix>/libexec/margins/enzyme`, off
`PATH`, so it never replaces or shadows an `enzyme` the user installed:

| Installer | `margins` | engine |
| --- | --- | --- |
| Homebrew formula | `$(brew --prefix)/bin/margins` | `<keg>/libexec/margins/enzyme` |
| `install.sh` | `~/.local/bin/margins` | `~/.local/libexec/margins/enzyme` |
| BB plugin | `~/.local/bin/margins` (when the plugin manages it) | `~/.local/libexec/margins/enzyme` |

**Run-time check.** `margins` considers `<exe dir>/../libexec/margins/enzyme`
when `<exe dir>/../libexec/margins` exists (an install), otherwise
`<exe dir>/enzyme` (the archive), then `$MARGINS_HOME/bin/enzyme`, and uses the
first whose `enzyme --version` equals the pin. An installed `margins` never
considers a user's own `enzyme` beside it in `~/.local/bin`. With none, it
fails listing each candidate and why it was rejected. An explicit
`MARGINS_ENZYME_BIN` must match the pin or the call fails.

**No self-update.** Margins runs `enzyme` with `ENZYME_HOME=$MARGINS_HOME`,
whose `configs/settings.enzyme` has `settings { updates disabled }`. Enzyme
updates its binary in only two ways: a background worker that `refresh`
spawns, and the explicit `enzyme update` command, which Margins never runs.
That setting turns the worker off (and the worker rechecks it before
downloading); independently, `refresh` never spawns it under an explicit
`--llm env|local|none`, which Margins always passes. Enzyme only ever swaps a binary installed in `~/.local/bin` by its own
installer; neither of the engine locations above is one.

**Bumping the engine.** Publish the enzyme release first: a new version, bumped
in enzyme-rust and tagged `vX.Y.Z` at the commit Margins needs. Then, in one
Margins PR, set `rev`
to that tag's commit, `version` to `X.Y.Z`, and each `sha256.<target>` to the
value in the release's `enzyme-<platform>.tar.gz.sha256`; run the gates
against it.

## Release order and BB plugin runtime pairing

The BB plugin pins its runtime exactly (`RUNTIME_RELEASE_VERSION` in
`integrations/bb-plugin-margins/src/runtime-manager.ts`) and installs from the
`dist/` committed on `main`. Plugin and server upgrade as a pair; there is no
back-compat layer. A remote or overridden server with a mismatched protocol
fails with an explicit "upgrade both" error.

Release in this order:

1. The enzyme release named in `scripts/enzyme-cli.pin` is published on
   `byenzyme/enzyme`, and the pin on `main` carries its `version` and a
   `sha256` for every release target (see
   [Bumping the engine](#the-bundled-enzyme-engine)). Without them the build
   job fails at "Fetch the pinned enzyme engine".
2. The full local Linux gate passes on `main` (`scripts/local-gate linux`).
3. The macOS gate and the Mac smoke checklist pass on the attached Mac bb host.
4. Merge the PR that bumps `RUNTIME_RELEASE_VERSION` and its rebuilt `dist/`
   **immediately** before tagging. Between that merge and the published release,
   fresh plugin installs point at a runtime that does not exist yet.
5. Bump and tag:
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
6. Verify the published archives and a fresh BB plugin install against the new
   release, including that the plugin placed `enzyme` beside the runtime and
   at `~/.local/libexec/margins/enzyme`.

## Required secrets and permissions

All release secrets are **environment secrets on the `official-cli-release`
GitHub Environment** of `byenzyme/margins`, not repository secrets. Only jobs
that declare `environment: official-cli-release` can read them: `build` and
`publish` in `cli-release.yml` (`validate` reads no secrets and stays outside
the environment), `bump` in `version-bump.yml`, and
`build` in `cli-release-validation.yml`.

| Secret | Used by | Purpose |
| --- | --- | --- |
| `MARGINS_GOOGLE_OAUTH_CLIENT_JSON` | build, dry run | The downloaded Desktop OAuth client JSON. The CLI build embeds it through `option_env!`; source and lockfiles contain no client credential. A public source build may instead set a runtime file or JSON environment variable. |
| `APPLE_CERTIFICATE_BASE64` | macOS build, dry run | Base64 Developer ID Application `.p12`. |
| `APPLE_CERTIFICATE_PASSWORD` | macOS build, dry run | Password for that `.p12`. |
| `APPLE_TEAM_ID` | macOS build, dry run | Team for `notarytool`. |
| `APPLE_ID` | macOS build, dry run | Apple ID for `notarytool`. |
| `APPLE_PASSWORD` | macOS build, dry run | App-specific password for `notarytool`. |
| `HOMEBREW_TAP_TOKEN` | publish | Separate fine-grained PAT limited to `byenzyme/homebrew-margins`, with **Contents: read and write**. It needs no access to releases or private source. |

`ENZYME_RUST_DEPLOY_KEY`, `ENZYME_RUST_READ_TOKEN`, `MARGINS_RELEASE_TOKEN` and
the `TAURI_*` secrets are no longer used and can be deleted. Margins no longer
fetches enzyme-rust, so also remove the `margins-release` deploy key from
`byenzyme/enzyme-rust`. A missing secret fails its step with an `::error::`
naming it.

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
builds the official feature composition, fetches the pinned enzyme asset, embeds `MARGINS_GOOGLE_OAUTH_CLIENT_JSON`, and on macOS
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
`capture_unavailable`. It runs the probe with `MARGINS_GOOGLE_OAUTH_CLIENT_JSON`
and `MARGINS_GOOGLE_OAUTH_CLIENT_FILE` cleared and against a throwaway
`HOME`/`MARGINS_HOME`, so only the client embedded at build time counts and the
probe cannot touch user data. Local builds without the secret can set
`MARGINS_SMOKE_OAUTH_CLIENT=runtime-dummy`, which supplies a structurally valid
dummy client at run time and prints that the embedded client was not verified;
the release action never sets it. The contract tests composition and linkage, not hardware
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
hermetic workspace (the preset setup plan and apply, through the pinned engine
from `MARGINS_ENZYME_BIN` or `scripts/enzyme-bin`) and live capture, and
inspects WAV/SQLite durability and shutdown health. A local build has no
embedded Google OAuth client, so the composition check falls back to the
runtime dummy (logged as a NOTE) unless the binary embeds one;
`MARGINS_CORE_PRODUCT_OAUTH_CLIENT=embedded` requires it. Logs are preserved under `/tmp/margins-core-product-smoke.*`.

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
   and `brew test margins` passes; the test checks the bundled engine's
   version.
7. The bundled `enzyme` (re-signed, hardened runtime) starts from the archive
   and from the Homebrew keg without a Gatekeeper prompt, `margins` finds it,
   and a local-model catalyst run works through it.

Repeat the basic installed-binary/capture check on an Intel Mac when Intel is a
supported release tier. Linux CI can validate the linked provider and TUI
composition, but a release that claims Linux live capture should additionally
be exercised on a representative PipeWire/PulseAudio desktop with real devices.
