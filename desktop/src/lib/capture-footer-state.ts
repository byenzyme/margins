export type CaptureFooterPosture = "pausing" | "paused" | "recording";

export function captureFooterPosture(capturePhase: string | undefined, paused: boolean): CaptureFooterPosture {
  if (capturePhase === "pausing") return "pausing";
  if (paused) return "paused";
  return "recording";
}
