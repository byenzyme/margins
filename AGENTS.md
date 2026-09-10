# Margins Agent Instructions

This file is the canonical map for agents working in this repo. `CLAUDE.md`
contains legacy/project notes, but new agent-facing workflow instructions should
land here first.

## Core Rules

- Preserve unrelated user changes. This repo often has a dirty worktree.
- For agent or prompt behavior experiments, read
  `desktop/PROMPT_BEHAVIOR_EVALUATION.md` before designing arms or interpreting
  results. Separate posture, context, decision policy, rendering, and protocol;
  preserve assembled prompts; and label causal claims honestly.
- The canonical repository remote is
  `https://github.com/byenzyme/margins-desktop.git`. Work in the checkout bb
  provides and verify `git remote get-url origin` before building; do not switch
  to a nearby clone merely because it has existing artifacts.
- Coordinate Rust and Tauri builds through `scripts/cargo-lane`; it derives the
  canonical per-host target from Git's common directory, reports the current
  build owner, and prevents bb worktrees on the same machine from entering the
  shared target concurrently:
  ```bash
  scripts/cargo-lane status
  scripts/cargo-lane shared -- cargo check
  scripts/cargo-lane disposable -- cargo test --workspace
  ```
  Use `scripts/cargo-lane isolated -- <command>` only for a narrow independent
  check/test when another same-host build is active and waiting would materially
  delay useful validation. It acquires a fixed lifecycle-owned scratch slot,
  disables incremental output, and removes the target on exit. At most two isolated builds
  may run per host; additional isolated work queues. The wrapper also refuses a
  new isolated build unless free space is at least 20 GiB and at least twice the
  measured shared-target size. Keep release, packaging, signing, Tauri reinstall,
  installed-app verification, and evidence-binary generation on the shared lane.
  Different machines have independent lanes and targets. Do not create ad hoc
  targets, bypass the wrapper, or clean the shared target while any process is
  using it. The global `rust-build-concurrency` skill contains the provider-neutral
  decision and diagnosis workflow.
  On a writable host lane, high-churn tests, examples, benchmarks, E2E compiles,
  and feature matrices must use `scripts/cargo-lane disposable -- <command>`.
  Disposable and isolated work
  use the same two fixed-path scratch slots, set `CARGO_INCREMENTAL=0`, and remove
  the entire slot on success, failure, or handled signals. Reusing fixed paths
  preserves sccache key stability; the next slot acquisition recovers residue
  left by an interrupted process. The shared lane uses separate
  `margins-cargo-target` final artifacts and a `margins-cargo-build` intermediate
  cache. `scripts/cargo-lane prune` removes abandoned free scratch slots and
  pre-split legacy intermediates only while the shared lane is idle.
  The lane also has optional host-level `sccache` integration. Do not install or
  reconfigure `sccache` without user authorization. By default,
  `MARGINS_CARGO_LANE_SCCACHE=auto` uses `sccache` only when it is already on
  `PATH`; `off` disables lane-managed wrapper changes; `required` fails early if
  `sccache` is unavailable. A preexisting `RUSTC_WRAPPER` or CMake compiler
  launcher is preserved. When active, the lane configures both `RUSTC_WRAPPER`
  and missing `CMAKE_C_COMPILER_LAUNCHER` / `CMAKE_CXX_COMPILER_LAUNCHER`
  values so CMake-built dependencies such as llama.cpp use the same cache.
  Canonical npm scripts that build Tauri enter the shared lane themselves; the
  wrapper is re-entrant, so wrapping one of those npm commands in an outer shared
  lane remains safe. Use
  `scripts/cargo-lane status` to see the effective wrapper state and
  `scripts/cargo-lane sccache-stats` for deliberate `sccache --show-stats`
  diagnostics.
- For broad release/E2E/feature-matrix builds, set `CARGO_INCREMENTAL=0` to avoid
  retaining large one-off incremental graphs.
- Restart the Tauri process after Rust/Tauri backend changes. Vite hot-reload is
  enough for frontend-only TS/CSS changes.
