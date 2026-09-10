import { Copy, ExternalLink, Trash2, X } from "lucide";

import type { MemoLine, SessionInfo } from "../lib/tauri";
import type { SessionJobState } from "../lib/lifecycle-state";
import { formatElapsed } from "../lib/format";
import { esc, iconSvg, js } from "../lib/html";
import {
  agentEventMessage,
  defaultDistillTrace,
  type AgentSidebarMessage,
  type DistillTraceEvent,
} from "../lib/distill-trace";
import { memoIdForIndex, previewBlocksWithLineBuffer, type MarginsGroundingUse, type GroundedNoteBlock, type GroundedNoteState } from "../lib/grounding";

export type ArtifactCacheEntry = { aligned?: string; note?: string; loading?: boolean };
export type FollowupChatMessage = { role: "user" | "assistant"; text: string };

export interface DistillTabRenderContext {
  session: SessionInfo;
  cache: ArtifactCacheEntry;
  processing: boolean;
  captureNote: boolean;
  settingsReady: boolean;
  currentAiMode: string;
  friendlyVaultPath: string;
  obsidianVaultReady: boolean;
  contextReady: boolean;
  currentProgress: number;
  groundedState: GroundedNoteState;
  memoLines: MemoLine[];
  trace: DistillTraceEvent[];
  followupChat: FollowupChatMessage[];
  renderMarkdown: (markdown: string) => string;
  noteError?: string;
  cancelled?: boolean;
  finalizingCapture?: boolean;
  jobState?: SessionJobState;
  captureHealth: { atSecs: number; label: string }[];
  // People added to the session after its note was written; empty when there
  // is nothing new to reprocess with (or the suggestion was dismissed).
  reprocessCandidates: string[];
  /** Show the first-note payoff callout (one-shot, gated by localStorage). */
  showFirstNotePayoff?: boolean;
}

export function groundedNoteVisibleMarkdown(state: GroundedNoteState): string {
  const visible = state.blocks.map(block => block.markdown).join("\n");
  return state.lineBuffer ? `${visible}\n${state.lineBuffer}` : visible;
}

export function renderDistillTab(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  if (ctx.noteError) {
    return renderNoteErrorPanel(ctx);
  }

  if (ctx.cancelled) {
    return renderCancelledPanel(ctx);
  }

  // An active refine turn re-streams the revised note, so let processing win
  // over the previously-saved note view.
  if (ctx.cache.note && !ctx.processing) {
    return `
      <div class="grounded-note-layout unified-note-flow note-workspace-grid">
        <section class="artifact-panel note-panel grounded-note-main">
          ${renderNoteViewActions(ctx)}
          ${renderGroundedNote(ctx, false)}
          ${ctx.showFirstNotePayoff ? renderFirstNotePayoff() : ""}
        </section>
        ${renderPiSidePanel(ctx, false)}
      </div>
    `;
  }

  if (!ctx.processing && ctx.groundedState.blocks.length) {
    return `
      <div class="grounded-note-layout unified-note-flow note-workspace-grid">
        <section class="artifact-panel note-panel grounded-note-main">
          ${renderNoteViewActions(ctx)}
          ${renderGroundedNote(ctx, false)}
          ${ctx.showFirstNotePayoff ? renderFirstNotePayoff() : ""}
        </section>
        ${renderPiSidePanel(ctx, false)}
      </div>
    `;
  }

  if (ctx.processing || ctx.groundedState.blocks.length) {
    // Re-running over an existing note: keep the prior note on screen (dimmed)
    // until the new stream produces real text, so it doesn't blink out to a
    // placeholder and re-layout. The surrounding chrome is identical to the
    // live-stream state below, so the only thing that changes when the first
    // token lands is the note body itself — no layout jump.
    const hasNewDraft = ctx.groundedState.blocks.some(block => block.markdown.trim());
    const rewriting = ctx.processing && Boolean(ctx.cache.note);
    const awaitingRewriteDraft = rewriting && !hasNewDraft;
    const heading = rewriting ? "Rewriting note" : "Writing note";
    const statusText = rewriting
      ? (awaitingRewriteDraft
        ? "Rewriting this note — your current version stays until the new one is ready."
        : "Streaming the revised note as it lands.")
      : ctx.jobState?.message || "Margins is lining up your marks with the transcript, then checking your notes for useful context.";
    const noteBody = awaitingRewriteDraft
      ? `<div class="review-content inline-review grounded-note distill-buffer prior-note-fading note-reflowing">${ctx.renderMarkdown(ctx.cache.note!)}</div>`
      : renderGroundedNote(ctx, true);
    return `
      <div class="grounded-note-layout unified-note-flow streaming-note-layout note-workspace-grid">
        <section class="artifact-panel note-panel grounded-note-main ${rewriting ? "is-rewriting" : ""}">
          <div class="panel-heading panel-heading-row">
            <span>${heading}</span>
            <span class="panel-heading-actions">
              <span class="panel-pill streaming-pill"><span class="spinner tiny-spinner"></span> ${heading}</span>
              ${ctx.processing ? `<button class="subtle compact-panel-action" onclick="window.__cancelProcessing(${js(s.name)})">Cancel</button>` : ""}
            </span>
          </div>
          <div class="note-status-strip">${statusText}</div>
          ${noteBody}
        </section>
        ${renderPiSidePanel(ctx, true)}
      </div>
    `;
  }

  if (ctx.captureNote || s.status === "synthesized") {
    return renderMissingNoteFallback(ctx);
  }

  return renderCapturedMemoPage(ctx);
}

