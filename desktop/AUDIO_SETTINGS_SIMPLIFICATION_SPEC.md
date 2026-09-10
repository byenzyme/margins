# Audio Settings Simplification — Implementation Spec

Audience: implementer (Codex) + reviewer. Self-contained. App is internally
"Margins" (externally "aside"). Vanilla-TS DOM app in `desktop/src`, Tauri/Rust
backend in `desktop/src-tauri/src`, shared Rust crate in `src/`.

Build/validate env (MANDATORY before any cargo/Tauri command):

```bash
export CARGO_TARGET_DIR=/Users/example/Hacks/aside-cargo-target
```

Validate with:

```bash
cd desktop && npm run dev            # if not already on :5173
cd desktop && npm run ux:cdp -- --scenarios settings-audio,settings-audio-blocked,settings-audio-ready
cd desktop && npm run build
cd desktop/src-tauri && cargo test
cd desktop/src-tauri && cargo check
```

Always open the generated PNGs in `desktop/ux-shots/`, not just the text dump.
Preserve unrelated dirty-worktree changes.

---

## Current-state map (files/lines cited against HEAD at spec time)

### Audio settings pane
- `desktop/src/render/settings.ts`
  - `renderAudioSettingsSection()` (≈270–344): "Audio readiness" card with
    **Open Microphone** + **Open System Audio** buttons; a Microphone
    `<select>` + Refresh; a "Test microphone" card; a "Test computer audio"
    card (with a restart callout when `restart_recommended`); a **Default
    speakers for imports** `<select>` (`import_speaker_count`).
  - `renderSpeechSettingsSection()` (≈235–268): "Local transcription"
    (badged **Recommended**). A `model-prep-card` with **Check/Refresh**,
    **Cancel**, **Clear** buttons; a `#speech-model-status` line; an
    `<details>` "Advanced local transcription options" containing a
    `#parakeet-model-dir` text input and the **"Label speakers in single-track
    recordings"** checkbox (`#rust-diarization-enabled`).
  - `settingsSections()` (≈434–475): the "audio" section combines
    `renderAudioSettingsSection` + `renderSpeechSettingsSection`; its nav dot is
    `status: audioReady ? "ok" : "warn"` where
    `audioReady = ctx.audioSetupReady && Boolean(settings.parakeet_model_dir)`.
- `desktop/src/main.ts`
  - Handlers: `__openPrivacyPane` (1887), `__testSystemAudioTap` (1906),
    `__testAudioInput` (4374), `__prepareSpeechModels` (4111),
    `__cancelSpeechModelDownload` (4168), `__clearSpeechModels` (4188),
    `__openAudioSetup` → `openSettingsSection("audio")` (1883/1878),
    `__restartMarginsToAudioSetup` (1897).
  - `audioSetupReady()` (1740): mic ready AND tap ready (from test results or
    persisted `audio_input_ready`/`system_audio_ready`).
  - `captureReadyFromState()` (1735): project path + notesFolderReady +
    `project.readiness === "ready"`. **Audio is NOT part of the capture gate.**
- Backend: `desktop/src-tauri/src/audio_devices.rs` (`test_audio_input`,
  `test_system_audio_tap`, `classify_system_audio_tap`),
  `desktop/src-tauri/src/speech_models.rs`
  (`prepare_speech_models_blocking`, `clear_speech_models_blocking`,
  `SpeechModelPrepResult`, `speech-model-progress` event contract).

### New-capture permission flow (Problem 1)
- New-capture "+" button → `window.__startDefaultMeeting`
  (`sidebar.ts:92/96`; also `__startMeetingInProject` sidebar:278).
- `__startDefaultMeeting` (main.ts 1940) → `startNamedMeeting` (1964) →
  `beginOptimisticRecording` (1989) → `finishRecordingStart` (2065) →
  `startRecording(...)` Tauri command (2082). **`startRecording` is what
  triggers the inline macOS microphone + Screen/System-Audio permission
  prompts.** `startNamedMeeting` currently only gates on
  `captureReadyFromState()` (project/AI), never on audio permission.

