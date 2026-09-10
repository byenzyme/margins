import type { ProcessingEvent } from "./tauri";

export type DistillTraceEvent = {
  kind: "stage" | "tool" | "chat" | "error";
  label: string;
  detail: string;
  status?: "active" | "done" | "error";
};

export type AgentSidebarMessage = { title: string; detail: string; tone?: "error" | "done" | "active" };

export function agentEventMessage(event: DistillTraceEvent): AgentSidebarMessage {
  const text = `${event.label} ${event.detail}`.trim();
  const tone = event.status === "error" || event.kind === "error" ? "error" : event.status === "done" ? "done" : event.status === "active" ? "active" : undefined;
  if (tone === "error") return { title: "Couldn’t write the note yet", detail: `Your recording and marks are saved. ${friendlyToolDetail(event.detail || event.label)}`, tone };
  if (/loaded memo|aligned evidence|read evidence|session metadata/i.test(text)) return { title: "Prepare capture", detail: "Loaded the recording, marks, and transcript.", tone };
  if (/^read:/i.test(event.label)) return { title: "Read file", detail: `${event.label.replace(/^read:\s*/i, "")} — ${friendlyToolDetail(event.detail)}`, tone };
  if (/file reads/i.test(text)) return { title: "Read file", detail: friendlyToolDetail(event.detail), tone };
  if (/context scan|start_exploring|enzyme[_ ]petri|scanning your notes|scan/i.test(text)) return { title: "Reading your notes", detail: friendlyToolDetail(event.detail || "Mapping your notes' ideas before searching."), tone };
  if (/context search|semantic_search|enzyme[_ ]catalyze|finding related ideas|connected to|search query|related (?:notes|ideas)/i.test(text)) return { title: "Finding related ideas", detail: friendlySearchDetail(text), tone };
  if (/^grep:/i.test(event.label)) return { title: "Search files", detail: `${event.label.replace(/^grep:\s*/i, "")} — ${friendlyToolDetail(event.detail)}`, tone };
  if (/^ls:/i.test(event.label)) return { title: "List folder", detail: `${event.label.replace(/^ls:\s*/i, "")} — ${friendlyToolDetail(event.detail)}`, tone };
  if (/save note|margins_save_note/i.test(text)) return { title: "Save note", detail: /saved/i.test(text) ? friendlyToolDetail(event.detail || "Saved the note.") : "Saving the note.", tone };
  if (/processing complete|finished/i.test(text)) return { title: "Done", detail: "Finished writing the note.", tone: "done" };
  if (/thinking pass/i.test(text)) return { title: "Plan", detail: "Planning the note structure.", tone };
  if (/cleanup/i.test(event.label)) return { title: "Clean up", detail: event.detail || event.label, tone };
  return { title: friendlyStageTitle(event.label), detail: friendlyToolDetail(event.detail || event.label), tone };
}

export function friendlySearchDetail(text: string): string {
  // Resolved states first, so a finished search reads "Connected to …" cleanly.
  if (/no closely related notes/i.test(text)) return "No closely related notes found.";
  const connected = text.match(/connected to\s+(.*)$/i)?.[1]?.trim();
  if (connected) return `Connected to ${truncatePlain(connected, 110)}.`;
  const returned = text.match(/returned\s+(.*)$/i)?.[1]?.trim();
  if (returned) return `Found ${truncatePlain(returned.replace(/^notes?:\s*/i, ""), 96)}.`;
  // In-flight: capture the query but stop before any appended summary tokens
  // (the label and detail are concatenated, so an unbounded match would bleed).
  const query = text.match(/(?:search query|context search|finding related ideas):\s*(.+?)(?=\s+(?:search query|connected to|returned|no closely)|$)/i)?.[1]?.trim();
  if (query) return `Looking for: ${truncatePlain(query, 112)}`;
  return friendlyToolDetail(text);
}

export function friendlyToolDetail(text: string): string {
  return truncatePlain(text
    .replace(/^Tool (?:start|done|failed):\s*/i, "")
    .replace(/^(?:error:\s*)+/i, "")
    .replace(/\bmemo notes\b/gi, "marks")
    .replace(/\bmemo\b/gi, "marks")
    .replace(/\bSettings\b/g, "setup")
  , 140);
}

