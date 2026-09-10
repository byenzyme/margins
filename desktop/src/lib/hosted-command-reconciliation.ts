export type AmbiguousStopResolution = "completed" | "retained" | "unknown";

/** Exact-ID status is the authority after an ambiguous Stop response. A
 * missing ID means Stop committed and no phantom recovery heartbeat should be
 * installed; a present ID remains retryable. */
export function resolveAmbiguousHostedStop(
  exactStatusExists: boolean | null,
): AmbiguousStopResolution {
  if (exactStatusExists === false) return "completed";
  if (exactStatusExists === true) return "retained";
  return "unknown";
}

export function hostedRecordingLookupWasMissing(error: unknown): boolean {
  return /No hosted recording for ID/.test(String(error));
}

export function resolveAmbiguousHostedDiscard(
  exactStatusExists: boolean | null,
  recoveryPhase?: string | null,
): "completed" | "cleanup_pending" | "unknown" {
  if (exactStatusExists === false) return "completed";
  if (exactStatusExists === true && recoveryPhase === "cleanup_pending") return "cleanup_pending";
  return "unknown";
}
