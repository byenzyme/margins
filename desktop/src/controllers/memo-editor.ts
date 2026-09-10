import type { CaptureDeviceState, MemoLine } from "../lib/tauri";
import { formatDuration, formatElapsed, levelBars, levelToNormalized } from "../lib/format";
import { captureSignalReadiness, type CaptureSignalLane } from "../lib/capture-signal-state";
import type { RecordingStartupRenderState } from "../render/recording";

// The two small waveforms ease only toward measured native levels. There is no
// idle animation: a flat line means the callback is quiet (or not live yet).
let micWaveLevel = 0;
let systemWaveLevel = 0;

export interface MemoEditorRecordingStatus {
  is_recording: boolean;
  paused: boolean;
  elapsed_secs: number;
  capture_device: CaptureDeviceState;
  mic_level: number | null;
  mic_audio_frame_count: number;
  spk_level: number;
  system_audio_expected: boolean;
  system_audio_frame_count: number;
  tap_status: string;
  capture_phase?: string;
}

export interface MemoEditorContext {
  getMemoLines: () => MemoLine[];
  getMemoStartTime: () => number;
  getPendingLineStartSecs: () => number | null;
  setPendingLineStartSecs: (value: number | null) => void;
  getRecordingStatus: () => MemoEditorRecordingStatus;
  getRecordingStartupState?: () => RecordingStartupRenderState | null;
  getRecordingStatusLine: () => string;
  isFullRecordingView: () => boolean;
  renderMemoLineInner: (line: MemoLine, index: number) => string;
  /** Returns wall-clock label for a pause block ordinal. Used in gutter labels for paused lines. */
  getBlockWallClock?: (ordinal: number) => string | undefined;
  renderAudioHealth: () => string;
  isAudioPendingForNewMarks?: () => boolean;
  checkpointMemoLine?: (committedIndex: number) => void;
  requestBackchannelForMemo?: (committedIndex: number) => void;
  collapseBackchannelCards?: (currentIndex: number) => void;
  /** True when the clock is stopped (prep or paused). Lines committed in this state get block_ordinal stamped. */
  getClockStopped?: () => boolean;
  /** Current block ordinal (for block_ordinal stamping on committed lines). */
  getCurrentBlockOrdinal?: () => number;
}

let cmdHeld = false;
let cmdWatcherInstalled = false;

// Keep the empty row discoverable without suggesting what the user should think.
// A stable action label also avoids changing the prompt after every committed mark.
const MEMO_PLACEHOLDER = "Add a mark…";

function installCmdWatcher() {
  if (cmdWatcherInstalled) return;
  cmdWatcherInstalled = true;
  const onKey = (e: KeyboardEvent) => {
    const held = e.metaKey || e.ctrlKey;
    if (held !== cmdHeld) {
      cmdHeld = held;
      syncCmdArmed();
    }
  };
  window.addEventListener("keydown", onKey, true);
  window.addEventListener("keyup", onKey, true);
  window.addEventListener("blur", () => {
    if (cmdHeld) {
      cmdHeld = false;
      syncCmdArmed();
    }
  });
}

function syncCmdArmed() {
  const active = document.activeElement as HTMLElement | null;
  const focusedRow = active?.closest?.(".memo-line") as HTMLElement | null;
  for (const row of Array.from(document.querySelectorAll(".memo-line.memo-line-cmd-armed"))) {
    if (row !== focusedRow || !cmdHeld) row.classList.remove("memo-line-cmd-armed");
  }
  if (cmdHeld && focusedRow) {
    ensureCmdHint(focusedRow);
    focusedRow.classList.add("memo-line-cmd-armed");
  }
  const rail = document.querySelector(".backchannel-assist-rail");
  if (rail) rail.classList.toggle("cmd-armed", cmdHeld && !!focusedRow);
}

function ensureCmdHint(row: HTMLElement) {
  if (row.querySelector(".memo-cmd-hint")) return;
  const hint = document.createElement("div");
  hint.className = "memo-cmd-hint";
  hint.textContent = "⌘↩ cue";
  row.appendChild(hint);
}

/** Attach input handlers to all committed memo textareas. */
export function attachMemoEditHandlers(ctx: MemoEditorContext) {
  installCmdWatcher();
  const memoLines = ctx.getMemoLines();
  for (let i = 0; i < memoLines.length; i++) {
    const ta = document.getElementById(`memo-input-${i}`) as HTMLTextAreaElement;
    if (!ta) continue;
    setupCommittedTextarea(ctx, ta, i);
  }
  for (const row of Array.from(document.querySelectorAll(".memo-line"))) {
    ensureCmdHint(row as HTMLElement);
  }
}