### Model download reality (critical finding, drives Problem 3)
- `speech_models.rs::prepare_speech_models_blocking` does TWO things:
  1. `check_transcription_model()` → on macOS `check_fluid_coreml_model()`
     which only **checks** for FluidAudio CoreML assets via
     `live_backchannel::resolved_live_model_dir` (looks in
     `~/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2`,
     asset gate = `has_coreml_assets`: `Preprocessor.mlmodelc`,
     `Encoder.mlmodelc`, `Decoder.mlmodelc`, `JointDecision.mlmodelc`,
     `parakeet_vocab.json`). **There is NO download of the transcription model
     anywhere in Rust — it only verifies presence.**
  2. `prepare_diarization_models_now()` → `PolyvoiceDiarization::from_default_registry()`
     which **does** download/validate the diarization models on first use
     (polyvoice `ModelRegistry::default()`).
- So today the UI button labeled "Check" cannot make transcription ready if the
  assets are absent; it dead-ends with "Install FluidAudio models or set
  MARGINS_FLUID_COREML_MODEL_DIR". `clear_speech_models_blocking` only clears the
  polyvoice cache and explicitly leaves FluidAudio assets in place.

### "Label speakers in single-track recordings" (Problem 4)
- Checkbox `#rust-diarization-enabled` ↔ `settings.rust_diarization_enabled`.
- Consumed in `desktop/src-tauri/src/transcription.rs:206–210`:
  `use_polyvoice = !single_speaker_requested && (force_diarize ||
  settings.rust_diarization_enabled || env MARGINS_POLYVOICE_DIARIZE==1)`.
- Meaning: aside normally captures **stereo** (mic on one channel, system audio
  on the other) so speakers are separated by channel with no model. A
  "single-track" recording (mic-only / no system tap, or a mono import) can't be
  split by channel, so a diarization model (polyvoice) is needed to label
  speakers. Imports already drive this via `force_diarize` + `import_speaker_count`
  (`max_speakers`). The checkbox is a global "always diarize" power toggle that
  overlaps conceptually with the import speaker count and is exposed with jargon
  the user has no mental model for.

---

## Transcription-model state machine (Problem 3 — enumerate + simplify)

Model the local-transcription setup as ONE required capability with a single
status enum. Collapse the two internal assets (FluidAudio CoreML transcription +
polyvoice diarization) behind one user-facing "Local transcription" state. Do
NOT surface asset names, "FluidAudio", "CoreML", "polyvoice", "parakeet", or
folder paths in the primary UI (keep a raw path override only under an Advanced
disclosure).

User-facing states:

| State | Trigger | Primary copy | Action |
|---|---|---|---|
| `required` (not installed) | assets absent, never downloaded | "Local transcription isn't installed yet" | **Download** (primary) |
| `downloading` | download in progress | "Downloading… NN%" + progress bar | **Cancel** |
| `ready` | transcription assets present (+ diarization ready) | "Local transcription is ready" | (none; optional "Remove") |
| `partial` | transcription present but diarization missing/failed | "Ready to transcribe; speaker labeling still downloading" → auto-continue or retry | **Retry** |
| `failed` | download/network error (not cancel) | plain-language error + "Try again" | **Download** (retry) |
| `canceled` | user canceled | "Download canceled" | **Download** |
| `unsupported` | build w/o `coreml-asr`/`parakeet-asr`, or non-mac path | "Not available on this build" | (disabled) |

Rules:
- Treat `ready` as the ONLY state that satisfies the required-setup gate.
  Readiness signal = transcription assets present (persist
  `settings.parakeet_model_dir`); diarization is best-effort and must not block
  the `ready` state (downgrade to `partial` only if actively needed).
