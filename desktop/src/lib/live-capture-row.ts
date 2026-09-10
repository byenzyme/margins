import type { SessionInfo } from "./tauri";

/**
 * Re-assert the live (or starting) capture's row on a freshly listed session
 * array.
 *
 * The backend writes the session to disk *before* mic/system audio engage and
 * only registers it as recording once startup completes, so any session-list
 * refresh that lands inside that warmup window — or between a capture stop and
 * the next status poll — reports the capture as an ordinary settled note.
 * Whoever replaces the frontend `sessions` array with such a list silently
 * downgrades the live editable workspace to the read-only note view until the
 * next status merge flips it back. Routing every wholesale list replacement
 * through this guard keeps the capture row present and `recording` for as long
 * as the frontend believes a capture is starting or live.
 */
export function preserveLiveCaptureRow(
  listed: SessionInfo[],
  captureName: string | null,
  fallbackRow: SessionInfo | null,
  memoLineCount: number,
): SessionInfo[] {
  const name = captureName?.trim();
  if (!name) return listed;
  const index = listed.findIndex(s => s.name === name);
  if (index >= 0) {
    const next = listed.slice();
    next[index] = { ...listed[index], status: "recording", memo_line_count: memoLineCount };
    return next;
  }
  if (!fallbackRow) return listed;
  return [{ ...fallbackRow, status: "recording", memo_line_count: memoLineCount }, ...listed];
}

export interface RefreshMergeContext {
  /** Live/starting capture to keep as a `recording` row (see preserveLiveCaptureRow). */
  captureName: string | null;
  captureFallback: SessionInfo | null;
  memoLineCount: number;
  /** Session currently driving a distill/import run on the frontend. */
  processingName: string | null;
}

/**
 * Merge a freshly listed session array with the client-only state the backend
 * cannot know about yet. Beyond the live capture row, two more optimistic
 * surfaces are clobbered by wholesale replacement:
 *
 * - Import rows: `import_status`/`import_error`/`import_source_path` are
 *   client-only fields on optimistic rows. Error rows and queued placeholders
 *   exist nowhere on the backend; an in-flight import's row may be listed but
 *   without its progress badge.
 * - The optimistic `processing` stamp: reprocess/import flows stamp a row
 *   `processing` before the backend has written its processing_state, so a
 *   refresh in that window reports `unprocessed` and flips the UI back.
 */
export function mergeRefreshedSessions(
  listed: SessionInfo[],
  current: SessionInfo[],
  ctx: RefreshMergeContext,
): SessionInfo[] {
  let rows = preserveLiveCaptureRow(listed, ctx.captureName, ctx.captureFallback, ctx.memoLineCount);

  const missingImportRows: SessionInfo[] = [];
  for (const row of current) {
    if (!row.import_status) continue;
    // Error rows and queued placeholders are purely client-side and must
    // survive until the user acts on them; transcribing/note rows only while
    // their import is actually the active processing run — once the flow
    // clears processingName, the backend row is the truth and the badge drops.
    const preserve = row.import_status === "error"
      || row.import_status === "queued"
      || ctx.processingName === row.name;
    if (!preserve) continue;
    const index = rows.findIndex(s => s.name === row.name);
    if (index >= 0) {
      rows = rows === listed ? rows.slice() : rows;
      rows[index] = {
        ...rows[index],
        import_status: row.import_status,
        import_error: row.import_error ?? null,
        import_source_path: row.import_source_path ?? null,
      };
    } else {
      missingImportRows.push(row);
    }
  }
  if (missingImportRows.length) rows = [...missingImportRows, ...rows];

  if (ctx.processingName) {
    const index = rows.findIndex(s => s.name === ctx.processingName);
    const wasProcessing = current.find(s => s.name === ctx.processingName)?.status === "processing";
    if (index >= 0 && wasProcessing && rows[index].status === "unprocessed") {
      rows = rows === listed ? rows.slice() : rows;
      rows[index] = { ...rows[index], status: "processing" };
    }
  }

  return rows;
}
