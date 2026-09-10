# Margins UX Quality Spec

**Status:** v0.4 applied product philosophy - workshop in progress.
**Relationship to other docs:**

- `.pi/reports/design-north-star.md` - *taste*: the aspirational craft bar and 12 principles. Read it for the "why."
- `desktop/UX_REVIEW.md` - *method*: falsifiability-first review, personas, copy rules, state checklist. The human-review depth lives here.
- **This file** - *spec*: the reconciled product philosophy and scoreable principle set that guides design before building and evaluates it afterward. When docs disagree, **this file wins** and the others should be updated to match.

This spec exists because the north star and UX_REVIEW were written before the last few threads, and those threads settled arguments by taste that the docs never recorded. The most important: the streaming-note **percentage was removed in favor of skeletons because the number was long-running and inaccurate** - a progress cue that lied. UX_REVIEW still mandates "percent / stage / file." This spec records the resolution in **C3 - Honest signals only** and the judge enforces it.

---

## How the two layers work together

Every principle below is both generative and evaluative:

1. **Falsifiable claim (human layer).** A reviewer asserts what a real user can perceive, decide, or do from evidence across the whole experience - flow video, interaction timing, visible copy, generated note, transcript, provenance, and screenshots - in the form UX_REVIEW already teaches:
   > I believe **[persona]** can **[task]** under **[constraint]** because **[evidence artifact]**. False if **[counter-evidence]**.
2. **Scored rubric (judge layer).** A 0-3 score with observable signals per level, so the UX loop judge and a human can score reproducibly and track regressions across runs. The surface screenshot is only one artifact. Flow, microinteraction, copy, transcript/output quality, and provenance evidence are equally admissible. Each principle also has a **trip-wire**: a single observable that is an automatic failure regardless of everything else.

Scoring: **0 = violates / 1 = weak / 2 = solid / 3 = exemplary.**

The spec is organized into three rings:

- **Framing** is the load-bearing claim the whole spec serves. It is always consulted when a change alters the product promise, note shape, live assistance, or post-meeting review.
- **Core principles** are always scored on every touched screen or flow. Every Core principle must score **>=2** and be trip-wire-free.
- **Extended principles** are scored when a change touches that surface, behavior, or output. When applicable, every Extended principle must score **>=2** and be trip-wire-free.

Any trip-wire firing fails the run regardless of ring.

---

## Governing Doctrine

Margins exists to preserve the **live edge**, not the record: what caught your attention, the question you held internally, the opening you missed. The bottleneck is not writing; the bottleneck is noticing the question you should have asked while the conversation was still alive.

Receive before you evaluate. Hospitality is the posture before craft: "Hospitality requires a different lens — one that receives before it evaluates." Craft asks, "what can this become?" Hospitality asks, "what is this person bringing that I haven't made room for yet?" The craft reflex is a temptation "to fall out of relationship with humans."

Protect presence. The app should make the user more present in the room, not more occupied with the tool. It should help the user ask the better question, then return attention to the human encounter.

Donkey, not throne. Margins carries the story toward the human encounter, then gets out of the way. "Access without formation" is failure; the vault can otherwise absorb "what action should hold." The output should not be a polished substitute that ends engagement.

Evidence anchors: vault `inbox/2026-06-22 margins live articulation content directions.md` — "The bottleneck is not writing. The bottleneck is noticing the question you should have asked while the conversation was still alive." / "Margins should preserve the live edge… what question you held internally… what opening you missed." / "The pain is delayed articulation." Vault `inbox/2025-11-16-hospitality-not-craft.md` — "Hospitality requires a different lens — one that receives before it evaluates." Vault `inbox/2026-05-11 no weather in the song.md` — "the songs have no weather in them… the machine learned the shape of praise, but it never had anything to thank." / "access without formation… carrying the story toward the human encounter, then getting out of the way." And `comfortable in my head…` — "the vault absorbs what action should hold."

---

## Applied Principles

Use these principles before design as constraints on the moment, and after design as falsifiable claims over artifacts.

### Framing - Preserve the live edge, not the record

**Evidence anchors:** vault `inbox/2026-06-22 margins live articulation content directions.md` — "The bottleneck is not writing. The bottleneck is noticing the question you should have asked while the conversation was still alive." / "Margins should preserve the live edge… what question you held internally… what opening you missed." / "The pain is delayed articulation."

**Stance:** Most meeting tools preserve the *record*. Margins preserves the *live edge*: the user's live question, missed opening, and internal attention signal.

**Human stake:** Prevents delayed articulation from turning an alive human moment into a dead archive.

**Design the moment so that...** the user can recover "the question you almost asked / the opening you missed" faster than they can reread the transcript.

**In flow:** Entry privileges capture, not setup. Transitions carry marks forward. Completion returns the user to the next human question, not to app admiration.

**In the microinteraction:** A mark appears within 50-100ms, feels warm, persists, and later remains traceable into review without being reinterpreted as AI output.

**In copy:** Do ✓: "Marked 12:04." "Question to bring back." "Opening you almost took." Don't ✗: "AI captured insight." "Summary generated." "Key takeaway detected."

**On the surface:** Marks are visually warmer than system text and never demoted below transcript or generated prose.

**How to evaluate:** Falsifiable claim: a founder can recover a half-formed question fast enough to use it next time because marks are indexed as *live questions*, and review foregrounds the almost-asked question. Required artifacts: video, interaction log, generated note, transcript, provenance, screenshot. Score 0 if marks are plain transcript timestamps or rendered as AI summaries; 1 if marks get auto-summarized; 2 if marks are preserved as the user's attention signal; 3 if review explicitly surfaces the missed/almost-asked question.

**Trip-wire:** Attention marks rendered as AI summaries rather than the user's live signal.

---

## Presence & Restraint