function renderFirstNotePayoff(): string {
  return `
    <div class="first-note-payoff">
      <div class="first-note-payoff-text">
        <p class="first-note-payoff-head">Your first note is ready.</p>
        <p class="first-note-payoff-sub">It carries what you noticed, linked into the notes you already keep.</p>
      </div>
      <button class="link-button" onclick="window.__dismissFirstNotePayoff()">Got it</button>
    </div>
  `;
}

function renderCancelledPanel(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  return `
    <div class="captured-note-shell note-error-shell">
      <section class="captured-note-main">
        <div class="panel-heading panel-heading-row"><span>Note</span><span class="panel-pill note-cancelled-pill">Cancelled</span></div>
        <div class="note-error-card note-cancelled-card">
          <strong>Note writing was cancelled</strong>
          <p class="note-error-detail">No vault note was saved. Your recording and marks are still here.</p>
        </div>
        ${renderReadonlyMarks(ctx)}
      </section>
      <div class="captured-note-action-bar">
        <div class="captured-note-action-copy">
          <strong>Ready when you are</strong>
          <span>Write a fresh note from the same saved marks and transcript.</span>
        </div>
        <div class="note-error-actions">
          ${ctx.settingsReady
            ? `<button class="primary capture-ready-action" onclick="window.__processSession(${js(s.name)})">Write note</button>`
            : `<button class="primary" onclick="window.__nav('settings')">Sign in to make the note</button>`}
          <button class="subtle" onclick="window.__dismissCancelledNote(${js(s.name)})">Keep capture as-is</button>
        </div>
      </div>
    </div>
  `;
}

// Small, low-footprint actions anchored to the top-right of the note view
// itself, rather than the heavy Copy/Open pair that used to live in the header.
// Copy writes the note's plain markdown; Open jumps to the vault file.
function renderNoteViewActions(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  const openHandler = ctx.obsidianVaultReady && s.vault_note_path
    ? `window.__openInVault(${js(s.name)})`
    : `window.__openNote(${js(s.name)})`;
  const openButton = s.vault_note_path
    ? `<button class="note-view-action" aria-label="Open note" title="Open note" onclick="${openHandler}">${iconSvg(ExternalLink, "ui-icon button-icon")}<span>Open</span></button>`
    : "";

  // Reprocess suggestion: only when the note is saved, not currently
  // streaming, and people were added AFTER the note was written. Appears to
  // the LEFT of Copy, with a dismiss control beside it.
  const isDistilled = Boolean(ctx.cache.note);
  const isStreaming = ctx.processing;
  const reprocessButton = isDistilled && !isStreaming && ctx.reprocessCandidates.length >= 1
    ? renderReprocessButton(s.name, ctx.reprocessCandidates)
    : "";

  return `
    <div class="note-view-actions">
      ${reprocessButton}
      <button class="note-view-action review-copy" aria-label="Copy note as Markdown" title="Copy as Markdown" onclick="window.__copyReview()">${iconSvg(Copy, "ui-icon button-icon")}<span class="review-copy-label">Copy</span></button>
      ${openButton}
    </div>
  `;
}