- Prefer the real-state browser/CDP harness for fast desktop UX iteration before
  native UI automation. Use mock CDP scenarios only for fault injection or
  hard-to-produce edge states.

## Workspace Setup and Distillation

- Setup exists to make one knowledge practice legible to Margins and to persist
  only the minimum settings that keep that understanding true. A Workspace is the
  durable read/write/attention boundary for a single practice; `init`/`sync`
  materialize it. Exact-phrase recall proves a declared Source is reachable. When
  a grounded review is available, setup must also test one question that review
  promised. The setup skill and `margins guide workspace-setup` are the single
  source of truth for declaration, review, `init`, `sync`, proof, and optional
  plan/apply.
- Some builds report `recall.scan: true`: setup must consume the complete
  read-only `scan.v2` result as its evidence substrate, including coverage and
  curation candidates, representative samples, hierarchy, frontmatter, structural
  exclusions, current config, and available profiles. The skill—not a deterministic
  renderer—forms the grounded interpretation, leads with that understanding, and
  invites plain-language corrections before deriving settings. Source declarations,
  not scan, define the full recall boundary.
- A desired config is compiled with `workspace plan --desired`. The **final
  reviewed** plan is applied unchanged—`workspace apply` reads the plan's base
  revision, derives its retry identity, commits only the exact plan the user last
  saw, and refuses stale or altered plans. Never hand-edit plan JSON.
- Setup and distillation are separate. Setup makes recall ready and must not begin
  connected-note distillation. Distillation is latest-session-first: the skill
  resolves `transcript latest` (and, when needed, `artifacts latest`) inside the
  selected Workspace before retrieving note context. A selected session or supplied
  transcript, memo, text, or audio is an explicit override, not the co-equal
  default. Removing capture from a build must not remove read-only access to
  sessions that already exist.
- These are seams over existing contracts, not new surfaces. Do not introduce a
  second setup protocol, a new anchor schema, or a write/update mode for `scan`,
  and do not conflate setup with distillation.
- Preserve both verification lanes in `docs/setup-e2e-lanes.md`: the exact
  credential-free public export and the separate hosted grounded-review lane.

## Portable and macOS Platform Test Lanes

The default agent lane is portable and must run inside the managed sandbox with
no permission escalation. `recall` includes lookup, indexing, hosted-generator
policy, and orchestration; it deliberately does not link llama.cpp. Run:

```bash
cargo test --workspace --no-default-features --features recall
cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml \
  --no-default-features --features recall -- --test-threads=1
```

Apply the guarded shared `CARGO_TARGET_DIR` rule above when that cache is
writable by the execution environment; otherwise the worktree-local `target/`
is the sandbox-safe fallback. In that fallback, the worktree owns the whole
target lifecycle; do not redirect it into the host shared cache or request
permission escalation, and let worktree cleanup remove it.

If either command needs Xcode caches, CoreSimulator services, real audio
devices, CoreML, llama.cpp, or access outside its temp fixtures, treat that as a
modularization defect in the test—not a reason to escalate the sandbox.

The macOS platform lane is explicit opt-in for a developer or appropriately
provisioned CI runner. Its feature gates are capability boundaries, not the
portable test configuration:

```bash
# Native recorder/release-composition assertion (cidre/CoreAudio/Xcode).
scripts/cargo-lane disposable -- cargo test -p margins --no-default-features --features audio-capture \
  --test private_cli_composition packaged_binary_reports_private_native_composition

# Full native desktop composition (Tauri, CoreML, audio capture, llama.cpp).
scripts/cargo-lane disposable -- cargo test -p margins-desktop \
  --manifest-path desktop/src-tauri/Cargo.toml -- --test-threads=1

# Real local catalyst inference; requires an installed GGUF/native context.
ENZYME_HOME="$HOME/.enzyme" ENZYME_LOCAL_MODEL_TEST=1 \
  scripts/cargo-lane disposable -- cargo test -p enzyme-core --features local-llm \
  --test local_engine_smoke --test local_engine_concurrency -- --nocapture
```

`recall-local-model` is included in the shipped root and desktop defaults, so
release binaries retain offline catalyst generation. Use `recall` alone for
portable decision/lookup tests and `recall-local-model` only when exercising or
shipping the native inference capability.