export function friendlyStageTitle(label: string): string {
  const clean = label.replace(/_/g, " ").trim();
  return clean ? clean.charAt(0).toUpperCase() + clean.slice(1) : "Step";
}

export function truncatePlain(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

export function visiblePiTrace(trace: DistillTraceEvent[]): DistillTraceEvent[] {
  return trace.filter(event => event.kind === "tool" || event.kind === "error");
}

export function traceStatusText(event: DistillTraceEvent): string {
  if (event.status === "error") return "error";
  if (event.status === "done") return "done";
  if (event.kind === "tool") return "tool";
  return "stage";
}

export function defaultDistillTrace(): DistillTraceEvent[] {
  return [
    { kind: "stage", label: "Prepare capture", detail: "Loaded marks and aligned transcript.", status: "done" },
    { kind: "tool", label: "scanning your notes", detail: "Mapped your notes' ideas before searching.", status: "done" },
    { kind: "tool", label: "finding related ideas", detail: "Searched for notes connected through shared ideas.", status: "done" },
    { kind: "tool", label: "file reads", detail: "Read supporting notes before drafting.", status: "done" },
    { kind: "tool", label: "margins_save_note", detail: "Saved the note.", status: "done" },
  ];
}

export function appendProcessingTrace(trace: DistillTraceEvent[], event: ProcessingEvent): DistillTraceEvent[] {
  if (event.stage === "transcript" && event.entry) return trace;
  const msg = event.message || event.stage;
  let kind: DistillTraceEvent["kind"] = event.stage === "error" ? "error" : "stage";
  let label = stageLabel(event.stage);
  let detail = msg;
  let status: DistillTraceEvent["status"] = event.stage === "error" ? "error" : event.stage === "complete" ? "done" : "active";

  if (event.track === "capture") {
    label = "Prepare capture";
    status = event.phase === "captured" ? "done" : event.phase === "finalizing" ? "active" : status;
  } else if (event.track === "transcript") {
    label = "Prepare transcript";
    status = event.phase === "ready" ? "done" : event.phase === "failed" ? "error" : "active";
  } else if (event.track === "note") {
    label = event.phase === "saved" ? "Note saved" : "Write note";
    status = event.phase === "saved" ? "done" : event.phase === "failed" ? "error" : "active";
  }

  const richToolMatch = msg.match(/^Tool (start|done|failed):\s*(.+?)(?:\s+[—–-]\s+(.*))?$/);
  const legacyToolMatch = msg.match(/(?:Using note tool:|Note tool (?:finished|failed):)\s*([^\s]+)/);
  if (richToolMatch) {
    kind = "tool";
    label = richToolMatch[2].trim();
    detail = richToolMatch[3]?.trim() || (richToolMatch[1] === "start" ? "Tool call started." : "Tool call finished.");
    status = richToolMatch[1] === "failed" ? "error" : richToolMatch[1] === "done" ? "done" : "active";
  } else if (legacyToolMatch) {
    kind = "tool";
    label = friendlyToolName(legacyToolMatch[1]);
    const legacyTool = legacyToolMatch[1];
    detail = ["read", "grep", "find", "ls"].includes(legacyTool)
      ? "Older trace entry did not capture the path. Rerun note generation to record file paths."
      : msg.includes("failed") ? "Tool call failed." : msg.includes("finished") ? "Tool call finished." : "Tool call started.";
    status = msg.includes("failed") ? "error" : msg.includes("finished") ? "done" : "active";
  } else if (/^(Pi turn|Thinking pass)/i.test(msg)) {
    kind = "stage";
    label = "Thinking pass";
  } else if (/enzyme|semantic search|vault|context search|finding related ideas|scanning your notes|connected to/i.test(msg)) {
    kind = "tool";
    // Canonical (capitalized) labels so these merge with the resolved events
    // emitted by compactTraceEvent — a case mismatch here would leave the
    // in-flight "Looking for…" row stranded beside its later "Found…" row.
    label = /petri|explor|scan/i.test(msg) ? "Reading your notes" : "Finding related ideas";
  } else if (/read|grep|find|ls/i.test(msg)) {
    kind = "tool";
    label = "file reads";
  }

  const next = compactTraceEvent({ kind, label, detail, status });
  if (!next) return trace;
  return mergeTraceEvent(trace, next).slice(-28);
}

function compactTraceEvent(event: DistillTraceEvent): DistillTraceEvent | null {
  const text = `${event.label} ${event.detail}`.trim();
  if (/^AI model:/i.test(event.detail) || /Starting AI note distillation/i.test(text)) return null;
  if (/^Using desktop capture context/i.test(event.detail)) {
    return { kind: "stage", label: "Prepare capture", detail: "Loaded the marks and available transcript context.", status: "done" };
  }
  if (/Using stored .*transcript|offline transcription not needed/i.test(event.detail)) {
    return { kind: "stage", label: "Prepare capture", detail: "Using the transcript already saved with this capture.", status: "done" };
  }
  if (event.label === "Transcribe locally" || event.label === "Prepare transcript") {
    if (/Loaded .*transcription models|Loading Polyvoice diarization models|Transcribing segment|Diarized .*speaker turns/i.test(event.detail)) {
      return { kind: "stage", label: "Prepare capture", detail: event.detail, status: event.status };
    }
  }
  if (/Building full (?:aligned timeline|transcript)|Refreshing transcript before writing/i.test(event.detail)) {
    return { kind: "stage", label: "Prepare capture", detail: "Preparing the saved recording transcript.", status: "active" };
  }
  if (/(?:Aligned timeline|Transcript) ready/i.test(event.detail)) {
    return { kind: "stage", label: "Prepare capture", detail: "Loaded the saved recording transcript.", status: "done" };
  }
  if (/Preparing compact vault context/i.test(event.detail)) {
    return { kind: "tool", label: "Reading your notes", detail: "Looking for notes that may help this draft.", status: "active" };
  }
  if (/Mapped related note ideas with bounded Petri/i.test(event.detail)) {
    return { kind: "tool", label: "Reading your notes", detail: "Mapped related ideas in your notes.", status: "done" };
  }
  if (/Found related note snippets for/i.test(event.detail)) {
    const query = event.detail.match(/'([^']+)'/)?.[1];
    return { kind: "tool", label: "Finding related ideas", detail: query ? `Found related notes for: ${truncatePlain(query, 96)}` : "Found related notes.", status: "done" };
  }
  if (/Cheap context planner unavailable/i.test(event.detail)) {
    return { kind: "tool", label: "Finding related ideas", detail: "Using a deterministic note search plan because the prep model was unavailable.", status: "done" };
  }
  return event;
}