// Extract the first name from a raw people string. Values may be "[[Alice Bob]]"
// or plain "Alice Bob". Strip wikilink brackets and take the first whitespace token.
function personFirstName(raw: string): string {
  const stripped = raw.trim().replace(/^\[\[/, "").replace(/\]\]$/, "");
  return stripped.split(/\s+/)[0] || stripped;
}

function renderReprocessButton(sessionName: string, addedPeople: string[]): string {
  const firstNames = addedPeople.map(personFirstName).filter(Boolean);
  const label = `Reprocess with ${firstNames.join(", ")}`;
  const dismissLabel = "Dismiss reprocess suggestion";
  return `<button class="note-view-action reprocess-people" aria-label="${esc(label)}" title="${esc(label)}" onclick="window.__reprocessWithPeople(${js(sessionName)})">${esc(label)}</button>`
    + `<button class="note-view-action reprocess-dismiss" aria-label="${dismissLabel}" title="${dismissLabel}" onclick="window.__dismissReprocessPeople(${js(sessionName)})">${iconSvg(X, "ui-icon button-icon")}</button>`;
}

function renderMissingNoteFallback(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  const loading = ctx.cache.loading;
  if (loading) {
    return `
      <div class="captured-note-shell note-loading-shell" role="status" aria-label="Loading note">
        <section class="captured-note-main">
          <div class="review-content inline-review grounded-note note-loading-canvas" aria-hidden="true">
            <div class="note-loading-line note-loading-title"></div>
            <div class="note-loading-line note-loading-long"></div>
            <div class="note-loading-line note-loading-medium"></div>
            <div class="note-loading-line note-loading-short"></div>
            <div class="note-loading-line note-loading-long"></div>
            <div class="note-loading-line note-loading-medium"></div>
          </div>
        </section>
      </div>
    `;
  }
  const hasPath = Boolean(s.vault_note_path);
  const title = hasPath ? "Couldn’t open this note" : "This note isn’t available";
  const body = hasPath ? "Try again, or open it directly." : "It may have been moved or removed.";
  const showMakeAgain = !ctx.captureNote;
  return `
    <div class="captured-note-shell note-recovery-shell">
      <section class="captured-note-main">
        <div class="post-capture-hero note-recovery-hero">
          <div class="post-capture-hero-copy">
            <div class="eyebrow">Note</div>
            <h2>${esc(title)}</h2>
            <p>${esc(body)}</p>
          </div>
          ${showMakeAgain ? `
            <div class="captured-note-inline-actions note-recovery-actions">
              <button class="primary" onclick="window.__reprocessSession(${js(s.name)})">Make again</button>
            </div>
          ` : ""}
        </div>
      </section>
    </div>
  `;
}

function renderReadonlyMarks(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  const loadingMemos = ctx.cache.loading && s.memo_line_count > 0 && ctx.memoLines.length === 0;
  return `
    <div class="captured-memo-readonly" aria-label="Marked moments">
      ${ctx.memoLines.length > 0 ? ctx.memoLines.map(renderCapturedMemoLine).join("") : `
        <div class="captured-memo-empty">
          <strong>${loadingMemos ? "Loading marked moments…" : "No moments marked"}</strong>
          <span>${loadingMemos ? "Margins is reading the saved marks for this capture." : "You can still write a note from the transcript."}</span>
        </div>
      `}
    </div>
  `;
}

function renderNoteErrorPanel(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  const setupCopy = noteMakingSetupCopy(ctx.currentAiMode);
  const headline = noteFailureHeadline(ctx);
  return `
    <div class="captured-note-shell note-error-shell">
      <section class="captured-note-main">
        <div class="panel-heading panel-heading-row"><span>Note</span><span class="panel-pill note-error-pill">Needs attention</span></div>
        <div class="note-error-card">
          <p class="margo-safe-line">What you’ve captured so far is safe.</p>
          <strong>${esc(headline)}</strong>
          <p class="note-error-detail">${esc(ctx.noteError!)}</p>
          <p class="note-error-reassure">Your recording and marks are saved. Nothing was lost.</p>
        </div>
        ${renderReadonlyMarks(ctx)}
      </section>
      <div class="captured-note-action-bar">
        <div class="captured-note-action-copy">
          <strong>Try again</strong>
          <span>${ctx.settingsReady ? "Retry from the last saved checkpoint when one is available." : `${setupCopy.detail}, then retry.`}</span>
        </div>
        <div class="note-error-actions">
          ${ctx.settingsReady
            ? `<button class="primary capture-ready-action" onclick="window.__retrySession(${js(s.name)})">Try again</button>`
            : `<button class="primary" onclick="window.__nav('settings')">${setupCopy.action}</button>`}
          <button class="subtle" onclick="window.__dismissNoteError(${js(s.name)})">Keep capture as-is</button>
        </div>
      </div>
    </div>
  `;
}

