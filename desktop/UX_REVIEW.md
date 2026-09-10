# Margins Desktop UX Review Rubric

Margins is used while the user is cognitively occupied. The app should create confidence without demanding attention.

Core principle:

> Margins should feel like a calm appliance, not a developer tool.

Use this rubric before shipping meaningful desktop UX changes, especially changes to recording, Settings, processing, model setup, backchannel review, note generation, or error states.

## Review loop

Prefer the browser/CDP harness for fast screenshot review:

```bash
cd desktop
npm run dev
npm run ux:cdp -- --scenarios settings-audio,recording-healthy,recording-dead-tap,backchannel-built,distill-running,distill-error-pi-login,distill-complete
```

Artifacts land in `desktop/ux-shots/`.

For small/localized changes, choose scenarios from the selective screenshot matrix in `desktop/UX_CDP_LOOP.md` or `.pi/agent/skills/margins-ux-cdp/SKILL.md` instead of blindly running every scenario. For broad redesigns or shared layout/CSS changes, run the baseline set above.

For each selected scenario, inspect:

- screenshot
- visible text
- console logs
- whether the app feels calm, legible, and recoverable


## Design taste rubric

Use this section alongside the falsifiability method. A screen can pass comprehension and still be off-brand if it feels like a SaaS dashboard, developer console, or noisy copilot.

### Current visual baseline

Ground review in the app that exists today:

- default dark theme: charcoal backgrounds, low-contrast panels, thin borders;
- light theme: warm paper tones, not pure white;
- primary/accent: muted slate blue;
- human attention/backchannel accents: warm ochre;
- success/warning/error: green/yellow/red used sparingly and literally;
- typography: Avenir Next / SF Pro-style sans for UI, mono for timestamps, file paths, transcript excerpts, and memo marks;
- layout: compact sidebar, editorial main pane, card/panel surfaces with modest radii and minimal shadow;
- motion: short, quiet transitions and functional spinners only.

A screenshot should feel calm at 100% zoom and still legible when the user glances from a meeting.

### Visual identity checks

Passes if:

- the screen has one dominant reading/action path;
- panels use borders/spacing more than heavy shadows;
- accent color communicates state or meaning, not decoration;
- memo/backchannel marks are visually warmer than generated/system material;
- generated notes read like editorial Markdown, not chat output;
- advanced/provenance details look available but secondary.

Risks/fails if:

- the screen uses bright AI gradients, sparkles, or marketing illustration energy;
- more than one color is fighting for primary attention;
- a status chip or warning is decorative rather than meaningful;
- the UI feels like an admin settings console during the normal path;
- the transcript dominates the backchannel review over user marks.

### Typography and density checks

Passes if:

- headings are short and editorial;
- uppercase labels are two or three words;
- mono text is reserved for evidence-like material;
- button labels fit comfortably without wrapping;
- explanatory text appears where the user has time to read it.

Risks/fails if:

- the recording screen asks the user to read paragraphs;
- path/model/provider details appear in display headings;
- mono is used to make ordinary UI feel technical;
- small sidebar rows contain brand copy instead of state metadata.

### Interaction feel checks

Margins should feel like a calm appliance, then a notebook. Review for:

- **Glance:** can the user understand status in one look?
- **Mark:** can the user capture an attention mark without leaving the conversation?
- **Recover:** does every error show what is saved and what to do?
- **Inspect:** can a skeptical user see evidence/provenance without being forced into it?
- **Leave:** when done, can the user open the saved Markdown note and move on?

### Aspirational taste checks

These are not all fully implemented yet, but they should guide reviews:

- live assistant nudges should be margin notes, not a chat feed;
- source evidence should feel like footnotes/marginalia, not logs;
- the note-making trace should be readable by power users but visually subordinate to the note;
- command/keyboard affordances should feel quiet and Mac-native;
- light mode should feel like warm paper, not a generic web app.

### Things the desktop app should never do

