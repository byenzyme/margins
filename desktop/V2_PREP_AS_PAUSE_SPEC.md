# Implementation Spec — Margins v2: "prep as pause" (clock unification)

(Working spec, generated 2026-08-05. Delete after implementation lands. Source of truth for design rationale: obsidian/inbox/"2026-08-05-16-41-11 margins prep sketch hydration v1 design.md".)

## Interface contract (the boundary between the two packages)

These JSON shapes are frozen. The Rust package produces them; the TS package consumes them. Neither package may change them unilaterally.

### `MemoLine` (Rust `lib.rs:339` / TS `tauri.ts:236`) — gains block semantics

```jsonc
{
  "text": "string",
  "created_secs": 12.5,          // f64/number — meaningful ONLY when block_ordinal is null
  "edited_secs": 3.0,            // f64|null
  "draft_started_secs": 1.0,     // f64|null (existing)
  "audio_pending_at_mark": false,// bool (existing)
  "block_ordinal": 0             // NEW: u32|null. null = timed (clock was running). 0 = prep block. N = pause block after timeline segment N.
}
```

Rule (invariant, enforced in both packages): a line has `block_ordinal != null` **iff** it was created while the clock was stopped. When `block_ordinal != null`, `created_secs` is not a timeline mark and must be ignored by alignment/gutter-time rendering. Serde: `#[serde(default)] block_ordinal: Option<u32>` (defaults to `null` so existing persisted memos deserialize as timed).

### `hydrate_prep_sketch` command — gains block scoping + pulled exclusions + mid-meeting context

Invoke args (camelCase from TS, snake in Rust param names):

```jsonc
{
  "lines": [MemoLine, ...],          // ONLY the current clock-stopped block's committed lines
  "sessionName": "string",
  "people": ["string", ...],
  "eventTitle": "string|null",
  "blockOrdinal": 0,                  // NEW u32: which clock-stopped block this hydration is for
  "pulledTexts": ["string", ...],    // NEW: verbatim texts already pulled into the body this block; "don't re-offer"
  "meetingSoFar": "string|null"      // NEW: positioned prior-blocks/timeline context for mid-meeting (block_ordinal>0); null for prep block 0
}
```

### `prep-hydration` event — gains `block_ordinal`

```jsonc
{
  "session_name": "string",
  "block_ordinal": 0,               // NEW u32: echoes the request so the FE routes to the right block region
  "state": "hydrated|thin|quiet|warming|unavailable|error",
  "posture": "string|null",
  "marginalia": [
    { "anchor": "string|null", "kind": "carried|blind_spot|sharpen|counterevidence|context", "text": "string", "sources": ["string"] }
  ],
  "error": "string|null"
}
```

### `steer_prep_hydration` command — gains block scoping

```jsonc
{ "sessionName": "string", "blockOrdinal": 0, "instruction": "string" }
```

### Settings flag (Rust `Settings` struct → TS `Settings` type)

```jsonc
{ "auto_start_from_calendar": true }   // NEW bool, default true. Disables auto-start countdown when false.
```

Nothing else crosses the boundary. Auto-start countdown, postures, footer copy, block styling, content-hash identity, sketch-anchor ordering, compaction — all live entirely in the TS package.

---

## WORK PACKAGE 1 — Rust (`desktop/src-tauri/src` only)

### R1. `MemoLine` block field
- **`lib.rs:339`** add `#[serde(default)] block_ordinal: Option<u32>` to `struct MemoLine`.
- **`lib.rs:5263`** test helper `memo()` — add `block_ordinal: None`.
- Anywhere `MemoLine { ... }` is constructed in Rust (grep `MemoLine {`), add `block_ordinal: None` unless it is a pause-block line.

### R2. Ungate memo commands ("session-exists")
Currently `sync_memo`, `checkpoint_memo_line`, `request_backchannel_for_memo`, `steer_backchannel_for_memo` all do `guard.as_mut().ok_or("Not recording")?` then `ensure_memo_session`. Prep/pause typing must persist without an active `RecordingState`.

