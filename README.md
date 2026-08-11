# Margins

Meeting notes that compound.

Margins records your meetings from the terminal — no bot joins the call —
and turns them into structured notes in one folder you own. Every note lands
in the same vault as plain Markdown, so each new meeting is distilled with
the memory of the ones before it: the same people, projects, and decision
threads accumulate instead of scattering across a timeline you'll never
reread.

The difference from a transcription tool is what happens around the
transcript. While Margins records, you jot margin notes in a timestamped
memo editor — each line stamped against the recording clock. A line at 14:32
means *this mattered*. Transcription and diarization run on your machine,
your marginalia are aligned to what was being said when you wrote them, and
an agent skill distills the result into a real meeting artifact — decisions,
action items, open threads — grounded in what you flagged, not a generic
recap.

## Get started

```bash
brew install useenzyme/margins/margins
margins new standup
```

Linux x86_64 archives and SHA-256 checksums are published on the
[GitHub Releases](https://github.com/useenzyme/margins/releases) page. Install
the latest archive with:

```bash
tar -xzf margins-VERSION-x86_64-unknown-linux-gnu.tar.gz
install -m 0755 margins ~/.local/bin/margins
margins --help
```

The release binary provides the portable CLI without an ASR backend. Build
from source with `--features parakeet-onnx` when you need local transcription
on Linux.

`margins new` starts recording mic and system audio and opens the memo
editor. Type what you notice; end the session; then let the skill do the
rest.

The distillation skill ships as a Claude Code plugin in this repository
under `skills/`. Point your agent at a finished session and it transcribes,
aligns your memo lines, and publishes a structured note into your vault —
next to every note before it. The skill and its note templates are ordinary
readable text files: fork them and change what a meeting note *is*.

## Run the pipeline on any audio

You can also hand the CLI a file directly — a QuickTime recording, one
ffmpeg line, an old voice memo — and watch the same pipeline work, entirely
on your machine:

```bash
cargo install --locked --path crates/public/margins-cli --features coreml-asr
export MARGINS_FLUID_COREML_MODEL_DIR=~/models/fluid-coreml
margins transcribe meeting.wav --speakers 2 --memo notes.md
margins recent
```

`margins transcribe` decodes the audio, runs on-device ASR and diarization,
aligns the optional memo, and records the session in local SQLite. Models
are caller-supplied via `MARGINS_FLUID_COREML_MODEL_DIR` (Core ML) or
`MARGINS_PARAKEET_MODEL_DIR` (Parakeet ONNX).

Everything Margins stores is inspectable without Margins: sessions are
SQLite, notes are Markdown in your vault. Open them with sqlite3, grep,
Obsidian, or your own agents.

## Build with it

This repository is also a customization platform: the crates are composable
building blocks for remote meeting recording and transcription systems.
Applications can supply their own capture clients, transports, persistence
adapters, ASR and diarization providers, note templates, and agent
workflows on top of the same session and meeting contracts.

The workspace uses stable Rust:

```bash
cargo build --workspace --locked
cargo test --workspace --all-targets --no-default-features --locked
```

Design detail lives in [docs/architecture.md](docs/architecture.md) and
[docs/public-cli-extraction.md](docs/public-cli-extraction.md); the
contributor loop is in [CONTRIBUTING.md](CONTRIBUTING.md).

## Trust, security, and privacy boundaries

Nothing in these crates makes a network call or emits telemetry;
persistence is local SQLite and Markdown files. What's public here is
selected by a fail-closed exact allowlist with a deterministic exporter and
verifier, so you can check precisely what is in this tree —
[OPEN_SOURCE.md](OPEN_SOURCE.md) documents the boundary. The crates do not
provide authentication, transport encryption, consent UX, or retention
policy; an application embedding them must choose and verify those, and
should treat audio, transcripts, memos, and notes as sensitive personal
data.

Report suspected vulnerabilities privately to the maintainers; never
include meeting audio, transcripts, credentials, or other user data in an
issue.

## License

Apache 2.0 (see `LICENSE`). This tree is exported from a mixed development
repository through the audited boundary; its presence here is not a claim
that any crate has been published to a registry, and the license and
manifests do not grant trademark rights in the Margins name or branding.
