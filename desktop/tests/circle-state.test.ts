import assert from "node:assert/strict";
import test from "node:test";

import {
  circleAccessibleTitle,
  circleControls,
  parseCirclePhase,
} from "../src/lib/circle-state.ts";

test("untrusted circle phases fail closed to idle", () => {
  assert.equal(parseCirclePhase("recording"), "recording");
  assert.equal(parseCirclePhase("discarding"), "idle");
  assert.equal(parseCirclePhase(null), "idle");
});

test("secondary controls never expose Discard", () => {
  assert.deepEqual(circleControls("recording"), {
    pause: true,
    resume: false,
    finish: true,
    openMargins: true,
  });
  assert.deepEqual(circleControls("paused"), {
    pause: false,
    resume: true,
    finish: true,
    openMargins: true,
  });
  assert.deepEqual(Object.keys(circleControls("needs_attention")).sort(), [
    "finish",
    "openMargins",
    "pause",
    "resume",
  ]);
});

test("accessible title describes state and the Pad action", () => {
  assert.match(circleAccessibleTitle("recording"), /live/i);
  assert.match(circleAccessibleTitle("recording"), /active Pad/i);
});