- **`sync_memo` (`lib.rs:4035`)**: if `state.recording` is `None`, fall through to a **session-exists persist path**: resolve `work_dir` from `AppState.work_dir`, verify a session dir exists for `session_name` (or is the active prep session name), and call `persist_live_memo(&work_dir, &session_name, &lines)`. Do NOT error. When recording IS present, behavior unchanged (still calls `ensure_memo_session`).
- **`checkpoint_memo_line` (`lib.rs:4053`)**: when not recording, persist memo (`persist_live_memo`) and return `Ok(())` early — skip the transcript-checkpoint block (there is no live client). When a line has `block_ordinal != null`, also skip checkpoint even while recording (untimed lines are never transcript-anchored).
- **`request_backchannel_for_memo` (`lib.rs:4182`)**: unchanged gate — live cue is clock-running only. Add a guard: if the committed line's `block_ordinal != null`, return `Ok(())` (clock-stopped uses the prep lane, not the live backchannel).
- Add a small helper `fn ensure_session_writable(state, session_name) -> Result<PathBuf, String>` returning `work_dir` when either recording matches or the session exists on disk; use it in the not-recording branches.

### R3. Prep hydration request shape
- **`prep_ai.rs` `PrepAiRequest` (line 61)**: add
  ```rust
  pub(crate) block_ordinal: u32,
  pub(crate) pulled_texts: Vec<String>,
  pub(crate) meeting_so_far: Option<String>,
  ```
- **`PrepSnapshot` key change**: snapshots stored in `AppState.prep_snapshots: HashMap<String, PrepSnapshot>` keyed by `session_name`. Change key to composite `format!("{session_name}#{block_ordinal}")` so each clock-stopped block owns its own steer context. Add helper `fn prep_snapshot_key(session: &str, block: u32) -> String`. Update `hydrate_prep_sketch`, `steer_prep_hydration`.
- **`hydrate_prep_sketch` (`lib.rs:4316`)**: add params `block_ordinal: u32`, `pulled_texts: Vec<String>`, `meeting_so_far: Option<String>`. `sketch_text` is built from `lines` (FE scopes to current block). Populate the three new request fields. Store snapshot under composite key. Pass `block_ordinal` into `emit_prep_hydration`.
- **`emit_prep_hydration` / `emit_prep_hydration_error` (`lib.rs:4279`, `4300`)**: add `block_ordinal: u32` param, include in emitted JSON.
- **`steer_prep_hydration` (`lib.rs:4431`)**: add `block_ordinal: u32` param; composite key lookup; pass block_ordinal to emit.
- **dispatch.rs**: update arg structs for both commands (camelCase) with the new fields.

### R4. Pulled-line exclusion — `prep_ai.rs`
- **`build_hydration_prompt` (line 816)**: when `request.pulled_texts` non-empty, inject before the schema:
  ```
  Already in the user's notes (do NOT re-offer these; they are handled):
  - <pulled text 1>
  ```
- **`post_validate` (line 766)**: add `pulled_texts: &[String]` param. After existing filters, drop any marginalia whose `text` is a near-match to a pulled text. Near-match rule (deterministic): normalize both sides (lowercase, collapse whitespace, strip trailing punctuation); drop if normalized-equal OR either is a substring of the other with length ratio ≥ 0.6. Unit tests: exact match dropped, 60%+ substring dropped, unrelated kept.
- Thread `pulled_texts` through `run_prep_hydration_with_context` → `post_validate` call (line 748).

### R5. Mid-meeting context — `prep_ai.rs`
- **`build_hydration_prompt`**: when `request.meeting_so_far` is `Some`, insert after the frozen dossier/catalyze prefix and before the sketch tail (trigger-tail position keeps cache warm):
  ```
  Meeting so far (positioned context — the clock has been running; this is what has been discussed up to this pause):
  <meeting_so_far>
  ```

