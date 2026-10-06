# margins

Margins is a private backchannel for a life in motion. Capture the conversations you choose, jot what catches your attention, and turn the live edge into a connected note that lands in the folder where your thinking already lives.

![margins demo](assets/demo.gif)

## Why this exists

For people who want conversations to keep unfolding after they end. Margins is not a meeting bot and not a corporate notes app. It records locally, lets you mark the moments that matter while you stay present, then saves a connected Markdown note into the notes folder you choose. If that folder is an Obsidian vault, you get `[[wikilinks]]`; if it is just a folder of notes, you still get the finished note where your context lives.

Born from two years of the same workflow: record, take sparse notes, transcribe, manually stitch the two together, then hunt through old notes for connections. Margins automates the part where other people's thinking meets your own and needs to be carried forward.

## What makes it different

**Vault-native.** Clone it into your Obsidian vault or point it at one. Sessions, recordings, and metadata live in `.margins/`. The distillation output is a vault note, not an export. No sync, no import step — it's already where your thinking lives.

**Your vault is the context window.** Every meeting app can transcribe. None of them know what you've been thinking about for the past two years. Margins's skill searches your vault during distillation — grepping for concrete anchors, running semantic search against your own writing — and weaves those connections into the final note. The meeting doesn't exist in isolation; it lands in the middle of your existing work.

**AI-native where it matters.** The intelligence isn't compiled into the app. The distillation step is an agent skill — a plain markdown file ([`SKILL.md`](skills/margins/SKILL.md)) that teaches your agent how to use your vault as context. You can read it, edit it, swap the template. The skill orchestrates [Enzyme](https://enzyme.garden) search, transcript analysis, and note generation in natural language. No black-box features, no plugin system to learn.

**5 MB binary, ~3,100 lines of code.** A Rust binary for capture, a Python script for transcription cleanup, and a markdown skill file for distillation. Small enough to read the entire codebase in an afternoon, fork it, and make it yours.

**Fully local.** Recording, CoreML transcription, speaker recognition, and storage all happen on your machine. The only network call after one-time model setup is the LLM for distillation, through your existing configured provider.

## What it does