### C1 - Presence before artifact

**Stance:** The live surface makes the user more present in the room, not more occupied with the app.

**Human stake:** Prevents the tool from stealing the attention it claims to protect.

**Design the moment so that...** the live surface can be understood at a glance and then ignored until the user intentionally marks or stops.

**In flow:** Entry lands in the memo/capture posture. Warnings interrupt only when actionable. Completion hands attention back to the room or next conversation.

**In the microinteraction:** Start/mark/stop acknowledge quickly without continuous animation. Focus stays in the memo. Dismissal returns the user to the same place.

**In copy:** Do ✓: "Live." "3 marks." "Recording and marks are saved." Don't ✗: "Watch AI think." "Real-time intelligence feed." "Optimizing your call."

**On the surface:** Memo dominates; status is quiet; no animated waveform, multi-meter dashboard, or live AI feed competes with the room.

**How to evaluate:** Falsifiable claim: an in-meeting user can keep their eyes on the conversation because the live surface offers only glanceable status and the memo field dominates. Required artifacts: flow video, screenshot, visible-text snapshot. Score 0 for a competing live AI feed, animated waveform, or multi-meter dashboard; 1 for noisy status, >2 fighting colors, persistent motion, or coaching paragraphs; 2 for dominant memo, quiet status, actionable warnings only; 3 if the screen is calm from across the room and nothing moves except real state change.

**Trip-wire:** Anything on the recording surface auto-scrolls or animates continuously while audio is healthy.

### E4 - Calm density and restrained motion

**Stance:** Dense where density creates confidence, sparse where sparseness preserves attention; motion only confirms action or signals real state change.

**Human stake:** Prevents anxiety, spectacle, and decorative motion from becoming the main event.

**Design the moment so that...** the app reads as a calm appliance during capture and a notebook after, never a performance surface.

**In flow:** Lists, transitions, and completion states keep one dominant path. Reduced-motion users get the same clarity without movement.

**In the microinteraction:** Easing is short and functional. Motion stops after acknowledgment. Reduced-motion makes the core flow fully still.

**In copy:** Do ✓: "Note saved." "Open in Obsidian." "Retry transcription." Don't ✗: "Magic complete!" "Your brilliant AI note is ready!" "Watch the sparkle."

**On the surface:** Borders and spacing carry structure; semantic color is restrained; no confetti, bouncing badges, sparkles, or continuously shimmering AI panels.

**How to evaluate:** Falsifiable claim: a user glancing from a meeting can scan without anxiety because there is one dominant path and nothing moves without meaning. Required artifacts: video, screenshot, reduced-motion run. Score 0 for confetti, bouncing badges, or continuous shimmer; 1 for too sparse or too loud; 2 for calm density, semantic color, quiet transitions, `prefers-reduced-motion`; 3 if every motion is functional and reduced-motion is fully still.

**Trip-wire:** Celebratory or decorative animation anywhere in the core flow.

---

## Capture The Live Edge

### C2 - Attention marks are sacred

**Stance:** Marks are the user's in-the-moment signal - preserved, visually warmer than system text, and visibly folded into the note.

**Human stake:** Prevents the user's live attention from being swallowed by transcript or generic AI prose.

**Design the moment so that...** marking is a small act of trust: the user sees the mark land now and sees it matter later.

**In flow:** The mark lifecycle is visible: mark during capture, lead with marks in review, fold marks into note, cite marks in provenance.

**In the microinteraction:** Enter creates a timestamped mark in 50-100ms; the visible acknowledgment persists under 250ms unless the mark itself remains in the list; rollback is explicit if persistence fails.

**In copy:** Do ✓: "Marked 12:04." "3 marks shaped this note." "Source: mark at 12:04." Don't ✗: "AI captured insight." "Auto-summary." "Interesting moment found."

**On the surface:** Marks use the warm/attention treatment and are visually distinct from generated/system material.

**How to evaluate:** Falsifiable claim: a post-meeting reviewer can see that marks survived and shaped the note because they are timestamped, warm, and traceable into the connected note. Required artifacts: interaction log, generated note, provenance, screenshot. Score 0 if generated note discards or hides marks; 1 if marks are indistinguishable from generated/system material; 2 if marks are warm, timestamped, and lead review ahead of transcript; 3 if the note visibly grounds itself in marks during distill and after.

**Trip-wire:** Marks are not represented in the finished note or its provenance.

### E1 - Capture is a reflex

**Evidence anchors:** implementation surfaced *Commit-Time Choices*: irreversible work gets a visible pre-commit choice (`sidebar.ts:366/381`, `granola_import.rs:60/95`), in tension with "capture is a reflex, no decisions." Resolution: cheap/reversible actions are instant; irreversible/destructive actions get one visible confirm with a plan. Drift: collapsed sidebar hides the configure-card/drop-zone context (`styles.css:7081`). Vault `inbox/2026-06-08 chat with timour…`: "Excitement is not a commitment. It is a signal that deserves a container… let the aliveness be visible before the boundary arrives."

**Stance:** Start, mark, and stop are muscle memory - reachable while another app is frontmost. Capture the spark first; structure it after.

**Human stake:** Prevents setup, classification, or backend concern from costing the live moment.

**Design the moment so that...** cheap/reversible actions are instant, while irreversible/destructive work gets one visible confirm with a plan.

**In flow:** Intent-to-capture goes straight to recording; no folder/template/model choice blocks the spark. Import, transcribe, delete, and overwrite get explicit pre-commit framing.

**In the microinteraction:** Start responds within 100ms. Mark appears within 50-100ms. Stop says what is saved before what is still processing. Global shortcut works while another app is frontmost.

**In copy:** Do ✓: "Recording." "Marked." "Stop and save." "This will import and transcribe 4 files." Don't ✗: "Choose your knowledge workflow." "Select model before recording." "Configure diarization."

