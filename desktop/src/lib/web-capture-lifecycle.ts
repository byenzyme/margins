export interface HostedPauseLifecycle {
  pauseLocal(): Promise<void>;
  resumeLocal(): Promise<void>;
}

export function hostedPostureReconciliationAction(
  serverPaused: boolean,
  recorderState: "inactive" | "recording" | "paused",
): "pause" | "resume" | null {
  if (recorderState === "inactive") return null;
  if (serverPaused && recorderState === "recording") return "pause";
  if (!serverPaused && recorderState === "paused") return "resume";
  return null;
}

/** Apply the local browser transition first, then make the server authoritative.
 * If the request fails, roll the browser resources back so the two sides do
 * not remain observably split. */
export async function convergeHostedPauseState<T>(
  paused: boolean,
  lifecycle: HostedPauseLifecycle,
  updateServer: () => Promise<T>,
  readAuthoritative?: () => Promise<T>,
  isPaused: (status: T) => boolean = status => Boolean((status as { paused?: boolean }).paused),
): Promise<T> {
  const apply = paused ? lifecycle.pauseLocal : lifecycle.resumeLocal;
  const rollback = paused ? lifecycle.resumeLocal : lifecycle.pauseLocal;
  await apply.call(lifecycle);
  try {
    return await updateServer();
  } catch (error) {
    if (readAuthoritative) {
      try {
        const authoritative = await readAuthoritative();
        if (isPaused(authoritative) === paused) return authoritative;
      } catch {
        // The original mutation and reconciliation read both failed. Roll the
        // browser resources back to their last confirmed posture below.
      }
    }
    try {
      await rollback.call(lifecycle);
    } catch (rollbackError) {
      console.warn("[http-backend] failed to roll back browser pause state:", rollbackError);
    }
    throw error;
  }
}
