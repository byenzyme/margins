import assert from "node:assert/strict";
import test from "node:test";

import { pendingMemoDraftForSession } from "../src/lib/memo-ownership.ts";

const draft = {
  sessionName: "capture-a",
  text: "old pending thought",
  startSecs: 12,
};

test("pending memo draft stays with its capture", () => {
  assert.equal(pendingMemoDraftForSession(draft, "capture-a"), draft);
});

test("new capture rejects a prior capture draft", () => {
  assert.equal(pendingMemoDraftForSession(draft, "capture-b"), null);
  assert.equal(pendingMemoDraftForSession(draft, ""), null);
});
