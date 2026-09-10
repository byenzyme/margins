/** Pure interaction policy for the floating capture-circle renderer. */

export const COLLAPSED_CIRCLE_SIZE = { width: 112, height: 104 } as const;
export const EXPANDED_CIRCLE_WIDTH = 190;
export const CONTROL_ROW_HEIGHT = 40;
export const CONTROLS_VERTICAL_CHROME = 16;
export const PANEL_TRANSITION_MS = 160;

export function expandedCircleHeight(controlCount: number): number {
  const safeCount = Number.isFinite(controlCount)
    ? Math.max(0, Math.floor(controlCount))
    : 0;
  return COLLAPSED_CIRCLE_SIZE.height
    + safeCount * CONTROL_ROW_HEIGHT
    + CONTROLS_VERTICAL_CHROME;
}

/** Only the primary pointer begins a native window drag. */
export function shouldStartWindowDrag(button: number, isPrimary: boolean): boolean {
  return button === 0 && isPrimary;
}

export function panelTransitionDurationMs(reducedMotion: boolean): number {
  return reducedMotion ? 0 : PANEL_TRANSITION_MS;
}
