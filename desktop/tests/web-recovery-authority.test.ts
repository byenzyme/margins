import assert from "node:assert/strict";
import test from "node:test";

import {
  classifyHostedAuthorityFailure,
  missingHostedRecoveryAuthorityIds,
  reconcileHostedAuthorityFailure,
  selectWebRecoveryAuthority,
} from "../src/lib/web-recovery-authority.ts";

test("same-name recoveries remain independently addressable by opaque ID", () => {
  const authorities = new Map([
    ["opaque-a", { recordingId: "opaque-a", sessionName: "meeting", ownerId: "owner-a" }],
    ["opaque-b", { recordingId: "opaque-b", sessionName: "meeting", ownerId: "owner-b" }],
  ]);
  assert.equal(selectWebRecoveryAuthority(authorities, "opaque-a", "meeting")?.ownerId, "owner-a");
  assert.equal(selectWebRecoveryAuthority(authorities, "opaque-b", "meeting")?.ownerId, "owner-b");
  assert.equal(selectWebRecoveryAuthority(authorities, null, "meeting"), null, "display name must not select arbitrarily");
});

test("adding or removing B never clears A authority", () => {
  const authorities = new Map([
    ["opaque-a", { recordingId: "opaque-a", sessionName: "a", ownerId: "owner-a" }],
  ]);
  authorities.set("opaque-b", { recordingId: "opaque-b", sessionName: "b", ownerId: "owner-b" });
  authorities.delete("opaque-b");
  assert.equal(selectWebRecoveryAuthority(authorities, "opaque-a")?.ownerId, "owner-a");
});

test("server restart owner loss is reclaimable while missing recovery state is terminal", () => {
  assert.equal(
    classifyHostedAuthorityFailure(new Error("Capture authority for 'meeting' belongs to another browser operation")),
    "owner",
  );
  assert.equal(
    classifyHostedAuthorityFailure(new Error("No hosted recording for ID 'opaque-a'")),
    "missing",
  );
  assert.equal(classifyHostedAuthorityFailure(new Error("network unavailable")), "transient");
});

test("recovery discovery prunes only authorities whose exact IDs disappeared", () => {
  assert.deepEqual(
    missingHostedRecoveryAuthorityIds(["a", "b"], ["b", "c"]),
    ["a"],
  );
});

test("open tab reclaims its exact recovery after server restart", async () => {
  let reclaimed = 0;
  let invalidated = 0;
  const outcome = await reconcileHostedAuthorityFailure(
    new Error("Capture authority for 'meeting' belongs to another browser operation"),
    async () => { reclaimed += 1; },
    () => { invalidated += 1; },
  );
  assert.equal(outcome, "reclaimed");
  assert.equal(reclaimed, 1);
  assert.equal(invalidated, 0);
});

test("open tab invalidates a heartbeat after the exact recovery disappears", async () => {
  let invalidated = 0;
  const outcome = await reconcileHostedAuthorityFailure(
    new Error("No hosted recording for ID 'opaque-a'"),
    async () => assert.fail("missing state cannot be reclaimed"),
    () => { invalidated += 1; },
  );
  assert.equal(outcome, "invalidated");
  assert.equal(invalidated, 1);
});

test("heartbeat owner mismatch followed by live-owner reclaim rejection invalidates stale authority", async () => {
  const events: string[] = [];
  const outcome = await reconcileHostedAuthorityFailure(
    new Error("Capture authority for 'meeting' belongs to another browser operation"),
    async () => {
      events.push("claim");
      throw new Error("Capture 'meeting' still has a live browser owner or fresh audio transport");
    },
    error => events.push(`invalidate:${String(error)}`),
  );
  assert.equal(outcome, "invalidated");
  assert.deepEqual(events, [
    "claim",
    "invalidate:Error: Capture 'meeting' still has a live browser owner or fresh audio transport",
  ]);
});