**On the surface:** The capture surface has one obvious start/mark/stop path, memo focused by default, and no spinner.

**How to evaluate:** Falsifiable claim: a meeting-rushed user can start capture and mark a moment without reading setup prose or making a decision because start/mark/stop are single keyboarded actions acknowledged in <100ms. Required artifacts: video, timing log, keyboard log, screenshot. Score 0 if a modal asks for folder/template/people/model before capture; 1 if capture starts but mark insertion lags, spinner appears, or no global shortcut exists; 2 if start/mark/stop are fast, focused, single actions and no classification is required; 3 if a power user can run a whole session keyboard-only and irreversible work gets one visible confirm with a plan.

**Trip-wire:** Any spinner on the capture surface, or any blocking decision between intent-to-capture and recording.

---

## The Live-Assistant Doctrine: Pulled, Hospitable, Non-Concluding

### C4 - Generative live assistance, pulled not pushed

**Evidence anchors:** the north star's #1 violation is "a live AI feed that scrolls suggestions." But live steering now exists, and its purpose is generative: extend "make the user a better *listener*" -> a better *questioner*. The resolution is two clauses - **(a) pull, never push; (b) generative, not summarizing.**

**Stance:** The live assistant is dormant and collapsed until the user pulls it. When pulled, it helps the user ask a better question in the moment - product discovery, opening a landscape of discussion - not summarize or echo.

**Human stake:** Prevents AI from interrupting the human encounter or laundering transcript recap as wisdom.

**Design the moment so that...** the assistant is an invited, temporary aid to the user's attention, not a feed competing for authority.

**In flow:** Collapsed by default; invoked by the user; offers one grounded nudge; collapses cleanly back; leaves no demand to manage a queue.

**In the microinteraction:** Invocation acknowledges immediately; generation may show a small bounded pending state; dismissal restores focus and does not re-open itself.

**In copy:** Do ✓: "Thread I noticed..." "Why it may matter..." "Possible question..." Don't ✗: "Summary." "The customer wants..." "You should ask..."

**On the surface:** The rail is absent or collapsed during normal recording and never auto-scrolls.

**How to evaluate:** Falsifiable claim: an in-meeting user is never interrupted because the rail stays collapsed until invoked, and when invoked it offers a sharper question grounded in what was said, not a recap. Required artifacts: video, transcript, visible-text snapshot, interaction log. Score 0 if live AI surfaces uninvited, auto-scrolls, or pushes notifications; 1 if pull-only but recap/echo; 2 if dormant until pulled and offers grounded questions/threads that open discussion; 3 if it demonstrably advances discovery with a non-leading question the user would not have asked.

**Trip-wire:** Any live AI element visible or animating without the user invoking it.

### C7 - The user authors; the assistant asks, never answers

**Evidence anchors:** `docs/margins-live-assistant-persona.md` ("Disqualification is good"); `docs/margins-branding-and-positioning.md` ("helps users understand better, not manipulate better"); vault Enzyme notes ("The client writes the final draft, always" / "The AI asks, never answers"). Vault `inbox/2026-05-28 attune app trellis ai boundaries.md`: "AI handles the light lift… Human-recorded prompts carry the posture… a good facilitator you barely know is there."

**Stance:** The AI surfaces questions, options, and unresolved choices; the user makes the call. Dedup / collate / rephrase = allowed; name / conclude / speak-in-the-user's-voice = human-only.

**Human stake:** Prevents manipulation, ventriloquism, and the quiet theft of authorship.

**Design the moment so that...** unresolved choices remain open until the user endorses them, and AI scaffolding is separable from user words.

**In flow:** Live guidance can surface fit/no-fit/timing/risk. Review asks for judgment. Completion occurs when the user endorses, edits, or carries forward.

**In the microinteraction:** Accepting, editing, or rejecting AI phrasing is reversible. Focus stays with the user's editable words, not a locked generated block.

**In copy:** Do ✓: "Possible risk." "Worth asking?" "Keep, edit, or leave open." Don't ✗: "Close them." "Handle this objection." "The real issue is..."

**On the surface:** User-authored text, AI scaffolding, and source evidence are visually distinct.

**How to evaluate:** Falsifiable claim: a founder after a customer call can turn the note into their own follow-up because Margins highlights unresolved choices and asks for judgment, including risks/no-fit. Required artifacts: generated note, transcript, visible copy, provenance, video. Score 0 if output optimizes for pressure/closing or asserts unendorsed meaning; 1 if editable but no judgment hooks or biased toward yes; 2 if non-leading prompts, unresolved questions, disqualification, and separable scaffolding are visible; 3 if user endorsement is the structural completion moment and human-only roles stay reserved.

**Trip-wire:** Any live cue says or implies "handle this objection," "close them," or asserts meaning the user has not endorsed on ambiguous evidence.

### C9 - Hospitality before craft, receive before you reframe

**Evidence anchors:** vault `inbox/2025-11-16-hospitality-not-craft.md` — "Hospitality requires a different lens — one that receives before it evaluates." The craft reflex is named a *temptation* "to fall out of relationship with humans."

**Stance:** While the conversation is alive, Margins helps the user receive what is being brought; it does not reframe it into what it could become.

**Human stake:** Prevents the user from falling out of relationship with humans by prematurely converting presence into craft.

**Design the moment so that...** live help mirrors and holds space before it interprets, categorizes, or improves.

**In flow:** During the meeting, receive and mark. After the meeting, synthesis may begin. The live panel never leads with AI summaries/categories before the user has marked anything.

**In the microinteraction:** Live AI waits, mirrors, leaves negative space, and disappears without residue. No animation or copy implies the app is impatient to synthesize.

