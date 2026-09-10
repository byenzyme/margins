# Distillation Performance Notes

## Session Investigated

- Session: `2026-06-24-13-47`
- Topic: Cameron / DGX Spark / local AI box conversation
- Final note: `/Users/example/obsidian/inbox/2026-06-24-13-47-27-2026-06-24-13-47.md`
- Margins artifacts: `/Users/example/obsidian/inbox/.margins/2026-06-24-13-47_*`
- Pi session: `/Users/example/obsidian/inbox/.margins/pi-sessions/--Users-joshuapham-obsidian--/2026-06-24T21-33-02.222Z_fd99524c.jsonl`

## Timing Evidence

The older `*_distill_trace.jsonl` did not include event timestamps, so the best
evidence came from file mtimes plus the persisted Pi session timestamps.

### File Timeline

- `14:31:41` - `2026-06-24-13-47_seg0.wav` finalized
- `14:32:30` - memo and capture context written
- `14:33:01` - aligned transcript written
- `14:33:02` - Pi / GPT-5.5 session started
- `14:38:32` - note draft, grounding, distill trace, Pi pointer, and final note written

Approximate local prep costs:

- Recording finalization to capture context: ~49s
- Offline aligned timeline: ~31s

### Pi Session Timeline

From the Pi session JSONL:

- `21:33:02.222Z` - initial user prompt saved
- `21:35:38.236Z` - first assistant response, calling `enzyme_petri`
- `21:35:38.237Z` - `enzyme_petri` result recorded
- `21:35:38.237Z` - three `enzyme_catalyze` calls/results recorded
- `21:36:42.096Z` - user refine request: `other person is [[Cameron Reynoldson]]`
- `21:38:32.905Z` - refine complete

Known model-side costs:

- Initial prompt to first tool call: ~156s
- Initial prompt to first note completion: at most ~220s
- Refine prompt to refine complete: ~111s
- Initial prompt to final refined note: ~331s

The trace timestamps are coarse around tool results because the Pi session stores
tool-call/result messages together. The Enzyme result payloads themselves report
very small execution times for `catalyze` (`0.058s`, `0.013s`, `0.011s`), so the
main latency is not the CLI execution. It is model time and token volume around
tool use.

## Token / Payload Evidence

Initial run:

- Initial user prompt: ~79k characters
- First GPT-5.5 tool-call turn: `32,938` input tokens, `16` output tokens
- Full `enzyme_petri` output: ~190k characters
- Next GPT-5.5 tool-call turn after Petri: `73,817` input tokens
- Final GPT-5.5 answer turn after catalyze: `81,259` input tokens, `3,464` output tokens

Refine:

- Refine user prompt: ~940 characters
- GPT-5.5 refine answer: `84,433` input tokens, `3,014` output tokens
- Refine took ~111s because it resumed the full prior conversation, including
  the large Petri and Catalyze payloads.

## Bottleneck Assessment

The biggest confirmed bottleneck is GPT-5.5 repeatedly processing a large
context, not Enzyme runtime.

The most suspicious avoidable contributor is the unbounded `enzyme_petri` result:

- The app instructed the agent to run `enzyme_petri` first.
- The `enzyme_petri` tool accepted no parameters.
- It called full `enzyme petri --vault <vault>`.
- That returned ~190k characters.
- The next model call more than doubled from ~33k input tokens to ~74k input tokens.
- This large payload also persisted into the resumed refine context, making even
  a small name-fix refine take ~111s.

## Likely High-Leverage Changes

Keep GPT-5.5 as the final note writer, but reduce context-prep cost:

1. Make `enzyme_petri` query-ranked and bounded:
   - Require or strongly encourage `query`.
   - Default `top` to 8.
   - Default `catalyst_budget` to 2.
   - Clamp `top` to a small maximum, e.g. 12.

2. Update bundled distillation instructions:
   - Do not ask for a whole-vault slate.
   - Ask Petri to rank the slate against memo/transcript language.
   - Use the ranked Petri output only to calibrate 2-3 Catalyze queries.

3. Measure against this same session:
   - Re-run distillation on the existing `2026-06-24-13-47` artifacts.
   - Compare prompt-to-first-tool, first-pass completion time, Petri payload chars,
     and model input tokens.
   - Target: reduce first-pass model-side time by 30-50% without reducing final
     note quality.

