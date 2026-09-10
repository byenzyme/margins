// Pure, DOM-free helpers for the AI settings preview: the per-activity model
// summary line and the quick-assists section routing. Kept out of
// render/settings.ts so they can be unit-tested without pulling in the whole
// (vite-resolved) settings renderer.

import { esc } from "./html.ts";
import type { ResolutionPreview, Settings } from "./tauri";

/** Inner HTML of the per-activity model summary, rebuilt in place by main.ts on
 *  every debounced preview refresh. Reads the resolution preview so it reflects
 *  unsaved edits (mode, key, model, tier). Empty string when no preview yet. */
export function roleSummaryInner(preview: ResolutionPreview | null): string {
  if (!preview || preview.roles.length === 0) return "";
  const byRole = (role: string) => preview.roles.find(entry => entry.role === role);
  const distill = byRole("distill");
  if (!distill) return "";
  const cue = byRole("cue");
  const indexing = byRole("indexing");
  const overrideActive = Boolean(cue?.changed_by_override);
  const parts: string[] = [];

  if (preview.mode === "chatgpt") {
    if (overrideActive && cue) {
      parts.push(`Notes: ${esc(distill.model_label)} via your subscription`);
      parts.push(`Quick assists: ${esc(cue.model_label)} <em>(your choice)</em>`);
    } else {
      parts.push(`Notes &amp; quick assists: ${esc(distill.model_label)} via your subscription`);
    }
  } else if (overrideActive && cue) {
    parts.push(`Quick assists: ${esc(cue.model_label)} <em>(your choice)</em> — used for live cues and note re-runs`);
    parts.push(`Notes: ${esc(distill.model_label)} <em>(unchanged)</em>`);
    if (indexing) parts.push(`Note search: ${esc(indexing.model_label)}`);
  } else {
    if (cue && cue.model_id === distill.model_id) {
      parts.push(`Notes &amp; quick assists: ${esc(distill.model_label)}`);
    } else {
      parts.push(`Notes: ${esc(distill.model_label)}`);
      if (cue) parts.push(`Quick assists: ${esc(cue.model_label)}`);
    }
    if (indexing) parts.push(`Note search: ${esc(indexing.model_label)}`);
  }

  return parts
    .map(part => `<span class="ai-role-summary-item">${part}</span>`)
    .join(`<span class="ai-role-summary-sep" aria-hidden="true">·</span>`);
}

export type QuickAssistsSectionKind = "chatgpt" | "external-key" | "tier";

/**
 * Which quick-assists section to render:
 * - `chatgpt`: cues follow the subscription model — no tier control.
 * - `external-key`: a separate cue provider is configured in settings and
 *   managed outside this screen — surface a notice, never a tier control.
 * - `tier`: the normal Balanced / Fastest / Custom tier control.
 */
export function quickAssistsSectionKind(
  currentAiMode: string,
  settings: Pick<Settings, "backchannel_api_key">,
): QuickAssistsSectionKind {
  if (currentAiMode === "chatgpt") return "chatgpt";
  if (settings.backchannel_api_key?.trim()) return "external-key";
  return "tier";
}
