# Margins Tauri Desktop Multipass

This is the product/architecture target for the desktop shell: a non-developer-friendly, Enzyme-first app that keeps the existing `margins` capture artifacts isolated and reviewable.

## North star

Margins desktop should feel like a small Obsidian-native meeting notebook:

1. Record mic + system audio.
2. Jot sparse timestamped memo lines as the real-time attention signal.
3. Build a transparent backchannel timeline from memo + transcript.
4. Run an Enzyme-first distillation that connects the conversation to the vault.
5. Let the user review, chat about, and revise the final note before/after writing it to Obsidian.

The desktop app should not drift from `skills/margins/SKILL.md`. If a lightweight draft path exists, label it clearly as not fully connected.

## Workspace model

- Left sidebar: sessions, status, new recording, setup status.
- Main workspace per session:
  - **Backchannel** — aligned timeline / audit trail: memo, transcript, speaker labels, audio health.
  - **Distill** — wrapped skill execution: readiness checks, Enzyme discoveries, draft note, feedback loop.

Future tabs can split this further into Capture / Note, but the MVP can keep capture as a focused recording view and use Backchannel + Distill after recording.

## User-facing terminology

Prefer:

- `Record a conversation`
- `Backchannel`
- `Make connected note`
- `Vault connections`
- `Note created`
- `Ready to make note`

Avoid surfacing implementation terms by default: `process`, `synthesize`, `ch0`, `ch1`, `ENTRY`, `CLEANUP`, `unprocessed`, `API key`.

## Enzyme-first distillation contract

A real desktop Distill run should expose these phases:

1. Reading memo attention signals.
2. Transcribing audio.
3. Aligning memo and transcript.
4. Exploring the vault with Enzyme.
5. Searching related notes, people, tags, and themes.
6. Drafting the note.
7. Writing/updating the Obsidian note.

The UI should show Enzyme discoveries as trust-building cards before or during note generation. Desktop distillation now runs through the `pi_agent_rust` SDK instead of the older `desktop/agent/synthesize.py` direct-Claude prototype. The SDK path reuses Pi auth, including `pi /login` OAuth such as OpenAI Codex, and provides Enzyme-compatible tools named after the skill's MCP tools.

## Isolation / implementation guidance

- For desktop UX iteration, use the screenshot-driven Chrome/CDP harness in `desktop/UX_CDP_LOOP.md` before reaching for native UI automation. It exercises the Vite frontend with mocked Tauri commands and reproducible scenarios.
- Keep core recording/session code in Rust and reuse `margins::{recorder, session}`.
- Treat Python transcription/diarization as an isolated worker for now; distillation should stay in Rust via the Pi SDK and custom tools.
- Bundle worker scripts/templates as Tauri resources before shipping.
- Backend must validate session names even if the frontend sanitizes them.
- Use per-session temp dirs rather than global `/tmp/<session>` transcript paths.
- Stop and finalize recordings on app/window exit.
- Drain child stdout/stderr concurrently and propagate actionable errors.

## Implemented in this pass

- Tauri backend compile fix for vault path expansion.
- Backend session-name validation for record/delete/process/read commands.
- Removed unnecessary unsafe `Send`/`Sync` impls from desktop recording state.
- Sidebar-style session shell with Backchannel and Distill tabs.
- First-launch/home copy oriented around capture → backchannel → distill.
- Artifact loading into the Backchannel/Distill tabs.
- Rust Pi SDK distillation runner (`desktop/src-tauri/src/pi_distill.rs`) that:
  - reuses the user's Pi credentials/model selection, including Codex OAuth from `pi /login`
  - loads the Margins skill instructions
  - exposes Enzyme tools (`enzyme_petri`, `enzyme_catalyze`) that shell out to the `enzyme` CLI (`enzyme petri` / `enzyme catalyze`)
  - enforces final note persistence through `margins_save_note`
- Smoke example (`desktop/src-tauri/examples/pi_distill_smoke.rs`) verified the path against `openai-codex/gpt-5.5`, Enzyme petri/search tools, and note saving.
- Production Tauri build verified.

## Still needed

- Surface Pi SDK tool events as rich Enzyme discovery cards in the Distill tab instead of generic progress lines.
- Guided setup for vault, Enzyme, permissions, and Pi/Codex login.
- Resource packaging for Python scripts/templates.
- Processing error UX and stderr handling.
- App-exit recording finalization.
- Rich Markdown/Obsidian link rendering and exact `vault_note_path` opening.
- More CDP UX scenarios for setup/permissions, Pi login failures, and rich Enzyme discovery cards.