4. Longer-term split:
   - Run Enzyme query planning outside the GPT-5.5 final writer.
   - Give GPT-5.5 only transcript, memo, selected snippets, and note instructions.
   - Preserve GPT-5.5 for final synthesis.

## Implementation Thread Success Criteria

The implementation thread should not just patch the schema. It should produce
evidence:

- A code change that prevents full unqueried Petri output in normal distillation.
- A local reproduction command or script that can run against the Cameron session
  artifacts without re-recording audio.
- Before/after metrics:
  - total first-pass wall time
  - prompt-to-first-tool time
  - Petri output size
  - model input token counts if available from the Pi session
- final note output size
- A short judgment on whether note quality stayed acceptable.

## No-Spend Reproduction Path

Static metrics and bounded Enzyme payload comparison can be reproduced without
recording audio and without spending model/API resources:

```bash
cd desktop
npm run distill:perf -- --run-enzyme --write-fixture /tmp/margins-cameron-distill-fixture
```

This parses the persisted Pi session JSONL, measures the historical unqueried
`enzyme_petri` payload, runs only the local Enzyme CLI with:

```bash
enzyme petri --vault /Users/example/obsidian \
  --query "Cameron DGX Spark local AI box visible agent thinking home appliance trust Qwen VPS workflow" \
  --top 8 \
  --catalyst-budget 2
```

and writes a replay fixture from the existing Cameron memo, aligned timeline,
and capture context. The fixture command it prints is the opt-in real split
replay path. It spends model/API resources.

Important routing rule for this benchmark: OpenRouter/Gemini may be used only
for the prep planner. Do not route the final writer through OpenRouter for this
benchmark, or the run stops measuring the production ChatGPT-subscription
GPT-5.5 path.

```bash
cd desktop/src-tauri
export CARGO_TARGET_DIR=/Users/example/Hacks/margins-cargo-target
MARGINS_UX_FIXTURE_WORK_DIR=/tmp/margins-cameron-distill-replay \
MARGINS_UX_FIXTURE_VAULT_PATH=/Users/example/obsidian \
MARGINS_UX_E2E_PREP_AI_PROVIDER=margins-openai-compatible \
MARGINS_UX_E2E_PREP_OPENAI_BASE_URL=https://openrouter.ai/api/v1 \
MARGINS_UX_E2E_PREP_OPENAI_MODEL=google/gemini-3-flash-preview \
MARGINS_UX_E2E_PREP_OPENAI_API_KEY="$OPENAI_API_KEY" \
MARGINS_UX_E2E_AI_PROVIDER=openai-codex \
MARGINS_UX_E2E_OPENAI_MODEL=gpt-5.5 \
cargo run --example pi_distill_fixture -- /tmp/margins-cameron-distill-fixture
```

Do not run the real replay command unless the thread explicitly allows spend.
If the final writer errors with `openai-codex` OAuth, refresh ChatGPT/Codex
login; do not substitute OpenRouter as the final writer for this benchmark.

## Split Prep / Final Writer Implementation

Current implementation keeps the configured distillation model as the final note
writer, but moves first-pass vault retrieval out of that final writer session:

- A prep step creates a small retrieval plan (`petri_query`, 2-3
  `catalyze_queries`). When a separate faster model is configured for
  backchannel/cues, that model is used for planning; otherwise the plan falls
  back to deterministic memo/transcript extraction so GPT-5.5 is not used for
  prep planning.
- Rust executes bounded Enzyme calls directly:
  `enzyme petri --query ... --top 8 --catalyst-budget 2`, then up to three
  `enzyme catalyze --limit 5 ...` queries.
- Rust compacts those results into a `# Vault context bundle`.
- The final writer receives the transcript, memo, templates, and compact bundle
  with no grep/Enzyme tools enabled for first-pass distillation.
- Refine continues to use the saved final-writer Pi session, which now excludes
  large tool payloads for new first-pass runs.

Fixture-only prep overrides:

```bash
MARGINS_UX_E2E_PREP_AI_PROVIDER=margins-openai-compatible
MARGINS_UX_E2E_PREP_OPENAI_BASE_URL=https://openrouter.ai/api/v1
MARGINS_UX_E2E_PREP_OPENAI_MODEL=google/gemini-3-flash-preview
MARGINS_UX_E2E_PREP_OPENAI_API_KEY=...
```
