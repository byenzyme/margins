# Public CLI extraction plan

Status: executable patch plan; this document does not authorize or contain
production-code changes.

Baseline: `de72a57` (2026-08-10).

## Outcome and boundary

Extract the user-facing parser and every non-desktop command from
`src/main.rs` into `crates/public/margins-cli`, while preserving the existing
command names, flags, XML/JSON success output, project routing, and on-disk
formats. The public binary must remain useful without native capture: file
transcription, processing, recent/transcript queries, project management, and
artifact management are public commands. Native recording is supplied only by
an injected `margins_core::CaptureProvider`.

This is not a request to copy `src/main.rs` into a new crate. At the baseline,
that file imports the private/transitional root crate for terminal state,
Granola import, parsing, project settings, publishing, recorder ownership,
SQLite sessions, session indexing, and the TUI. Only `margins-core`,
`margins-media`, `margins-meeting-protocol`, and
`margins-meeting-runtime` currently exist under `crates/public/`.
`margins-store` and `margins-workflows`, already assigned ownership by
`docs/open-core-modularization.md`, are therefore hard prerequisites for a
standalone public CLI. A `margins-cli` manifest that points back to the root
`margins` package is not an extraction and must not land.

The final package has both targets:

```toml
[package]
name = "margins-cli"
version = "0.1.0"
edition = "2021"
license = "Apache-2.0"

[lib]
name = "margins_cli"
path = "src/lib.rs"

[[bin]]
name = "margins"
path = "src/main.rs"

[features]
default = []
coreml-asr = ["margins-media/coreml-asr"]
parakeet-onnx = ["margins-media/parakeet-onnx"]
polyvoice-diarization = ["margins-media/polyvoice-diarization"]

[dependencies]
anyhow = "1"
chrono = "0.4"
clap = { version = "4", features = ["derive"] }
crossterm = "0.28"
margins-core = { version = "0.1.0", path = "../margins-core" }
margins-media = { version = "0.1.0", path = "../margins-media" }
margins-store = { version = "0.1.0", path = "../margins-store" }
margins-workflows = { version = "0.1.0", path = "../margins-workflows" }
ratatui = "0.30"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

[dev-dependencies]
tempfile = "3"
```

No `cpal`, `cidre`, `windows`, Tauri, Objective-C capture binding, root
`margins`, or private crate may appear in this manifest or its no-default
dependency graph. ASR features name model adapters over caller-supplied PCM;
they do not enable device capture.

## Command-by-command dependency map

“Public service” names below are target owners, not permission to duplicate
their implementations in `margins-cli`. `ProjectService`, `SessionStore`, and
`ArtifactStore` are adapters exported by `margins-store`;
`ProcessWorkflow`, `TranscribeWorkflow`, and import/alignment helpers are
exported by `margins-workflows`.

