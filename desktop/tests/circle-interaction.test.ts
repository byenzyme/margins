import assert from "node:assert/strict";
import test from "node:test";

import {
  COLLAPSED_CIRCLE_SIZE,
  EXPANDED_CIRCLE_WIDTH,
  PANEL_TRANSITION_MS,
  expandedCircleHeight,
  panelTransitionDurationMs,
  shouldStartWindowDrag,
} from "../src/lib/circle-interaction.ts";

test("expanded controls preserve the collapsed footprint and add one row per action", () => {
  assert.deepEqual(COLLAPSED_CIRCLE_SIZE, { width: 112, height: 104 });
  assert.equal(EXPANDED_CIRCLE_WIDTH, 190);
  assert.equal(expandedCircleHeight(3), 240);
  assert.equal(expandedCircleHeight(4), 280);
  assert.equal(expandedCircleHeight(-2), 120);
  assert.equal(expandedCircleHeight(Number.NaN), 120);
});

test("only a primary pointer can start the native window drag", () => {
  assert.equal(shouldStartWindowDrag(0, true), true);
  assert.equal(shouldStartWindowDrag(0, false), false);
  assert.equal(shouldStartWindowDrag(1, true), false);
  assert.equal(shouldStartWindowDrag(2, true), false);
});

test("panel transition is smooth by default and immediate for reduced motion", () => {
  assert.equal(panelTransitionDurationMs(false), PANEL_TRANSITION_MS);
  assert.equal(panelTransitionDurationMs(true), 0);
});
