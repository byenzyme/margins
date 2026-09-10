import assert from "node:assert/strict";
import test from "node:test";

import {
  FAR_POLL_MS,
  NEAR_POLL_MS,
  blinkDelayMs,
  createStateChangeGate,
  globalPhysicalToLocalLogical,
  normalizedGaze,
  pointInInteractiveRects,
  pointInRect,
  pointerPollCadenceMs,
  type Rect,
} from "../src/lib/circle-pointer.ts";

const face: Rect = { left: 20, top: 30, right: 76, bottom: 86 };

test("global physical cursor coordinates become local logical points at 1x and 2x", () => {
  assert.deepEqual(
    globalPhysicalToLocalLogical({ x: 420, y: 260 }, { x: 300, y: 150 }, 1),
    { x: 120, y: 110 },
  );
  assert.deepEqual(
    globalPhysicalToLocalLogical({ x: 540, y: 370 }, { x: 300, y: 150 }, 2),
    { x: 120, y: 110 },
  );
  assert.deepEqual(
    globalPhysicalToLocalLogical({ x: 12, y: 8 }, { x: 2, y: 3 }, 0),
    { x: 10, y: 5 },
  );
});

test("gaze is shared, dead-zoned, clamped, and neutral outside its radius", () => {
  const center = { x: 48, y: 58 };
  assert.deepEqual(normalizedGaze(center, face), { nearby: true, x: 0, y: 0 });

  const deadZone = normalizedGaze({ x: 52, y: 60 }, face);
  assert.equal(deadZone.nearby, true);
  assert.equal(deadZone.x, 0);
  assert.equal(deadZone.y, 0);

  const rightEdge = normalizedGaze({ x: 208, y: 58 }, face);
  assert.equal(rightEdge.nearby, true);
  assert.equal(rightEdge.x, 1);
  assert.equal(rightEdge.y, 0);

  assert.deepEqual(
    normalizedGaze({ x: 48, y: 219 }, face),
    { nearby: false, x: 0, y: 0 },
  );
});

test("interactive rectangles include their edges but leave transparent gaps click-through", () => {
  const mark = { left: 8, top: 20, right: 64, bottom: 76 };
  const more = { left: 72, top: 35, right: 98, bottom: 61 };
  assert.equal(pointInRect({ x: 8, y: 20 }, mark), true);
  assert.equal(pointInRect({ x: 64, y: 76 }, mark), true);
  assert.equal(pointInInteractiveRects({ x: 80, y: 48 }, [mark, more]), true);
  assert.equal(pointInInteractiveRects({ x: 68, y: 48 }, [mark, more]), false);
  assert.equal(pointInInteractiveRects({ x: 5, y: 5 }, [mark, more]), false);
});

test("pointer polling and blink cadence stay within the restrained contract", () => {
  assert.equal(pointerPollCadenceMs(true), NEAR_POLL_MS);
  assert.equal(pointerPollCadenceMs(false), FAR_POLL_MS);
  assert.equal(blinkDelayMs(-1), 4_000);
  assert.equal(blinkDelayMs(0.5), 5_500);
  assert.equal(blinkDelayMs(2), 7_000);
});

test("state-change gating suppresses repeated native ignore-cursor setters", () => {
  const gate = createStateChangeGate(true);
  assert.equal(gate.shouldApply(true), false);
  assert.equal(gate.shouldApply(false), true);
  assert.equal(gate.shouldApply(false), false);
  assert.equal(gate.shouldApply(true), true);
  assert.equal(gate.current(), true);
});