| Invocation | Baseline implementation and concrete dependencies | Extracted owner/dependencies | No-native public behavior |
|---|---|---|---|
| bare `margins` | `cmd_attach`; `project`, `session`, `app`, `recorder`, `tui`, `publish` | `margins-cli::commands::capture`; injected `CaptureProvider`, `SessionStore`, terminal presenter | Resolve current project, then return `capture_unavailable`; make no session/segment/file mutation |
| `new [--title]` | `cmd_new_current`; `session`, `app`, `recorder`, `tui` | capture command plus injected provider/store | Return `capture_unavailable` before creating the session or current pointer |
| `attach [session]` | `cmd_attach`/`cmd_resume`; `session`, `app`, `recorder`, `tui` | capture command plus injected provider/store | Return `capture_unavailable` before changing the current pointer or appending a segment |
| `current` | `cmd_current`; `session` and `.margins/current` | `SessionStore` current-pointer and aggregate read | Fully available |
| `ls` / `list` | `cmd_list`; `session::list_sessions` | `SessionStore::list` | Fully available; preserve text output |
| `rename <title>` | `cmd_rename`; current pointer and `session::set_title` | `SessionStore` revision-checked title update | Fully available |
| `recent` | `cmd_recent`; `session_index`, `session`, transcript path fallback | `RecentWorkflow` over `SessionStore`/`ArtifactStore` and confined filesystem resolver | Fully available; preserve the 30-item limit and `<margins_recent>` XML |
| `transcript <id\|latest>` | `cmd_transcript` plus live JSONL assembly, memo merge, artifact/note fallbacks, speaker alias | `TranscriptViewWorkflow`; store metadata and registered artifacts; pure renderer | Fully available; preserve `view="full"` preference over `view="aligned"` and XML metadata |
| `artifacts <id\|latest>` | `cmd_artifacts`; artifact registry plus path existence | `ArtifactStore::list_for_session` and confined path resolver | Fully available; preserve XML and existence reporting |
| `artifacts-prune` | `cmd_artifacts_prune`; expiry query, confined deletion, row deletion | `ArtifactPruneWorkflow`; clock, `ArtifactStore`, confined filesystem | Fully available; reject absolute/traversal paths, delete files before registry rows, preserve summary XML |
| `transcribe <audio> [--name] [--memo] [--speakers]` | decode/resample via root media facade; concrete `offline_asr`/optional diarizer; session writes and ad-hoc renderer | `TranscribeWorkflow`; `margins-media`, injected `AsrBackend`/optional `DiarizationBackend`, `SessionStore`, `ArtifactStore` | Available when an ASR adapter is configured; otherwise stable `asr_unavailable`. It never depends on capture. Preserve mono/stereo mode selection and artifact paths |
| `process <session> [--speakers] [--align-only]` | all session segments; media decode; concrete ASR/diarizer; root `alignment`; session artifact upsert | `ProcessWorkflow`; public store/media/backend ports and alignment renderer | Fully available for `--align-only`; other modes require configured ASR, and `--speakers > 1` additionally requires diarization. Never require capture |
| `import granola <path>` | `granola_import`, `publish`, `session`, project vault target | `GranolaImportWorkflow`, public project/store adapters | Fully available; preserve destination and meeting XML |
| `agents install` | local file upsert plus an instruction string embedded in `main.rs` | `margins-workflows` public resource and idempotent file upsert; CLI presenter | Fully available; resource must live under the public workflow crate, not `desktop/` |
| `project list` | global project settings, resolver, XML renderer | `ProjectService` | Fully available; preserve XML and active marker |
| `project current` | project resolver | `ProjectService` | Fully available; preserve XML |
| `project use <selector>` | global settings mutation | `ProjectService::set_active` | Fully available; selector remains id/name/root/work-dir |
| `projects list [--json]` | duplicate list surface with XML/JSON presenters | same `ProjectService`; CLI-specific presenters | Fully available; do not collapse this compatibility alias into `project list` because `--json` differs |
| `projects add <path> [--name] [--inbox-folder] [--init]` | project registry, desktop-derived setup skill, optional `enzyme` child process | `ProjectService::add`, public setup-skill resource, injected `ProcessRunner` for Enzyme | Fully available. Registration succeeds even if optional Enzyme init warns, matching current behavior |
| `projects init [path]` | project resolver, `.margins` creation, setup skill, `enzyme` process | `ProjectInitWorkflow` plus injected `ProcessRunner` | Fully available; no Python dependency |

The current `ImportCommand`, `AgentsCommand`, `ProjectCommand`, and
`ProjectsCommand` nesting and the global `--project value` / `--project=value`
pre-parser remain byte-for-byte compatible at the clap boundary. Relative
audio, memo, and import paths continue to resolve against the invocation
directory before the CLI enters the selected project work directory.

## Composition and stable failures

`crates/public/margins-cli/src/lib.rs` owns parsing and dispatch but receives
all effects. The minimum API is:

```rust,ignore
pub struct CliServices {
    pub capture: Arc<dyn CaptureProvider>,
    pub sessions: Arc<dyn SessionRepository>,
    pub projects: Arc<dyn ProjectRepository>,
    pub asr: Arc<dyn AsrBackend>,
    pub diarization: Arc<dyn DiarizationBackend>,
    pub events: Arc<dyn EventSink>,
    pub clock: Arc<dyn Clock>,
    pub processes: Arc<dyn ProcessRunner>,
}

pub fn run<I, T>(
    services: &CliServices,
    invocation_dir: &Path,
    args: I,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<(), CliError>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone;
```