**In copy:** Do ✓: "What are they bringing?" "You marked this." "Possible opening." Don't ✗: "Action item." "Strategic takeaway." "This means..."

**On the surface:** Live view privileges marks and memo; synthesis controls are secondary and pulled.

**How to evaluate:** Falsifiable claim: a founder mid-call registers what a customer is bringing because Margins surfaces their own marks as echoes, not restructured takeaways. Required artifacts: flow video, transcript, live-panel copy snapshot, screenshot. Score 0 if live panel pushes summaries/action items unprompted; 1 if it mirrors but immediately editorializes; 2 if it mirrors marks plainly and synthesis is only on pull; 3 if it visibly holds back, leaving negative space for noticing.

**Trip-wire:** Any unrequested AI-authored conclusion/framework appears in the live view before the meeting ends.

---

## Honest Continuity Across Time

### C3 - Honest signals only

**Evidence anchors:** the streaming-note percentage was removed because on a long job the number was inaccurate - it implied a precision the system couldn't honor. A skeleton is honest: it claims only "the app is alive and here is the note's shape," both of which are true. A lying 38% costs more trust than no number at all. Vault `inbox/2026-05-28 attune app trellis ai boundaries.md`: "users need to understand the shape of the thing before they can trust themselves inside it… create space rather than surveillance."

**Stance:** Never render a progress, status, or readiness cue you cannot keep accurate. Prefer truthful liveness + shape over invented precision.

**Human stake:** Prevents temporal lying from corroding trust.

**Design the moment so that...** waiting states show the shape of the container and the boundary of the work, not false certainty.

**In flow:** Distillation stages are named and each states what it does and does not touch. When no honest measure exists, use skeletons and liveness, not percentages.

**In the microinteraction:** No naked spinner >1s. Liveness updates at least every few seconds. Stages never move backward unless the app names why.

**In copy:** Do ✓: "Writing note structure." "Aligning your marks with the transcript." "Vault notes are not being changed." Don't ✗: "38%." "Almost done" without evidence. "Just a moment" for a long job.

**On the surface:** Skeleton mirrors the note's eventual structure; stage labels are human, monotonic, and boundary-aware.

**How to evaluate:** Falsifiable claim: a user watching long-running work can trust what they see because every cue corresponds to a real, monotonic, accurate measure, and where none exists the app shows liveness + shape. Required artifacts: video, timing log, visible-text snapshots, backend event log when available. Score 0 for fabricated/non-monotonic/stalling progress numbers; 1 for naked spinner >1s; 2 for named stages and/or structural skeletons with no fabricated precision; 3 if waiting previews outcome shape, updates liveness, and explains boundaries without mechanism knowledge.

**Trip-wire:** Any quantified progress cue not backed by a real measured quantity.

### C5 - State is legible, stable, and honest

**Stance:** Every capture has one clear state, one primary action, and one preservation promise.

**Human stake:** Prevents confusion and silent failure from making the user wonder whether their work exists.

**Design the moment so that...** the user can return hours later and know exactly where a capture stands and what to do next.

**In flow:** Canonical lifecycle: Live -> Captured -> Making note -> Note saved / Needs attention. Broken work moves to Needs attention, not ambiguity.

**In the microinteraction:** Status changes do not steal focus. Background updates reconcile quietly. Primary action changes only when the state truly changes.

**In copy:** Do ✓: "Live." "Captured." "Making note." "Note saved." "Needs attention." "Recording and marks are saved." Don't ✗: "Paused" for failed startup. "Still writing" for failed job. Two competing primary buttons.

**On the surface:** Header, sidebar row, processing area, and recovery surface use the same state vocabulary.

**How to evaluate:** Falsifiable claim: a user returning hours later can name a capture's state and single next action from the header alone. Required artifacts: screenshots across lifecycle, video, visible-text snapshots. Score 0 if failed job presents as still writing or startup failure reads as paused; 1 if state vocabulary duplicates/drifts; 2 if one canonical label, one primary action, and preservation promise are present; 3 if state is legible across sidebar, header, processing, and recovery.

**Trip-wire:** Any failure does not become a visible, recoverable state.

### E8 - Continuity across renders

**Evidence anchors:** implementation; code-backed, highly judge-able. Consistently followed in `desktop/src/main.ts:519, 661, 727` (capture/restore focus, memo draft, sidebar focus); `styles.css:382` (focus-visible).

**Stance:** A re-render may update data but must never steal focus, reset disclosure, lose scroll, or drop in-progress text.

**Human stake:** Protects attention from background machinery and preserves the user's working state.

**Design the moment so that...** polling and background updates are invisible to the working user unless they require action.

**In flow:** Search/edit/type/open-details workflows survive refreshes, sidebar updates, and modal changes.

**In the microinteraction:** Focus, cursor, selection, scroll, open `<details>`, drafts, and disclosures survive background updates.

**In copy:** Do ✓: keep the user's draft unchanged. If a conflict exists, say "Your draft is preserved." Don't ✗: replace typed text with refreshed server text or toast away focus loss.

**On the surface:** No visible jump, collapse, scroll reset, or focus ring teleport during background polling.

**How to evaluate:** Falsifiable claim: a keyboard-heavy user can search/edit/type through background polling because focus, cursor, open details, scroll, and draft text are restored after render. Required artifacts: interaction video, keyboard log, DOM/focus log. Score 0 if re-renders lose active input/focus; 1 if focus survives on one surface but scroll/details reset; 2 if focus/scroll/draft survive normal polling; 3 if keyboard semantics, selection, scroll, drafts survive across workspace, sidebar, and modals.

**Trip-wire:** A background update causes an active text input to lose typed content.

### E9 - Optimistic UI must reconcile

**Evidence anchors:** implementation; pairs with C3/C5. Consistently followed - `main.ts:1716` (recording optimistic state merges to real session), `main.ts:1964/1986` (import placeholders reconcile or become error rows), `lib.rs:851` (backend cleans up failed starts).

