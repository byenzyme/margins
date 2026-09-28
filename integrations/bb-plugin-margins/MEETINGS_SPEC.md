# Margins for bb: Meetings, memo pads, and distill-to-thread

Status: implemented through the no-LLM E2E lane (2026-09-25). The real-LLM
distillation and independent review steps require separate authorization.
Builds on the panel/overlay/notepad already on this branch.

## Goal

The bb plugin should feel like Granola: while a meeting runs you see one quiet
recording status, a subtle live audio level, pause and stop, and a blank memo
pad — nothing else. After the meeting you can refine the memo, then distill it
into a connected note through a new bb thread. The note lands in the configured
Margins Workspace.

## Ownership model

The meeting, not the thread, is the primary object.

| Object | Owns | Does not own |
|---|---|---|
| Meeting (Margins session in the resolved Workspace) | audio, transcript, memo pad, lifecycle (live → ended → distilled) | any thread |
| Recorder (browser content script or Mac bridge) | the single live audio stream on that device | the memo pad |
| Meetings page selection | which memo pad is on screen | any stored state |
| Thread | one distillation conversation that references a meeting | the recording |
| Plugin storage | routing and heartbeat facts only (unchanged) | audio, transcript, memo bodies |

Consequences:

- Recording state is keyed by `sessionId`, not `threadId`. Remove
  `browserCaptureOwner.threadId` / `nativeBridgeOwner.threadId` as the control
  key and the "paired to another bb thread" message. Controls work from
  anywhere.
- Changing threads never affects capture or which meeting is live.
- A meeting has 0..n distillation threads. A thread never controls capture.

## Surfaces

### Left sidebar row: Meetings (`app.slots.navPanel`)

- Row title "Meetings", icon `Mic`.
- `experimental_sidebarAccessory`: a tiny level meter/waveform driven by the
  live input level while recording; a static paused glyph while paused; empty
  otherwise. Presentational only, no controls.
- Clicking the row opens the Meetings page with:
  1. the live meeting's memo pad, if one is recording or paused;
  2. otherwise the most recent ended, undistilled meeting;
  3. otherwise an empty state with Start.

### Meetings page

- Memo pad is the main area: a full, blank, editable page. No transcript, host
  picker, elapsed-time dashboard, sample counts, or processing controls.
- A compact meeting list beside it, grouped:
  - **Live** (at most one per recorder, pinned first, clearly labelled);
  - **Ready to refine** (ended, not distilled);
  - **Distilled** (links to note and thread(s)).
  - **Archived** (collapsed by default; hidden from the recent groups until opened).
- Selecting a meeting switches memo pads. Doing so while recording does not stop
  the recording; the live pad keeps its "Live" header so the user cannot confuse
  which pad they are typing into.
- Memo pads stay editable after the meeting ends (refinement). Writes stay
  revisioned as today.
- The persistent recording overlay is the only Pause/Stop control set. The
  Meetings page shows the live status and memo without duplicating those
  buttons. Start is hidden while a meeting is live and the empty state has its
  own single Start action.
- Manual Mac pairing and recording diagnostics move to plugin settings.

### Completed meeting controls

- The title can be renamed. A quiet details line shows saved duration, audio
  source, Workspace, and the bb project where capture began. Project is
  provenance; all projects resolving to the same Workspace see its meetings.
- Keep the transcript status and View/Transcribe/Retry with **Make note →** in
  one action area below the memo. The note opens as a new thread in the current
  bb project when one is selected; otherwise say "Choose project in composer"
  and let the composer ask. The recording's starting project remains
  provenance and does not force the distillation thread there.
- **More** contains Archive and Discard. Archive is reversible and hides the
  meeting from the recent groups without changing its source data. Discard
  requires explicit confirmation and permanently removes the recording,
  transcript, and memo through the Workspace service. A linked note or bb
  thread remains, and confirmation warns that its source link will break.
- A new meeting opens its memo pad immediately, even while audio is preparing.
  Show the selected audio source, Workspace, and starting project there. The
  recording overlay remains the sole Pause/Stop control set.

### Overlay (`experimental_appOverlay`, exists)