### R6. Distill capture_context: blocks as positioned context
- **`render_capture_context_markdown` (`lib.rs:6453`)** currently splits into sorted timeline + flat "Untimed memo / reflection lines" tail. Replace untimed tail with **positioned pause blocks**:
  - Extend `export_memo` to emit a block marker for `block_ordinal != null` lines: line prefix `[block N]` instead of `[MM:SS]`. `parse_memo_markdown_for_context` returns block lines grouped by ordinal.
  - Render block 0 → `## Prep (block 0)` section BEFORE the timeline; block N>0 → `### Pause block {N} (untimed)` subsection inserted in timeline output after the last timed line of segment N.
- Alignment exclusion: the memo→transcript alignment path (grep `align`, `seconds_to_ms(line.created_secs)` at `lib.rs:5095`, `5173`) must **skip** lines where `block_ordinal.is_some()`: filter `lines.iter().filter(|l| l.block_ordinal.is_none())` before the alignment map is built.

### R7. Slug fix — NO Rust change needed
Rust `slugify` (`lib.rs:7123`) already maps non-alphanumerics to `-`. The "/" loss is a TS bug (see T9). Do not touch.

### R8. Settings flag
- **`settings.rs`**: add `#[serde(default = "default_true")] pub auto_start_from_calendar: bool` to `Settings` (add `fn default_true() -> bool { true }` if absent). Serde handles get_settings automatically. No Rust consumer.

### R9. Rust verification checklist
1. cargo test passes; new post_validate near-match tests pass.
2. cargo check compiles.
3. With no active recording, `sync_memo` with an on-disk session's name writes the memo file (no "Not recording" error).
4. `hydrate_prep_sketch` with `pulledTexts:["..."]` drops near-match marginalia (inspect `_prep_trace.jsonl`).
5. `blockOrdinal:2, meetingSoFar:"..."` — trace `hydration_prompt` shows meeting-so-far block.
6. `prep-hydration` event payload contains `block_ordinal`.
7. Memo with block-0 + timed lines → capture-context markdown has `## Prep (block 0)` before timeline; alignment has no anchor for block-0 lines.
8. `get_settings` returns `auto_start_from_calendar: true` by default.

---

## WORK PACKAGE 2 — TS/CSS (`desktop/src` + `desktop/test-harness/mock-tauri.ts`)

### Core reframe: one view, two postures
Delete the separate `prep` view/branch. The capture surface is the single surface. Clock state:
- `clockRunning` ⇔ `recordingStatus.is_recording && !recordingStatus.paused`.
- `clockStopped` ⇔ `!clockRunning` (pre-start prep at 0:00, mid-meeting pause, post-meeting-before-finish).

Module state:
```ts
let currentBlockOrdinal = 0;          // 0 before first start; set on each pause
let armedFromCalendar = false;
let autoStartDeadline: number | null = null;
let autoStartTimer: ReturnType<typeof setInterval> | null = null;
```

### T1. MemoLine block field + gutter rendering
- **`tauri.ts:236`**: add `block_ordinal?: number | null` to `MemoLine`.
- **`memo-editor.ts` `commitPendingLine` (372)**: stamp new lines: clock stopped → `block_ordinal: currentBlockOrdinal`; running → `null`. Add `getClockStopped: () => boolean` and `getCurrentBlockOrdinal: () => number` to `MemoEditorContext`; wire from main.ts.
- **`recording.ts` `renderMemoLineInner` (333)**: `block_ordinal != null` → gutter renders pause glyph (`⏸`, class `memo-gutter block-gutter`) instead of `formatElapsed`; row classes `memo-line-block` (+ `memo-line-block-prep` for ordinal 0). No wall-clock label for ordinal 0; ordinal > 0 gets `<span class="memo-block-label">paused · {wallClock}</span>` (wall clock captured at block entry, T5).
- **`memo-editor.ts`** pending gutter (`appendNewMemoInput` 310, `updateRecordingHeader` 267): clock stopped → pause glyph, no ticking. Guard `updateRecordingHeader`'s pending-gutter update with `if (!clockStopped)`.
- **`memo-editor.ts` input handler (322)**: clock stopped → don't set `pendingLineStartSecs`.

