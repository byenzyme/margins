import assert from "node:assert/strict";
import test from "node:test";

import {
  isMicrophoneVeryQuiet,
  microphoneLevelState,
  microphoneQuietWarning,
} from "../src/lib/microphone-level.ts";

test("unknown hosted microphone telemetry is not classified as silence", () => {
  assert.equal(microphoneLevelState(null), "unknown");
  assert.equal(isMicrophoneVeryQuiet(null), false);
  assert.equal(microphoneQuietWarning(null), null);
});

test("hosted microphone levels classify real quiet and audible samples", () => {
  assert.equal(microphoneLevelState(0.00002), "quiet");
  assert.equal(isMicrophoneVeryQuiet(0.00002), true);
  assert.equal(microphoneQuietWarning(0.00002), "Mic is very quiet.");

  assert.equal(microphoneLevelState(0.01), "audible");
  assert.equal(isMicrophoneVeryQuiet(0.01), false);
  assert.equal(microphoneQuietWarning(0.01), null);
});
