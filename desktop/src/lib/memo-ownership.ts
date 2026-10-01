import type { MemoLine } from "./tauri";

export interface PendingMemoDraft {
  sessionName: string;
  text: string;
  startSecs: number | null;
}

export interface PendingMemoSyncContext {
  createdSecs: number;
  blockOrdinal: number | null;
  audioPendingAtMark?: boolean;
}

export function pendingMemoDraftForSession(
  draft: PendingMemoDraft | null,
  sessionName: string | null | undefined,
): PendingMemoDraft | null {
  const expected = sessionName?.trim();
  return draft && expected && draft.sessionName === expected ? draft : null;
}

/** Build a durable snapshot without committing the pending editor row in the UI. */
export function memoLinesWithPendingDraft(
  lines: MemoLine[],
  draft: PendingMemoDraft | null,
  sessionName: string | null | undefined,
  context: PendingMemoSyncContext,
): MemoLine[] {
  const ownedDraft = pendingMemoDraftForSession(draft, sessionName);
  const text = ownedDraft?.text.trim() ?? "";
  if (!text) return lines.map(line => ({ ...line }));

  return [
    ...lines.map(line => ({ ...line })),
    {
      text,
      created_secs: context.createdSecs,
      edited_secs: null,
      draft_started_secs: context.blockOrdinal === null ? ownedDraft?.startSecs ?? null : null,
      audio_pending_at_mark: context.audioPendingAtMark || undefined,
      block_ordinal: context.blockOrdinal,
    },
  ];
}