### T2. Ungate editing while paused
- Paused recording view still renders full editor with pending input editable (verify `renderRecording` doesn't lock — it doesn't).
- On commit while clock stopped: route to `syncMemo(memoLines, sessionName)` persistence; do NOT call `checkpointMemoLine`/`requestBackchannelForMemo`.

### T3. Pause footer one-liner
- **`recording.ts`** footer: when paused, single line: `Paused — notes still land, untimed · <button __resumeRecording>Resume</button>`. This replaces the stop/discard cluster as primary in the paused posture (keep Stop/discard reachable only if already present as overflow).

### T4. Auto-start countdown
- New entry `window.__openCaptureFromCalendar()` replaces the calendar-card Prep/Start-now split: opens capture surface clock-stopped at 0:00, pre-armed, not recording. Seeds `prepEventTitle`, `prepPeople`, `currentBlockOrdinal = 0`, `armedFromCalendar = true`; `currentView = "recording"` with `recordingStatus.is_recording = false`.
- Countdown: if `armedFromCalendar && settings.auto_start_from_calendar && calendarSuggestion?.start` future → `autoStartDeadline = Date.parse(start)`, 1s interval timer. Footer (clock-stopped, armed): `Starts when the meeting does · {M:SS} · <Start now> · <hold off>`.
  - Start now → `beginOptimisticRecordingFromPrep(...)`.
  - hold off → clear deadline + `armedFromCalendar = false`; footer reverts to manual `Start`.
  - Timer at 0 → same path as Start now. Never start without visible countdown.
- Timer lives at module scope; if it fires while user is elsewhere, navigate to capture view then begin. Check `!recordingStatus.is_recording` before firing.
- Blank capture: no countdown, manual Start only.
- Settings: add `auto_start_from_calendar` to TS `Settings` type + a checkbox "Auto-start captures at event time" in settings UI. False → no countdown.

### T5. Pause/resume → block ordinal
- **`__pauseRecording` (4897)**: after successful pause, set `currentBlockOrdinal` = number of completed timeline segments; record wall-clock in `blockWallClockByOrdinal: Map<number,string>`. Editor keeps working; new lines get this ordinal.
- **`__resumeRecording` (4929)**: compaction — un-pulled fragments of the block being left compact to single quiet frozen lines (per-block archived region; never rewritten). Clear live margin for that block. Clock running → new lines timed.
- First start from prep (block 0 → running): same compaction at `beginOptimisticRecordingFromPrep`.

### T6. Hydrate affordance replaces auto-on-settle
- Delete `schedulePrepSettle`, `prepSettleTimer`, `prepAutoHydrationFired` auto-fire (1895–1907; call site 2097).
- Visible margin affordance whenever `clockStopped && committedLinesInCurrentBlock ≥ 1`: label `Look over this` (no hydration yet for this block) / `Look again` (after). Never rendered while clock running. Click → `firePrepHydration()` for current block.
- Cmd+Enter-on-empty-pending alias (1883): generalize gate from `prepPhase` to `clockStopped`.

### T7. Block-scoped margin regions + supersession
Replace flat `prepMarginalia` / index-keyed sets:
```ts
type BlockMargin = {
  marginalia: PrepHydrationMarginalia[];
  state: PrepHydrationEvent["state"] | null;
  consumed: Set<string>;   // content-hash keys
  expanded: Set<string>;   // content-hash keys
  pulledTexts: string[];
};
let blockMargins = new Map<number, BlockMargin>();
```
- Content-hash identity: key consumed/expanded by FNV-1a-style hash of `anchor + " " + text`. Replace all index-keyed usages.
- Full supersession: on `prep-hydration` event, REPLACE that block's marginalia wholesale.
- Sketch-anchor order: stable-sort marginalia by index of `anchor` within current block's committed text (null anchors last, original order among ties).
- Pull: `__pullPrepMarginalia` adds text to block's `pulledTexts`, marks content-hash consumed, inserts as pending line (render-first, then set `#memo-input-new` value/focus — keep the v1 fix). `pulledTexts` rides next hydrate/steer request.
- Per-block regions: `prepMarginaliaHydrationRailHtml` → `blockMarginRailHtml(ordinal)`. Live region = current block; earlier blocks render compacted frozen lines, read-only.

