export const meta = {
  name: 'design-demand-research',
  description: 'Web-research sweep: what people actually want from a meeting-capture app, and what "excellently designed" means in this category',
  whenToUse: 'Rerun whenever you want fresh market/design-demand signal for Margins. Iterate by editing the ANGLES list or passing args like {focus: "pricing"}.',
  phases: [
    { title: 'Sweep', detail: 'parallel search agents, one discourse angle each' },
    { title: 'Synthesize', detail: 'merge into a ranked demand report' },
  ],
}

// Iterate here: add/remove/sharpen angles, then rerun. Each angle is one search agent.
const ANGLES = [
  {
    key: 'granola-discourse',
    prompt: `Research what real users say about Granola (the macOS meeting notes app) — both praise and criticism. Search Reddit (r/macapps, r/productivity, r/ObsidianMD), Hacker News, and X/Twitter discourse. What do people cite when they call it well-designed? What frustrates them (pricing, cloud dependence, summary quality, missing features)? Capture verbatim quotes with sources.`,
  },
  {
    key: 'incumbent-churn',
    prompt: `Research why people abandon or complain about Otter.ai, Fireflies, Fathom, and tl;dv. Search Reddit, HN, G2/Capterra review lowlights, and "alternatives to" threads. Focus on: bot-joining fatigue, trust/privacy objections, transcript-dump overload, UI complaints. What would make someone switch? Capture verbatim quotes with sources.`,
  },
  {
    key: 'craft-bar',
    prompt: `Research what the Mac app community considers excellent app design in 2025-2026. Search for discourse and teardowns on Things 3, Linear, Raycast, Superhuman, and recent "best designed Mac apps" threads (HN, Reddit r/macapps, design Twitter, Brian Lovin / app teardown posts). Extract the SPECIFIC qualities people name — latency, keyboard-first, restraint, native feel, onboarding — not generic praise. Capture quotes with sources.`,
  },
  {
    key: 'privacy-local-first',
    prompt: `Research demand for local-first / on-device meeting transcription. Search Reddit, HN, and privacy-focused communities for discourse on: not wanting audio in the cloud, company policies banning notetaker bots, on-device Whisper apps (MacWhisper, superwhisper) reception. How much do people care, and what do they trade off for it? Capture quotes with sources.`,
  },
  {
    key: 'pkm-integration',
    prompt: `Research how knowledge-management users (Obsidian, Notion, Logseq communities) want meeting notes to land in their system. Search r/ObsidianMD, Obsidian forum, PKM discourse: what do they do with transcripts, what makes an AI summary actually useful vs noise, desire for templates/structure, linking to existing notes. Capture quotes with sources.`,
  },
  {
    key: 'jtbd-pain',
    prompt: `Research the underlying jobs-to-be-done and pains around meeting notes generally: search for discourse on "I never reread my meeting notes", action-item follow-through, 1:1 and discovery-call note-taking habits, people taking notes on paper to stay present. What outcome are people actually buying? Capture quotes with sources.`,
  },
]

const SWEEP_SCHEMA = {
  type: 'object',
  required: ['findings', 'quotes'],
  properties: {
    findings: {
      type: 'array',
      items: {
        type: 'object',
        required: ['insight', 'evidence', 'strength'],
        properties: {
          insight: { type: 'string', description: 'One concrete thing people want / hate / praise' },
          evidence: { type: 'string', description: 'Summary of supporting evidence with source URLs' },
          strength: { type: 'string', enum: ['strong', 'moderate', 'weak'], description: 'How widespread/consistent the signal is' },
        },
      },
    },
    quotes: {
      type: 'array',
      items: {
        type: 'object',
        required: ['text', 'source'],
        properties: { text: { type: 'string' }, source: { type: 'string' } },
      },
    },
  },
}

const focus = args && args.focus ? `\n\nEXTRA FOCUS for this run: ${args.focus}` : ''

phase('Sweep')
const sweeps = await parallel(
  ANGLES.map((a) => () =>
    agent(
      `${a.prompt}${focus}\n\nUse WebSearch and WebFetch (load via ToolSearch if needed). Do at least 4-6 distinct searches with varied phrasing before concluding. Return only well-sourced findings — no speculation.`,
      { label: `sweep:${a.key}`, phase: 'Sweep', schema: SWEEP_SCHEMA, model: 'sonnet' }
    )
  )
)

const collected = sweeps
  .map((s, i) => ({ angle: ANGLES[i].key, ...(s || { findings: [], quotes: [] }) }))
  .filter((s) => s.findings.length)

log(`${collected.length}/${ANGLES.length} angles returned findings`)

phase('Synthesize')
const report = await agent(
  `You are synthesizing market/design-demand research for Margins, a macOS meeting-capture app (mic + system audio, no bot, timestamped memos while you listen, local transcription, distills into an Obsidian-style vault). First read docs/positioning.md and docs/margins-branding-and-positioning.md in this repo for context on current positioning.

Here is the raw research, grouped by angle:

${JSON.stringify(collected, null, 2)}

Write the file .pi/reports/category-design-demand.md with:
1. **What people are really buying** — the ranked jobs-to-be-done, weighted by signal strength across angles.
2. **What "excellently designed" means in this category** — the specific, named qualities users cite, mapped to what they'd mean for Margins's screens.
3. **Why people churn from incumbents** — the failure modes Margins must never reproduce.
4. **The gap Margins can own** — where strong demand meets weak incumbent execution, tied to Margins's actual architecture (local, bot-free, vault-native).
5. **Best quotes** — the 10-15 most vivid verbatim quotes with sources, as ammunition for copy/positioning.
Cross-reference findings that reinforce each other across angles. Be honest where signal is weak. Then return a 10-line executive summary as your final text.`,
  { label: 'synthesize', phase: 'Synthesize', model: 'sonnet' }
)

return { report, angles: collected.map((c) => ({ angle: c.angle, findings: c.findings.length })) }
