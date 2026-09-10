// Quick-assists ("backchannel") speed tier — the single source of truth for how
// the settings UI maps a tier choice to the persisted override fields, and back.
// Kept DOM-free and side-effect-free so it can be unit-tested directly and
// shared by both the renderer (render/settings.ts) and the collector (main.ts).

import type { Settings } from "./tauri";

/** Model used by the "Fastest" tier and pinned for included-mode indexing. */
export const FASTEST_CUE_MODEL = "google/gemini-3.1-flash-lite";

export type CueTier = "balanced" | "fastest" | "custom";

/** Derive the current tier from the persisted override fields. Balanced = no
 *  override; Fastest = the flash-lite model; Custom = any other explicit model. */
export function cueTierFromSettings(settings: Settings): CueTier {
  if (settings.backchannel_same_as_distill !== false) return "balanced";
  const model = settings.backchannel_model?.trim();
  if (!model) return "balanced";
  if (model === FASTEST_CUE_MODEL) return "fastest";
  return "custom";
}

export type CueBackchannelFields = Pick<
  Settings,
  "backchannel_same_as_distill" | "backchannel_model" | "backchannel_api_key" | "backchannel_base_url"
>;

/**
 * Persisted backchannel fields for a tier selection.
 *
 * `tier === null` means the tier control was not rendered (ChatGPT mode, or an
 * externally-configured separate cue key managed outside this screen) — in that
 * case the persisted values are preserved verbatim so the settings-file escape
 * hatch keeps working at runtime. The cue key / base URL are never editable in
 * the UI and are always carried through untouched.
 */
export function backchannelFieldsForTier(
  tier: CueTier | null,
  customModel: string,
  persisted: CueBackchannelFields,
): CueBackchannelFields {
  const preserved = {
    backchannel_api_key: persisted.backchannel_api_key || null,
    backchannel_base_url: persisted.backchannel_base_url || null,
  };
  if (tier === null) {
    return {
      backchannel_same_as_distill: persisted.backchannel_same_as_distill ?? true,
      backchannel_model: persisted.backchannel_model || null,
      ...preserved,
    };
  }
  if (tier === "fastest") {
    return { backchannel_same_as_distill: false, backchannel_model: FASTEST_CUE_MODEL, ...preserved };
  }
  if (tier === "custom") {
    const model = customModel.trim();
    // An empty custom field means "nothing chosen yet" — fall back to Balanced
    // so we never persist a separate-cue flag with no model behind it.
    return { backchannel_same_as_distill: model ? false : true, backchannel_model: model || null, ...preserved };
  }
  return { backchannel_same_as_distill: true, backchannel_model: null, ...preserved };
}
