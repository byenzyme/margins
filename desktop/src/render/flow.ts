import type { MemoLine, ProcessingEvent, TranscriptEntry } from "../lib/tauri";
import { formatDuration, formatElapsed } from "../lib/format";
import { esc, js } from "../lib/html";
import { agentEventMessage, visiblePiTrace, type DistillTraceEvent } from "../lib/distill-trace";

export interface SummaryRenderContext {
  sessionName: string;
  durationSecs: number;
  memoLines: MemoLine[];
  windowDragRegion: string;
  settingsOverlay: string;
}

export interface ProcessingRenderContext {
  sessionName: string;
  steps: ProcessingEvent[];
  trace: DistillTraceEvent[];
  currentProgress: number;
  transcriptEntries: TranscriptEntry[];
  windowDragRegion: string;
  settingsOverlay: string;
}

export interface ReviewRenderContext {
  sessionName: string;
  content: string;
  obsidianVaultReady: boolean;
  vaultPath: string | null;
  renderedMarkdown: string;
  windowDragRegion: string;
  settingsOverlay: string;
}

export function renderSummaryView(ctx: SummaryRenderContext): string {
  return `
    ${ctx.windowDragRegion}
    <div class="nav-bar">
      <h1>${esc(ctx.sessionName)}</h1>
      <button onclick="window.__nav('home')">Close</button>
    </div>
    <div class="view">
      <div class="summary-view">
        <div class="summary-stats">
          <div class="stat">
            <div class="stat-value">${formatDuration(ctx.durationSecs)}</div>
            <div class="stat-label">Duration</div>
          </div>
          <div class="stat">
            <div class="stat-value">${ctx.memoLines.length}</div>
            <div class="stat-label">Marks</div>
          </div>
        </div>

        ${ctx.memoLines.length > 0 ? `
          <div class="summary-memo">
            <div class="summary-memo-header">Marked moments</div>
            <div class="summary-memo-content">
              ${ctx.memoLines.map(line => `
                <div class="summary-memo-line">
                  <span class="summary-ts">${formatElapsed(line.created_secs)}</span>
                  <span>${esc(line.text)}</span>
                </div>
              `).join("")}
            </div>
          </div>
        ` : `
          <div class="empty-state" style="padding: 30px 20px;">
            <p>No moments marked. Margins can still write a note from the transcript.</p>
          </div>
        `}

        <div class="summary-actions">
          <button class="primary" onclick="window.__processSession(${js(ctx.sessionName)})">
            Write note
          </button>
          <button onclick="window.__nav('home')">Back to capture</button>
        </div>
      </div>
    </div>
    ${ctx.settingsOverlay}
  `;
}

export function renderProcessingView(ctx: ProcessingRenderContext): string {
  const stages = [
    { key: "prepare", label: "Gathering capture context" },
    { key: "align", label: "Preparing marked moments" },
    { key: "synthesize", label: "Writing note" },
    { key: "complete", label: "Done" },
  ];

  const latestStageIdx = Math.max(
    0,
    ...ctx.steps.map(event => {
      const idx = stages.findIndex(stage => stage.key === event.stage);
      return idx >= 0 ? idx : 0;
    })
  );
  const latestEvent = ctx.steps[ctx.steps.length - 1];
  const errorEvent = latestEvent?.stage === "error" ? latestEvent : null;
  const errorText = errorEvent?.message.replace(/^Error:\s*(Error:\s*)?/, "") || "Could not write the note.";

  return `
    ${ctx.windowDragRegion}
    <div class="nav-bar">
      <h1>${esc(ctx.sessionName)}</h1>
      <div></div>
    </div>
    <div class="view">
      <div class="processing-view">
        ${stages.map((stage, stageIdx) => renderPipelineStep(stage, stageIdx, latestStageIdx, ctx.steps, ctx.trace)).join("")}
        <div class="progress-bar">
          <div class="fill" style="width: ${ctx.currentProgress * 100}%"></div>
        </div>
        ${errorEvent ? `
          <div class="processing-error">
            <strong>Couldn’t write the note yet</strong>
            <span>${esc(errorText)}</span>
            <div>
              <button class="primary" onclick="window.__processSession(${js(ctx.sessionName)})">Retry</button>
              <button onclick="window.__nav('settings')">Open setup</button>
              <button class="subtle" onclick="window.__cancelProcessing(${js(ctx.sessionName)})">Cancel</button>
            </div>
          </div>
        ` : `
          <div class="processing-actions">
            <button class="subtle" onclick="window.__cancelProcessing(${js(ctx.sessionName)})">Cancel</button>
          </div>
        `}
        ${ctx.transcriptEntries.length > 0 ? `
          <div class="transcript-preview">
            <div class="transcript-preview-header">Transcript</div>
            <div class="transcript-preview-content" id="transcript-scroll">
              ${ctx.transcriptEntries.map(entry => {
                const ts = formatElapsed(entry.start_ms / 1000);
                const speaker = `Speaker ${entry.channel + 1}`;
                return `
                  <div class="transcript-entry channel-${entry.channel}">
                    <span class="transcript-speaker">${speaker}</span>
                    <span class="transcript-ts">${ts}</span>
                    <span class="transcript-text">${esc(entry.text)}</span>
                  </div>
                `;
              }).join("")}
            </div>
          </div>
        ` : ""}
      </div>
    </div>
    ${ctx.settingsOverlay}
  `;
}