The precise repository/workflow trait split may live in the owning crates, but
the CLI entrypoint must accept injected services, explicit paths, and writers.
It must not read process globals except in the ten-line binary composition
target. This makes command tests deterministic and lets the private native
binary inject the same services without forking the parser.

`crates/public/margins-cli/src/main.rs` constructs SQLite/project adapters,
feature-selected model adapters (or typed unavailable adapters),
`UnavailableCaptureProvider`, the system clock/process runner, and calls
`run`. The private native composition constructs the same `CliServices` but
replaces only `capture` with the native provider. There is no
`native-capture` Cargo feature in the public crate and no dynamic-library ABI.

Before `new`, bare, or `attach` mutates durable state, dispatch calls
`capture.capabilities()`. If `available == false`, the command returns:

```text
<margins_error code="capture_unavailable">capture is unavailable in this build</margins_error>
```

on stderr, writes nothing to stdout, and exits 69. Provider start failures with
`CaptureErrorCode::Unavailable` use the same code. Permission and device
failures retain distinct stable codes (`capture_permission_denied`,
`capture_device_lost`, and `capture_open_failed`) and exit 1. Portable commands
must never consult capture capabilities.

For injected capture, session creation, current-pointer selection, and segment
registration happen as one command transaction around provider startup:

1. validate capabilities and permission;
2. reserve the session/segment and artifact destination;
3. start the provider with public IDs and a `CaptureObserver` owned by the CLI;
4. commit the current pointer only after start succeeds;
5. drive the terminal view from public snapshots/events;
6. on finish, persist only the `ArtifactDescriptor` values returned by the
   handle.

If start fails, the reservation is rolled back or marked `NeedsAttention`; it
must not leave a current pointer to an unstarted session. The CLI never sees a
`cpal::Device`, stream, ring, recorder handle, or segment writer. `attach`
always appends a segment to the selected stable session ID; it never creates a
suffixed session for an interruption or multipart call.

ASR unavailability follows the same rule. `transcribe` and non-align-only
`process` return `asr_unavailable`/exit 69 before creating or changing session
artifacts. `process --align-only` does not inspect the ASR provider.
`--speakers > 1` returns `diarization_unavailable` before writes when no
diarizer is configured.

## Exact build-green patch stack

Each patch below must build and test on its own. Do not add a knowingly red
workspace member and repair it in a later commit.

### Patch 1: finish the public prerequisites

Add `crates/public/margins-store` and `crates/public/margins-workflows` as
specified by `docs/open-core-modularization.md`, then move ownership rather
than making CLI-local copies:

- `margins-store`: extract the SQLite schema/migrations and session, artifact,
  recent-index, current-pointer, vault-note, and project registry adapters from
  `src/session.rs`, `src/session_index.rs`, and the settings portion of
  `src/project.rs`.
- Extend the public store contracts for the operations the table requires:
  current pointer get/set, title update, full aggregate/recent query, artifact
  list/upsert/expiry/delete, vault-note lookup, and project list/current/use/add.
  Revision checking remains in the store; do not expose raw SQLite connections.
- `margins-workflows`: extract `src/alignment.rs`, portable project resolution,
  Granola import, processing/transcription orchestration, artifact confinement,
  transcript-view assembly, and public agent/setup resources.
- Move model adapters promised by the architecture into feature-gated
  `margins-media` modules implementing the existing public `AsrBackend` and
  `DiarizationBackend` ports. Add typed unavailable adapters for default
  no-feature composition.
- Remove the existing `src/project.rs` reach into
  `desktop/src-tauri/resources`; the public setup skill is embedded from
  `crates/public/margins-workflows/resources/skills/`.

Green gate:

```bash
export CARGO_TARGET_DIR=/Users/joshuapham/Hacks/margins-cargo-target
cargo test -p margins-core --all-targets
cargo test -p margins-media --no-default-features --all-targets
cargo test -p margins-store --all-targets
cargo test -p margins-workflows --no-default-features --all-targets
cargo tree -p margins-workflows --no-default-features --prefix none
```

