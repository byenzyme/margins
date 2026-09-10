import type { ProcessingEvent } from "./tauri";

export type CapturePhase = "recording" | "paused" | "finalizing" | "captured";
export type TranscriptPhase = "none" | "live_partial" | "preparing" | "ready" | "failed";
export type NotePhase = "idle" | "queued" | "preparing_context" | "writing" | "saving" | "saved" | "failed" | "cancelled";
export type ProcessingTrack = "capture" | "transcript" | "note";

export type TrackState<TPhase extends string> = {
  phase: TPhase;
  message: string;
  updatedAtMs: number;
};

export type SessionLifecycleState = {
  session: string;
  capture: TrackState<CapturePhase>;
  transcript: TrackState<TranscriptPhase>;
  note: TrackState<NotePhase>;
};

export type SessionJobState = {
  status: "capturing" | "captured" | "processing" | "saved" | "failed" | "cancelled";
  message: string;
};

export function newSessionLifecycleState(session: string): SessionLifecycleState {
  return {
    session,
    capture: { phase: "captured", message: "Capture saved.", updatedAtMs: 0 },
    transcript: { phase: "none", message: "Transcript not prepared yet.", updatedAtMs: 0 },
    note: { phase: "idle", message: "Ready to write note.", updatedAtMs: 0 },
  };
}

export function reduceSessionLifecycle(
  current: SessionLifecycleState | undefined,
  event: ProcessingEvent,
  fallbackSession: string,
): SessionLifecycleState {
  const session = event.session || fallbackSession;
  const next = cloneLifecycle(current || newSessionLifecycleState(session));
  next.session = session;

  const track = normalizeProcessingTrack(event.track);
  if (track && event.phase) {
    // `note_stream` events carry the streamed note body itself in `message`
    // (each chunk is raw note text — the first is literally "---", then YAML
    // frontmatter, then prose). That is note content, not a status line: it has
    // its own render surface in the note body, and echoing it here flashes raw
    // fragments in the status strip above the heading. Keep a stable phase label.
    const message = event.stage === "note_stream"
      ? lifecycleMessage(track, event.phase)
      : event.message || lifecycleMessage(track, event.phase);
    const updatedAtMs = event.emitted_at_ms ?? Date.now();
    if (track === "capture" && isCapturePhase(event.phase)) {
      next.capture = { phase: event.phase, message, updatedAtMs };
    } else if (track === "transcript" && isTranscriptPhase(event.phase)) {
      next.transcript = { phase: event.phase, message, updatedAtMs };
    } else if (track === "note" && isNotePhase(event.phase)) {
      next.note = { phase: event.phase, message, updatedAtMs };
    }
    return next;
  }

  return reduceLegacyProcessingEvent(next, event);
}

export function deriveSessionJobState(state: SessionLifecycleState | undefined): SessionJobState {
  if (!state) return { status: "processing", message: "Writing a note from the recording and marks..." };

  if (state.note.phase === "failed") return { status: "failed", message: state.note.message || "Could not finish the note." };
  if (state.note.phase === "cancelled") return { status: "cancelled", message: state.note.message || "Note writing was cancelled." };
  if (state.note.phase === "saved") return { status: "saved", message: state.note.message || "Note saved." };
  if (state.note.phase === "saving") return { status: "processing", message: state.note.message || "Saving note." };
  if (state.note.phase === "writing") return { status: "processing", message: state.note.message || "Writing note." };
  if (state.note.phase === "preparing_context") return { status: "processing", message: state.note.message || "Preparing context." };
  if (state.transcript.phase === "failed") return { status: "failed", message: state.transcript.message || "Could not prepare transcript." };
  if (state.transcript.phase === "preparing") return { status: "processing", message: state.transcript.message || "Preparing transcript." };
  if (state.capture.phase === "finalizing") return { status: "capturing", message: state.capture.message || "Finishing capture." };
  if (state.capture.phase === "captured") return { status: "captured", message: state.capture.message || "Capture saved." };
  return { status: "processing", message: state.note.message || "Writing a note from the recording and marks..." };
}

export function sessionLifecycleFromRecording(
  session: string,
  phase: CapturePhase,
  message: string,
): SessionLifecycleState {
  const state = newSessionLifecycleState(session);
  state.capture = { phase, message, updatedAtMs: Date.now() };
  state.note = { phase: "idle", message: "Ready to write note.", updatedAtMs: Date.now() };
  return state;
}

function reduceLegacyProcessingEvent(state: SessionLifecycleState, event: ProcessingEvent): SessionLifecycleState {
  const updatedAtMs = event.emitted_at_ms ?? Date.now();
  if (event.stage === "note_stream") {
    state.note = { phase: "writing", message: "Writing note.", updatedAtMs };
  } else if (event.stage === "complete") {
    state.note = { phase: "saved", message: event.message || "Note saved.", updatedAtMs };
  } else if (event.stage === "error") {
    state.note = { phase: "failed", message: event.message || "Could not finish the note.", updatedAtMs };
  } else if (event.stage === "align" || event.stage === "transcribe") {
    const ready = /ready|using prepared|using stored/i.test(event.message || "");
    state.transcript = {
      phase: ready ? "ready" : "preparing",
      message: event.message || (ready ? "Transcript ready." : "Preparing transcript."),
      updatedAtMs,
    };
  } else if (event.stage === "prepare" || event.stage === "context") {
    state.note = { phase: "preparing_context", message: event.message || "Preparing context.", updatedAtMs };
  } else if (event.stage === "synthesize") {
    state.note = { phase: "writing", message: event.message || "Writing note.", updatedAtMs };
  } else if (event.stage === "cleanup") {
    state.note = { phase: "saving", message: event.message || "Saving note.", updatedAtMs };
  }
  return state;
}

function cloneLifecycle(state: SessionLifecycleState): SessionLifecycleState {
  return {
    session: state.session,
    capture: { ...state.capture },
    transcript: { ...state.transcript },
    note: { ...state.note },
  };
}

function lifecycleMessage(track: ProcessingTrack, phase: string): string {
  if (track === "capture") {
    if (phase === "finalizing") return "Finishing capture.";
    if (phase === "captured") return "Capture saved.";
    if (phase === "paused") return "Capture paused.";
    return "Recording.";
  }
  if (track === "transcript") return phase === "ready" ? "Transcript ready." : "Preparing transcript.";
  if (phase === "saved") return "Note saved.";
  if (phase === "saving") return "Saving note.";
  if (phase === "writing") return "Writing note.";
  if (phase === "preparing_context") return "Preparing context.";
  if (phase === "failed") return "Could not finish the note.";
  return "Ready to write note.";
}

function normalizeProcessingTrack(track: ProcessingEvent["track"]): ProcessingTrack | null {
  return track === "capture" || track === "transcript" || track === "note" ? track : null;
}

function isCapturePhase(phase: string): phase is CapturePhase {
  return ["recording", "paused", "finalizing", "captured"].includes(phase);
}

function isTranscriptPhase(phase: string): phase is TranscriptPhase {
  return ["none", "live_partial", "preparing", "ready", "failed"].includes(phase);
}

function isNotePhase(phase: string): phase is NotePhase {
  return ["idle", "queued", "preparing_context", "writing", "saving", "saved", "failed", "cancelled"].includes(phase);
}
