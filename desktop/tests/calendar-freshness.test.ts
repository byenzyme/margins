import assert from "node:assert/strict";
import test from "node:test";

import { calendarFreshnessLabel } from "../src/lib/calendar-freshness.ts";

test("fresh Calendar evidence has no warning", () => {
  assert.equal(calendarFreshnessLabel({ status: "fresh", stale: false }), null);
});

test("retained Calendar snapshots expose refresh failure", () => {
  assert.equal(
    calendarFreshnessLabel({ status: "error", stale: true, reason: "refresh_failed" }),
    "Calendar refresh failed; showing the last available event",
  );
});