- Red dot, "Recording 12:04" / "Paused", pause/resume, stop. Nothing else.
- Clicking the label opens the Meetings page on the live pad (not a thread).

### Thread panel tab "Margins" (`threadPanelAction`, exists, repurposed)

- In a distillation thread: shows that thread's source meeting memo pad
  (read/edit) and a link to the produced note.
- Elsewhere: shortcut to the live memo pad, or a link to the Meetings page.

## Starting a second meeting

Only one live capture per recorder. Start while a meeting is live prompts
"Stop and save the current meeting, then start?" — never two streams from one
device. Multiple *ended* meetings awaiting refinement/distillation are normal
and each has its own memo pad and threads. Concurrent live capture from two
devices is out of scope.

## Capture choices

Keep channel count, downmixing, and speaker count out of the meeting controls.
Browser capture has one microphone stream; native capture retains separate mic
and computer-audio channels. Speaker labeling is a transcription decision for
audio where channel separation does not identify speakers. If automatic labels
need correction, offer that choice during transcript refinement rather than
before recording.

## Menu-first recording on a Mac

Evidence (2026-09-26 live run): on a Mac, Meetings **Start** always used the
browser microphone. Helium raised no permission prompt and delivered 77 s of
digital silence, so the meeting saved with an empty transcript. The user
expected bb to be the UI for Margins Menu. Margins Menu can only join bb through
**Connected Workspace** after typing an SSH alias/URL and Workspace, and bb's
own Start path needs a hidden manual pairing code.

Principle: **bb is the screen; Margins Menu is the recorder; the Workspace is
the store.** When a Mac recorder is available, it records (mic + computer
audio, macOS permissions owned by the signed Menu app). The browser microphone
is a labelled fallback only.

### Topologies (both must work with the same UX)