**Stance:** Latency hiding is allowed, but every optimistic object must resolve to the real backend object or become a visible retry/error state.

**Human stake:** Gives the user truthful immediacy without orphaning their work.

**Design the moment so that...** immediate feedback never pretends completion; it promises only that the action was received.

**In flow:** Optimistic rows reconcile, roll back, or become retryable. Name collisions, queued work, stale callbacks, and backend cleanup are handled visibly.

**In the microinteraction:** Feedback appears within 500ms, then changes to "saved," "retry," or "failed" when backend truth arrives.

**In copy:** Do ✓: "Saving..." then "Saved." "Import queued." "Retry import." Don't ✗: "Done" before backend confirmation. Silent vanishing. Duplicate rows with no explanation.

**On the surface:** Placeholder and real object share identity and do not appear as separate competing records.

**How to evaluate:** Falsifiable claim: a rushed capturer acts before backend completion because a row/button changes immediately and later resolves to the real session or retryable error. Required artifacts: interaction log, backend log, video, screenshot. Score 0 if optimistic rows orphan/duplicate/vanish; 1 if feedback never reconciles; 2 if recording/import states reconcile, roll back, or show retry; 3 if edge cases and cleanup are handled.

**Trip-wire:** An optimistic row persists after the backend returns a different real object.

---

## Trust & Consent

### C6 - Humane failure preserves agency

**Stance:** Failures preserve work, name the problem plainly, and offer one recommended recovery plus secondary options.

**Human stake:** Prevents failure from turning into fear, shame, or loss of agency.

**Design the moment so that...** the user learns what happened, what is safe, and the next small action without technical diagnosis.

**In flow:** Failure appears where the work lives. Preserved recording/marks stay accessible. Retry path works. Secondary options are visually separated from destructive actions.

**In the microinteraction:** Failed actions acknowledge within 500ms when possible, preserve focus, and do not return the user to home with only a toast.

**In copy:** Do ✓: "Your recording and marks are saved." "Sign in to make the note." "Retry transcription." Don't ✗: "Error." "Something went wrong." "Failed."

**On the surface:** One recommended action, one secondary action, one dismiss; destructive and recovery actions are visually separated.

**How to evaluate:** Falsifiable claim: a user hitting auth/audio/transcription failure still feels safe because the error states what is preserved and gives one primary recovery. Required artifacts: video, visible-text snapshot, screenshot. Score 0 if failure discards state or only says Error; 1 if recovery exists but is hidden or preservation is omitted; 2 if plain problem + preservation + one recovery; 3 if recommended/secondary/dismiss are clear and capture remains accessible.

**Trip-wire:** An error does not say whether recording/marks are saved.

### C8 - Consent-legible boundary

**Evidence anchors:** capture readiness vs AI readiness split in `sidebar.ts:48`, `main.ts:1348/1398`; data-boundary copy in `distill.ts:192`, `settings.ts:249`; `docs/margins-strategy-session-...md` ("local solves data exposure, but not consent"; warns against "invisible AI strategist"); `.pi/reports/category-design-demand.md` (uninvited bots/non-consenting recording = churn). **Drift:** settings mark Note-making AI "required" and disable save until AI is ready (`settings.ts:120/492`), which can imply AI is required for capture.

**Stance:** Local capture readiness is separate from AI readiness, and the UI names exactly when marks/transcript/vault snippets leave the machine. Margins must be easy to disclose; never a secret bot or hidden strategist.

**Human stake:** Protects trust with other participants, not just the user's private comfort.

**Design the moment so that...** the user can explain what Margins does before using it and can see what each mode will and will not touch before it acts.

**In flow:** Capture can start without cloud auth. Before transcript, marks, or vault snippets leave device, the user sees what moves and why.

**In the microinteraction:** Disabled states distinguish capture readiness from AI readiness. Consent disclosures open without losing place or blocking local capture.

**In copy:** Do ✓: "Capture works locally." "To make a note, transcript and marks are sent to your note-making AI." "I use a local note assistant to mark moments." Don't ✗: "Secret copilot." "Invisible strategist." "AI required" for local capture.

**On the surface:** Local/cloud/live-AI boundaries are visible at point of use; per-mode controls are plain.

**How to evaluate:** Falsifiable claim: a privacy-sensitive user can start local capture before AI sign-in and explain to another participant what Margins does because capture readiness is independent and each AI transfer is named. Required artifacts: settings screenshot, capture video, visible-text snapshot. Score 0 for secret-copilot/bot framing, false-privacy posture, or AI auth blocking capture; 1 for generic local/private copy without boundaries; 2 for local/cloud/live-AI boundaries at point of use and independent capture; 3 for plain consent language, per-mode controls, and auditable traces.

**Trip-wire:** "Start capture" is disabled solely because AI is not connected, or copy frames Margins as a secret advantage / auto-joining bot.

### E5 - Inspectable provenance

**Evidence anchors:** `docs/margins-branding-and-positioning.md` ("generated claims should be inspectable through marks, transcript excerpts, or vault references"); `.pi/reports/backchannel-suggestion-audit-...duc.md` (logging gap: prompts/context unrecoverable); vault `inbox/2026-02-07 tweet drafts tools taste...` ("instead of a clean summary it shows you the archaeology — threads of attention that accumulated into conviction"). Vault `inbox/2026-05-18 relationship as unmanufactured presence.md` warns against "engineer-brained pastoring" — "do not hide the root system, but let someone encounter its fruit before asking them to understand its genealogy."

**Stance:** Every important generated claim traces to a user-facing source - a mark, transcript moment, or vault reference - inside the UI, never only in logs and never on faith.

