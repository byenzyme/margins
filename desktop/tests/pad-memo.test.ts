import assert from "node:assert/strict";
import test from "node:test";

import { parsePadMemo } from "../src/lib/pad-memo.ts";

test("Pad renders timed, edited, startup, and paused canonical memo lines", () => {
  assert.deepEqual(parsePadMemo([
    "[00:04] first mark",
    "[01:02 ~01:10] edited mark",
    "[02:00] (audio not live yet) early mark",
    "[block 2] paused thought",
    "# ignored heading",
  ].join("\n")), [
    { text: "first mark", timestamp: "00:04", blockOrdinal: null },
    { text: "edited mark", timestamp: "01:02", blockOrdinal: null },
    { text: "early mark", timestamp: "02:00", blockOrdinal: null },
    { text: "paused thought", timestamp: "Paused", blockOrdinal: 2 },
  ]);
});