/** Set up a committed line's textarea with edit + navigation handlers. */
function setupCommittedTextarea(ctx: MemoEditorContext, textarea: HTMLTextAreaElement, index: number) {
  const ta = textarea.cloneNode(true) as HTMLTextAreaElement;
  textarea.replaceWith(ta);

  // Auto-resize on load
  ta.style.height = "auto";
  ta.style.height = ta.scrollHeight + "px";

  ta.addEventListener("input", () => {
    const memoLines = ctx.getMemoLines();
    ta.style.height = "auto";
    ta.style.height = ta.scrollHeight + "px";
    memoLines[index].text = ta.value;
    memoLines[index].edited_secs = (Date.now() - ctx.getMemoStartTime()) / 1000;
    // Update gutter to show edit indicator
    const gutter = ta.closest(".memo-line")?.querySelector(".memo-gutter");
    if (gutter) {
      gutter.classList.add("edited");
      const editTs = formatElapsed(memoLines[index].edited_secs!);
      // Keep the gutter to one timestamp: once edited, the modified time is
      // more useful than the original mark time and does not crowd the text.
      gutter.textContent = editTs;
    }
  });

  ta.addEventListener("focus", () => {
    ta.closest(".memo-line")?.classList.add("memo-line-active");
    syncCmdArmed();
  });

  ta.addEventListener("blur", () => {
    const row = ta.closest(".memo-line");
    row?.classList.remove("memo-line-active");
    row?.classList.remove("memo-line-cmd-armed");
    syncCmdArmed();
  });

  ta.addEventListener("keydown", (e) => {
    // Enter is also used by WebKit/macOS to finish inline completion and by
    // IMEs to commit composed text. Never turn that acceptance keystroke into
    // memo navigation, deletion, or a cue request.
    if (e.isComposing || e.keyCode === 229 || e.defaultPrevented) return;
    if (e.key === "ArrowUp" && ta.selectionStart === 0) {
      e.preventDefault();
      focusMemoLine(ctx, index - 1);
    } else if (e.key === "ArrowDown" && ta.selectionStart === ta.value.length) {
      e.preventDefault();
      focusMemoLine(ctx, index + 1);
    } else if (e.key === "Backspace" && ta.selectionStart === 0 && ta.selectionEnd === 0) {
      e.preventDefault();
      deleteMemoLine(ctx, index);
    } else if (e.key === "Enter" && !e.shiftKey && !e.metaKey && !e.ctrlKey) {
      e.preventDefault();
      focusMemoLine(ctx, index + 1);
    } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      const nextText = ta.value.trim();
      const memoLines = ctx.getMemoLines();
      if (memoLines[index].text !== nextText) {
        memoLines[index].text = nextText;
        memoLines[index].edited_secs = (Date.now() - ctx.getMemoStartTime()) / 1000;
      }
      ctx.collapseBackchannelCards?.(index);
      ctx.requestBackchannelForMemo?.(index);
    }
  });
}

/** Focus a specific memo line by index (-1 and beyond-last are handled). */
function focusMemoLine(ctx: MemoEditorContext, index: number) {
  const memoLines = ctx.getMemoLines();
  if (index < 0) return;
  if (index >= memoLines.length) {
    // Focus the pending input
    const pending = document.getElementById("memo-input-new") as HTMLTextAreaElement;
    if (pending) pending.focus();
    return;
  }
  const ta = document.getElementById(`memo-input-${index}`) as HTMLTextAreaElement;
  if (ta) {
    ta.focus();
    ta.selectionStart = ta.selectionEnd = ta.value.length;
  }
}

/** Delete a committed memo line, merging its text into the previous line. */
function deleteMemoLine(ctx: MemoEditorContext, index: number) {
  const memoLines = ctx.getMemoLines();
  const deletedText = memoLines[index].text;
  memoLines.splice(index, 1);

  let focusIndex = index;
  let cursorPos: number | null = null;
  if (index > 0 && index - 1 < memoLines.length) {
    // Merge deleted text into previous line.
    focusIndex = index - 1;
    cursorPos = memoLines[focusIndex].text.length;
    if (deletedText) {
      memoLines[focusIndex].text += " " + deletedText;
      memoLines[focusIndex].edited_secs = (Date.now() - ctx.getMemoStartTime()) / 1000;
    }
  } else if (memoLines.length > 0) {
    // Deleted line 0 - focus the new line 0.
    focusIndex = 0;
  }

  renderCommittedMemoRows(ctx);

  if (memoLines.length === 0) {
    const pending = document.getElementById("memo-input-new") as HTMLTextAreaElement;
    if (pending) pending.focus();
    return;
  }

  const ta = document.getElementById(`memo-input-${focusIndex}`) as HTMLTextAreaElement;
  if (ta) {
    ta.focus();
    ta.selectionStart = ta.selectionEnd = cursorPos ?? ta.value.length;
  }
}

