import assert from "node:assert/strict";
import test from "node:test";

import { memoLinesWithPendingDraft, pendingMemoDraftForSession } from "../src/lib/memo-ownership.ts";

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

test("periodic memo sync includes the unfinished line without mutating committed state", () => {
  const committed = [{ text: "first", created_secs: 2, edited_secs: null }];
  const synced = memoLinesWithPendingDraft(committed, draft, "capture-a", {
    createdSecs: 15,
    blockOrdinal: null,
    audioPendingAtMark: true,
  });

  assert.equal(committed.length, 1);
  assert.deepEqual(synced, [
    committed[0],
    {
      text: "old pending thought",
      created_secs: 15,
      edited_secs: null,
      draft_started_secs: 12,
      audio_pending_at_mark: true,
      block_ordinal: null,
    },
  ]);
});

test("periodic memo sync excludes drafts owned by another capture", () => {
  const committed = [{ text: "first", created_secs: 2, edited_secs: null }];
  assert.deepEqual(
    memoLinesWithPendingDraft(committed, draft, "capture-b", { createdSecs: 15, blockOrdinal: null }),
    committed,
  );
});
