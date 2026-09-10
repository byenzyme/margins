import assert from "node:assert/strict";
import test from "node:test";

import {
  hostedRecordingLookupWasMissing,
  resolveAmbiguousHostedDiscard,
  resolveAmbiguousHostedStop,
} from "../src/lib/hosted-command-reconciliation.ts";

test("lost successful Stop response removes phantom recovery authority", () => {
  assert.equal(resolveAmbiguousHostedStop(false), "completed");
  assert.equal(hostedRecordingLookupWasMissing(new Error(
    "No hosted recording for ID 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa'",
  )), true);
});

test("ambiguous Stop retains same-tab authority when exact state still exists or lookup fails", () => {
  assert.equal(resolveAmbiguousHostedStop(true), "retained");
  assert.equal(resolveAmbiguousHostedStop(null), "unknown");
  assert.equal(hostedRecordingLookupWasMissing(new Error("network unavailable")), false);
});

test("lost successful Discard clears phantom state but cleanup-pending remains retryable", () => {
  assert.equal(resolveAmbiguousHostedDiscard(false), "completed");
  assert.equal(resolveAmbiguousHostedDiscard(true, "cleanup_pending"), "cleanup_pending");
  assert.equal(resolveAmbiguousHostedDiscard(null), "unknown");
});