function noteFailureHeadline(ctx: DistillTabRenderContext): string {
  const stage = ctx.session.failed_stage;
  const timedOut = /timed?\s*out|timeout/i.test(ctx.noteError || "");
  if (stage === "transcribe") return timedOut ? "Transcription timed out" : "Transcription failed";
  if (stage === "distill") return timedOut ? "Note generation timed out" : "Note generation failed";
  if (stage === "save") return "Note save failed";
  return "Couldn’t finish the note";
}

function renderCapturedMemoPage(ctx: DistillTabRenderContext): string {
  const s = ctx.session;
  const loadingMemos = ctx.cache.loading && s.memo_line_count > 0 && ctx.memoLines.length === 0;
  const markCount = ctx.memoLines.length;
  const status = ctx.finalizingCapture
    ? (ctx.jobState?.message || "Finishing capture...")
    : loadingMemos
    ? "Loading marks…"
    : `${markCount} mark${markCount === 1 ? "" : "s"}`;
  const setupCopy = noteMakingSetupCopy(ctx.currentAiMode);
  const finishAction = ctx.finalizingCapture
    ? `<button class="primary capture-finish-action" disabled>Finishing...</button>`
    : ctx.settingsReady
    ? `<button class="primary capture-finish-action" onclick="window.__processSession(${js(s.name)})">Write note</button>`
    : `<button class="primary capture-finish-action" onclick="window.__nav('settings')">${setupCopy.action}</button>`;
  const hint = ctx.finalizingCapture
    ? (ctx.jobState?.message || "Saving the final audio and transcript before note writing starts.")
    : ctx.settingsReady
    ? (ctx.contextReady
      ? "Write a note from these marks, the transcript, and related notes."
      : "Write a note from this conversation now. Connect notes for richer context later.")
    : `Your capture is saved. ${setupCopy.detail} in Settings to make the note.`;
  const deleteAction = ctx.finalizingCapture
    ? `<button class="subtle danger captured-delete-action action-with-icon" disabled>${iconSvg(Trash2, "ui-icon button-icon")}<span>Delete</span></button>`
    : `<button class="subtle danger captured-delete-action action-with-icon" onclick="window.__deleteSession(${js(s.name)})">${iconSvg(Trash2, "ui-icon button-icon")}<span>Delete</span></button>`;
  return `
    <div class="recording-view embedded-recording captured-recording">
      <div class="recording-header quiet-recording-header">
        <div class="recording-help"><strong>Marks</strong><span>${esc(status)}</span></div>
      </div>
      <div class="memo-editor captured-memo-editor">
        ${!ctx.contextReady && !ctx.finalizingCapture ? renderContextMissingNudge() : ""}
        ${markCount > 0 ? ctx.memoLines.map(renderCapturedMemoLine).join("") : `
          <div class="captured-memo-empty">
            <strong>${loadingMemos ? "Loading marks…" : "No moments marked"}</strong>
            <span>${loadingMemos ? "Margins is reading the saved marks for this capture." : "Margins can still make a note from the transcript."}</span>
          </div>
        `}
      </div>
      <div class="recording-footer">
        <div class="recording-footnote">${esc(hint)}</div>
        <div class="spacer"></div>
        ${deleteAction}
        ${finishAction}
      </div>
    </div>
  `;
}

function renderContextMissingNudge(): string {
  return `
    <div class="context-missing-nudge" role="note">
      <strong>Built from this conversation alone</strong>
      <span>Connect a vault or markdown folder when you want distillation to draw on people, threads, and past notes.</span>
      <button class="subtle" onclick="window.__nav('settings')">Connect notes</button>
    </div>
  `;
}