function mergeTraceEvent(trace: DistillTraceEvent[], event: DistillTraceEvent): DistillTraceEvent[] {
  const next = [...trace];
  if (event.kind === "tool" || event.label === "Prepare capture" || event.label === "Write note") {
    const idx = findMergeTarget(next, event);
    if (idx >= 0) {
      next[idx] = {
        ...event,
        detail: event.detail || next[idx].detail,
        status: event.status || next[idx].status,
      };
      return next;
    }
  }
  const prev = next[next.length - 1];
  if (prev && prev.kind === event.kind && prev.label === event.label && prev.detail === event.detail) {
    next[next.length - 1] = { ...prev, status: event.status || prev.status };
    return next;
  }
  next.push(event);
  return next;
}

function findMergeTarget(trace: DistillTraceEvent[], event: DistillTraceEvent): number {
  for (let i = trace.length - 1; i >= 0; i--) {
    const existing = trace[i];
    if (existing.kind !== event.kind || existing.label !== event.label) continue;
    if (event.label === "Finding related ideas") return i;
    if (existing.status === "active" || existing.label === "Prepare capture" || existing.label === "Reading your notes") return i;
  }
  return -1;
}

export function stageLabel(stage: string): string {
  switch (stage) {
    case "prepare": return "Prepare capture";
    case "transcribe": return "Prepare transcript";
    case "align": return "Prepare moments";
    case "synthesize": return "Write note";
    case "note_stream": return "Write note";
    case "cleanup": return "Retention cleanup";
    case "complete": return "Note saved";
    case "error": return "Paused";
    default: return stage;
  }
}

export function friendlyToolName(tool: string): string {
  if (tool.includes("enzyme_petri") || tool.includes("start_exploring_vault")) return "scanning your notes";
  if (tool.includes("enzyme_catalyze") || tool.includes("semantic_search")) return "finding related ideas";
  if (tool === "margins_save_note") return "save note";
  if (["read", "grep", "find", "ls"].includes(tool)) return `${tool}: path not captured`;
  return tool;
}