- Progress: reuse the existing `speech-model-progress` event
  (`{stage,message,progress}`) and the existing `#speech-model-status`
  progress-bar renderer in `__prepareSpeechModels` (main.ts 4123–4134). Emit a
  smooth 0→1 `progress` across the actual download (bytes-based), not the current
  hardcoded 0.0/0.82/1.0 stages. Message copy must be plain-language ("Downloading
  local transcription…"), never asset filenames.

### REQUIRED backend change: real transcription-model download
`prepare_speech_models_blocking` must, when `check_transcription_model` reports
absent AND on the macOS `coreml-asr` build, **download** the FluidAudio CoreML
parakeet model set into the default dir
(`~/Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2`) with
byte-based progress and honoring the existing `cancel: Arc<AtomicBool>`.

Implementation notes for the downloader:
- Source: Hugging Face repo `FluidInference/parakeet-tdt-0.6b-v2-coreml`.
  Enumerate files at runtime via the tree API
  `https://huggingface.co/api/models/FluidInference/parakeet-tdt-0.6b-v2-coreml/tree/main?recursive=true`
  (do not hardcode the manifest; `.mlmodelc` are directories with nested files).
  Download each blob from
  `https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/resolve/main/<path>`.
- Download into a temp dir, verify `has_coreml_assets` equivalent, then
  atomically rename into place so a canceled/failed run never leaves a
  half-populated model dir that `has_coreml_assets` would falsely accept.
- Reuse whatever HTTP client is already a dependency (check `Cargo.toml`; prefer
  `reqwest` blocking or `ureq` if already present) rather than adding a new dep.
- Emit `speech-model-progress` with real fractional progress (sum of bytes).
- Respect `cancel` between/within file downloads; on cancel, remove the temp dir
  and return the existing "Model preparation canceled." error.
- After success, set `parakeet_ready=true` and `parakeet_model_dir=<default dir>`
  in `SpeechModelPrepResult` (unchanged serde contract; the frontend already
  persists `parakeet_model_dir`).
- Keep the non-mac `parakeet-asr` and no-backend `#[cfg]` branches behaving as
  `unsupported`/existing messages. Only the macOS `coreml-asr` path gains the
  downloader.

FALLBACK (only if the HF download proves infeasible within the timebox): keep
verification semantics but the UI must NOT claim "Download"; instead show the
`unsupported`/`required` state with a truthful message and a link action. Do not
ship a button labeled "Download" that only checks. State clearly in the PR which
path was taken.

`clear_speech_models_blocking`: make "Remove" also remove the downloaded
FluidAudio model dir (currently it only clears polyvoice and leaves FluidAudio),
so Remove ↔ Download are symmetric. Guard removal to the app-managed default dir
only; never delete a user-provided `MARGINS_FLUID_COREML_MODEL_DIR`/override path.

---

## Problem 1 — capture button routes to Audio settings (no inline prompt)

Goal: pressing "+" / New capture must never trigger inline macOS mic/system-audio
prompts. If audio isn't set up, route the user into Settings → Audio.

Change in `desktop/src/main.ts`:
- Add an audio-readiness gate to the capture entry point. In
  `startNamedMeeting` (1964), BEFORE `beginOptimisticRecording`, after the
  existing project/AI gate, check audio readiness:
  - Ready condition: `audioSetupReady()` is true AND local transcription is
    `ready` (`Boolean(settings.parakeet_model_dir)`), consistent with the
    Problem-3 required gate.
  - If not ready: do NOT start capture. Call `openSettingsSection("audio")`
    (opens the settings overlay on the Audio pane) and return. Optionally set a
    one-line banner state on the Audio pane ("Finish audio setup to start
    capturing") so the reason is obvious.
- Do this for every capture entry that reaches `startNamedMeeting`
  (`__startDefaultMeeting`, `__startNew`, `__startMeetingInProject`,
  `__startNewFrom`) — they all funnel through `startNamedMeeting`, so gating
  there covers all of them. Verify `__startMeetingInProject` (3814) still routes
  correctly.
- Keep the existing behavior where audio genuinely IS ready: capture starts as
  today. The recording-view fallbacks (`renderAudioHealth` retry/"Audio setup")
  remain for mid-capture drops; those are out of scope.

Note: the inline prompt originates from the Rust `startRecording` command
touching the mic/tap. Gating in the frontend before `startRecording` is the fix;
no Rust change is required for Problem 1. Do not remove the OS permission
requests from `startRecording` itself (they remain the mechanism used by the
Test buttons / real capture once the user is in Settings).

---

## Problem 2 — audio pane information hierarchy

Rewrite `renderAudioSettingsSection()` to this simplified hierarchy (top→bottom):

1. **Microphone** (source management with the capture-view +/- bottom-bar
   pattern):
   - Remove the entire "Audio readiness" helper card and BOTH **Open
     Microphone** and **Open System Audio** buttons. Rationale: testing triggers
     the needed permission prompts, so a separate "open privacy pane" action is
     redundant. (Keep `__openPrivacyPane` as a fallback only inside the
     blocked/restart callout described below — the system-audio "blocked" state
     still needs an escape hatch to System Settings + restart.)
   - Present the microphone as a source row using the **`.capture-speaker-control`
     bottom-bar aesthetic** the user likes (see
     `renderCaptureSpeakerControl` in `session-workspace.ts:365` and
     `.capture-speaker-control` CSS in `styles.css:4636`): a compact pill/bar
     with a **Test microphone** control and a live level meter, plus **+ / −**
     affordances to add/choose or clear the mic device (── "+" opens the device
     list to pick a mic, "−" reverts to System Default). Keep the underlying
     `input_device_name` setting and `__refreshDevices`/`__testAudioInput`
     wiring; only the presentation changes. There must be **microphone buttons
     only** — no separate "speaker"/system-audio Test button in this row.
   - Computer/system audio is NOT a user-managed source button here. It is
     captured automatically. Fold its status into the single Test action: when
     the user runs Test, exercise mic capture; system-audio readiness is
     established during real setup/Test. If the system-audio tap is `blocked`
     (`systemAudioTestResult.restart_recommended`), show the existing restart
     callout (Open System Settings + Restart Margins and check again) — this is
     the ONLY place `__openPrivacyPane('system-audio')` survives.
   - Net: eliminate the standalone "Test computer audio" card as a primary
     control; keep only the blocked→restart recovery affordance.

2. **Default speakers for imports** (keep — but restyle):
   - Replace the `<select>` (`#import-speaker-count`) with the +/- stepper
     pattern (reuse/generalize `renderCaptureSpeakerControl`; values Auto/1/2/3/4+
     mapping to `import_speaker_count` 0..4). Keep the hint about 4+ → up to 8.

3. **Local transcription** (from Problem 3): single required capability card with
   the state machine above; primary **Download** / progress / **Ready** / Remove.
   Move `#parakeet-model-dir` raw path override into an Advanced `<details>` and
   remove the "Label speakers…" checkbox (Problem 4).

Elevate the audio section from "Recommended" to **Required** badging where audio
+ local transcription block capture (see gating below). Update
`settingsSections()` audio `status` + the `requiredBadge(...)` tiers accordingly.

---

## Problem 4 — remove the "single-track diarization" toggle; auto-decide

- Remove the `#rust-diarization-enabled` checkbox and its label/hint from
  `renderSpeechSettingsSection` and the DOM read at main.ts:4425. Keep the
  `settings.rust_diarization_enabled` field for back-compat deserialization but
  stop exposing it in the UI.
- Speaker labeling becomes automatic and driven by the speaker-count intent the
  user already expresses:
  - Imports: unchanged — `force_diarize` + `import_speaker_count`/`max_speakers`.
  - Live single-track recordings (mic-only / no system tap): auto-enable
    diarization when the effective speaker count is Auto or >1 AND the recording
    lacks a usable second channel. Implement by deriving `use_polyvoice` in
    `transcription.rs` from the recording's track/channel reality + speaker-count
    hint instead of the standalone `rust_diarization_enabled` flag. Concretely:
    keep `force_diarize` and the `max_speakers==Some(1)` short-circuit; replace
    the `settings.rust_diarization_enabled` disjunct with a
    "single-track && multi-speaker-intent" condition. Preserve the
    `MARGINS_POLYVOICE_DIARIZE` env override as a debug escape hatch.
- If keeping the auto-derivation fully correct requires knowing whether a live
  capture was single-track, thread that from the existing tap/channel state
  (the recorder already knows mic-diarized vs stereo — see
  `live_transcription_mode === "mic_diarized"` in recording.ts and the tap
  status). Reviewer must confirm no behavior regression for normal stereo
  captures (they should NOT diarize).

---

## Copy rules (voicing) — MANDATORY

All user-facing copy in the touched settings/audio UI must follow the repo
voicing docs. Cite and obey:
- `desktop/UX_REVIEW.md` → "Copy rules" (~line 291): prefer OUTCOME language;
  one-time tasks read as one-time; errors are recoverable (what happened / your
  recording+memo are preserved / what to do next); long work shows progress
  (percent / stage / item / cancel); keep advanced/technical detail SECONDARY
  (behind Advanced/expandable), never in primary copy.
- `desktop/UX_QUALITY_SPEC.md` → per-principle copy Do/Don't lists (esp. the
  "operational plain copy" principle ~line 466/476 and "long-running work" ~252).

Hard don'ts in PRIMARY (non-advanced) copy: no implementation names or mechanism
detail — no "FluidAudio", "CoreML", "Parakeet/TDT/ONNX", "Polyvoice",
"diarization backend", "ASR", crate names, "Keychain", "OpenRouter", raw model
folders/paths, or provider/model IDs in the normal path. Use "session/memo/
distill/backchannel/Enzyme/Pi" only where already unavoidable; prefer
"capture / mark / transcript / evidence / note / local transcription /
speaker labels".

Do:
- Short, outcome-first labels: "Download speech models", "Local transcription is
  ready", "Label speakers", "Computer audio is captured automatically".
- One-time framing for the model download (it is a one-time setup, not a
  recurring check): the primary action reads as a one-time task; after success it
  reads as done ("Local transcription is ready"), not "Refresh/Check".
- Error copy that reassures the recording is safe and gives the next step, e.g.
  "Could not download speech models. Your recording setup is unchanged. Check
  your connection and try Download again."
- Progress copy that is plain-language and evidence-based (stage/percent), never
  a bare spinner >1s and never fabricated precision.

The user explicitly flagged existing settings copy as TOO LONG and TOO
IMPLEMENTATION-SPECIFIC. Sweep the copy in the audio + local-transcription UI
(and any adjacent settings strings you touch) to be shorter and outcome-focused;
move unavoidable technical detail (raw model-folder override, base URL, provider
keys) into the existing Advanced `<details>` disclosures. Do not lengthen hints.

Validate copy against the screenshots: the `settings-audio*.txt` visible-text
dumps must contain no mechanism names in primary copy.

## Acceptance / validation

- `settings-audio` screenshot: no "Open Microphone"/"Open System Audio" buttons;
  a mic source row with +/- bottom-bar styling and a single Test + meter; imports
  speaker stepper; one "Local transcription — Required" card with a Download
  action; no "Label speakers in single-track recordings" checkbox.
- `settings-audio-blocked`: system-audio blocked still surfaces the restart
  callout (Open System Settings + Restart).
- `settings-audio-ready`: mic Ready + local transcription Ready; audio nav dot
  green.
- New-capture "+" with audio not ready opens Settings → Audio and does NOT start
  an optimistic recording / does NOT fire OS prompts. With audio ready, capture
  starts as before.
- `cargo test` passes (esp. `audio_devices` classify tests); `cargo check` and
  `npm run build` clean. If the downloader is added, add at least a unit test for
  URL/manifest construction and the temp-dir→atomic-rename guard.
- Preserve unrelated dirty-worktree changes; keep `src/lib/tauri.ts` a thin
  facade; put scenario changes in `desktop/test-harness/mock-tauri.ts` if new
  states need fixtures.