The tree output must contain no capture/desktop dependency. The root binary
continues to compile through temporary re-exports after this patch.

### Patch 2: add the public CLI library and portable commands

Create exactly these files (small helper modules may be split further without
changing ownership):

```text
crates/public/margins-cli/
├── Cargo.toml
├── README.md
├── src/
│   ├── args.rs
│   ├── commands/
│   │   ├── artifacts.rs
│   │   ├── import.rs
│   │   ├── mod.rs
│   │   ├── process.rs
│   │   ├── projects.rs
│   │   ├── sessions.rs
│   │   └── transcript.rs
│   ├── error.rs
│   ├── lib.rs
│   ├── main.rs
│   └── output.rs
└── tests/
    ├── command_contract.rs
    ├── multipart_parity.rs
    └── standalone.rs
```

Move `Args` and all command enums to `args.rs`; move XML escaping and output
formatting to `output.rs`; route the portable commands in the table through
public services. `error.rs` owns machine codes and exit mapping. Keep
`main.rs` composition-only. Register `crates/public/margins-cli` in the root
workspace only after `cargo test --manifest-path
crates/public/margins-cli/Cargo.toml --no-default-features --all-targets`
passes.

This patch must not yet delete root dispatch. Instead, make the root binary a
temporary parity caller of `margins_cli::run` for the portable command set, so
the application retains one parser and output contract while native capture is
finished. No command may have two independently edited clap definitions.

Green gate:

```bash
cargo fmt --all -- --check
cargo test -p margins-cli --no-default-features --all-targets
cargo check -p margins-cli --no-default-features --bin margins
cargo tree -p margins-cli --no-default-features --prefix none
```

### Patch 3: inject capture and retire root dispatch

Add `commands/capture.rs` and `terminal.rs`, adapting the TUI to
`CaptureProvider` snapshots/events. Add an in-memory fake provider for tests;
do not feature-gate parser code. Change the private native CLI composition to
inject `margins-capture-native`. Then replace `src/main.rs` with the temporary
private composition shim (and ultimately remove the root binary when desktop
composition owns it).

Required tests cover unavailable-before-mutation, injected start/finish,
permission failure, failed-start rollback, stable ID across attach, and exact
error output/exit status.

Green gate:

```bash
cargo test -p margins-cli --no-default-features --all-targets
cargo check -p margins-cli --no-default-features --bin margins
cargo check -p margins-desktop --no-default-features --features native-cli --bin margins
```

### Patch 4: switch the skill from Python

Only after Patch 2's multipart tests pass:

- Rewrite `skills/margins/SKILL.md` mechanical steps to invoke
  `margins process <session> [--speakers N] [--align-only]` and
  `margins transcribe <audio> --name <session> [--memo <path>]
  [--speakers N]` directly.
- Translate the documented legacy spelling `--num-speakers N` to
  `--speakers N` in instructions, not in a Python subprocess wrapper.
- Use `margins recent`, `margins transcript`, and `margins artifacts` for
  resolution. Do not teach agents to query SQLite or glob all transcripts.
- Remove `skills/margins/scripts/margins.py` and
  `skills/margins/scripts/test_margins.py`; their alignment assertions move to
  Rust integration tests. No Python, ffmpeg, ffprobe, NumPy, sklearn,
  `fluidaudiocli`, or `diarize` prerequisite remains in the skill.

The public skill still performs Enzyme-assisted synthesis after mechanical
processing; this patch changes only the audio/alignment transport.

### Patch 5: allowlist and prove the standalone export

Update `open-source-boundary.json` atomically with Patch 4:

- replace the two-file Python `cli` scope with all tracked files under
  `crates/public/margins-cli/` and make `Cargo.toml`, `README.md`, `src/lib.rs`,
  `src/main.rs`, and the three integration tests required;
- add equivalent explicit scopes for `margins-store` and
  `margins-workflows`, including public embedded resources;
- keep `src/main.rs`, `desktop/**`, and `crates/private/**` denied;
- include `docs/public-cli-extraction.md` in the public docs scope;
- remove the deleted Python files from every required/include list;
- add forbidden dependency/import expressions for `margins`,
  `margins-capture-native`, `margins-desktop`, `cpal`, `cidre`, `tauri`, and
  paths escaping `crates/public`.