- Never use animation to manufacture excitement during recording.
- Never make the live assistant compete with the person speaking.
- Never put a celebratory AI success state above the saved note destination.
- Never bury “recording/marks are saved” in an error state.
- Never make a required setup step look optional, or an optional advanced detail look required.
- Never let legacy/internal terms (`distill`, `session id`, `Enzyme`, model names) leak into the main path unless the user opened advanced/provenance details.

## Falsifiability-first review method

Do not review UX by saying “this feels good” or “this seems clear.” Those are weak claims. Review by making testable claims about what a real user could perceive, decide, and do from the screen alone.

A good UX review statement has this shape:

> I believe **[persona]** can **[complete task]** under **[constraint]** because **[visible evidence]**. This would be false if **[counter-evidence]**.

Examples:

- Strong: “A meeting-rushed user can start recording in under 10 seconds because the only primary button says Record and audio readiness appears above the fold. This would be false if the user must open Settings or interpret technical warnings first.”
- Weak: “The recording screen is intuitive.”
- Strong: “A privacy-conscious user can tell model download is separate from meeting upload because the copy says one-time download for local transcription. This would be false if the same panel also says model access without explaining what leaves the device.”
- Weak: “Privacy seems fine.”

When reviewing, actively try to disprove quality. Ask: “What screenshot evidence would convince me this is not good?” If you cannot name disconfirming evidence, the claim is too vague.

### Evidence hierarchy

Prefer direct UI evidence over intention:

1. **Best:** visible screenshot/text proves the user can act correctly.
2. **Good:** scenario behavior proves the state is recoverable.
3. **Weak:** code suggests the state may work.
4. **Invalid:** designer/developer intent without visible user evidence.

### Required review output format

For each meaningful UX change, write a short review in this format:

```markdown
## UX review

### User promise
What should this screen make possible for a user?

### Falsifiable claims
- Claim: ...
  Evidence: ...
  Could be false if: ...
  Verdict: pass / risk / fail

### Persona pass/fail
- Meeting-rushed user: pass / risk / fail — why
- In-meeting user: pass / risk / fail — why
- Post-meeting reviewer: pass / risk / fail — why
- Privacy-conscious user: pass / risk / fail — why
- Power user: pass / risk / fail — why

### Flow risks
Where could the user hesitate, misunderstand, or lose trust?

### Required changes before ship
- ...
```

A review with no “risk” or “fail” entries is suspicious. Either the change is tiny, or the reviewer did not try hard enough to falsify it.

## Personas / lenses

### 1. About to join a meeting

The user has seconds, not minutes. They want to know recording will work before pressure starts.

Criteria:

- Can I tell mic + computer audio are ready in under 10 seconds?
- Are setup tasks phrased as user outcomes, not implementation details?
- Are one-time tasks clearly one-time: login, permissions, model downloads?
- Does Settings avoid scary jargon unless marked as advanced?
- Are primary setup CTAs obvious?

Good copy:

- “Download speech models”
- “Label speakers in single-track recordings”
- “Computer audio is ready”
- “One-time download for local transcription and speaker labels”

Avoid in primary UI:

- “Polyvoice CoreML”
- “int8 Parakeet TDT”
- “rust-diarization feature disabled”
- raw feature names, crate names, backend names

Golden question:

> Would I trust this right before joining an important call?

### 2. In the meeting

The user is trying to stay present. Margins should not compete with the conversation.

Criteria:

- Recording status is unmistakable.
- Audio health is glanceable, not noisy.
- Warnings are calm and actionable.
- Memo entry is frictionless: type, press Enter, continue listening.
- No modal interruptions unless recording is truly compromised.
- The UI should not require reading paragraphs during recording.

Golden question:

> Would I trust this while someone important is talking?

### 3. Meeting just ended

The user wants to know what happened and what to do next.

Criteria:

- The primary CTA is obvious: build backchannel / make note.
- Processing progress explains stages in human terms.
- Long tasks never freeze the app.
- Cancel, retry, and clear states are obvious where relevant.
- Failures preserve artifacts and explain recovery.
- Completion states say what was created and where it went.

Golden question:

> If processing fails, do I still feel safe?