function renderCommittedMemoRows(ctx: MemoEditorContext) {
  const editor = document.getElementById("memo-editor");
  if (!editor) return;
  const pendingRow = document.getElementById("memo-row-pending");

  for (const row of Array.from(editor.querySelectorAll(".memo-line:not(#memo-row-pending)"))) {
    row.remove();
  }

  ctx.getMemoLines().forEach((memo, index) => {
    const row = document.createElement("div");
    row.className = "memo-line";
    row.innerHTML = ctx.renderMemoLineInner(memo, index);
    if (pendingRow) editor.insertBefore(row, pendingRow);
    else editor.appendChild(row);
    const ta = row.querySelector("textarea") as HTMLTextAreaElement;
    if (ta) setupCommittedTextarea(ctx, ta, index);
    ensureCmdHint(row);
  });
}

export function updateRecordingHeader(ctx: MemoEditorContext) {
  const recordingStatus = ctx.getRecordingStatus();
  const timerEl = document.getElementById("rec-timer");
  if (timerEl) timerEl.textContent = formatDuration(recordingStatus.elapsed_secs);
  const statusText = document.getElementById("rec-status-text");
  if (statusText) statusText.textContent = ctx.getRecordingStatusLine();

  const micEl = document.getElementById("mic-bar");
  if (micEl) micEl.innerHTML = levelBars(recordingStatus.mic_level, 12);

  const spkEl = document.getElementById("spk-bar");
  if (spkEl) spkEl.innerHTML = levelBars(recordingStatus.spk_level, 12);

  const readiness = captureSignalReadiness(recordingStatus, ctx.getRecordingStartupState?.() ?? null);
  micWaveLevel = updateCaptureSignalLane("mic", recordingStatus.mic_level, micWaveLevel, readiness.mic);
  systemWaveLevel = updateCaptureSignalLane("system", recordingStatus.spk_level, systemWaveLevel, readiness.system);

  const hero = document.getElementById("mobile-capture-hero");
  if (hero) {
    hero.className = `mobile-capture-hero ${readiness.mic.state}${recordingStatus.capture_phase === "pausing" ? " pausing" : ""}`;
    hero.setAttribute("aria-label", `Microphone: ${readiness.mic.label}`);
    const heroTimer = document.getElementById("mobile-capture-timer");
    if (heroTimer) heroTimer.textContent = formatElapsed(recordingStatus.elapsed_secs);
    const heroState = document.getElementById("mobile-capture-state");
    if (heroState) heroState.textContent = readiness.mic.label;
  }

  const healthEl = document.getElementById("audio-health-slot");
  if (healthEl) healthEl.innerHTML = ctx.renderAudioHealth();

  // Update the pending line's gutter timestamp (clock-stopped: gutter is static pause glyph, skip)
  if (!ctx.getClockStopped?.()) {
    const pendingLineStartSecs = ctx.getPendingLineStartSecs();
    if (pendingLineStartSecs !== null) {
      const gutterEl = document.getElementById("memo-gutter-pending");
      if (gutterEl) gutterEl.textContent = formatElapsed(pendingLineStartSecs);
    }
  }
}

function updateCaptureSignalLane(
  id: "mic" | "system",
  peak: number | null,
  easedLevel: number,
  readiness: CaptureSignalLane,
): number {
  const target = levelToNormalized(peak);
  const next = easedLevel + (target - easedLevel) * 0.38;
  const waveform = document.getElementById(`${id}-waveform`);
  waveform?.style.setProperty("--signal-level", next.toFixed(3));

  const lane = document.getElementById(`${id}-signal-lane`);
  if (lane) {
    lane.className = `capture-signal-lane ${readiness.state}`;
    const name = id === "mic" ? "Mic" : "Computer";
    lane.setAttribute("aria-label", `${name}: ${readiness.label}`);
  }
  const state = document.getElementById(`${id}-signal-state`);
  if (state) state.textContent = readiness.label;
  return next;
}