## Native CoreML rolling harness

The native rolling harness is distinct from the headless ONNX PCM injector: it
starts the macOS CoreML live worker and feeds its real bounded queue from a WAV.
It is ignored because it requires local FluidAudio models and takes roughly a
minute of local inference.

```bash
cd desktop
export MARGINS_FLUID_COREML_MODEL_DIR="$HOME/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2"
export MARGINS_COREML_ROLLING_WAV=/absolute/path/to/spoken-mono-s16.wav
npm run test:native-coreml-rolling
```

The test accelerates capture in two ordered chunks and uses a cfg(test)-only
three-second checkpoint cadence. It asserts rolling words, checkpoint and final
journal durability, Slice B audio-endpoint drain/qualification, Auto reuse vs
explicit-speaker offline refresh selection, and a real drop-injected rejection.

## Desktop UX CDP Loop

For screenshot-driven desktop UX work, read `desktop/UX_CDP_LOOP.md`.
For remote/headless development, use the consolidated entrypoint documented in
`desktop/REMOTE_HEADLESS_DEV.md`.

Quick run:

```bash
cd desktop
npm run ux:cdp:real
```

Mock/fault-state run:

```bash
cd desktop
npm run dev
npm run ux:cdp -- --scenarios settings-audio,recording-healthy,recording-dead-tap,distill-complete
```

Real-state artifacts land in gitignored `desktop/ux-shots-real/`; mock artifacts
land in `desktop/ux-shots/`: screenshots, visible text, AX tree, console logs,
and `report.md`.

Use the real-state loop for fast visual/state review against Rust HTTP backend
behavior. The mock loop runs Chrome/Vite with mocked Tauri commands. Neither
validates native macOS WebView behavior, real audio, packaging, or Pi auth.

## Desktop Devtools / Style Tweak Loop

Use Tauri devtools mode when the user wants to tweak live desktop styles from
Web Inspector or edit CSS/TS from Zed with dev hot-reload:

```bash
cd desktop
npm run tauri:dev:tools
```

This starts the Tauri dev app with `MARGINS_OPEN_DEVTOOLS=1`, so Web Inspector
opens on launch. Edits from Zed flow through the normal Vite/Tauri dev server.

For an installed macOS app with Web Inspector enabled:

```bash
cd desktop
npm run app:reinstall:devtools
```

That path builds with the `devtools` Tauri feature and launches the installed
app executable directly so the `MARGINS_OPEN_DEVTOOLS=1` environment variable is
inherited. Do not use this path for normal release/install verification; use
`npm run app:reinstall` unless devtools are explicitly needed.

## Desktop First-Run Install Test

When the user asks to reinstall for first-run testing, test the launch
experience, or see what a brand-new user would see in the installed macOS app,
use the clean first-run install profile:

```bash
cd desktop
npm run app:reinstall:first-run
```

This installs a separate `/Applications/Margins First Run.app`, clears the
isolated `first-run-test` profile before launch, and starts the executable with
that profile so normal `/Applications/Margins.app` settings are untouched.

FluidAudio CoreML assets are intentionally a shared, read-mostly cache at
`~/Library/Application Support/FluidAudio/Models`: they are additive,
size- and presence-verified ~464 MB model files, never profile-specific
settings or user data. First-run profiles share that cache by default. For a fully isolated
model-storage E2E, set `MARGINS_FIRST_RUN_MODEL_DIR` to a disposable model
directory before `npm run app:reinstall:first-run`; it is forwarded as
`MARGINS_FLUID_COREML_MODEL_DIR` and will trigger a separate model download.

## Desktop Hybrid UX E2E Loop

For product-aware end-to-end UX improvement, read:

- `desktop/UX_E2E_LOOP.md`
- `desktop/test-harness/ux-loop/README.md`
- `desktop/UX_REVIEW.md`
- `desktop/TAURI_MULTIPASS.md`

Invoke this loop when the user asks to improve or evaluate a user job, flow,
information architecture, copy, responsiveness, note generation, provenance,
refinement, or the in-between states after capture/import.

