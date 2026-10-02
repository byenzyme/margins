export const meta = {
  name: 'design-roadmap-implement',
  description: 'Implement design-synthesis-roadmap items with a quality hierarchy: opus specs and reviews, sonnet implements, haiku does mechanical edits',
  whenToUse: 'Run a wave of roadmap items: args {tiers: [0]} (default) or {items: [5, 19]}. Iterate by rerunning with different selections. Never uses fable — orchestrator synthesizes outside.',
  phases: [
    { title: 'Spec', detail: 'opus architect per item: precise change spec', model: 'opus' },
    { title: 'Implement', detail: 'sonnet (haiku for mechanical) executes specs; overlapping-file groups run sequentially', model: 'sonnet' },
    { title: 'Review', detail: 'opus reviews each diff against acceptance criteria, fixes small issues', model: 'opus' },
    { title: 'Harness', detail: 'ux:cdp screenshot pass judged against the UX_REVIEW taste rubric', model: 'opus' },
  ],
}

// Roadmap items from .pi/reports/design-synthesis-roadmap.md. `refs` point spec agents at source detail.
// `mechanical: true` routes implementation to haiku instead of sonnet.
const ITEMS = [
  { id: 1, tier: 0, title: 'Failed note-making job must surface as a visible, recoverable state (never "still writing")', refs: 'ux-ia-audit finding #4; roadmap Tier 0 item 1', mechanical: false },
  { id: 2, tier: 0, title: 'Recording startup failure must not read as "paused"; live cue/backchannel failures surface in capture UI, not console', refs: 'ux-ia-audit findings #13 and #15; roadmap Tier 0 item 2', mechanical: false },
  { id: 3, tier: 0, title: 'Calm audio warnings: one recommended action + one secondary + dismiss; integrate degradation into capture lifecycle', refs: 'design-audit finding #11; ux-ia-audit #14; roadmap Tier 0 item 3', mechanical: false },
  { id: 4, tier: 1, title: 'Live capture empty state: quiet coach element, memo column constrained ~760px, teaches Enter-marks-the-moment', refs: 'design-audit finding #1; roadmap Tier 1 item 4', mechanical: false },
  { id: 5, tier: 1, title: 'Keyboard commands for start/mark/stop capture, including a system-wide shortcut while another app is frontmost', refs: 'ux-ia-audit finding #6; roadmap Tier 1 item 5; may need Tauri global-shortcut work in src-tauri', mechanical: false },
  { id: 6, tier: 1, title: 'Capture-surface latency: no spinners or async-feeling interactions on memo entry, mark insertion, save acknowledgment', refs: 'category-design-demand section 2; roadmap Tier 1 item 6', mechanical: false },
  { id: 7, tier: 2, title: 'Vocabulary purge: capture/marks/connected note everywhere; remove session/memo/distill/synthesize/process/vendor names from user-facing copy', refs: 'ux-ia-audit exec summary; docs/app-copy-guidelines.md legacy-drift list; roadmap Tier 2 item 7', mechanical: false },
  { id: 8, tier: 2, title: 'Resolve "backchannel" triple-overload: one referent for the live AI rail, rename marks and brand uses', refs: 'ux-ia-audit finding #2; roadmap Tier 2 item 8', mechanical: false },
  { id: 9, tier: 2, title: 'Sidebar restructure: New capture / Active now / Needs attention / Recent captures / Saved notes', refs: 'ux-ia-audit findings #1, #9 and proposed target IA; roadmap Tier 2 item 9', mechanical: false },
  { id: 10, tier: 2, title: 'Collapse note lifecycle to ~4 user-visible statuses: Recording / Ready to make note / Making note / Note saved (+ Needs attention)', refs: 'ux-ia-audit finding #3; roadmap Tier 2 item 10', mechanical: false },
  { id: 11, tier: 3, title: 'Strict type scale; retire most uppercase tracking micro-labels (.session-kicker, .eyebrow, panel headings)', refs: 'design-audit finding #3; roadmap Tier 3 item 11', mechanical: false },
  { id: 12, tier: 3, title: 'Semantic color contract: slate-blue action, warm amber attention, green saved/ready only, red destructive/ending only', refs: 'design-audit finding #4; roadmap Tier 3 item 12', mechanical: false },
  { id: 13, tier: 3, title: 'Geometry tokens: four radii, consistent button heights, one hairline color', refs: 'design-audit finding #5; roadmap Tier 3 item 13', mechanical: true },
  { id: 14, tier: 3, title: 'Reading measures: 680px prose / 760px workspace, centered, for note-writing and finished-note views', refs: 'design-audit findings #8 and #10; roadmap Tier 3 item 14', mechanical: true },
  { id: 15, tier: 3, title: 'prefers-reduced-motion support: disable pulse/spinner/caret animations', refs: 'design-audit finding #13; roadmap Tier 3 item 15', mechanical: true },
  { id: 16, tier: 4, title: 'Split one-time readiness (notes destination, AI, transcription, audio) from ongoing preferences', refs: 'ux-ia-audit findings #5, #10; roadmap Tier 4 item 16', mechanical: false },
  { id: 17, tier: 4, title: 'Settings: provider/API/audio diagnostics behind advanced disclosures; fix required-vs-optional labeling', refs: 'design-audit finding #6; roadmap Tier 4 item 17', mechanical: false },
  { id: 18, tier: 4, title: 'Unbury templates: surface template choice where it shapes the connected note', refs: 'ux-ia-audit finding #11; roadmap Tier 4 item 18', mechanical: false },
  { id: 19, tier: 5, title: 'Connected note output: YAML frontmatter matching vault schema, auto-wikilinks, consolidated action items, marks-first layout', refs: 'category-design-demand gap 1; roadmap Tier 5 item 19; may span skills/margins and src-tauri distill code', mechanical: false },
  { id: 20, tier: 5, title: 'Post-capture screen rebuilt around the decision: "Ready to make the connected note?" hero with CTA adjacent to marks', refs: 'design-audit finding #2; roadmap Tier 5 item 20', mechanical: false },
]