| | bb server / project host | Browser | Recorder | Where audio goes |
|---|---|---|---|---|
| A. Remote | Linux server (e.g. jpham-server) | Helium on the Mac | Margins Menu on the Mac | Menu streams to the Workspace service on the project host |
| B. Local | bb running on the Mac | Browser/app on the same Mac | Margins Menu on the same Mac | Menu records into the same Workspace service on this Mac |
| C. No recorder | any | phone / other computer / Menu absent | browser microphone | project host Workspace service (today's path) |

The difference between A and B is only the destination URL and credential the
recorder receives. The recorder protocol, the pairing flow, and the bb UI are
identical. In B, do not run a second, competing capture store: the plugin and
the Menu must resolve the same machine Margins home and Workspace, and one
runtime owns capture.

### Pairing: the destination travels with the pairing

- The Meetings page, when opened in a browser on a Mac, probes the Menu's
  loopback bridge (exact-origin allowlist; handle Chrome Private Network Access
  preflight for an HTTPS bb origin calling 127.0.0.1).
- If found and not yet paired with this bb project's Workspace, show one quiet
  line: "Record with Margins Menu (mic + computer audio)" → **Connect**.
- **Connect** asks the plugin backend for a scoped, revocable capture grant for
  the resolved Workspace (Workspace id, service URL reachable from the Mac, and
  a short-lived token bound to that Workspace) and hands it to the Menu over the
  loopback bridge. The user types nothing: no SSH alias, URL, Workspace id, or
  pairing code.
  - A: the service URL must be reachable from the Mac without SSH config. Prefer
    an authenticated plugin HTTP route on the bb origin the browser already
    uses (proxy to the project-host margins-server), if bb plugin routes can
    carry the capture upload/stream; otherwise report the smallest honest
    alternative (e.g. `bb connect expose`) before building it.
  - B: the service URL is loopback on the same Mac.
- The Menu stores the grant, shows the connected Workspace name, and can
  **Disconnect**. Grants are revocable from bb settings.
- The typed Connected Workspace fields in the Menu remain only as an advanced
  fallback.

### Recording

- bb **Start** → the Menu starts recording (Menu bar icon turns red); bb shows
  the same pill, live pad, and level dot, driven by the Menu's reported mic
  level.
- Margins Menu offers an idle-only **Microphone** picker with the system default
  and available inputs. Its choice persists on this Mac and applies to the next
  connected Workspace recording without changing the macOS default. bb shows
  the selected microphone before Start and names it in a no-audio warning. An
  unavailable pinned input must be visible and must block Start with a clear
  instruction to choose another input; do not silently fall back to a different
  microphone. The Menu still owns this device setting in topology A.
- Starting from the Menu → bb shows that meeting live automatically.
- Pause/Resume/Stop work from either surface; one meeting, two controls.
- On stop, the Menu finalizes into the Workspace; bb shows **Ready to refine**
  and the transcript status.
- No Mac recorder reachable → Start asks once: "Record with this browser's
  microphone only?" and the pill reads "Microphone only".

### No-audio guard (all recorders)

If no non-silent audio arrives within ~3 s of Start (or after resume), the pill
shows **"No audio — check microphone"** with the likely fix for that recorder
(macOS Privacy → Microphone for the browser; Menu permissions for the Menu). A
meeting must never silently record only zeros.

### Verification

- A: Helium on the Mac against jpham-server: Connect → Start from bb →
  speak → Stop → transcript contains the spoken words; also start from the Menu
  and confirm bb joins; exactly one runtime; closing check clean.
- B: bb running on the Mac: same journey, one capture store, same Workspace as
  the Menu.
- C: phone/browser fallback labelled Microphone only; no-audio guard fires on a
  muted input.
- Follow AGENTS.md "Leave no capture running unannounced" for every run.

## Runaway capture guards

Review evidence from the Mac: Margins Menu left `meeting-7` recording for about
23 hours unattended, consuming 233% CPU and 17.7 GiB in temporary `.f32` audio
lanes while the disk reached 99% full. A second orphaned `margins-live` runtime
used about 55% CPU without a session. Capture needs these guards:

- Warn before an automatic stop for prolonged silence and before a hard maximum
  recording duration. If the user does not resume or extend capture, stop and
  finalize the session through the normal recording lifecycle.
- Check free disk space before starting and periodically during capture,
  including the growth of temporary audio lanes. Refuse a start with too little
  space and stop an active capture with enough reserve to finalize safely,
  before the disk is exhausted.
- Margins Menu must reap any previous capture runtime, including an orphan with
  no live session, before spawning a new runtime. A recorder must not retain two
  competing runtimes after restart or recovery.
- A stale session marked active whose audio files are missing must have an
  explicit recovery or discard path. It must not remain permanently live in
  the UI or block a new capture; preserve the reason for the discard.

## Distill to note

On an ended meeting: **Make note in new thread →**. After a linked note's
memo revision changes: **Update note in new thread →**.

- Seeds the shared new-thread composer with `useComposer().setText(...)` and a
  meeting mention pill via `insertMention(...)`, then calls
  `navigate.toCompose({ focusPrompt: true })` in the current bb project. The user
  presses Enter. No backend `threads.spawn`: the
  prefilled composer keeps the user's on-screen provider/model/permission
  settings.
- The visible draft is a plain request plus the meeting pill. At send time, the
  pill resolves to agent-only `<margins-context-v1>` JSON carrying `sessionId`,
  Workspace id, pinned memo revision, bb project id, note action, and current
  transcription status. The visible draft never includes ids or server errors.
- Opening a composer does not freeze or mark the memo distilled. The new thread
  uses a saved memo revision as its input; the memo remains editable. Once a
  note is associated, a later memo edit offers "Update note in new thread".
  The note and thread appear in the Distilled meeting's links.
- The distill skill resolves the destination from the Workspace (below), writes
  the note, and records the thread ↔ meeting link and the distilled memo
  revision on the Margins session so the Meetings list can show
  "Distilled → note · thread". Memo edits after that revision show as
  "edited since distillation" and can be distilled again deliberately.
- If the transcript is not ready, keep the existing `transcribePinnedSession`
  path before prefilling.

## Workspace resolution and note destination

Destination and meeting storage both come from a Margins Workspace, not from
the bb project folder.

Resolution order for a bb project:

1. **Per-project setting** (optional): "Margins Workspace for this project",
   stored in plugin project settings. Replaces the env-only
   `MARGINS_BB_REMOTE_WORKSPACE` as the user-facing control.
2. **Global default Workspace**: one new setting in machine-level Margins config
   (alongside model selection).
3. Neither set: the panel asks the user to choose or set up a Workspace
   (workspace-setup skill); it does not silently fall back to `<project>/.margins`.

Within the Workspace:

- The `Home` Source is the write boundary. Add an optional `note_folder`
  (relative to the Home root, e.g. `inbox`) to the Home binding, changed only
  through the normal `workspace plan --desired` → reviewed `apply` path.
- The distill skill asks `margins` for the Home root and `note_folder` instead of
  walking up from `.margins/` (`skills/margins/SKILL.md`, "Where the note
  lands"). Unset `note_folder` = Home root (today's behavior).
- Meetings (audio, transcript, memo) are stored in the Workspace's capture
  store, so every bb project resolving to the same Workspace sees the same
  Meetings list.

For this user: default Workspace = the Obsidian vault with `note_folder =
"inbox"`, so meetings from any code project land in `/workspace/obsidian/inbox/`
unless that project overrides its Workspace.

## End-to-end test

Headless on the VPS using `desktop/REMOTE_HEADLESS_DEV.md`, driving bb web with
agent-browser, with full video and per-step screenshots:

1. Chrome with `--use-fake-device-for-media-stream
   --use-file-for-fake-audio-capture=<meeting.wav>`; a disposable
   `MARGINS_HOME` whose global default Workspace has Home = a vault fixture copy
   and `note_folder = "inbox"`; bb project on a code folder with no override.
2. Start. Assert: overlay shows Recording; sidebar accessory level is nonzero and
   changing; no controls beyond pause/stop.
3. Type in memo pad; pause; resume; stop. Assert meeting moves to Ready to refine.
4. Switch to another thread and back mid-recording. Assert capture and memo pad
   are unaffected and the Meetings row still opens the live pad.
5. Edit the memo after stop; assert revision saved.
6. Distill to note. Assert a new-thread composer opens in the project with the
   prompt (session id, Workspace id, memo revision) and nothing is sent.
7. Send (real LLM — requires explicit approval per run). Assert a note appears
   under `<vault>/inbox/`, the meeting shows Distilled with note + thread links,
   and the thread's Margins tab shows the source memo.
8. Hand artifacts to an independent reviewer for the Granola-simplicity judgment.

Steps 1–6 run without model spend and should be the CI-able lane.

## Implementation slices

1. Workspace resolution: global default Workspace in machine config; per-project
   plugin setting; `note_folder` on Home binding via plan/apply; CLI read of
   resolved destination; distill skill uses it.
2. Re-key capture ownership by `sessionId`; store sessions in the resolved
   Workspace.
3. Meetings `navPanel` page + sidebar level accessory + list/pad switching;
   slim overlay; move diagnostics/pairing to settings.
4. Distill to note via `toCompose`; session ↔ thread link and distilled revision;
   repurposed thread tab.
5. E2E harness lanes (no-LLM lane, then approved LLM lane).

## Implementation notes

- This bb SDK does not expose project-scoped plugin settings. The optional
  project Workspace choice is stored in plugin KV under the bb project id and
  edited from the Meetings page and plugin settings.
- `navigate.toCompose` has no project argument. The draft names the bb project;
  users select that project in bb's composer before sending. The no-LLM runner
  verifies this picker step without submitting the prompt.
- Browser microphone recording drives the changing sidebar input meter. The
  native bridge currently exposes sample counts but no audio amplitude; its
  accessory uses a static activity indication until the bridge reports a
  measured level.
- Hosted browser capture finalizes a local WAV without a remote producer
  authority record. Its transcription request uses the headless WAV ASR path;
  a memo-only terminal live checkpoint does not count as a transcript. The
  spoken-audio harness requires an actual Parakeet timeline line.
- `test-harness/no-llm.mjs` covers steps 1–6 in an isolated bb instance. Step 7
  and the independent step 8 review were not run.
