import assert from "node:assert/strict";
import test from "node:test";

import { speakerCountToMax } from "../src/lib/speaker-count.ts";

test("Auto does not request forced speaker-count retranscription", () => {
  assert.equal(speakerCountToMax(0), null);
});

test("explicit speaker counts retain their offline diarization caps", () => {
  assert.equal(speakerCountToMax(1), 1);
  assert.equal(speakerCountToMax(2), 2);
  assert.equal(speakerCountToMax(3), 3);
  assert.equal(speakerCountToMax(4), 8);
});
