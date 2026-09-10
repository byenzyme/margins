export interface PendingMemoDraft {
  sessionName: string;
  text: string;
  startSecs: number | null;
}

export function pendingMemoDraftForSession(
  draft: PendingMemoDraft | null,
  sessionName: string | null | undefined,
): PendingMemoDraft | null {
  const expected = sessionName?.trim();
  return draft && expected && draft.sessionName === expected ? draft : null;
}