function noteMakingSetupCopy(mode: string): { action: string; detail: string } {
  if (mode === "included") {
    return { action: "Check settings", detail: "Check note settings" };
  }
  if (mode === "api") {
    return { action: "Add API key", detail: "Add your API key" };
  }
  return { action: "Sign in to make the note", detail: "Sign in to ChatGPT" };
}

function renderCapturedMemoLine(line: MemoLine): string {
  const isEdited = line.edited_secs !== null;
  const displayTs = formatElapsed(isEdited ? line.edited_secs! : line.created_secs);
  const preLive = line.audio_pending_at_mark
    ? `<span class="memo-audio-pending" title="Marked before audio capture was confirmed">audio pending</span>`
    : "";
  return `
    <div class="memo-line captured-memo-line">
      <div class="memo-gutter ${isEdited ? "edited" : ""}">${displayTs}${preLive}</div>
      <div class="captured-memo-text">${esc(line.text)}</div>
    </div>
  `;
}

function renderGroundedNote(ctx: DistillTabRenderContext, streaming: boolean): string {
  const state = ctx.groundedState;
  const baseline = streaming && state.baseline?.length ? state.baseline : null;
  let body: string;
  let showSkeleton = streaming;
  if (baseline) {
    // Reprocess-in-place: the previous note stays on screen and each paragraph
    // is swapped for its rewrite only once that paragraph has fully streamed in.
    // The final block of `state.blocks` is still accumulating lines, so it
    // stays hidden behind its baseline counterpart until the next block opens.
    const completed = state.blocks.slice(0, Math.max(0, state.blocks.length - 1));
    const remaining = baseline.slice(completed.length);
    body = [
      ...completed.map((block, index) => renderGroundedBlock(ctx, block, index)),
      ...remaining.map((block, index) => renderGroundedBlock(ctx, block, completed.length + index, {
        baseline: true,
        rewriting: index === 0,
      })),
    ].join("");
    showSkeleton = remaining.length === 0;
  } else {
    // While streaming, preview the in-progress line as part of its trailing
    // block so prose appears in place and grows monotonically into its settled
    // form — instead of the whole first paragraph staying invisible until its
    // newline lands. The preview is guarded so partial YAML/markers never show.
    const source = streaming ? previewBlocksWithLineBuffer(state) : state.blocks;
    const blocks = source.length ? source : [{ markdown: "", uses: [] }];
    body = blocks.map((block, index) => renderGroundedBlock(ctx, block, index)).join("");
  }
  return `
    <div class="review-content inline-review grounded-note distill-buffer ${streaming ? "streaming" : ""}">
      ${body}
      ${showSkeleton ? renderStreamSkeleton(state.blocks.length) : ""}
      ${renderMemoSourceStack(ctx, streaming)}
      ${state.warnings.length ? `<div class="marker-warning">${esc(state.warnings[state.warnings.length - 1])}</div>` : ""}
    </div>
  `;
}

// Shimmering placeholder shapes shown beneath the streamed note while Margins is
// still writing. There's no honest percentage to show, so instead of a spinner
// or progress number we render skeleton paragraphs. The shape is picked
// pseudo-randomly from `blockCount`, so each time a real paragraph slides in the
// placeholder below it re-rolls into a fresh, unpredictable arrangement while it
// waits for the next one — building a little anticipation for what's coming.
//
// The pick is a deterministic hash of `blockCount` rather than `Math.random()`:
// the panel rebuilds its innerHTML on every streaming tick, so a fresh random
// roll per render would make the skeleton flicker between shapes mid-section.
// Keying the choice to `blockCount` holds it stable while a section streams and
// only re-rolls when the next block lands. It also beats a plain modulo, which
// would just cycle the array in visible order.
//
// Every variant has the SAME structure — one short lead line plus a four-line
// paragraph — so only the widths shuffle, never the height. The panel anchors
// at the bottom of the scroll, and a skeleton that changed height between ticks
// would yank that anchor up or down. Keeping the height constant lets the
// recompose read as "more on the way" without disturbing the view.
const STREAM_SKELETON_SHAPES: number[][][] = [
  [[44], [98, 92, 100, 70]],
  [[38], [97, 88, 100, 76]],
  [[52], [100, 95, 84, 58]],
  [[40], [90, 100, 86, 72]],
  [[48], [100, 82, 94, 64]],
  [[36], [96, 90, 100, 78]],
  [[46], [88, 100, 90, 66]],
  [[42], [100, 86, 96, 74]],
];

