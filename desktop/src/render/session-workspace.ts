import {
  AlertCircle,
  CheckCircle2,
  FileText,
  LoaderCircle,
  MoreHorizontal,
  Minus,
  Flag,
  Play,
  Radio,
  Plus,
  RefreshCw,
  RotateCcw,
  Tag,
  UserRound,
  Trash2,
  UsersRound,
  type IconNode,
} from "lucide";

import type { MemoLine, RecordingStatus, SessionInfo } from "../lib/tauri";
import { renderCaptureFooterAction, renderCaptureSignalStrip, renderMobileCaptureHero, type RecordingStartupRenderState } from "./recording";
import { formatDuration, formatTime } from "../lib/format";
import { esc, iconSvg, js } from "../lib/html";
import { jobBannerCancelAction } from "../lib/job-banner-action";
import {
  frontmatterDateLabel,
  isCaptureNote,
  sessionFrontmatterPeople,
  sessionFrontmatterTags,
  sessionFrontmatterType,
} from "../lib/session-model";

export interface LiveBackchannelRenderContext {
  elapsedLabel: string;
  statusLine: string;
  capturePhase?: string;
  /** True when capture is paused (segment finalized, resumable). */
  paused: boolean;
  /** Whether notes + AI setup is complete, so Finish can make the note. */
  settingsReady: boolean;
  currentAiMode: string;
  stopLabel: string;
  stopHint: string;
  stopShortcutHint?: string;
  speakerCount: number;
  audioHealthHtml: string;
  recordingStatus: RecordingStatus;
  startupState: RecordingStartupRenderState | null;
  memoLines: MemoLine[];
  renderMemoLine: (line: MemoLine, index: number) => string;
  backchannelRailHtml?: string;
  /** True when the clock is stopped (paused mid-meeting) — swap in the block rail. */
  clockStopped?: boolean;
  /** Prep-hydration "Look over this" rail for the current clock-stopped block. */
  blockMarginRailHtml?: string;
  /** Clock-stopped capture before audio has started. */
  preStart?: boolean;
  /** Calendar auto-start countdown shown during pre-start prep. */
  autoStartCountdownText?: string | null;
  /** Why a requested/automatic prep start could not begin. */
  prepStartBlockedReason?: string | null;
  /** Mobile capture posture (hosted web on a narrow screen): rolling waveform
   *  hero replaces the footer signal strip. */
  mobileCapture?: boolean;
}

export interface SessionWorkspaceRenderContext {
  session: SessionInfo;
  title: string;
  /** User-driven description only (empty when the capture has just a timestamp). */
  titleDescription: string;
  /** Formatted capture timestamp shown alongside the title. */
  dateLabel: string;
  captureNote: boolean;
  savedNoteTitleMeta: string;
  /** Whether the Obsidian vault is connected, for the header Open handler. */
  obsidianVaultReady: boolean;
  /** True once a note (capture or connected) is loaded and not processing — surfaces Copy in the header. */
  settledNote: boolean;
  /** Browser-local ownership copy for an active hosted capture. */
  liveCaptureMeta?: string;
  peopleSuggestions: string[];
  globalActionsHtml: string;
  primaryPanelHtml: string;
}

export interface JobBannerRenderContext {
  jobName: string;
  error: boolean;
  progress: number;
  message: string;
  errorTitle?: string;
}

export function sessionStatusLabel(status: SessionInfo["status"]): string {
  switch (status) {
    case "recording": return "Recording";
    case "processing": return "Writing note";
    case "failed": return "Needs attention";
    case "synthesized": return "Saved";
    case "unprocessed": return "Ready to write note";
  }
}

function sessionStatusIcon(status: SessionInfo["status"]): IconNode {
  switch (status) {
    case "recording": return Radio;
    case "processing": return LoaderCircle;
    case "failed": return AlertCircle;
    case "synthesized": return FileText;
    case "unprocessed": return CheckCircle2;
  }
}

export function renderSessionStatus(s: SessionInfo, label = sessionStatusLabel(s.status)): string {
  const captureNote = isCaptureNote(s);
  return `<span class="session-status ${captureNote ? "capture-note" : s.status}" aria-label="${esc(label)}" title="${esc(label)}">${iconSvg(captureNote ? FileText : sessionStatusIcon(s.status), "ui-icon status-icon")}<span>${esc(label)}</span></span>`;
}

