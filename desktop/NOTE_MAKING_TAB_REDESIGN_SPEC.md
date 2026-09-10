# Note-making tab redesign — implementation spec

Work in this worktree: `/Users/example/.bb/worktrees/env_4m3ree6eyr/aside`.
All frontend paths below are relative to `desktop/`. Export
`CARGO_TARGET_DIR=/Users/example/Hacks/aside-cargo-target` before any cargo command.

## Design intent (context)

The Settings → Note-making pane currently shows two peer decisions:
"Note-making AI" (3 radios: Included / ChatGPT subscription / Your API key) and
"Live cues" (2 radios: "Same model as distillation" / "Separate fast model" with its
own API key + base URL + model fields). Problems: "distillation" is jargon that appears
nowhere else in the pane; the separate live-cue block demands a second API key even when
the user wants the same provider with a faster model; and the backend already applies
sensible defaults (Included mode automatically uses a fast Haiku model for cues via
`configure_ai_for_backchannel`), so the radio is not only confusing but inaccurate.

Target design — "one decision, quiet automation":

1. The only decision is **who writes your notes**: three stacked selectable option
   cards (radio semantics, `name="ai-mode"`). The selected card expands to show its
   setup inline; unselected cards show only title + one-line description.
2. **Live cues are not a decision.** Below the cards, a short section states what live
   cues automatically do in the current mode, plus a collapsed `<details>` advanced
   override: a model field that reuses the note-making credentials by default, with
   optional key/base-URL fields for a genuinely separate provider.
3. Never use the word "distillation" in this pane. Say "writing your note" /
   "note-making".

## 1. `src/render/settings.ts`

### `renderAiSettingsSection(ctx)`

Replace the plain radio-group + mode body with three option cards inside the existing
`settings-group required-group` wrapper (keep `id="ai-notes-group"` and the
`missing` class logic and the `Note-making AI ${requiredBadge("required")}` label):

```html
<div class="ai-option-cards" role="radiogroup" aria-label="Note-making AI">
  <!-- one per mode: included | chatgpt | api -->
  <label class="ai-option-card {selected: ctx.currentAiMode === mode}">
    <input type="radio" name="ai-mode" value="included" {checked} onchange="window.__setAiMode('included')" />
    <div class="ai-option-card-body">
      <div class="ai-option-card-head">
        <strong>Included</strong>
        <span class="recommended-chip">Recommended</span>   <!-- Included card only -->
        <span class="ai-option-status ok">Ready</span>      <!-- see status rules -->
      </div>
      <p class="ai-option-desc">Works out of the box — no account or API key needed.</p>
      <!-- expanded body, only when selected -->
    </div>
  </label>
  ...
</div>
```

Card copy:
- **Included** — chip "Recommended"; desc: `Works out of the box — no account or API key needed.`
- **ChatGPT subscription** — desc: `Sign in with your ChatGPT Plus or Pro subscription.`
- **Your own API key** — desc: `OpenAI, OpenRouter, or any OpenAI-compatible provider.`