For the required research-context pass, prefer the local Enzyme CLI against the
user's Obsidian notes. The hosted/MCP route may be unavailable in this workspace.

**Do not confuse the two stores, and know exactly where they still overlap.**
The standalone `enzyme` CLI below is for the *agent research* pass and reads
`~/.enzyme/enzyme.db`. Product retrieval is a different path: Margins indexes
and reads `$MARGINS_HOME/workspaces/<id>/index.db` in-process. When testing
product retrieval, declare Sources and pass `--workspace <id>` to `margins init`;
initializing an Enzyme fixture leaves the product path unindexed.

The boundary as it actually stands:

| Concern | State |
| --- | --- |
| Auth/credentials | Uncrossed. Margins uses an injected desktop credential or explicit env; it never discovers Enzyme auth. |
| Recall database | Uncrossed. `$MARGINS_HOME/workspaces/<id>/index.db`. |
| Local model *files* | Intentionally shared at `~/.enzyme/models/`. |
| Model selection | Machine-level Margins config. |
| Excluded folders | Workspace policy in `$MARGINS_HOME/workspaces/<id>/config.toml`. |

Margins builds an ephemeral engine configuration from those declarations; it
does not discover Enzyme policy or Sources from cwd.

```bash
enzyme status --vault ~/obsidian
enzyme petri --vault ~/obsidian --query "<job/concern>" --top 8
enzyme catalyze --vault ~/obsidian --limit 6 "<focused product/context query>"
```

Use 2-3 targeted `enzyme catalyze` queries after `petri`. Record short
breadcrumbs in the UX brief; do not paste large note bodies into reports.

This loop is not a static scenario checklist. It should:

1. retrieve product/user context, including Obsidian/Enzyme when useful;
2. form a job-specific UX brief;
3. plan a dynamic Playwright/CDP journey;
4. run browser evidence collection;
5. run a real LLM distillation fixture when explicitly allowed;
6. judge video/timing/generated-note evidence;
7. patch one bounded issue;
8. rerun and compare.

Common normalized invocation:

```text
job: customer-call-connected-note
concern: users cannot tell what is happening before the first note token
allow_llm: yes | no
mode: report-only | patch-if-bounded
fixture: customer-call
```

Current real-LLM substrate check:

```bash
cd desktop
npm run ux:e2e:distill-fixture
```

That command may spend model/API resources. Ask or confirm before running it
unless the user explicitly allows real LLM calls.

## bb Thread Routing

Use `bb` child threads when the loop benefits from isolated judgment. Keep the
parent thread as coordinator: pass artifacts, enforce stop conditions, and decide
whether to patch, rerun, or escalate.

**Do not spin up Codex threads with a sandbox.** Spawn them with
`--permission-mode full`, never `workspace-write` or `readonly`. Codex's
`workspace-write` sandbox uses nested bubblewrap, which fails at startup in this
environment (`bwrap: setting up uid map: Permission denied` / `loopback: Failed
RTM_NEWADDR`), blocking every command before the repo is even readable. The
per-thread worktree already provides isolation, so `full` (no sandbox) is both
safe and required. This applies to any provider that wraps execution in
bubblewrap.

**Installed-app verification (`app:reinstall` + computer use) must run in the
primary checkout, not a worktree.** Computer use itself works from any
environment — it drives the machine-global `/Applications/Margins.app`, so a
worktree agent CAN screenshot and click the running app (verified: a worktree
Codex thread drove the real app and created a live session). The problem is
coherence, not capability: the app is built and installed from one checkout, and
both `/Applications/Margins.app` and the shared `CARGO_TARGET_DIR` are single
machine-global singletons. An agent that edits code in an isolated worktree and
then verifies the installed app is verifying a *different* checkout's build; and
parallel worktree agents that each `app:reinstall` race and clobber the one
install. So any thread that reinstalls and then confirms behavior against the
running app must run in **direct mode on the primary checkout** (spawn with
`--environment <primary checkout/env>`, never `--new-environment worktree`), and
only one such thread at a time. Pure code/build/test work can still fan out to
worktrees — the singleton constraint is specifically the install + live-app
verification loop. (Prefer `app:reinstall:devtools` for that thread so it can
also read the WKWebView console; the release build ships without devtools.)