const REPORTS = '.pi/reports/design-synthesis-roadmap.md, .pi/reports/desktop-design-audit.md, .pi/reports/desktop-ux-ia-audit.md, .pi/reports/design-north-star.md, .pi/reports/category-design-demand.md'

const GROUND_RULES = `Ground rules (non-negotiable):
- Repo: Margins, a Tauri meeting-capture app. Frontend in desktop/src (TS + styles.css), Rust in desktop/src-tauri.
- The working tree has in-flight uncommitted changes on branch margins-tauri-desktop-pi-sdk. NEVER run git checkout/restore/stash/reset or revert anything you did not write.
- Do not commit. Leave changes in the working tree.
- Typecheck after editing: cd desktop && npx tsc --noEmit. For Rust: export CARGO_TARGET_DIR=$HOME/.cache/margins-cargo-target then cargo check from desktop/src-tauri.
- Match existing code style. UI copy must follow docs/app-copy-guidelines.md.`

const tiers = (args && args.tiers) || [0]
const explicitIds = args && args.items
const selected = ITEMS.filter((i) => (explicitIds ? explicitIds.includes(i.id) : tiers.includes(i.tier)))
if (!selected.length) return { error: 'No items matched the tiers/items selection.' }
log(`Wave: ${selected.length} items — ${selected.map((i) => `#${i.id}`).join(', ')}`)

const SPEC_SCHEMA = {
  type: 'object',
  required: ['files', 'plan', 'acceptance', 'risk'],
  properties: {
    files: { type: 'array', items: { type: 'string' }, description: 'Every file this change will touch (repo-relative)' },
    plan: { type: 'string', description: 'Precise change plan: functions/selectors to modify, new states, exact copy strings' },
    acceptance: { type: 'array', items: { type: 'string' }, description: 'Checkable acceptance criteria' },
    risk: { type: 'string', enum: ['low', 'medium', 'high'] },
    outOfScope: { type: 'string', description: 'What this item deliberately does not change' },
  },
}

const IMPL_SCHEMA = {
  type: 'object',
  required: ['status', 'summary', 'filesTouched', 'typecheckPassed'],
  properties: {
    status: { type: 'string', enum: ['done', 'partial', 'blocked'] },
    summary: { type: 'string' },
    filesTouched: { type: 'array', items: { type: 'string' } },
    typecheckPassed: { type: 'boolean' },
    notes: { type: 'string', description: 'Deviations from spec, follow-ups, blockers' },
  },
}

const REVIEW_SCHEMA = {
  type: 'object',
  required: ['approved', 'issues'],
  properties: {
    approved: { type: 'boolean' },
    issues: { type: 'array', items: { type: 'string' } },
    fixesApplied: { type: 'string', description: 'Small fixes the reviewer applied directly, if any' },
  },
}

phase('Spec')
const specced = (
  await parallel(
    selected.map((item) => () =>
      agent(
        `You are the architect for one design-roadmap work item in the Margins repo. Produce a precise, minimal change spec — you change NOTHING yourself.

Item #${item.id}: ${item.title}
Source detail: ${item.refs}. Read the relevant sections of these reports for full context: ${REPORTS}. Also read desktop/UX_REVIEW.md (design taste rubric) and docs/app-copy-guidelines.md if copy is involved.

Read the actual code before speccing: desktop/src/main.ts, desktop/src/render/, desktop/src/actions/, desktop/src/state/, desktop/src/styles.css, and desktop/src-tauri/src/ if native work is needed. The spec must name real functions, selectors, and files — an implementer should not need to make judgment calls. Keep scope tight: this item only.

${GROUND_RULES}`,
        { label: `spec:#${item.id}`, phase: 'Spec', schema: SPEC_SCHEMA, model: 'opus' }
      ).then((spec) => spec && { item, spec })
    )
  )
).filter(Boolean)

if (!specced.length) return { error: 'All spec agents failed or were skipped.' }
log(`${specced.length}/${selected.length} specs ready; grouping by file overlap`)

// Group items whose specs touch overlapping files; groups run in parallel, items within a group sequentially.
const groups = []
for (const entry of specced) {
  const overlapping = groups.filter((g) => entry.spec.files.some((f) => g.files.has(f)))
  if (!overlapping.length) {
    groups.push({ files: new Set(entry.spec.files), entries: [entry] })
  } else {
    const target = overlapping[0]
    for (const g of overlapping.slice(1)) {
      g.entries.forEach((e) => target.entries.push(e))
      g.files.forEach((f) => target.files.add(f))
      groups.splice(groups.indexOf(g), 1)
    }
    target.entries.push(entry)
    entry.spec.files.forEach((f) => target.files.add(f))
  }
}
// Lower-tier (more urgent) items first within each group.
groups.forEach((g) => g.entries.sort((a, b) => a.item.tier - b.item.tier || a.item.id - b.item.id))
log(`${groups.length} independent group(s): ${groups.map((g) => g.entries.map((e) => `#${e.item.id}`).join('+')).join(' | ')}`)

const results = await parallel(
  groups.map((g) => async () => {
    const out = []
    for (const { item, spec } of g.entries) {
      const impl = await agent(
        `Implement exactly this spec in the Margins repo working tree. Do not expand scope.

Item #${item.id}: ${item.title}

SPEC:
${JSON.stringify(spec, null, 2)}

Read each file in the spec before editing. After all edits run: cd desktop && npx tsc --noEmit (and cargo check with CARGO_TARGET_DIR=$HOME/.cache/margins-cargo-target if you touched Rust). If the spec conflicts with what you find in the code, implement the closest faithful version and record the deviation in notes — do not silently skip.

${GROUND_RULES}`,
        { label: `impl:#${item.id}`, phase: 'Implement', schema: IMPL_SCHEMA, model: item.mechanical ? 'haiku' : 'sonnet' }
      )
      if (!impl) {
        out.push({ id: item.id, title: item.title, impl: null, review: null })
        continue
      }
      const review = await agent(
        `You are the senior design-engineering reviewer for one change in the Margins repo. Judge it against its acceptance criteria and the design taste rubric in desktop/UX_REVIEW.md.

Item #${item.id}: ${item.title}
SPEC: ${JSON.stringify(spec)}
IMPLEMENTER REPORT: ${JSON.stringify(impl)}

Run git diff -- ${spec.files.map((f) => `'${f}'`).join(' ')} and read the surrounding code, not just the diff. Check: every acceptance criterion met; no scope creep; copy follows docs/app-copy-guidelines.md; no native-feel violations (cursor:pointer, hover highlights on rows/buttons, transition flicker); typecheck actually passes (rerun cd desktop && npx tsc --noEmit yourself). You may directly fix small issues (wrong copy, missed selector, style nit) — record them in fixesApplied. Reject (approved: false) only for substantive problems an implementer must redo.

${GROUND_RULES}`,
        { label: `review:#${item.id}`, phase: 'Review', schema: REVIEW_SCHEMA, model: 'opus' }
      )
      out.push({ id: item.id, title: item.title, impl, review })
    }
    return out
  })
)

const flat = results.filter(Boolean).flat()
const approved = flat.filter((r) => r.review && r.review.approved)
log(`${approved.length}/${flat.length} items approved by review; running harness pass`)

phase('Harness')
const harness = await agent(
  `All approved roadmap changes for this wave are now in the Margins working tree. Run the screenshot harness and judge the result against the design taste rubric.

Wave results: ${JSON.stringify(flat.map((r) => ({ id: r.id, title: r.title, status: r.impl && r.impl.status, approved: r.review && r.review.approved })))}

Steps: read desktop/UX_CDP_LOOP.md. Ensure the Vite dev server is up (check first — it may already be running; otherwise start cd desktop && npm run dev in the background and wait for it). Then run: cd desktop && npm run ux:cdp -- --scenarios settings-audio,recording-healthy,recording-dead-tap,distill-complete. Read desktop/ux-shots/report.md and LOOK at the screenshots. Judge each scenario against the "Design taste rubric" section of desktop/UX_REVIEW.md. Report per-scenario: pass/fail, specific regressions or taste violations introduced by this wave, and anything broken (blank screens, console errors).

${GROUND_RULES}`,
  { label: 'harness-judge', phase: 'Harness', model: 'opus' }
)

return {
  wave: selected.map((i) => i.id),
  groups: groups.map((g) => g.entries.map((e) => e.item.id)),
  items: flat.map((r) => ({
    id: r.id,
    title: r.title,
    status: r.impl ? r.impl.status : 'agent-failed',
    typecheckPassed: r.impl ? r.impl.typecheckPassed : false,
    approved: r.review ? r.review.approved : false,
    issues: r.review ? r.review.issues : ['no review ran'],
    notes: r.impl && r.impl.notes,
  })),
  harness,
}