export function renderReviewView(ctx: ReviewRenderContext): string {
  const vaultReady = ctx.obsidianVaultReady && ctx.vaultPath;
  const primary = vaultReady
    ? `
        <div class="primary-split">
          <button class="primary primary-split-face" onclick="window.__openInVault()">Open in Obsidian</button>
          <details class="session-more-actions session-more-actions--primary">
            <summary aria-label="More open options">▾</summary>
            <button class="subtle" onclick="window.__openNote(${js(ctx.sessionName)})">Open in app</button>
          </details>
        </div>`
    : `<button class="primary" onclick="window.__openNote(${js(ctx.sessionName)})">Open note</button>`;
  const statusText = vaultReady ? "Saved to project" : "Note saved";
  return `
    ${ctx.windowDragRegion}
    <div class="review-view">
      <div class="nav-bar">
        <div class="nav-title-block">
          <h1>${esc(ctx.sessionName)}</h1>
          <div class="review-status"><span class="review-status-check">✓</span> <span>${statusText}</span></div>
        </div>
        ${primary}
      </div>
      <div class="review-content">${ctx.renderedMarkdown}</div>
      <div class="review-footer">
        <button class="danger" id="review-rerun" data-session="${esc(ctx.sessionName)}" onclick="window.__reprocessSession(${js(ctx.sessionName)})">Re-run processing</button>
        <div class="spacer"></div>
        <div class="review-footer-cluster">
          <button class="subtle review-copy" onclick="window.__copyReview()"><span class="review-copy-label">Copy</span> <span class="kbd-chip">⌘C</span></button>
          <button class="subtle" onclick="window.__nav('home')">Back to captures</button>
        </div>
      </div>
    </div>
    ${ctx.settingsOverlay}
  `;
}

function renderPipelineStep(stage: { key: string; label: string }, stageIdx: number, latestStageIdx: number, steps: ProcessingEvent[], trace: DistillTraceEvent[]): string {
  const events = steps.filter(event => event.stage === stage.key);
  const lastEvent = events[events.length - 1];
  const completed = stageIdx < latestStageIdx;
  const isActive = stageIdx === latestStageIdx && events.length > 0;
  const isFinalDone = stage.key === "complete" && events.length > 0;

  let icon = '<span class="step-icon-empty"></span>';
  if (completed || isFinalDone) {
    icon = '<span class="step-icon-done">✓</span>';
  } else if (isActive) {
    icon = '<span class="spinner"></span>';
  }

// Under "Writing note", show the vault context-fetch as discrete,
  // legible steps (what idea it looked for, which notes it connected to) instead
  // of a single mutating line — so the search reads as deliberate, not random.
  const contextSteps = stage.key === "synthesize" && (isActive || completed) ? renderContextSteps(trace) : "";

  return `
    <div class="pipeline-step ${completed ? "completed" : ""} ${isActive ? "active" : ""}">
      <div class="step-icon">${icon}</div>
      <div class="step-content">
        <div class="step-title">${stage.label}</div>
        ${contextSteps || (lastEvent ? `<div class="step-detail">${esc(lastEvent.message)}</div>` : "")}
      </div>
    </div>
  `;
}

// Collapse the flat start/done trace into one line per vault lookup, preferring
// the finished state so a search shows "Connected to …" once it resolves.
function renderContextSteps(trace: DistillTraceEvent[]): string {
  const lookups = visiblePiTrace(trace).filter(event =>
    /finding related ideas|scanning your notes|context search|context scan/i.test(`${event.label} ${event.detail}`)
  );
  const byLabel = new Map<string, DistillTraceEvent>();
  for (const event of lookups) {
    const prior = byLabel.get(event.label);
    if (!prior || event.status === "done" || event.status === "error") byLabel.set(event.label, event);
  }
  const items = [...byLabel.values()].map(agentEventMessage);
  if (!items.length) return "";
  return `
    <ul class="context-steps">
      ${items.map(message => `
        <li class="context-step ${message.tone || ""}">
          <span class="context-step-title">${esc(message.title)}</span>
          <span class="context-step-detail">${esc(message.detail)}</span>
        </li>
      `).join("")}
    </ul>
  `;
}