**Human stake:** Lets the user trust without submitting to opaque authority or mechanism worship.

**Design the moment so that...** the user encounters the fruit before genealogy, while provenance remains available and human-readable.

**In flow:** Result first, trace available. "How this was made" is one layer down and does not block the primary experience.

**In the microinteraction:** Source chips expand within 500ms without layout jumps, preserve focus, and collapse back cleanly.

**In copy:** Do ✓: "How this was made." "From mark at 12:04." "Related vault note." Don't ✗: "Confidence: high." "Trust score." Raw debug trace as product copy.

**On the surface:** Source chips/timestamps/vault snippets are close to generated claims without overwhelming the note.

**How to evaluate:** Falsifiable claim: a reviewer can explain why a note block or live cue appeared because the UI exposes source marks/timestamps/vault snippets via "How this was made." Required artifacts: generated note, transcript, provenance UI video, screenshot. Score 0 if claims have no traceable source; 1 if debug trace exists but not in product UI; 2 if source chips/timestamps explain output without blocking; 3 if every major claim has human-readable provenance and the note traces accumulation into judgment.

**Trip-wire:** A high-level conclusion or live suggestion asserts intent/emotion with no visible source evidence.

---

## Vault-Native Memory

### E3 - Vault as working garden (was E3+E11)

**Evidence anchors:** `docs/margins-strategy-session-...md` ("Works with your vault" must respect conventions); `.pi/reports/desktop-ux-ia-audit.md` (note format not "advanced" for Obsidian users); vault `inbox/2026-03-07 how i use enzyme` ("Choosing a tag is thinking"), `inbox/2026-03-08 your writing speaks back` ("protect the chaos longer than feels comfortable"; "this is a working garden… please be careful").

**Stance:** The finished note feels born in the vault, not exported into it. Vault-native means matching the user's existing schema and protecting their working mess rather than imposing a new taxonomy.

**Human stake:** Prevents Margins from replacing the user's garden with a proprietary store, generic dump, or forced organization.

**Design the moment so that...** capture becomes a note in the user's existing garden, and any new structure is suggested as a separate, optional step.

**In flow:** Destination, title, frontmatter, tags, people, links, and follow-ups are visible before completion. Open/Obsidian/Copy acknowledge within 500ms. Optional organization happens later.

**In the microinteraction:** Save confirms the real path. Open in Obsidian opens or reports failure. Copy confirms exactly what was copied. No auto-clean runs without confirmation.

**In copy:** Do ✓: "Saved to `Customers/Acme.md`." "Open in Obsidian." "Keep tags as suggested?" Don't ✗: "AI note complete!" "Export to vault." "We organized your tags."

**On the surface:** Editorial Markdown, correct frontmatter, clear destination, primary Open action, and visible fit with sample notes.

**How to evaluate:** Falsifiable claim: a messy-vault power user keeps the first generated note without cleanup because it reuses their folder/filename/frontmatter/tags/links and exposes new structure as optional. Required artifacts: generated note, saved file path, vault sample comparison, screenshot, interaction log. Score 0 for proprietary store, generic dump, forced schema, or chat transcript with Markdown as afterthought; 1 if Markdown saves but ignores templates/links/frontmatter or destination is unclear; 2 if editorial Markdown, destination, frontmatter, tags, people/projects, template basics, and Open action are correct; 3 if the note is indistinguishable from one the user wrote and reveals latent structure while keeping the garden intact.

**Trip-wire:** The note lands outside the chosen folder, Margins creates new tags/categories without explicit confirmation, or a celebratory AI success state sits above the saved-note destination.

### E6 - The note speaks in the user's voice

**Evidence anchors:** vault `inbox/2026-03-08 your writing speaks back to you...` ("Enzyme's value isn't retrieval. It's recognition." / "it comes back in your voice").

**Stance:** Generated output preserves the user's own phrases, tensions, and unfinished questions. The payoff is recognition - "that's mine" - not retrieval - "that was found."

**Human stake:** Prevents the user's thought from being flattened into generic assistant voice.

**Design the moment so that...** the note reads like a thoughtful reader of the user's archive wrote it back to them, while leaving authorship with the user.

**In flow:** User phrases move from marks/vault/transcript into headings, questions, and tensions. The user can edit or decline AI phrasing before treating it as final.

**In the microinteraction:** Accept/edit/reject preserves agency; edits do not break provenance or rewrite user language elsewhere without consent.

**In copy:** Do ✓: preserve quoted user phrases; "Keep this phrasing?" "Use your wording." Don't ✗: consultant abstractions such as "leverage stakeholder alignment" unless the user said them.

**On the surface:** User phrases and unresolved tensions visibly shape headings and claims.

**How to evaluate:** Falsifiable claim: a journaling-heavy user recognizes their own thinking because key phrases and unresolved tensions return in their language. Required artifacts: generated note, transcript, vault snippets, visible copy. Score 0 for generic AI summary voice; 1 for occasional quotes but mostly paraphrase; 2 if recurring phrases/tensions shape headings and claims; 3 if it feels like a thoughtful reader of the user's archive wrote it back to them.

**Trip-wire:** The note replaces the user's distinctive language with abstract consultant phrasing.

### E10 - Metadata is navigation

**Evidence anchors:** implementation; has drift. `sidebar.ts:546/604` (filters), `session-workspace.ts:217` (editable people pills), `lib.rs:473/633` + `granola_import.rs:423/465` (person/org note creation). **Drift:** capture-note metadata is read-only chips (`session-workspace.ts:152/174`) while editable pills exist only on the non-capture path (`:217`).

**Stance:** People, tags, type, dates, and source are memory handles: navigation, filtering, and correction affordances, not body-only decoration.

**Human stake:** Prevents memory from becoming unfindable or uncorrectable after the note is written.