export function renderSessionKicker(s: SessionInfo): string {
  if (isCaptureNote(s)) {
    const time = frontmatterDateLabel(s) || formatTime(s.start_time);
    return esc(time);
  }
  const duration = s.duration_secs > 0 ? ` · ${formatDuration(s.duration_secs)}` : "";
  const timeAndDuration = `${formatTime(s.start_time)}${duration}`;
  if (s.status === "recording") return esc(timeAndDuration);
  if (s.status === "unprocessed") return `<span class="session-kicker-plain">Recorded ${esc(timeAndDuration)}</span>`;
  if (s.status === "synthesized") return "";
  const label = sessionStatusLabel(s.status);
  return `<span class="session-kicker-status ${s.status}">${iconSvg(sessionStatusIcon(s.status), "ui-icon kicker-icon")}<span>${esc(label)}</span></span><span class="session-kicker-separator">·</span>${esc(timeAndDuration)}`;
}

export function renderSessionWorkspace(ctx: SessionWorkspaceRenderContext): string {
  const s = ctx.session;
  const live = s.status === "recording";
  const liveMeta = live ? (ctx.liveCaptureMeta || "Capturing locally") : "";
  const titleMeta = ctx.captureNote
    ? ""
    : [ctx.dateLabel ? esc(ctx.dateLabel) : "", liveMeta].filter(Boolean).join(" · ");
  const removeLabel = ctx.captureNote ? "Remove" : "Delete";
  const kicker = renderSessionKicker(s);
  const overflowMenu = `
    <details class="session-more-actions">
      <summary aria-label="More actions" title="More actions">${iconSvg(MoreHorizontal, "ui-icon menu-icon")}</summary>
      <div class="session-more-menu" role="menu">
        <button class="subtle action-with-icon" role="menuitem" onclick="window.__loadArtifacts(${js(s.name)})">${iconSvg(RefreshCw, "ui-icon button-icon")}<span>Refresh</span></button>
        ${ctx.settledNote && !ctx.captureNote ? `<button class="subtle action-with-icon" id="review-rerun" role="menuitem" data-session="${esc(s.name)}" onclick="window.__reprocessSession(${js(s.name)})">${iconSvg(RotateCcw, "ui-icon button-icon")}<span>Re-run processing</span></button>` : ""}
        <button class="subtle danger action-with-icon" role="menuitem" onclick="window.__deleteSession(${js(s.name)})">${iconSvg(Trash2, "ui-icon button-icon")}<span>${removeLabel}</span></button>
      </div>
    </details>
  `;
  return `
    <div class="session-workspace ${ctx.captureNote ? "capture-note-workspace" : ""} ${live ? "recording-workspace" : ""}">
      <header class="session-header">
        <div class="session-header-main">
          ${kicker ? `<div class="session-kicker">${kicker}</div>` : ""}
          ${ctx.captureNote ? `
            <input
              class="session-title-input"
              aria-label="Note title"
              placeholder="Add a title"
              title="${esc(ctx.title)}"
              value="${esc(ctx.titleDescription || ctx.title)}"
              onkeydown="window.__sessionTitleKeydown(event, ${js(s.name)})"
              onblur="window.__saveSessionTitle(${js(s.name)}, this.value)"
            />
          ` : `
            <input
              class="session-title-input"
              aria-label="Capture title"
              placeholder="Add a title"
              title="${esc(ctx.title)}"
              value="${esc(ctx.titleDescription)}"
              onkeydown="window.__sessionTitleKeydown(event, ${js(s.name)})"
              onblur="window.__saveSessionTitle(${js(s.name)}, this.value)"
            />
          `}
          ${titleMeta ? `<div class="session-title-meta">${titleMeta}</div>` : ""}
          ${live ? "" : renderSavedNoteHeaderMeta(s, ctx.peopleSuggestions)}
        </div>
        <div class="session-header-actions">
          ${ctx.globalActionsHtml}
          ${live ? "" : overflowMenu}
        </div>
      </header>

      <div class="session-tab-panel primary-session-panel">
        ${ctx.primaryPanelHtml}
      </div>
    </div>
  `;
}

function renderSavedNoteHeaderMeta(s: SessionInfo, peopleSuggestions: string[]): string {
  const groups = savedNoteHeaderGroups(s, peopleSuggestions);
  return groups.length
    ? `<div class="saved-note-meta-row">${groups.join("")}</div>`
    : "";
}

