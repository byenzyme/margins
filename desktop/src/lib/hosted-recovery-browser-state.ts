import type { RecordingStatus, WebRecordingRecoveryStatus } from "./tauri";

/** Browser-only navigation state for hosted recoveries. Display names are
 * deliberately absent from identity decisions. */
export class HostedRecoveryBrowserState {
  private recoveriesById = new Map<string, WebRecordingRecoveryStatus>();
  private selectedId: string | null = null;
  private hydratedId: string | null = null;

  discover(recoveries: readonly WebRecordingRecoveryStatus[]): void {
    this.recoveriesById = new Map(recoveries.map(recovery => [recovery.recording_id, recovery]));
    if (this.selectedId && !this.recoveriesById.has(this.selectedId)) {
      this.selectedId = null;
      this.hydratedId = null;
    }
  }

  list(): WebRecordingRecoveryStatus[] {
    return [...this.recoveriesById.values()];
  }

  select(recordingId: string): void {
    if (!this.recoveriesById.has(recordingId)) {
      throw new Error(`Hosted recovery ${recordingId} is no longer available.`);
    }
    this.selectedId = recordingId;
    if (this.hydratedId !== recordingId) this.hydratedId = null;
  }

  clearSelection(recordingId?: string): void {
    if (recordingId && this.selectedId !== recordingId) return;
    this.selectedId = null;
    this.hydratedId = null;
  }

  selectedRecordingId(): string | null {
    return this.selectedId;
  }

  markHydrated(recordingId: string): void {
    if (this.selectedId === recordingId) this.hydratedId = recordingId;
  }

  selectedMemoReady(recordingId: string | null | undefined): boolean {
    return Boolean(recordingId && this.selectedId === recordingId && this.hydratedId === recordingId);
  }

  acceptsSelectedPoll(requestedRecordingId: string, status: RecordingStatus): boolean {
    return this.selectedId === requestedRecordingId
      && status.web_recording_id === requestedRecordingId;
  }

  isRecovery(recordingId: string | null | undefined): boolean {
    return Boolean(recordingId && this.recoveriesById.has(recordingId));
  }

  /** Global status can represent active B or, on legacy paths, retained A.
   * Only an ID absent from the explicit recovery collection is active. */
  activeStatus(status: RecordingStatus): RecordingStatus | null {
    const recordingId = status.web_recording_id;
    return status.is_recording && recordingId && !this.recoveriesById.has(recordingId)
      ? status
      : null;
  }

  /** Reconcile an exact recovery mutation without allowing compatibility
   * fallback status to replace a separately active capture. */
  completeRecovery(
    recordingId: string,
    recoveries: readonly WebRecordingRecoveryStatus[],
    globalStatus: RecordingStatus,
  ): RecordingStatus | null {
    this.clearSelection(recordingId);
    this.discover(recoveries);
    return this.activeStatus(globalStatus);
  }
}

export function sameHostedRecording(
  left: Pick<RecordingStatus, "web_recording_id">,
  right: Pick<RecordingStatus, "web_recording_id">,
): boolean {
  return Boolean(left.web_recording_id && left.web_recording_id === right.web_recording_id);
}