**Design the moment so that...** metadata helps the user return to people/projects later and correct the garden when the system guessed wrong.

**In flow:** Metadata appears in rows, filters, chips, saved notes, and editable people pills. Imports/calendar consistently create and link people/org notes.

**In the microinteraction:** Pill edits are reversible, acknowledged within 500ms, and reflected in filters without losing place.

**In copy:** Do ✓: "Edit people." "Filter by person." "Source: Granola import." Don't ✗: hide people/tags only in frontmatter or call correction "advanced metadata."

**On the surface:** Rows, filters, chips, and correction controls expose people/tags/type/source.

**How to evaluate:** Falsifiable claim: a reviewer finds and corrects metadata because people/tags/type/source appear in rows, filters, chips, and editable pills. Required artifacts: screenshots, interaction video, saved note/frontmatter. Score 0 if metadata is body-only; 1 if displayed but not filterable/editable; 2 if filters, chips, editable people pills exist; 3 if imports/calendar consistently create and link people/org notes.

**Trip-wire:** A saved note with frontmatter people/tags has no visible people/tag/filter affordance.

### E7 - One object language, poetic only at the edges

**Evidence anchors:** `.pi/reports/desktop-ux-ia-audit.md` (canonical object model); `docs/app-copy-guidelines.md` (hide Enzyme/Pi/MCP/distill; "show safety with evidence, no exclamation points"); `docs/positioning.md` ("poetic only at the edges").

**Stance:** Primary UI uses one stable object model - capture -> marks -> transcript/evidence -> connected note - and plain, operational copy. Poetry/brand voice lives only at empty states and marketing.

**Human stake:** Prevents users from having to translate internal taxonomy while they are working.

**Design the moment so that...** every noun maps 1:1 to a visible object and every active surface says what is happening, what is safe, and the next small action.

**In flow:** Object names stay stable from capture through review, provenance, filters, errors, and saved-note states.

**In the microinteraction:** Focus behavior reinforces the object being acted on; labels do not mutate after clicks except through real state changes.

**In copy:** Do ✓: "capture," "mark," "transcript," "evidence," "note." Don't ✗: "session," "memo," "distill," "backchannel," "Enzyme," "Pi," provider/model names in the normal path.

**On the surface:** Nav, filters, confirmations, errors, and saved states use minimal words and the same object model.

**How to evaluate:** Falsifiable claim: a first-time user predicts what each row/action affects because nouns map 1:1 to visible objects and workflow copy is verb-first and concrete. Required artifacts: visible-text snapshot, screenshots, interaction video. Score 0 for contradictory nouns or hype/jargon as primary labels; 1 if legacy terms leak; 2 if capture/mark/transcript/note stay consistent and copy is short/safety-aware; 3 if the whole product holds the same object model and minimal words.

**Trip-wire:** "backchannel" means both user marks and AI help on the same screen, or any internal term (`distill`, `Pi`, model names) appears as a primary label.

---

## Pacing

### E2 - Progressive disclosure is the default IA

**Evidence anchors:** vault *Show The Gap* / *daily 2025-07-10* ("usefulness should be mind-blowingly evident, low stakes, very fast").

**Stance:** The main path shows the next useful action; advanced detail is one layer down. Show value before explaining mechanism.

**Human stake:** Prevents setup, architecture, and internal vocabulary from crowding out low-stakes usefulness.

**Design the moment so that...** the user sees a concrete before->after and the next small action, not the pipeline.

**In flow:** Onboarding, empty states, and completion demonstrate raw call -> marked moment -> connected note. Advanced details live behind `More`, `Advanced`, or `<details>`.

**In the microinteraction:** Disclosures open predictably, preserve focus/scroll, and do not auto-expand during the core path.

**In copy:** Do ✓: "More." "Setup details." "Advanced." "See how this was made." Don't ✗: "distill," "session id," "Enzyme," "Pi," model names, base URLs, diarization modes in the normal path.

**On the surface:** Main path is clean; configuration shrinks over time; product value is visible before mechanism.

**How to evaluate:** Falsifiable claim: a new user completes the core flow without encountering model folders, base URLs, provider names, or template grammar because those live behind advanced disclosures. Required artifacts: flow video, visible-text snapshot, screenshots. Score 0 if model/API/diarization/vault path sits directly on recording or primary surface; 1 if advanced detail is present but de-emphasized; 2 if advanced detail is behind disclosures and concrete before->after is shown; 3 if normal capture never exposes backend names and the product shows value before mechanism.

**Trip-wire:** Legacy/internal terms (`distill`, `session id`, `Enzyme`, `Pi`, model names) appear in the normal path without the user opening provenance/advanced.

---

## Formation Over Consumption - Capstone

### C10 - Access without formation is a failure, donkey not throne

**Evidence anchors:** vault `inbox/2026-05-11 no weather in the song.md` — "the songs have no weather in them… the machine learned the shape of praise, but it never had anything to thank." / "access without formation… carrying the story toward the human encounter, then getting out of the way." And `comfortable in my head…` — "the vault absorbs what action should hold."

**Stance:** Margins must not offer a frictionless glow that lets the user skip the formation the moment was for. It carries the story toward the human encounter, then gets out of the way - "a donkey, not a throne."

**Human stake:** Prevents access from replacing the human action and formation the conversation required.

**Design the moment so that...** the output points the user back toward the next conversation instead of presenting a polished substitute that ends engagement.

**In flow:** Completion preserves rough texture and makes the next human encounter the natural continuation without adding a separate forward-looking callout.

**In the microinteraction:** Completion actions acknowledge save/open/copy, then settle. No celebratory loop or engagement hook keeps the user in the app.

**In copy:** Do ✓: "Open question." "Bring this next time." Don't ✗: "Final summary." "Everything you need." "AI masterpiece."