### T8. Call sites + event routing
- `firePrepHydration`: lines = committed lines with `block_ordinal === currentBlockOrdinal`; `blockOrdinal`; `pulledTexts` from block state; `meetingSoFar` = for ordinal > 0 the rendered timeline-so-far (timed memo lines + any live transcript text) up to this pause, else null.
- `tauri.ts hydratePrepSketch(lines, sessionName, people, eventTitle, blockOrdinal, pulledTexts, meetingSoFar)`; mirror http-backend + mock.
- `steerPrepHydration(sessionName, blockOrdinal, instruction)`.
- `PrepHydrationEvent` type: add `block_ordinal: number`.
- `onPrepHydration` handler (7119): route by `event.block_ordinal` into `blockMargins`; replace wholesale; re-render if capture view showing. Gate on session name match (works for both armed-prep and recording).

### T9. Slug fix
Three inline name builders in main.ts (1929, 1982, 3143) use `.replace(/\s+/g,"-").replace(/[^a-z0-9-]/g,"")` which drops `/`. Extract shared `slugifyName(name)`: `name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 90)`. Use at all three sites. `Personalization/CNI Advising` → `personalization-cni-advising`.

### T10. Delete v1 prep-view scaffolding
- Remove `currentView === "prep"` branch (886), `renderPrepPhase` (1831), `__enterPrepPhase`/`__cancelPrepPhase` prep-view chrome — fold seeding into `__openCaptureFromCalendar` + clock-stopped capture surface. Keep `beginOptimisticRecordingFromPrep`/`startNamedMeetingFromPrep` (seed path still used at first-start).
- Calendar card (`renderHomeIntro`/sidebar): single action → `__openCaptureFromCalendar`.

### T11. Harness mock (`mock-tauri.ts`)
- `hydratePrepSketch`: accept new args; echo `block_ordinal` in emitted event; drop mock marginalia whose text appears in `pulledTexts`.
- `steerPrepHydration`: accept `blockOrdinal`.
- Mock pause/resume statuses let FE derive clock state; mock settings include `auto_start_from_calendar: true`.

### T12. Verification checklist (computer-use tester)
1. Open capture from calendar card → 0:00, clock-stopped, pause glyph in pending gutter, footer `Starts when the meeting does · M:SS · Start now · hold off`.
2. Countdown decrements; `hold off` → countdown gone, manual Start.
3. Countdown reaching 0 → auto-start; lines now get ticking timestamps.
4. Type ≥1 line clock-stopped → `Look over this` appears; click → fragments with pull affordances; after first run label = `Look again`; re-run fully replaces (no accumulation).
5. Pull a fragment → lands as editable pending line; `Look again` → pulled text NOT re-offered.
6. Mute then `Look again` → new fragments don't inherit muted state.
7. Record, type timed lines, pause → editor editable, pause glyph, footer one-liner, pause lines set off with `paused · H:MM PM` label.
8. `Look over this` available during pause; resume → block's unpulled fragments compact frozen.
9. Prep block lines never show wall-clock label; mid-meeting blocks do.
10. "Personalization/CNI Advising" → `personalization-cni-advising`.
11. Settings flag off → no countdown, manual Start.
12. typecheck/build passes.

### Edge cases (both packages)
- Hydration mid-flight at clock start: cancel/ignore in-flight block-0 hydration (drop pending warming); keep rendered fragments; compact. FE guards `onPrepHydration` to ignore block-0 events after recording started.
- Pause during startup: `__pauseRecording` blocks while `recordingStartup?.phase === "starting"`; don't bump ordinal until pause succeeds.
- Event fires while holding off: timer cleared on hold-off; callback re-checks `armedFromCalendar && settings.auto_start_from_calendar`.
- Resume after real pause (ordinal > 0): no countdown ever; Resume is manual.
