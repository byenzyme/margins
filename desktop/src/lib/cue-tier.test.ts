// Quick-assists speed-tier round-trip. Run with:
//   node --test --experimental-strip-types src/lib/cue-tier.test.ts
//
// Guarantees the tier control maps cleanly to the persisted override fields and
// back: Balanced clears the override, Fastest pins the flash-lite model, Custom
// preserves a typed model, and the separate cue key / base URL are never
// clobbered by the tier control.
import { test } from "node:test";
import assert from "node:assert/strict";

import {
  FASTEST_CUE_MODEL,
  backchannelFieldsForTier,
  cueTierFromSettings,
  type CueBackchannelFields,
} from "./cue-tier.ts";
import type { Settings } from "./tauri.ts";

function settings(fields: Partial<Settings>): Settings {
  return fields as Settings;
}

const NO_PERSISTED: CueBackchannelFields = {
  backchannel_same_as_distill: null,
  backchannel_model: null,
  backchannel_api_key: null,
  backchannel_base_url: null,
};

test("cueTierFromSettings derives the tier from override fields", () => {
  assert.equal(cueTierFromSettings(settings({})), "balanced");
  assert.equal(cueTierFromSettings(settings({ backchannel_same_as_distill: true })), "balanced");
  // Separate-cue flag but no model yet is still Balanced.
  assert.equal(cueTierFromSettings(settings({ backchannel_same_as_distill: false })), "balanced");
  assert.equal(
    cueTierFromSettings(settings({ backchannel_same_as_distill: false, backchannel_model: FASTEST_CUE_MODEL })),
    "fastest",
  );
  assert.equal(
    cueTierFromSettings(settings({ backchannel_same_as_distill: false, backchannel_model: "anthropic/claude-haiku-4.5" })),
    "custom",
  );
});

test("Balanced clears the override", () => {
  const result = backchannelFieldsForTier("balanced", "", NO_PERSISTED);
  assert.equal(result.backchannel_same_as_distill, true);
  assert.equal(result.backchannel_model, null);
});

test("Fastest sets the flash-lite model", () => {
  const result = backchannelFieldsForTier("fastest", "", NO_PERSISTED);
  assert.equal(result.backchannel_same_as_distill, false);
  assert.equal(result.backchannel_model, FASTEST_CUE_MODEL);
});

test("Custom preserves a typed model", () => {
  const result = backchannelFieldsForTier("custom", "  my/custom-model  ", NO_PERSISTED);
  assert.equal(result.backchannel_same_as_distill, false);
  assert.equal(result.backchannel_model, "my/custom-model");
});

test("Custom with an empty model falls back to Balanced", () => {
  const result = backchannelFieldsForTier("custom", "   ", NO_PERSISTED);
  assert.equal(result.backchannel_same_as_distill, true);
  assert.equal(result.backchannel_model, null);
});

test("absent tier control preserves persisted override fields", () => {
  const persisted: CueBackchannelFields = {
    backchannel_same_as_distill: false,
    backchannel_model: "some/legacy-model",
    backchannel_api_key: "cue-key",
    backchannel_base_url: "https://cue.example/v1",
  };
  const result = backchannelFieldsForTier(null, "", persisted);
  assert.equal(result.backchannel_same_as_distill, false);
  assert.equal(result.backchannel_model, "some/legacy-model");
  assert.equal(result.backchannel_api_key, "cue-key");
  assert.equal(result.backchannel_base_url, "https://cue.example/v1");
});

test("the separate cue key / base URL survive every tier", () => {
  const persisted: CueBackchannelFields = {
    backchannel_same_as_distill: false,
    backchannel_model: null,
    backchannel_api_key: "cue-key",
    backchannel_base_url: "https://cue.example/v1",
  };
  for (const tier of ["balanced", "fastest", "custom"] as const) {
    const result = backchannelFieldsForTier(tier, "x/y", persisted);
    assert.equal(result.backchannel_api_key, "cue-key", `${tier} key`);
    assert.equal(result.backchannel_base_url, "https://cue.example/v1", `${tier} base url`);
  }
});

test("round-trips a Fastest selection back to the same tier", () => {
  const fields = backchannelFieldsForTier("fastest", "", NO_PERSISTED);
  assert.equal(cueTierFromSettings(settings(fields)), "fastest");
});
