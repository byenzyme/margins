export type CirclePhase =
  | "idle"
  | "starting"
  | "recording"
  | "paused"
  | "finalizing"
  | "needs_attention";

const PHASES = new Set<CirclePhase>([
  "idle",
  "starting",
  "recording",
  "paused",
  "finalizing",
  "needs_attention",
]);

export function parseCirclePhase(value: string | null | undefined): CirclePhase {
  return value && PHASES.has(value as CirclePhase) ? value as CirclePhase : "idle";
}

export function circleStatusLabel(phase: CirclePhase): string {
  switch (phase) {
    case "starting": return "Starting";
    case "recording": return "Live";
    case "paused": return "Paused";
    case "finalizing": return "Finishing";
    case "needs_attention": return "Needs attention";
    case "idle": return "Idle";
  }
}

export interface CircleControls {
  pause: boolean;
  resume: boolean;
  finish: boolean;
  openMargins: boolean;
}

export function circleControls(phase: CirclePhase): CircleControls {
  return {
    pause: phase === "recording",
    resume: phase === "paused",
    finish: phase === "recording" || phase === "paused" || phase === "needs_attention",
    openMargins: true,
  };
}

export function circleAccessibleTitle(phase: CirclePhase): string {
  const status = circleStatusLabel(phase).toLowerCase();
  return `Margins capture — ${status}. Click to toggle the active Pad.`;
}
