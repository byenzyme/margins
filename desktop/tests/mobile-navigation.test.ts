import assert from "node:assert/strict";
import test from "node:test";

import {
  isMobileNavigationViewport,
  MOBILE_NAVIGATION_QUERY,
  sidebarToggleLabel,
} from "../src/lib/mobile-navigation.ts";

test("mobile navigation uses the shared phone breakpoint", () => {
  const queries: string[] = [];
  const host = {
    matchMedia(query: string) {
      queries.push(query);
      return { matches: true };
    },
  };

  assert.equal(isMobileNavigationViewport(host), true);
  assert.deepEqual(queries, [MOBILE_NAVIGATION_QUERY]);
});

test("mobile navigation is closed when matchMedia is absent or does not match", () => {
  assert.equal(isMobileNavigationViewport(undefined), false);
  assert.equal(isMobileNavigationViewport({ matchMedia: () => ({ matches: false }) }), false);
});

test("toggle labels describe navigation on mobile and a sidebar on desktop", () => {
  assert.equal(sidebarToggleLabel(true, true), "Open navigation");
  assert.equal(sidebarToggleLabel(false, true), "Close navigation");
  assert.equal(sidebarToggleLabel(true, false), "Expand sidebar");
  assert.equal(sidebarToggleLabel(false, false), "Collapse sidebar");
});
