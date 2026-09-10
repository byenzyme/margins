import type { RecordingStatus, WebRecordingRecoveryStatus } from "./tauri";

/** Resolve the compatibility `get_recording_status` response into an active
 * capture. Hosted servers may return the oldest retained recovery when there
 * is no active capture, so `is_recording` alone is not an active-only signal. */
export function activeHostedPreflightStatus(
  status: RecordingStatus,
  recoveries: readonly WebRecordingRecoveryStatus[],
): RecordingStatus | null {
  const recordingId = status.web_recording_id;
  if (!status.is_recording || !recordingId) return null;
  const recoveryStatus = [
    ...(status.web_recoveries ?? []),
    ...recoveries,
  ].some(recovery => recovery.recording_id === recordingId);
  return recoveryStatus
    ? null
    : status;
}

/** Production HTTP preflight with injectable backend calls for deterministic
 * integration coverage of backend-shaped responses. */
export async function runHostedStartPreflight(
  loadStatus: () => Promise<RecordingStatus>,
  loadRecoveries: () => Promise<WebRecordingRecoveryStatus[]>,
): Promise<RecordingStatus | null> {
  const [status, recoveries] = await Promise.all([loadStatus(), loadRecoveries()]);
  return activeHostedPreflightStatus(status, recoveries);
}