Provider/model routing:

- Use **Codex** (`--provider codex`, usually default model `gpt-5.5`) for purely
  code, Rust, structural, backend, performance, test harness, or mechanical
  verification tasks.
- Use **Claude Code Opus 4.8** (`--provider claude-code --model 'claude-opus-4-8[1m]'`)
  for UI/product judgment tasks: layout, visual hierarchy, copy, information
  architecture, microinteractions, Playwright video review, and UX brief/judge
  work.
- Prefer independent maker/checker separation. The thread that patches UI should
  not be the final judge of whether the UX improved.

Recommended hybrid UX E2E split:

- flow-map thread (FIRST, required per `desktop/UX_QUALITY_SPEC.md` "Applying the
  spec"): Codex, `--reasoning-level high --permission-mode readonly`, run against
  the live working tree (not a worktree off HEAD). Reconstructs the state/transition
  graph by journey. Unit of every finding is a state or transition, never a string;
  copy is out of scope. Output feeds every later pass the set of transitions to
  assert claims about and score.
- research thread: Obsidian/Enzyme + product docs -> UX brief and falsifiable claims
- journey planner thread: UX brief + app state -> Playwright/CDP journey
- runner thread: execute journey and collect video/screenshots/timings/generated note
- judge thread: evaluate artifacts against the brief
- patch thread: make one bounded UX improvement
- verifier thread: rerun the same journey and compare before/after
- memory thread: update loop observations for future runs

Use Codex for runner/verifier/harness implementation unless the work turns on
layout or product judgment. Use Claude Opus 4.8 for research/judge/planner when
the task is UX-sensitive.

For Playwright journey work, prefer a CLI/test-runner path that records the full
journey video. Preserve the raw Playwright video, convert to MP4 when the
toolchain supports it, and hand stable artifact paths to judge/verifier threads.

## Headless Linux E2E Harness (real backend on a VPS)

Running or driving Margins on this headless Linux box — the real `margins-server`
backend over HTTP + the Vite frontend, driven with agent-browser, plus the
mock/CDP screenshot lanes — is documented in full in
**`desktop/REMOTE_HEADLESS_DEV.md`** (the source of truth). Go there before any
VPS E2E work; it covers, in order:

- **Which lane** (mock CDP vs real CDP screenshots vs interactive real agent-browser).
- **Prerequisites**: `npm run build` so `dist/` exists (else the server build
  fails with a RustEmbed → `E0599` cascade); `agent-browser install`; ffmpeg.
- **Spin up** `test-harness/local-e2e/run.sh`, **port hygiene** across worktrees,
  and scoped cleanup (never kill a neighbor's harness).
- **Driving with agent-browser**: the gateway-IP gotcha (reach the host at
  `172.18.0.1`, not `127.0.0.1`), physical-click-vs-`eval`-click, and the
  insecure-context clipboard shim.
- **Hermetic setup / CLI-install E2E** (`run.sh --hermetic-bin`: installs into a
  temp HOME, leak-checks + self-cleans) — drive it agentically, not via a
  committed script.
- Real distillation, optional headless ASR, the env-var reference, and what still
  needs a native macOS pass.

## Release / Build Pointers

Before preparing or approving a CLI release candidate, run the ecological
Workspace setup review with `scripts/workspace-setup-rollout-review.py` by
following `docs/workspace-setup-rollout-review.md`. Give the agent only the
fixed ordinary-user prompt produced by the harness, preserve the complete local
transcript, and have an independent reviewer judge the rollout as a whole. Do
not add component hints or maintain a state-transition brief for this review.
The CI regression verifies only the harness's capture and hard-gate mechanics;
it does not replace the real agent rollout against the release-candidate binary.

For native CLI core-product verification, use
`scripts/core-product-smoke.sh`; the full and zero-compile iteration commands
are documented in `docs/official-cli-release.md`.

See `CLAUDE.md` for release flow, Homebrew tap notes, and legacy build commands.
