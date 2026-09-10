/** Pure geometry and cadence policy for the floating capture mark. */

export interface Point {
  x: number;
  y: number;
}

export interface Rect {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

export interface GazeVector {
  nearby: boolean;
  x: number;
  y: number;
}

export const GAZE_RADIUS = 160;
export const GAZE_DEAD_ZONE = 0.12;
export const NEAR_POLL_MS = 40;
export const FAR_POLL_MS = 160;

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

/** Convert Tauri's desktop-global physical pixels into DOM-local logical px. */
export function globalPhysicalToLocalLogical(
  cursor: Point,
  windowOrigin: Point,
  scaleFactor: number,
): Point {
  const scale = Number.isFinite(scaleFactor) && scaleFactor > 0 ? scaleFactor : 1;
  return {
    x: (cursor.x - windowOrigin.x) / scale,
    y: (cursor.y - windowOrigin.y) / scale,
  };
}

/**
 * Return a shared, radially normalized eye-gaze vector. The dead zone removes
 * tiny neutral-position jitter; outside the activation radius is exactly calm.
 */
export function normalizedGaze(
  point: Point,
  faceRect: Rect,
  radius = GAZE_RADIUS,
  deadZone = GAZE_DEAD_ZONE,
): GazeVector {
  const centerX = (faceRect.left + faceRect.right) / 2;
  const centerY = (faceRect.top + faceRect.bottom) / 2;
  const dx = point.x - centerX;
  const dy = point.y - centerY;
  const distance = Math.hypot(dx, dy);
  const safeRadius = Math.max(1, radius);

  if (distance > safeRadius) return { nearby: false, x: 0, y: 0 };

  const deadZoneDistance = safeRadius * clamp(deadZone, 0, 0.95);
  if (distance <= deadZoneDistance) return { nearby: true, x: 0, y: 0 };

  const magnitude = clamp(
    (distance - deadZoneDistance) / (safeRadius - deadZoneDistance),
    0,
    1,
  );
  return {
    nearby: true,
    x: clamp((dx / distance) * magnitude, -1, 1),
    y: clamp((dy / distance) * magnitude, -1, 1),
  };
}

export function pointInRect(point: Point, rect: Rect): boolean {
  return point.x >= rect.left && point.x <= rect.right
    && point.y >= rect.top && point.y <= rect.bottom;
}

export function pointInInteractiveRects(point: Point, rects: Rect[]): boolean {
  return rects.some(rect => pointInRect(point, rect));
}

/** 25 Hz nearby and 6.25 Hz when calm/far. */
export function pointerPollCadenceMs(nearby: boolean): number {
  return nearby ? NEAR_POLL_MS : FAR_POLL_MS;
}

/** Partial blink spacing, clamped to the approved four-to-seven-second range. */
export function blinkDelayMs(randomUnit: number): number {
  return 4_000 + clamp(randomUnit, 0, 1) * 3_000;
}

/** Stateful equality gate used to avoid repeated native setter calls. */
export function createStateChangeGate<T>(initial: T): {
  current(): T;
  shouldApply(next: T): boolean;
} {
  let current = initial;
  return {
    current: () => current,
    shouldApply(next: T): boolean {
      if (Object.is(current, next)) return false;
      current = next;
      return true;
    },
  };
}
