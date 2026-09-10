import assert from "node:assert/strict";
import test from "node:test";

import { jobBannerCancelAction } from "../src/lib/job-banner-action.ts";

test("failed job banner dismisses the persisted error", () => {
  assert.equal(jobBannerCancelAction(true), "__dismissNoteError");
});

test("active job banner still cancels the running process", () => {
  assert.equal(jobBannerCancelAction(false), "__cancelProcessing");
});