1. **Records** stereo audio (mic + system audio) while you type timestamped notes in a terminal editor
2. **Transcribes** locally with FluidAudio CoreML and identifies speakers with Polyvoice
3. **Aligns** transcript and memo on a shared timeline
4. **Distills** into a structured vault note with connections to your existing notes via [Enzyme](https://enzyme.garden)

## Install

### Build from source

Contributors can build the public CLI and run the full public workspace gate
without credentials for the private Enzyme recall engine:

```bash
scripts/local-gate public
scripts/cargo-lane shared -- cargo build -p margins-cli --bin margins-public --no-default-features --locked
```

Margins links no private code; recall runs the `enzyme` CLI pinned in
`scripts/enzyme-cli.pin`, which official archives ship. To use Google integration in a source build, supply
your own Desktop OAuth client JSON through `MARGINS_GOOGLE_OAUTH_CLIENT_FILE` or
`MARGINS_GOOGLE_OAUTH_CLIENT_JSON`. See [CONTRIBUTING.md](CONTRIBUTING.md).

### Official CLI

Homebrew and GitHub Release binaries are official builds from the private
Margins source-of-truth. They compose this repository's public open-core crates
with non-public platform capture adapters; a source build of the public export
alone is intentionally not the same recording-capable artifact. The public
development binary is `margins-public`; released recall-capable product
artifacts install `margins`.

```bash
# Install the recorder and CLI (Apple Silicon)
brew install byenzyme/margins/margins

# Detect/download local models, then copy an agent setup handoff
margins setup

# Start a meeting
margins new
```

Run `margins setup` from the directory you want as the Margins base. It is
idempotent: it reuses compatible FluidAudio, Polyvoice, and local recall model
caches already on the Mac, or downloads and verifies them once. After machine
setup, it prints a minimal prompt to paste into your agent; the embedded guide
tells the agent to run `margins scan`, review the vault's folders, tags, links,
and running logs, scope recall policy in `~/.margins/config.toml`, run `margins
init`, and prove retrieval with `margins recall`.
Offline ranked recall is available only when that local recall model is
installed and `[llm]` is configured for local mode; hosted mode requires its
non-expired setup bundle.
`margins new` and `margins attach` load local speech models as soon as the
session identity is resolved, overlapping warm-up with session and TUI
initialization.

For imported recordings, speaker diarization is enabled by requesting more than
one speaker:

```bash
margins transcribe recording.m4a --name customer-call --speakers 2
```

Source checkout install (Linux or macOS):

```bash
./install.sh
```

On macOS, `./install.sh` builds the **full-featured** CLI — system-audio
capture, on-device ASR (`coreml-asr`), speaker diarization (`polyvoice-coreml`),
and the local catalyst model (`recall-local-model`) — installs it as
`~/.local/bin/margins`, and grants the terminal the Screen & System Audio
Recording permission. One `margins` then records, transcribes, and generates
catalyst bridges.

On Linux (and CI), `./install.sh` installs the **portable recall-only** CLI —
`scripts/install-official-cli.sh` with its default `MARGINS_CLI_PROFILE=recall`,
which uses `--features recall` (no `recall-local-model`, capture, or ASR) so
notes can be indexed and searched without linking any native backend. Native
recording and on-device ASR are macOS-only. To force the full native build
elsewhere, run `MARGINS_CLI_PROFILE=full scripts/install-official-cli.sh`.

### Local web app, including mobile browsers

Margins can also run as a local web app from this checkout. The web server
serves the built desktop UI and stores state on the machine running the server.
This is useful for trying the app in a browser or opening it from a phone on the
same network.

From a source checkout:

```bash
# Build the browser UI that the server embeds.
cd desktop
npm install
npm run build
cd ..

# Build the local server binary.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(pwd)/target}"
cargo build --manifest-path desktop/src-tauri/Cargo.toml \
  --bin margins-server \
  --features server

# Start the local web app through the package launcher.
npx --yes ./packages/margins
```

The launcher prints the URL, usually `http://127.0.0.1:8787`, and the auth token
path. For another device on your LAN:

```bash
MARGINS_HOST=0.0.0.0 npx --yes ./packages/margins
```

Then open `http://<your-computer-ip>:8787` from the other device.

Mobile recording requires a secure browser context. `http://127.0.0.1` works for
local desktop testing, but a phone connecting over the LAN usually needs HTTPS
through a trusted local reverse proxy or tunnel before the browser will allow
microphone capture. Keep the printed auth token private when exposing the server
outside localhost.

The web path does not provide every native desktop capability yet. Browser
recording uploads microphone audio to the local server, while system-audio
capture, native file dialogs, app restart/install actions, and some desktop
integrations remain native-app-only.

### Install the agent skills

`margins setup` installs the `/margins` and `/watermark` skills into Claude Code, Codex, and Cursor automatically. After setup, run `/margins <session-name>` in any of them.

## Usage

### Record

```bash
margins new                         # new current session; opens TUI + records
margins new --title "Team standup" # optional human-readable title
margins                              # attach to current session; adds a segment
margins attach                      # explicit spelling of bare `margins`
margins attach <session-id>         # make an older session current and attach
margins current                     # inspect the current session
margins rename "New title"          # retitle it without changing its stable id
margins ls                          # list all sessions
```

Margins generates stable session IDs and remembers the current session. Stopping
and returning later adds another segment to that session automatically. Run
`margins new` only when beginning a genuinely separate meeting; it replaces the
current pointer without deleting the previous session.

The TUI is a timestamped notepad. Each line gets a `[MM:SS]` timestamp when you start typing it. Edit a line later and it shows `[MM:SS ~MM:SS]`.

Keybindings: `Ctrl+D` switch mic, `Ctrl+S` save, `Ctrl+C` stop and save. After
stopping, Margins offers to turn the session into a note while the rolling
transcriber finishes its final buffered audio. If you accept before the
transcript is ready, the CLI shows that finalization progress and starts the
note as soon as the terminal transcript is durable.

On quit, the memo is published to your Obsidian vault if the vault-local
`.margins/config.toml` publishing/import config is configured. That file is
separate from the machine-global `~/.margins/config.toml` recall policy used by
`margins guide workspace-setup`, `margins scan`, `margins init`, and `margins
recall`.

### Transcribe + distill

Use the `/margins` agent skill for the full pipeline:

```
/margins standup              # transcribe → align → distill → vault note
/margins standup --align-only # just transcribe and align, no distillation
```

Or run transcription standalone:

```bash
margins process standup              # all recorded segments + memo alignment
margins process standup --speakers 3 # mono group recording with diarization
margins transcribe call.m4a --name call --speakers 2
```

Keep aligned Markdown visible to vault sync without exposing audio, SQLite, or
live checkpoints:

```bash
margins archive on      # move aligned transcripts to _margins/
margins archive off     # move them back to .margins/
margins archive status
```

## Vault integration

Optional. Create vault-local `.margins/config.toml` for note publishing/import
defaults:

```toml
[vault]
path = "~/obsidian"
folder = "inbox"
filename = "{{date:%Y-%m-%d-%-H-%M-%S}}"
open_in_obsidian = true
```

Distillation templates live in `.margins/templates/` alongside the rest of Margins's session metadata. The skill/app seeds missing defaults (`1on1-idea-exchange.md`, `discovery-call.md`, `group-conversation.md`, `talk-reflection.md`) from the bundled skill templates without overwriting customized files.

Recall setup uses the machine-global `~/.margins/config.toml`, scoped by exact
vault path. Keep that policy in the workspace-setup guide flow; do not confuse
it with the vault-local publishing config above.

## How it works

**Recording**: Rust captures mic audio via cpal and system audio via Core Audio tap. It stores separate mic/system lanes as durable 16 kHz PCM batches while you record. `margins audio-export [meeting_id]` creates scriptable stereo WAV files when you need them. Existing session WAVs remain available. Switch mic devices mid-session with `Ctrl+D` — each switch creates a new audio segment with proper timeline offsets.

**Transcription**: the native Rust CLI preserves recorder stereo as separate mic/system channels. Mono group audio can be diarized with `--speakers N`; multipart sessions inherit their persisted segment offsets automatically.

**Alignment**: `margins process` interleaves transcript segments with memo lines on a shared millisecond timeline, grouping them into windows. Memo lines act as attention signals — they tell the distillation step what was worth writing down in the moment. Use `--align-only` to rebuild this view without rerunning ASR.

**Distillation**: The skill explores your vault via Enzyme — trending entities, semantic search, grep for concrete anchors — then drafts a structured note weighted by what your memo flagged. Connections you'd never search for manually show up as `[[wikilinks]]` in the final output.

## Project structure

```
skills/margins/
  SKILL.md          Agent skill — the distillation brain (300 lines)
src/
  alignment.rs      Native transcript + memo timeline alignment
  main.rs           Transitional shim into the standalone public Rust CLI
  recorder.rs       Stereo audio capture (mic + system)
  app.rs            Timestamped editor state
  tui.rs            Terminal UI (ratatui)
  session.rs        Session metadata (JSON)
  publish.rs        Vault note creation on session end
  parser.rs         Markdown ↔ editor round-tripping
  text_helpers.rs   Word/char boundary helpers
```

Sessions are stored in `.margins/` with SQLite session and memo records and durable audio chunks. WAV exports and transcript artifacts are available alongside them.

## License

[Apache License 2.0](LICENSE)