function savedNoteHeaderGroups(s: SessionInfo, peopleSuggestions: string[]): string[] {
  const groups: string[] = [];
  const type = sessionFrontmatterType(s);
  const tags = [...new Set(sessionFrontmatterTags(s).map(tag => tag.replace(/^#/, "")))].slice(0, 3);
  if (type) {
    groups.push(`<span class="saved-note-meta saved-note-type">${iconSvg(FileText, "ui-icon saved-note-meta-icon")}<span>${esc(type)}</span></span>`);
  }
  groups.push(renderPeopleEditor(s, peopleSuggestions));
  if (tags.length) {
    groups.push(`<span class="saved-note-meta saved-note-tags">${iconSvg(Tag, "ui-icon saved-note-meta-icon")}<span>${tags.map(tag => `#${esc(tag)}`).join(" ")}</span></span>`);
  }
  return groups;
}

export function renderJobBanner(ctx: JobBannerRenderContext | null): string {
  if (!ctx) return "";
  return `
    <div class="job-banner ${ctx.error ? "error processing-error" : ""}">
      <div class="job-banner-top">
        <div>
          <strong>${ctx.error ? esc(ctx.errorTitle || "Couldn’t finish the note") : "Writing note"}</strong>
          <span>${esc(ctx.jobName)}</span>
        </div>
        <div class="job-progress-label">${Math.round(Math.max(0, Math.min(1, ctx.progress)) * 100)}%</div>
      </div>
      <div class="job-banner-message">${esc(ctx.message)}</div>
      ${ctx.error ? `<div class="job-banner-message">Your recording and marks are saved.</div>` : ""}
      <div class="progress-bar compact"><div class="fill" style="width: ${Math.max(4, Math.min(100, ctx.progress * 100))}%"></div></div>
      <div class="job-banner-actions">
        ${ctx.error ? `
          <button class="primary" onclick="window.__retrySession(${js(ctx.jobName)})">Try again</button>
          <button onclick="window.__nav('settings')">Open setup</button>
        ` : ""}
        <button class="subtle" onclick="window.${jobBannerCancelAction(ctx.error)}(${js(ctx.jobName)})">Cancel</button>
      </div>
    </div>
  `;
}

function renderPeopleEditor(s: SessionInfo, suggestions: string[]): string {
  const people = sessionFrontmatterPeople(s);
  const suggestionMenu = renderPeopleSuggestions(s.name, suggestions);
  if (!people.length && !s.calendar_event_title) {
    return `
      <div class="people-inline people-inline-empty" data-session="${esc(s.name)}">
        <span class="people-empty-add">
          <label class="people-add-label" for="people-input-${esc(s.name)}">
            <span class="people-label icon-label" title="People">${iconSvg(UsersRound, "ui-icon people-icon")}<span class="visually-hidden">People</span></span>
            <span class="people-add-plus">+</span>
            <input id="people-input-${esc(s.name)}" class="people-pill-input" type="text" placeholder="Add person…" autocomplete="off" oninput="window.__peopleInputInput(event)" onkeydown="window.__peopleInputKeydown(event, ${js(s.name)})" />
          </label>
          ${suggestionMenu}
        </span>
      </div>
    `;
  }

  return `
    <div class="people-inline" data-session="${esc(s.name)}">
      <div class="people-inline-head"><span class="people-label icon-label" title="People">${iconSvg(UsersRound, "ui-icon people-icon")}<span class="visually-hidden">People</span></span></div>
      <div class="people-pill-row">
        ${people.map(person => `
          <span class="person-pill">${esc(person)} <button title="Remove ${esc(person)}" onclick="window.__removePerson(${js(s.name)}, ${js(person)})">×</button></span>
        `).join("")}
        <span class="people-add-shell">
          <label class="people-add-label" for="people-input-${esc(s.name)}">
            <span class="people-add-plus">+</span>
            <input id="people-input-${esc(s.name)}" class="people-pill-input" type="text" placeholder="Add" autocomplete="off" oninput="window.__peopleInputInput(event)" onkeydown="window.__peopleInputKeydown(event, ${js(s.name)})" />
          </label>
          ${suggestionMenu}
        </span>
      </div>
    </div>
  `;
}

function renderPeopleSuggestions(sessionName: string, suggestions: string[]): string {
  if (!suggestions.length) return "";
  return `
    <span class="people-suggestion-menu" role="listbox" aria-label="People suggestions">
      ${suggestions.slice(0, 8).map(person => `
        <button
          type="button"
          class="people-suggestion-item"
          role="option"
          data-person="${esc(person)}"
          onmousedown="event.preventDefault()"
          onclick="window.__selectPersonSuggestion(${js(sessionName)}, ${js(person)})"
        >
          ${iconSvg(UserRound, "ui-icon people-suggestion-icon")}
          <span>${esc(person)}</span>
        </button>
      `).join("")}
    </span>
  `;
}

export function renderLiveBackchannel(ctx: LiveBackchannelRenderContext): string {
  const footerAction = renderCaptureFooterAction({
    startupState: ctx.startupState,
    capturePhase: ctx.capturePhase,
    paused: ctx.paused,
    preStart: ctx.preStart,
    autoStartCountdownText: ctx.autoStartCountdownText,
    prepStartBlockedReason: ctx.prepStartBlockedReason,
    finishLabel: ctx.settingsReady ? "Finish" : noteMakingSetupCopy(ctx.currentAiMode).shortAction,
    pauseLabel: ctx.stopLabel,
  });
  const discardAction = renderLiveCaptureDiscardAction(ctx);
  const mobileHero = ctx.mobileCapture && !ctx.preStart && (!ctx.clockStopped || ctx.paused)
    ? renderMobileCaptureHero(ctx.recordingStatus, ctx.startupState)
    : "";
  return `
    <div class="recording-view embedded-recording${ctx.mobileCapture ? " mobile-capture" : ""}">
      <div class="recording-header quiet-recording-header">
        <div class="recording-help"><strong>Marks</strong><span id="rec-status-text">${esc(ctx.statusLine)}</span></div>
        ${discardAction}
      </div>
      ${mobileHero}
      <div id="audio-health-slot">${ctx.audioHealthHtml}</div>
      ${(() => {
        // Clock-stopped (mid-meeting pause) shows the prep block-margin rail; while
        // the clock runs it shows the live backchannel rail.
        const railHtml = ctx.clockStopped ? (ctx.blockMarginRailHtml ?? "") : (ctx.backchannelRailHtml ?? "");
        const editorHtml = `
          <div class="memo-editor" id="memo-editor">
            ${ctx.memoLines.map((line, index) => ctx.renderMemoLine(line, index)).join("")}
          </div>`;
        return railHtml
          ? `<div class="live-backchannel-layout">${editorHtml}${railHtml}</div>`
          : editorHtml;
      })()}
      <div class="recording-footer${ctx.paused ? " paused-posture" : ""}">
        ${ctx.paused ? "" : `<span class="${ctx.preStart ? "capture-footer-reserved" : ""}">${renderCaptureSignalStrip(ctx.recordingStatus, ctx.startupState)}</span>`}
        ${ctx.paused ? "" : `<div class="recording-footnote ${ctx.preStart ? "capture-footer-reserved" : ""}">${
          `Enter marks the moment · ${ctx.stopShortcutHint ? `${esc(ctx.stopShortcutHint)} pauses capture · ` : ""}${esc(ctx.stopHint)}`}</div>`}
        <div class="spacer"></div>
        ${footerAction}
      </div>
    </div>
  `;
}

function renderLiveCaptureDiscardAction(ctx: LiveBackchannelRenderContext): string {
  if (ctx.preStart) return "";
  if (ctx.startupState?.phase === "starting") return "";
  if (ctx.startupState?.phase === "failed") return "";
  if (ctx.capturePhase === "pausing" || ctx.capturePhase === "finalizing") return "";
  return `
    <button class="icon-button capture-discard-action" aria-label="Discard capture" title="Discard capture" onclick="window.__discardActiveRecording()">
      ${iconSvg(Trash2, "ui-icon button-icon")}
    </button>
  `;
}

function renderCaptureSpeakerControl(count: number): string {
  const clamped = Math.max(0, Math.min(4, Math.round(Number.isFinite(count) ? count : 0)));
  const value = clamped === 0 ? "auto" : String(clamped);
  const speakerLabel = clamped === 0 ? "Auto speaker count" : `${clamped} speaker${clamped === 1 ? "" : "s"}`;
  const decrementDisabled = clamped <= 0 ? "disabled" : "";
  const incrementDisabled = clamped >= 4 ? "disabled" : "";
  return `
    <div class="capture-speaker-control" title="Speaker count before writing the note" aria-label="Speaker count before writing the note">
      <button class="capture-speaker-step" ${decrementDisabled} aria-label="Fewer speakers" title="Fewer speakers" onclick="window.__adjustCaptureSpeakerCount(-1)">${iconSvg(Minus, "ui-icon button-icon")}</button>
      <button class="capture-speaker-value" aria-label="${esc(speakerLabel)}" title="${esc(speakerLabel)}" onclick="window.__cycleCaptureSpeakerCount()">
        ${iconSvg(clamped === 1 ? UserRound : UsersRound, "ui-icon button-icon")}
        <span>${esc(value)}</span>
      </button>
      <button class="capture-speaker-step" ${incrementDisabled} aria-label="More speakers" title="More speakers" onclick="window.__adjustCaptureSpeakerCount(1)">${iconSvg(Plus, "ui-icon button-icon")}</button>
    </div>
  `;
}

function noteMakingSetupCopy(mode: string): { shortAction: string; detail: string } {
  if (mode === "included") {
    return { shortAction: "Check settings", detail: "Check note settings" };
  }
  if (mode === "api") {
    return { shortAction: "Add API key", detail: "Add your API key" };
  }
  return { shortAction: "Sign in to write note", detail: "Sign in to ChatGPT" };
}