Status chip (right-aligned in head), per mode, shown on every card (not just selected):
- included: `ctx.aiStatus`-independent — ready iff the included setup is ready. The
  render context only exposes `aiReady` for the *current* mode, so add what you need:
  extend `SettingsRenderContext` with `includedAiReady: boolean` and
  `chatgptAuthenticated` is already available via `ctx.aiStatus.chatgpt_authenticated`;
  API-key readiness = `Boolean(ctx.settings.api_key?.trim())`. Wire `includedAiReady`
  from `includedAiStatus.included_ready` in `settingsRenderContext()` in `src/main.ts`.
  Chip text: `Ready` (class `ok`) when ready; otherwise `Set up needed` (class `warn`)
  on the selected card only — unselected non-ready cards show no chip (avoid warning
  noise for paths the user didn't choose).

Expanded body per selected mode — reuse the existing markup and element ids verbatim so
`collectSettingsFromDom()` and handlers keep working:
- included: existing `model-prep-card` with `Set up` button (`window.__setupIncludedAi`)
  and readiness text; keep the existing hint but drop "Best for first setup." (the chip
  says it now): `No API key is shown or stored in settings; the usage-limited key is kept in macOS Keychain.`
- chatgpt: existing sign-in `model-prep-card` + `#ai-model-chatgpt` input + datalist.
  Replace the hint with: `Your note is written once per capture, so favor the most capable model your subscription allows. Leave blank for the default.`
- api: existing `#ai-base-url`, `#api-key`, `#ai-model` inputs; keep hint
  `Works with OpenAI and compatible providers such as OpenRouter. The key is stored in macOS Keychain.`

### `renderBackchannelSettingsSection(ctx)`

Remove the radio group and `__setBackchannelMode` entirely. New structure (keep
`id="backchannel-ai-group"`):

```html
<div class="settings-group" id="backchannel-ai-group">
  <label>Live cues</label>
  <p class="backchannel-auto-line">{autoLine}</p>
  <details class="advanced-setting" {open iff override active}>
    <summary>Use a faster model for live cues</summary>
    <input type="text" id="backchannel-model" value=... placeholder="Model for live cues (e.g. google/gemini-3-flash)" oninput="window.__updateSettingsSaveState()" />
    <div class="hint">Cues fire many times per capture, so a small fast model keeps them snappy. Uses your note-making provider unless you add a key below.</div>
    <input type="password" id="backchannel-api-key" value=... placeholder="API key (optional — uses your note-making key)" autocomplete="off" oninput="window.__updateSettingsSaveState()" />
    <input type="text" id="backchannel-base-url" value=... placeholder="Base URL (optional, e.g. https://openrouter.ai/api/v1)" oninput="window.__updateSettingsSaveState()" />
    <div class="hint">Keys are stored in macOS Keychain. Clear all fields to go back to the automatic default.</div>
  </details>
</div>
```

`autoLine` by `ctx.currentAiMode` (when no override active):
- included: `During capture, live cues automatically use a fast included model — nothing to set up.`
- chatgpt: `During capture, live cues use the same ChatGPT model that writes your notes.`
- api: `During capture, live cues use the same provider and model that write your notes.`

Override active = `ctx.settings.backchannel_same_as_distill === false` and
(`backchannel_model` or `backchannel_api_key` non-empty). When active, the details
element renders `open` and the auto line instead reads:
`During capture, live cues use the custom model below.`
Drop the `requiredBadge("optional")` from the label — the whole section is now
informational-plus-advanced, and the badge implied a decision.

## 2. `src/main.ts`

- Delete `window.__setBackchannelMode` (and its declaration in `src/actions/types.ts`).
- In `collectSettingsFromDom()`, derive the flag from field contents instead of the
  removed radios:
  ```ts
  backchannel_same_as_distill: (() => {
    const model = (document.getElementById("backchannel-model") as HTMLInputElement | null)?.value.trim();
    const key = (document.getElementById("backchannel-api-key") as HTMLInputElement | null)?.value.trim();
    if (document.getElementById("backchannel-model")) return !(model || key);
    return settings.backchannel_same_as_distill ?? true;
  })(),
  ```
  Keep the existing collection of `backchannel_api_key` / `backchannel_base_url` /
  `backchannel_model` values. Note the fields are now always present in the DOM when the
  pane is rendered (inside `<details>`), which simplifies this.
- Add `includedAiReady: includedAiStatus.included_ready` to `settingsRenderContext()`
  and to the `SettingsRenderContext` interface.

## 3. `src-tauri/src/ai_config.rs` — model-only override

In `configure_ai_for_backchannel`, support "same credentials, different model": when
`backchannel_same_as_distill == Some(false)` and there is **no** usable
`backchannel_api_key` but `backchannel_model` is set, resolve the distill config and
swap in the backchannel model. Concretely, replace the final fallback block so the
resolution order is:

1. `same_as_distill == Some(false)` and backchannel key present → separate provider
   (existing branch, unchanged).
2. `same_as_distill == Some(false)` and no key but `backchannel_model` present →
   for included mode: included key + OpenRouter base + backchannel model;
   for chatgpt mode: `("openai-codex", backchannel_model, None)` tuple;
   for api mode: `api_key`/`ai_base_url` + backchannel model.
   Implementation hint: compute `configure_ai_from_note_settings(settings)` (or the
   included branch) and replace the model element of the returned tuple with the
   backchannel model — but note the tuple shapes differ (openai-compatible returns
   base-url-encoded provider), so the cleanest is a small helper that mirrors
   `configure_ai_from_note_settings` with a model parameter. Keep it simple and add
   unit tests:
   - api mode + model-only override → api key/base with cue model.
   - included mode + model-only override → included key with cue model.
   - chatgpt mode + model-only override → codex provider with cue model.
   - existing tests must keep passing (`backchannel_uses_separate_model_when_opted_in`,
     `included_backchannel_uses_fast_model`, etc.).
3. included mode (no override) → fast included model (existing, unchanged).
4. `same_as_distill != Some(false)` → distill config (existing, unchanged).
5. Fallback → distill config (existing, unchanged).

Also check `src-tauri/src/settings.rs` `preserve_backchannel_routing` (~line 366): it
currently treats a separate route as "flag false AND key present". With model-only
overrides the key can be absent — extend that condition to also preserve routing when
`backchannel_model` is present. Read the surrounding code first and keep its intent.

## 4. Mock scenarios — `test-harness/mock-tauri.ts` + `src/main.ts` init

Add two scenarios to the `MockScenario` union and `createState()`:
- `settings-ai`: same settings fixture as `settings-audio` (first-run-ish: `ai_mode:
  "included"` — change from chatgpt so the recommended default state is shown, api_key
  null), included NOT ready.
- `settings-ai-api`: the default fixture (ai_mode "api", masked key) plus
  `backchannel_same_as_distill: false`, `backchannel_model:
  "google/gemini-3-flash-preview"`, key/base-url null — shows the model-only override
  with the details open.

In `src/main.ts` scenario init (~line 4657–4698): for both new scenarios set
`settingsOverlayOpen = true` and `settingsActiveSection = "ai"` (mirror how
`settings-audio` is handled, including adding them to the `settingsOverlayOpen`
condition). Check `mock-tauri.ts` line ~234 (`const authed = ...`) and the `createState`
special cases for scenario names that gate settings fixtures — add the new names where
appropriate so `settings-ai` uses the first-run-style fixture.

## 5. `src/styles.css`

Add styles for `.ai-option-cards`, `.ai-option-card` (block label, border, radius,
padding, pointer cursor; `.selected` gets accent border + slightly raised background),
`.ai-option-card-head` (flex row, gap), `.recommended-chip` (small pill, accent tint —
model after the existing `.required-badge` pills), `.ai-option-status.ok/.warn` (reuse
existing ok/warn color vars), `.ai-option-desc` (muted small text),
`.backchannel-auto-line` (muted). Hide the native radio dot or keep it small and
aligned — match the app's existing dark aesthetic and density (look at
`.model-prep-card`, `.radio-group`, `.required-badge` for tokens). Keyboard focus must
be visible: style `:focus-visible` on the card via the inner input
(`.ai-option-card:has(input:focus-visible)`).

## 6. Docs

`UX_CDP_LOOP.md`: add `settings-ai` and `settings-ai-api` to the scenario list and a
matrix row: `| Note-making AI / live cues | settings-ai, settings-ai-api |`.

## Validation (required before you finish)

1. `cd desktop && npm run build` — clean.
2. `export CARGO_TARGET_DIR=/Users/example/Hacks/aside-cargo-target && cd desktop/src-tauri && cargo test ai_config` (and `cargo check`).
3. Vite dev server is already running on http://localhost:5173 (leave it running; if
   dead, `cd desktop && npm run dev` in background).
4. `cd desktop && npm run ux:cdp -- --scenarios settings-ai,settings-ai-api,settings-audio`
   — open and LOOK at the PNGs. Check: no clipped text, no awkward wraps, selected card
   obviously selected, Recommended chip legible, details closed in `settings-ai`, open
   with model filled in `settings-ai-api`, save-row readiness copy still correct.
5. Also exercise interactively via CDP or a quick evaluate: switch modes
   included→chatgpt→api and confirm cards expand/collapse and the live-cues auto line
   updates; type into the cue model field and confirm Save state updates.

Do NOT commit. Report: changed files, test/build output summary, screenshot paths, and
any deviations from this spec with reasons.