// Stable-but-shuffled pick: an LCG-style hash spreads consecutive blockCounts
// across the array out of order, so insertions don't march through the shapes
// in sequence. We nudge off any choice that repeats the previous block's so two
// sections in a row never wear the same skeleton.
function pickSkeletonVariant(blockCount: number): number {
  const len = STREAM_SKELETON_SHAPES.length;
  const hash = (n: number) => ((n * 2654435761 + 0x9e3779b9) >>> 0) % len;
  let variant = hash(blockCount);
  if (blockCount > 0 && variant === hash(blockCount - 1)) {
    variant = (variant + 1) % len;
  }
  return variant;
}

function renderStreamSkeleton(blockCount: number): string {
  const variant = pickSkeletonVariant(blockCount);
  const paras = STREAM_SKELETON_SHAPES[variant].map(lines => `
    <div class="skel-para">
      ${lines.map(width => `<span class="skel-line" style="width:${width}%"></span>`).join("")}
    </div>
  `).join("");
  return `<div class="stream-skeleton" data-skeleton-variant="${variant}" aria-hidden="true">${paras}</div>`;
}

function renderMemoSourceStack(ctx: DistillTabRenderContext, streaming: boolean): string {
  if (!streaming) return "";
  const memos = ctx.memoLines;
  if (memos.length === 0) return "";
  const consumed = consumedMemoIds(ctx.groundedState);
  const remaining = memos
    .map((memo, index) => ({ memo, id: memoIdForIndex(index) }))
    .filter(item => !consumed.has(item.id));
  if (remaining.length === 0) return "";
  return `
    <div class="note-memo-source-stack" aria-label="Remaining marks">
      <div class="note-memo-source-heading">
        <strong>${remaining.length} mark${remaining.length === 1 ? "" : "s"} not yet woven in</strong>
        <span>These marks stay visible until the note accounts for them.</span>
      </div>
      ${remaining.map(({ memo, id }) => `
        <div class="memo-line note-buffer-memo-line" data-memo-id="${esc(id)}">
          <div class="memo-gutter">${formatElapsed(memo.created_secs)}</div>
          <div class="memo-text"><div class="note-buffer-memo-text">${esc(memo.text)}</div></div>
        </div>
      `).join("")}
    </div>
  `;
}

function renderGroundedBlock(
  ctx: DistillTabRenderContext,
  block: GroundedNoteBlock,
  index: number,
  opts?: { baseline?: boolean; rewriting?: boolean },
): string {
  if (!block.markdown.trim() && block.uses.length === 0) return "";
  const rendered = highlightGroundedQuotes(ctx.renderMarkdown(block.markdown), block.uses);
  const key = opts?.baseline ? `base-${index}` : String(index);
  const classes = [
    "grounded-block",
    block.uses.length ? "has-grounding" : "",
    opts?.baseline ? "baseline-stale" : "",
    opts?.rewriting ? "rewriting" : "",
  ].filter(Boolean).join(" ");
  return `
    <section class="${classes}" data-stream-block="${key}"${opts?.baseline ? ' data-stream-baseline="1"' : ""} data-stream-size="${esc(String(block.markdown.length))}">
      ${rendered}
    </section>
  `;
}

function highlightGroundedQuotes(html: string, uses: MarginsGroundingUse[]): string {
  let next = html;
  for (const use of uses) {
    const quote = use.note_quote?.trim();
    if (!quote) continue;
    const escapedQuote = esc(quote);
    if (!escapedQuote || !next.includes(escapedQuote)) continue;
    const memoIds = (use.memo_ids || []).join(", ");
    const title = memoIds ? `From ${memoIds.replace(/m0*/g, "mark ")}` : "From a captured mark";
    next = next.replace(
      escapedQuote,
      `<mark class="memo-quote-highlight" data-memo-ids="${esc(memoIds)}" title="${esc(title)}">${escapedQuote}</mark>`,
    );
  }
  return next;
}

function consumedMemoIds(state: GroundedNoteState): Set<string> {
  return new Set([
    ...state.consumedMemoIds,
    ...state.blocks.flatMap(block => block.uses).flatMap(use => use.memo_ids || []).map(id => id.toLowerCase()),
  ]);
}