### 4. Reviewing the backchannel

The user needs evidence, not a transcript wall.

Criteria:

- The timeline is chunked by meaningful moments.
- Memo notes are visually privileged.
- Transcript is supporting evidence, not the whole interface.
- Speaker/audio/memo cues are scannable.
- The user can decide whether the generated note will be grounded enough.
- Empty, partial, and error states tell the user what evidence exists.

Golden question:

> Can I reconstruct the meeting’s shape without reading everything?

### 5. Privacy / local-first user

The user wants to understand what leaves the machine.

Criteria:

- Local vs cloud/model boundaries are explicit.
- Audio, transcript, memo, and note data flow is legible.
- Retention settings are clear.
- “Model access” does not imply mystery upload.
- Model downloads are clearly separate from meeting content.
- The app avoids implying a meeting bot or external participant.

Golden question:

> Can a privacy-conscious user explain what Margins does with their data?

### 6. Power user / debugging

Advanced controls should exist without polluting the normal path.

Criteria:

- Advanced fields are available but visually secondary.
- Model folders, API base URL, and provider choices are recoverable.
- Error messages include enough detail for debugging.
- Normal users do not need to understand implementation details.
- Technical names appear only when useful for action or diagnosis.

Golden question:

> Is implementation detail available but not imposed?

## Copy rules

### Prefer outcome language

Use:

- “Download speech models”
- “Prepare local transcription”
- “Label speakers”
- “Make note”
- “Build backchannel”
- “Computer audio is silent”

Avoid primary copy like:

- “Run ASR backend”
- “Enable Polyvoice diarization”
- “Load TDT int8 ONNX”
- “Invoke Rust pipeline”

### Make errors recoverable

Errors should answer:

1. What happened?
2. Did we preserve my recording/memo?
3. What can I do next?

Example:

> Could not download speech models. Your recording setup is unchanged. Check your connection and try Download again.

### Make long-running work visible

Long tasks must show at least one of:

- progress percent
- current stage
- current file/item
- cancel button when cancellation is meaningful

The app must remain responsive during downloads, model loads, transcription, distillation, and cleanup.

### Keep advanced detail secondary

Good pattern:

- Primary: “Download speech models”
- Secondary hint: “Advanced: choose an existing model folder, or leave blank to let Margins download one.”

Bad pattern:

- Primary: “Download compact Parakeet TDT int8 model + Polyvoice CoreML diarization model”

## State checklist

Review these states for every feature that touches them:

- Empty / first run
- Ready
- In progress
- Success
- Warning
- Error
- Canceled
- Retry after failure
- Clear/reset after success
- Offline or missing dependency

## Desktop scenarios to keep healthy

At minimum, inspect these before merging broad desktop UX work:

- `settings-audio`
- `recording-healthy`
- `recording-dead-tap`
- `backchannel-built`
- `distill-running`
- `distill-error-pi-login`
- `distill-complete`

Add scenarios when introducing new durable states, such as model download progress or canceled downloads.

## Falsifiable quality gates

Use these as pass/fail gates. A screen can be beautiful and still fail.

### Comprehension gate

Pass condition:

- A user can state what the screen is for and what to do next using only visible text.

Fails if:

- The primary action depends on interpreting implementation words.
- The screen explains architecture before user value.
- Two actions appear equally primary.

Review prompt:

> What is the most likely wrong interpretation of this screen?

### Action gate

Pass condition:

- The user can identify the next action in under 3 seconds.

Fails if:

- The CTA is below the fold for the common case.
- A secondary action is more visually prominent than the primary action.
- The action label is a noun or system term rather than a user verb.

Review prompt:

> If the user is tired, what will they click first? Is that correct?

### Trust gate

Pass condition:

- The UI tells the user enough to trust that recording, processing, or recovery is safe.

Fails if:

- An error does not say whether recording/memo data is preserved.
- A long-running task has no progress or liveness cue.
- A warning is scary but not actionable.
- The app freezes during work.

Review prompt:

> What would make the user worry they lost something?

### Privacy gate

Pass condition:

