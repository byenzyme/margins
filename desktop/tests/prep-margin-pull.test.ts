import assert from "node:assert/strict";
import test from "node:test";

import { appendPulledMarginLine } from "../src/lib/prep-margin-pull.ts";

test("pulled marginalia immediately becomes an editable clock-stopped memo line", () => {
  const lines = [];
  const index = appendPulledMarginLine(lines, "  A useful prior thread.  ", 12.5, 2);

  assert.equal(index, 0);
  assert.deepEqual(lines, [{
    text: "A useful prior thread.",
    created_secs: 12.5,
    edited_secs: null,
    draft_started_secs: null,
    block_ordinal: 2,
  }]);
});

test("empty marginalia cannot create an invisible memo line", () => {
  const lines = [];
  assert.equal(appendPulledMarginLine(lines, "   ", 3, 1), null);
  assert.deepEqual(lines, []);
});