// The right rail is ONE thread, not two stacked widgets. "How this note was
// made" is just the first, system-authored turn (turn 0); every refine the user
// asks for is the next turn. The provenance, the chat, and the composer all read
// as a single evolving conversation about the note. The composer stays pinned at
// the bottom across every state — present while the note is still being written,
// disabled (never removed) so the anchor never jumps and the user always knows
// where to ask for the next change.
function renderPiSidePanel(ctx: DistillTabRenderContext, streaming: boolean): string {
  const refinable = canRefine(ctx);
  // A refine re-stream is distinguished from an initial distillation by the
  // presence of an already-saved note: turn 0 exists, so the live activity
  // belongs to a *new* turn rather than to the provenance disclosure.
  const rewriting = streaming && Boolean(ctx.cache.note);
  return `
    <margins class="pi-side-panel agent-side-panel note-thread-panel">
      ${streaming ? "" : ctx.session.vault_note_path ? `
        <div class="side-saved-line">
          <span class="side-saved-dot">✓</span>
          <span class="side-saved-path">${esc(ctx.friendlyVaultPath)}</span>
        </div>` : ""}
      <div class="note-thread" data-note-thread>
        ${renderOriginTurn(ctx, streaming)}
        ${ctx.followupChat.map(renderThreadMessage).join("")}
        ${rewriting ? renderRefineLiveTurn() : ""}
      </div>
      ${renderNoteComposer(ctx, streaming, refinable)}
    </margins>
  `;
}

// Turn 0 — Margins authored the note on its own. While the very first draft is
// still streaming (no saved note yet) this turn IS the live status, so the
// making-of steps show expanded with a writing pulse. Once a note exists the
// same steps collapse behind a disclosure, demoting provenance to "available if
// you want it" so the composer can lead.
function renderOriginTurn(ctx: DistillTabRenderContext, streaming: boolean): string {
  if (streaming && !ctx.cache.note) {
    return `
      <div class="thread-turn thread-origin is-live">
        <div class="thread-live-head">
          <span class="thread-live-label">Writing note</span>
          <span class="writing-dots" aria-label="Writing"><i></i><i></i><i></i></span>
        </div>
        <div class="agent-message-list live-agent-list">
          ${renderAgentMessages(ctx, true)}
        </div>
      </div>
    `;
  }
  return `
    <div class="thread-turn thread-origin">
      <details class="agent-detail-log">
        <summary>How this note was made</summary>
        <div class="agent-message-list">
          ${renderAgentMessages(ctx, false)}
        </div>
      </details>
    </div>
  `;
}

function renderThreadMessage(message: FollowupChatMessage): string {
  const role = message.role === "user" ? "user" : "assistant";
  const roleLabel = message.role === "user" ? `<div class="thread-role">You</div>` : "";
  return `
    <div class="thread-turn ${role}">
      ${roleLabel}
      <div class="thread-body">${esc(message.text)}</div>
    </div>
  `;
}

// The live tail of a refine turn: the user's request is already in the thread as
// a `user` message; this is Margins picking it up. We deliberately don't replay
// the full step list here — the accumulated trace still holds turn 0's steps —
// just a calm "revising" pulse so the in-flight turn reads as active.
function renderRefineLiveTurn(): string {
  return `
    <div class="thread-turn assistant is-live">
      <div class="thread-live-head">
        <span class="thread-live-label">Revising the note</span>
        <span class="writing-dots" aria-label="Revising"><i></i><i></i><i></i></span>
      </div>
    </div>
  `;
}

