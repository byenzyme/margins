import type { RecordingStatus } from "./tauri";

/**
 * Reconcile the persisted setup result with stronger evidence from a live
 * native session. A tap that was previously tested can no longer be treated
 * as ready once an expected system channel fails to deliver any frames.
 */
export function systemAudioReadinessAfterRecordingStatus(
  persistedReady: boolean,
  status: Pick<
    RecordingStatus,
    "is_recording" | "system_audio_expected" | "system_audio_observed" | "tap_status"
  >,
): boolean {
  if (!persistedReady) return false;
  const liveDeliveryFailed = status.is_recording
    && status.system_audio_expected === true
    && status.system_audio_observed !== true
    && status.tap_status === "blocked";
  return !liveDeliveryFailed;
}