- The user can distinguish local capture/model work from content sent to an AI/model service.

Fails if:

- “Model” is used ambiguously for both local speech models and note-generation AI.
- Large downloads are implicit or automatic without an explicit user action.
- Meeting content upload is implied but not explained.

Review prompt:

> What might the user incorrectly believe is being uploaded?

### Flow gate

Pass condition:

- The screen advances the user through Ready → Record → Backchannel → Note → Done.

Fails if:

- The screen introduces concepts unrelated to the current step.
- Setup decisions interrupt recording/review.
- The user must remember a choice from a previous screen.
- An advanced option is presented as a normal required step.

Review prompt:

> Which part of the core flow does this UI serve? If none, why is it visible?

### Evidence gate

Pass condition:

- The reviewer can cite screenshot text, visual hierarchy, or scenario behavior as evidence.

Fails if:

- The review relies on code intent.
- The review says “seems clear” without naming the visible signal.
- The review cannot articulate what would disprove its conclusion.

Review prompt:

> What visible evidence proves this is good? What visible evidence would prove it is bad?

## High-signal review questions

Ask these while looking at screenshots. Answer with evidence, not vibes:

1. What is the one thing the user should do next? What visible hierarchy proves it?
2. What is the most likely wrong click or wrong interpretation?
3. Is anything scary without being actionable?
4. Is any implementation detail leaking into normal-user copy?
5. Would the app still feel trustworthy under time pressure? What could falsify that?
6. Does every long task prove the app is alive and cancellable when appropriate?
7. If something fails, is the user’s recording/memo clearly safe?
8. Can a privacy-conscious user tell what happens locally vs externally?
9. Are advanced controls present but visually subordinate?
10. What would a brand-new user be unable to infer from this screen alone?

## UX simplicity and flow goals

Margins has only a few core jobs. The UI should make those jobs feel linear even when the underlying system is complex.

### Desired flow

The happy path should read as:

1. **Ready** — vault/model/audio setup is good enough.
2. **Record** — capture audio and jot timestamped thoughts.
3. **Backchannel** — review raw evidence: memo, speakers, transcript moments.
4. **Note** — turn the evidence into a connected vault note.
5. **Done** — know where the note went and what was retained.

If a screen introduces a concept that does not support this flow, it probably belongs in advanced settings, logs, or an error detail.

### Simplicity goals

- One primary action per screen or panel.
- Use nouns users understand: recording, backchannel, note, vault, speaker labels.
- Hide backend choices until they become necessary for recovery.
- Prefer progressive disclosure over permanent explanation text.
- Keep Settings as a readiness checklist, not a control panel.
- Keep recording UI sparse enough to glance at mid-conversation.
- Keep review UI structured around moments, not raw data.
- Keep completion UI focused on outcome: what was created, where it lives, what is next.

### Flow smell checklist

A UX change is suspect if:

- The user must understand more than one new concept to continue.
- Two CTAs compete visually.
- A setup detail appears during recording.
- A recovery action is hidden behind technical wording.
- A screen explains how Margins works before saying what to do.
- A long-running step has no visible state change.
- The user has to remember something from a previous screen.
- The app asks for a decision that could safely be a default.

### Preferred interaction patterns

- **Primary action:** one obvious button, verb-first: “Record”, “Download”, “Build backchannel”, “Make note”.
- **Secondary action:** lower contrast: “Refresh”, “Clear”, “Retry”, “Advanced”.
- **Danger/action reversal:** explicit but scoped: “Delete session”, “Clear downloaded models”.
- **Cancel:** only visible while cancellable work is in progress.
- **Advanced fields:** visible only when useful, with short hints and safe defaults.

### Golden simplicity question

> Can a tired user move from setup to note without learning any implementation names?

## Non-negotiables

- Never freeze the app during downloads or processing.
- Never hide the safety/recovery path after an error.
- Never make transcript text the only primary backchannel UI.
- Never require users to understand backend names to complete setup.
- Never surprise users with large downloads; use an explicit action.
- Never imply meeting content leaves the machine unless it does.
