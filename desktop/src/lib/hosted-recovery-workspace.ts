export class HostedRecoveryWorkspaceStore<TSnapshot, TArtifact> {
  private readonly activeSnapshots = new Map<string, TSnapshot>();
  private readonly finishedArtifacts = new Map<string, TArtifact>();

  rememberActive(recordingId: string, snapshot: TSnapshot): void {
    this.activeSnapshots.set(recordingId, snapshot);
  }

  takeActive(recordingId: string): TSnapshot | null {
    const snapshot = this.activeSnapshots.get(recordingId) ?? null;
    this.activeSnapshots.delete(recordingId);
    return snapshot;
  }

  queueFinished(recordingId: string, artifact: TArtifact): void {
    this.finishedArtifacts.set(recordingId, artifact);
  }

  finished(recordingId: string): TArtifact | null {
    return this.finishedArtifacts.get(recordingId) ?? null;
  }
}

export function hostedMemoSyncAllowed(options: {
  recordingId: string | null | undefined;
  selectedRecoveryId: string | null;
  selectedRecoveryHydrated: boolean;
  locallyOwned: boolean;
}): boolean {
  if (!options.recordingId || !options.locallyOwned) return false;
  if (!options.selectedRecoveryId) return true;
  return options.selectedRecoveryId === options.recordingId
    && options.selectedRecoveryHydrated;
}

/** Shared production boundary for Finish and Discard. The mutation completes
 * first, then restoration must finish (including memo/editor hydration) before
 * the handler may restart polling or run a sync tick. */
export async function executeHostedRecoveryMutation<TResult, TActive>(options: {
  completedRecordingId: string;
  mutate: () => Promise<TResult>;
  restoreActive: (completedRecordingId: string) => Promise<TActive>;
}): Promise<{ result: TResult; active: TActive }> {
  const result = await options.mutate();
  const active = await options.restoreActive(options.completedRecordingId);
  return { result, active };
}
