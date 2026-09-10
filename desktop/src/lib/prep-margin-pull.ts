import type { MemoLine } from "./tauri";

/**
 * Pulling prep marginalia is a memo mutation, not a transient DOM fill. The
 * returned index lets the caller focus the still-editable committed textarea
 * after rendering and persist the same state to the active capture.
 */
export function appendPulledMarginLine(
  lines: MemoLine[],
  text: string,
  createdSecs: number,
  blockOrdinal: number,
): number | null {
  const trimmed = text.trim();
  if (!trimmed) return null;
  const index = lines.length;
  lines.push({
    text: trimmed,
    created_secs: Math.max(0, createdSecs),
    edited_secs: null,
    draft_started_secs: null,
    block_ordinal: blockOrdinal,
  });
  return index;
}
