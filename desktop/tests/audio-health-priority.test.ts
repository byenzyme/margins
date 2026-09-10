import assert from "node:assert/strict";
import test from "node:test";

import { shouldShowCaptureDeviceToast } from "../src/lib/audio-health-priority.ts";

test("mic recovery cannot mask actionable computer-audio health", () => {
  for (const status of ["connecting", "blocked", "quiet", "silent", "dead"]) {
    assert.equal(shouldShowCaptureDeviceToast(status, null), false, status);
  }
  assert.equal(shouldShowCaptureDeviceToast("ok", "Computer audio needs attention"), false);
});

test("mic recovery can appear when computer audio is healthy or not expected", () => {
  assert.equal(shouldShowCaptureDeviceToast("ok", null), true);
  assert.equal(shouldShowCaptureDeviceToast("not_expected", null), true);
});