Extend `.github/workflows/open-source-boundary.yml` to copy the exported
`crates/public` directory to a second temporary root with the monorepo path no
longer reachable, then run:

```bash
cargo metadata --manifest-path "$PUBLIC_TMP/margins-cli/Cargo.toml" --format-version 1
cargo test --manifest-path "$PUBLIC_TMP/margins-cli/Cargo.toml" --no-default-features --all-targets
cargo tree --manifest-path "$PUBLIC_TMP/margins-cli/Cargo.toml" --no-default-features --prefix none \
  > "$RUNNER_TEMP/margins-cli-tree.txt"
```

Fail if metadata contains a package `source` path outside `$PUBLIC_TMP` or if
the tree contains any forbidden native/private package. Run the exporter twice
from the same index tree and compare file manifests and SHA-256s. This
manifest-path build is intentional: it proves the CLI and all path dependencies
stand alone without exporting the transitional root package. A later virtual
public-workspace manifest may improve ergonomics, but is not required to make
this extraction build-green.

## Multipart and Python parity gate

`tests/multipart_parity.rs` replaces the important behavior of Python
`parse_transcripts`, which enumerates transcript parts and adds each stored
segment's `offset_ms`. Use tiny checked-in PCM fixtures or generated WAVs and a
deterministic fake `AsrBackend`; never load a real model in these tests.

The required cases are:

1. Create one session with segment ordinals 0 and 1 in reverse insertion order,
   offsets 0 and 90,000 ms, and distinct fake words. `process` must read ordinal
   order, apply each offset exactly once, sort by `(start_ms, channel)`, write
   one `<session>_transcript.json`, and register one durable transcript artifact.
2. Re-run `process --align-only`. It must read that already session-relative
   combined JSON, apply no segment offset a second time, make zero ASR calls,
   preserve word timestamps, and update only the aligned artifact.
3. Process two stereo parts. Each part must keep local/user channel 0 and
   remote/other channel 1; the second part receives its session offset without
   swapping channels. `--speakers 1` on stereo still means channel separation,
   matching current `src/main.rs`.
4. Process mono multipart audio with `--speakers 2`. Diarization labels must be
   stable across parts or explicitly remapped by the workflow; no part may
   restart labels and silently merge different speakers. Missing diarization
   fails before transcript/artifact writes.
5. Include memo entries immediately before, exactly at, and after the part
   boundary. At equal timestamps transcript sorts before memo, and a memo closes
   the preceding context window, matching the Python tests.
6. Run `attach` twice with a fake injected capture provider. The store contains
   one session with monotonically ordered segment ordinals and one stable
   session ID/current pointer; no `-2` or other suffixed meeting is created.
7. Golden-test XML for `recent`, `transcript`, and `artifacts` after multipart
   processing, including `segments="2"`, full-over-aligned transcript fallback,
   and one registered transcript artifact whose path exists.

During the transition, run the same fixture through the old Python align path
and the new Rust path, normalize only headings/UUIDs/creation timestamps, and
assert equal ordered memo/transcript events, segment offsets, channel labels,
and counts. Delete the Python implementation only after this differential test
passes. The differential harness itself is transitional and is deleted in the
same patch as the Python files; the Rust golden fixture remains.

## Completion criteria

The extraction is complete only when all of these are true:

- every command in the table has one clap definition and one public dispatcher;
- capture-free commands pass with `UnavailableCaptureProvider` and no native
  dependencies in `cargo tree`;
- capture commands fail before mutation when unavailable and work through an
  injected fake/native provider without exposing platform handles;
- process/transcribe use public media/backend ports, and multipart offsets are
  applied exactly once;
- the skill contains no Python compatibility path;
- the allowlisted tree builds/tests from a separate temporary directory; and
- the full private desktop/native CLI regression matrix stays green.

No command rename, database migration, output redesign, dynamic plugin ABI, or
production behavior change beyond the explicit stable unavailable failures is
part of this extraction.