**On the surface:** The connected note carries open questions and attention-edge above generic recap polish.

**How to evaluate:** Falsifiable claim: a user finishes a session more ready for the next conversation because Margins preserves the rough edges and open threads already present in the note. Required artifacts: generated note, completion video, visible-text snapshot, screenshot. Score 0 if note is a closed glossy summary; 1 if summary plus generic next steps; 2 if rough texture and open questions are preserved; 3 if the note carries open threads without adding redundant UI or generic follow-up prompts.

**Trip-wire:** The primary output is a frictionless summary with no preserved attention-edge or forward question.

---

## Cross-Cutting Standards

**Copy tone:** Active surfaces say what is happening, what is safe, and the next small action. Live-assistant copy is suggestive, not assertive: observed thread / cautious reason / possible question. No poetry in errors, progress, consent, or recording.

**Role grammar:** AI may dedup, collate, rephrase, retrieve, and ask. The human names, concludes, endorses, speaks in their own voice, and decides what formation requires.

**Latency and microinteraction budgets:** Start responds within 100ms. Mark appears in 50-100ms and acknowledgment persists under 250ms. No naked spinner >1s. Action feedback appears within 500ms. Local search responds within 50ms. Focus, scroll, drafts, and disclosures survive background updates.

**Transition and carry-forward rules:** Carry forward what matters; defer what pulls the user out of the room. Stop-capture says what is saved, then what is still processing. Completion returns the user to the next human question, not the app's triumph.

**Evaluation artifacts:** Use video for flow, timing logs for microinteraction, visible-text snapshots for copy, generated-note/transcript/provenance for output quality, and screenshots only for surface/UI.

---

## Applying the spec

Every review runs three passes in order: **flow map (structural)** -> **human review (depth)** -> **judge review (regression gate)**. The flow map runs first because the other two reason *over* it: a falsifiable claim about whether the user can recover the live edge is unscoreable until you know the transition graph the user actually moves through.

**Flow map (structural pre-pass) - REQUIRED each review:** spin up a dedicated read-only thread (e.g. `bb thread spawn ... --provider codex --reasoning-level high --permission-mode readonly`) whose only job is to reconstruct the app's state/transition graph from code. Why a separate thread: reading render functions biases a reviewer toward copy and surface; flow lives in the *traversal* - what carries forward at each edge, where focus lands, what is reversible - which a static read of one screen never surfaces. So the flow map is deliberately quarantined from the copy/surface review and given its own hard rule.

- **Unit of every finding is a state or a transition, never a string.** Copy, label, and wording problems are explicitly out of scope for this pass; the human/judge passes own those. If the flow thread notices a copy bug, it ignores it.
- **Run against the live working tree, not a worktree off HEAD.** Uncommitted/untracked code is part of the flow; a worktree branched off the last commit will report real files as "missing" and misnumber line anchors.
- **Cut the work by journey, not by principle** (principle-anchored swaths drag findings toward copy). Minimum journeys: entry & capture-start; in-capture spine; capture -> note; failure & recovery; background/return/navigation.
- **Deliverables per journey:** (1) nodes - every user-perceptible state with the status value/condition that produces it (file:line); (2) edges - every transition with its trigger, what carries forward (marks/memo draft/transcript/focus/scroll/disclosure), what is dropped, where focus/scroll lands, the latency/optimism window, and reversibility; (3) dead-ends & traps - states with no forward edge, states entered unintentionally, second-action-required traps, silent reversals; (4) carry-forward ledger - does the user's live edge survive to the next conversation, and at which edge does it degrade; (5) spec flow-promise check - does the real graph keep "transitions carry marks forward," "completion returns the user to the next human question," the canonical lifecycle `Live -> Captured -> Making note -> Note saved / Needs attention` (or are there unnamed states like paused/cancelled/degraded), and "stop says what is saved"; (6) top flow smells, ranked, each phrased "user is in state X, does Y, expects Z, gets W."

Feed the flow map's nodes/edges/dead-ends into the human and judge passes as the set of transitions to assert claims about and score.

**Human review (depth):** for a meaningful change, write the UX_REVIEW.md review block (User promise / Falsifiable claims / Persona pass-fail / Flow risks / Required changes), and assert the claims for Framing, the Core principles, and any Extended principles the change touches. A review with no "risk" or "fail" is suspect.

**Judge review (regression gate):** the UX loop judge scores each touched flow/screen 0-3 per Core principle using video, interaction logs, visible text, generated note, transcript, provenance, and screenshot evidence as appropriate. It then scores any Extended principle whose surface, behavior, or output the change touches. The judge reports trip-wires and fails the run if any trip-wire fires or any applicable principle drops below 2. Scenarios at minimum: `settings-audio`, `recording-healthy`, `recording-dead-tap`, `backchannel-built`, `distill-running`, `distill-error-pi-login`, `distill-complete`.

## Open questions for the next workshop pass

1. **Where C4's "better question" gets graded.** Generative quality is hard to score from a screenshot alone - it needs a transcript-aware judge pass, not only a visual one.
2. **Reconciling the source docs.** Once this stabilizes, edit UX_REVIEW.md's "must show percent" line and the north star's progress budget to point here, or keep them as historical taste and let this file override.
3. **Scenario coverage for the applied philosophy.** C4 (pulled live AI), C3 (skeleton/no-fake-progress), C8 (consent boundary), and E5 (available-not-imposed provenance) need dedicated scenarios - e.g. `backchannel-pulled`, `distill-skeleton`, `consent-boundary`, `note-provenance` - that collect video, timing logs, copy snapshots, generated-note/transcript/provenance, and screenshots.

## Final taste rule (unchanged from north star)

When two directions tie on the rubric, choose the one that makes this truer:

> I stayed present, marked only what mattered, and left with a connected note that already belongs in my knowledge system.
