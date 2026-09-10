export const MICROPHONE_QUIET_THRESHOLD = 0.0001;

export type MicrophoneLevelState = "unknown" | "quiet" | "audible";

export function microphoneLevelState(level: number | null | undefined): MicrophoneLevelState {
  if (typeof level !== "number" || !Number.isFinite(level)) return "unknown";
  return level < MICROPHONE_QUIET_THRESHOLD ? "quiet" : "audible";
}

export function isMicrophoneVeryQuiet(level: number | null | undefined): boolean {
  return microphoneLevelState(level) === "quiet";
}

export function microphoneQuietWarning(level: number | null | undefined): string | null {
  return isMicrophoneVeryQuiet(level) ? "Mic is very quiet." : null;
}
