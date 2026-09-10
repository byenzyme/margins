// Capture uses 0 for Auto. Imports intentionally never pass Auto and therefore
// continue to resolve their configured 1-4 selection through this same mapping.
export function speakerCountToMax(value: number): number | null {
  if (!Number.isFinite(value) || value <= 0) return null;
  const count = Math.max(1, Math.min(4, Math.round(value)));
  return count >= 4 ? 8 : count;
}