export function appendNewMemoInput(ctx: MemoEditorContext) {
  installCmdWatcher();
  const editor = document.getElementById("memo-editor");
  if (!editor) return;

  const existing = document.getElementById("memo-row-pending");
  if (existing) existing.remove();

  const clockStopped = ctx.getClockStopped?.() ?? false;
  const row = document.createElement("div");
  row.className = "memo-line";
  row.id = "memo-row-pending";
  // Clock-stopped: show pause glyph; clock running: empty gutter (filled when typing starts)
  const gutterContent = clockStopped ? "⏸" : "";
  row.innerHTML = `
    <div class="memo-gutter${clockStopped ? " block-gutter" : ""}" id="memo-gutter-pending">${gutterContent}</div>
    <div class="memo-text">
      <textarea id="memo-input-new" rows="1" spellcheck="false" autocomplete="off" placeholder="${MEMO_PLACEHOLDER}"></textarea>
    </div>
  `;
  editor.appendChild(row);
  ensureCmdHint(row);

  const textarea = row.querySelector("textarea")!;
  textarea.focus();
  ctx.setPendingLineStartSecs(null);

  textarea.addEventListener("input", () => {
    textarea.style.height = "auto";
    textarea.style.height = textarea.scrollHeight + "px";
    // Clock-stopped: don't set pendingLineStartSecs (no timeline mark)
    if (!clockStopped && ctx.getPendingLineStartSecs() === null && textarea.value.trim()) {
      ctx.setPendingLineStartSecs((Date.now() - ctx.getMemoStartTime()) / 1000);
      const gutterEl = document.getElementById("memo-gutter-pending");
      if (gutterEl) gutterEl.textContent = formatElapsed(ctx.getPendingLineStartSecs()!);
    }
  });

  textarea.addEventListener("focus", () => {
    syncCmdArmed();
  });

  textarea.addEventListener("blur", () => {
    row.classList.remove("memo-line-cmd-armed");
    syncCmdArmed();
  });

  textarea.addEventListener("keydown", (e) => {
    // Let the platform finish autocomplete/IME composition without also
    // committing the pending mark. The subsequent deliberate Enter remains
    // the capture shortcut.
    if (e.isComposing || e.keyCode === 229 || e.defaultPrevented) return;
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      commitPendingLine(ctx, textarea, e.metaKey || e.ctrlKey);
    } else if (e.key === "Backspace" && textarea.value === "" && ctx.getMemoLines().length > 0) {
      e.preventDefault();
      focusMemoLine(ctx, ctx.getMemoLines().length - 1);
    } else if (e.key === "ArrowUp" && textarea.selectionStart === 0) {
      e.preventDefault();
      focusMemoLine(ctx, ctx.getMemoLines().length - 1);
    }
  });

  editor.scrollTop = editor.scrollHeight;
}

function commitPendingLine(ctx: MemoEditorContext, textarea: HTMLTextAreaElement, requestBackchannel = false) {
  const text = textarea.value.trim();
  if (!text) return;

  const memoLines = ctx.getMemoLines();
  const committedSecs = (Date.now() - ctx.getMemoStartTime()) / 1000;
  const clockStopped = ctx.getClockStopped?.() ?? false;
  const blockOrdinal = clockStopped ? (ctx.getCurrentBlockOrdinal?.() ?? 0) : null;
  memoLines.push({
    text,
    created_secs: committedSecs,
    edited_secs: null,
    draft_started_secs: clockStopped ? null : ctx.getPendingLineStartSecs(),
    audio_pending_at_mark: ctx.isAudioPendingForNewMarks?.() || undefined,
    block_ordinal: blockOrdinal,
  });

  const editor = document.getElementById("memo-editor");
  const pendingRow = document.getElementById("memo-row-pending");
  if (editor && pendingRow) {
    const idx = memoLines.length - 1;
    const committed = document.createElement("div");
    committed.className = "memo-line";
    committed.innerHTML = ctx.renderMemoLineInner(memoLines[idx], idx);
    editor.replaceChild(committed, pendingRow);

    const commitTextarea = committed.querySelector("textarea") as HTMLTextAreaElement;
    if (commitTextarea) setupCommittedTextarea(ctx, commitTextarea, idx);
  }

  const committedIndex = memoLines.length - 1;
  if (requestBackchannel) {
    ctx.collapseBackchannelCards?.(committedIndex);
    ctx.requestBackchannelForMemo?.(committedIndex);
  } else {
    ctx.checkpointMemoLine?.(committedIndex);
  }

  ctx.setPendingLineStartSecs(null);
  appendNewMemoInput(ctx);
}