// The persistent composer. Always rendered, never swapped out — only its enabled
// state changes. While a turn runs it's disabled with a busy affordance so the
// user can't type into a note that's mid-rewrite (and isn't surprised when the
// box vanishes). Before a note is refinable it's disabled with a hint instead of
// being replaced by an empty-state card.
function renderNoteComposer(ctx: DistillTabRenderContext, streaming: boolean, refinable: boolean): string {
  const name = ctx.session.name;
  const safeName = esc(name);
  const busy = streaming;
  const disabled = busy || !refinable;
  const hint = busy
    ? (ctx.cache.note
      ? "Rewriting this note — it stays put until the new version lands."
      : "Writing the note… you'll be able to refine it once it's saved.")
    : refinable
      ? ""
      : "Re-run processing to enable refining this note.";
  const placeholder = busy
    ? ""
    : "Ask for a change — e.g. tighten the summary, pull out the decisions, drop the small talk…";
  return `
    <div class="note-composer ${busy ? "is-busy" : ""} ${disabled ? "is-locked" : ""}">
      <div class="note-composer-heading">Refine this note</div>
      <textarea
        id="followup-input-${safeName}"
        class="composer-input"
        placeholder="${placeholder}"
        ${disabled ? "disabled" : ""}
        onkeydown="if((event.metaKey||event.ctrlKey)&&event.key==='Enter'){event.preventDefault();window.__sendFollowup(${js(name)});}"></textarea>
      <div class="composer-foot">
        <span class="composer-hint">${esc(hint)}</span>
        ${busy
          ? `<span class="composer-busy"><span class="writing-dots" aria-label="${ctx.cache.note ? "Revising" : "Writing"}"><i></i><i></i><i></i></span></span>`
          : `<button class="primary" ${refinable ? "" : "disabled"} onclick="window.__sendFollowup(${js(name)})">Apply change <span class="kbd-chip">⌘↵</span></button>`}
      </div>
    </div>
  `;
}

// Refining resumes the saved Pi conversation, so it's only available once the
// note has actually been distilled and saved to a known path.
function canRefine(ctx: DistillTabRenderContext): boolean {
  return Boolean(ctx.cache.note) && (ctx.session.status === "synthesized") && Boolean(ctx.session.vault_note_path);
}

function renderAgentMessages(ctx: DistillTabRenderContext, streaming: boolean): string {
  // While streaming, never substitute the all-"done" default trace for an empty
  // real one: with a long recording the prepare/transcription phase emits no
  // tool events for a while, and painting "Saved the note." green there reads as
  // finished when nothing has happened yet. Use the real trace as-is so the
  // in-progress fallback below shows instead. The default (all-done) preview is
  // only honest once the note is settled — i.e. the non-streaming provenance log.
  const source = streaming ? ctx.trace : traceForRender(ctx.trace);
  const trace = source.filter(event => event.kind !== "chat" && !/^(Thinking pass|Pi turn)$/i.test(event.label));
  const fallback: AgentSidebarMessage[] = streaming
    ? [
      { title: "Write note", detail: "Turning the marks and transcript into a meeting note.", tone: "active" },
      { title: "Use marks", detail: "Marks disappear from the buffer as their ideas are folded in." },
    ]
    : [{ title: "Saved", detail: "The note is saved. Quote timestamps are attached where transcript sources were used." }];
  const messages = dedupeMessagesByTitle(trace.length ? trace.map(agentEventMessage) : fallback);
  // The genuinely in-flight step is the last one still marked active — only it
  // should shimmer as "working" (header text + border), so the user's eye lands
  // on what Margins is doing right now instead of every prior step pulsing.
  const liveIndex = streaming ? lastActiveIndex(messages) : -1;
  return messages.map((message, index) => `
    <div class="agent-message ${message.tone || ""} ${index === liveIndex ? "is-live" : ""}">
      <strong>${esc(message.title)}</strong>
      <span>${esc(message.detail)}</span>
    </div>
  `).join("");
}

// Two trace entries can resolve to the same title — e.g. an in-flight
// "Looking for: …" search and its later "Found related notes" result — because
// the raw events carry different labels and never merge upstream. Collapse them
// by title here, keeping the latest (most-resolved) content at the slot where
// the title first appeared, so the list shows one row per step that simply
// updates in place rather than two stacked "querying" / "found" copies.
function dedupeMessagesByTitle(messages: AgentSidebarMessage[]): AgentSidebarMessage[] {
  const order: string[] = [];
  const latest = new Map<string, AgentSidebarMessage>();
  for (const message of messages) {
    if (!latest.has(message.title)) order.push(message.title);
    latest.set(message.title, message);
  }
  return order.map(title => latest.get(title)!);
}

function lastActiveIndex(messages: AgentSidebarMessage[]): number {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i].tone === "active") return i;
  }
  return -1;
}

function traceForRender(trace: DistillTraceEvent[]): DistillTraceEvent[] {
  return trace.length ? trace : defaultDistillTrace();
}
