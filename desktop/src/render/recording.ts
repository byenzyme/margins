import { LoaderCircle, Pause, Play, Trash2 } from "lucide";
import type { DeviceInfo, MemoLine, RecordingStatus } from "../lib/tauri";
import { formatElapsed, levelBars, levelToNormalized } from "../lib/format";
import { esc, iconSvg } from "../lib/html";
import { shouldShowCaptureDeviceToast } from "../lib/audio-health-priority";
import { captureSignalReadiness, type CaptureSignalLane } from "../lib/capture-signal-state";
import { captureFooterPosture } from "../lib/capture-footer-state";
import { isTapDevice } from "../lib/tap-device";
import { microphoneQuietWarning } from "../lib/microphone-level";
import type { WebMicrophoneFailureCategory } from "../lib/web-microphone-permission";

export type TapRecoveryState = "idle" | "recovering" | "recovered" | "failed";
export type RecordingStartupPhase = "starting" | "failed";

export interface RecordingStartupRenderState {
  phase: RecordingStartupPhase;
  message: string;
  error?: string | null;
  cancelRequested?: boolean;
  waitingForPermission?: boolean;
  webMicrophoneFailure?: WebMicrophoneFailureCategory | null;
}

export interface RecordingRenderContext {
  devices: DeviceInfo[];
  recordingStatus: RecordingStatus;
  statusLine: string;
  startupState: RecordingStartupRenderState | null;
  memoLines: MemoLine[];
  selectedDeviceName: string | null;
  settingsReady: boolean;
  dismissedTapNoticeSession: string | null;
  tapRecoverySession: string | null;
  tapRecoveryState: TapRecoveryState;
  tapRecoveryError: string | null;
  captureDeviceSwitchToast: CaptureDeviceToast | null;
  backchannelRailHtml?: string;
  memoBackchannelButtonHtml?: (index: number) => string;
  /** Clock-stopped affordances */
  clockStopped?: boolean;
  /** Auto-start countdown: text to show in footer (e.g. "Starts when the meeting does · 2:34"). Null = no countdown. */
  autoStartCountdownText?: string | null;
  prepStartBlockedReason?: string | null;
  /** Block-margin rail HTML for current block (clock-stopped surface). */
  blockMarginRailHtml?: string;
  /** Returns wall-clock label for a given block ordinal (e.g. "1:30 PM"). Used for pause line gutter labels. */
  blockWallClock?: (ordinal: number) => string | undefined;
  /** Mobile capture posture (hosted web on a narrow screen): rolling waveform
   *  hero replaces the footer signal strip; header collapses. */
  mobileCapture?: boolean;
}

export interface CaptureFooterActionContext {
  startupState: RecordingStartupRenderState | null;
  capturePhase?: string;
  paused: boolean;
  preStart?: boolean;
  autoStartCountdownText?: string | null;
  prepStartBlockedReason?: string | null;
  finishLabel?: string;
  pauseLabel?: string;
}

export type CaptureDeviceToast =
  | { kind: "switch"; deviceName: string; previousDevice: string; retryDevice: null }
  | { kind: "fallback"; deviceName: string; pinnedName: string | null; retryDevice: null }
  | { kind: "recovery"; deviceName: string | null; retryDevice: null }
  | { kind: "switch_failed"; deviceName: string | null; previousDevice: string | null; retryDevice: string | null }
  | { kind: "preference_error"; message: string; retryDevice: null };

const WAVEFORM_SHAPE = [0.34, 0.58, 0.82, 0.48, 1, 0.66, 0.88, 0.52, 0.3];

function renderSignalLane(
  id: "mic" | "system",
  name: string,
  peak: number | null,
  readiness: CaptureSignalLane,
): string {
  const level = levelToNormalized(peak).toFixed(3);
  const bars = WAVEFORM_SHAPE
    .map(height => `<i style="height:${Math.round(height * 100)}%" aria-hidden="true"></i>`)
    .join("");
  return `
    <span class="capture-signal-lane ${readiness.state}" id="${id}-signal-lane" aria-label="${esc(name)}: ${esc(readiness.label)}">
      <span class="capture-signal-name">${esc(name)}</span>
      <span class="capture-waveform" id="${id}-waveform" style="--signal-level:${level}" aria-hidden="true">${bars}</span>
      <span class="capture-signal-state" id="${id}-signal-state">${esc(readiness.label)}</span>
    </span>
  `;
}

export function renderCaptureSignalStrip(
  status: RecordingStatus,
  startupState: RecordingStartupRenderState | null,
): string {
  const readiness = captureSignalReadiness(status, startupState);
  return `
    <span class="capture-signal-strip" aria-label="Audio capture readiness">
      ${renderSignalLane("mic", "Mic", status.mic_level, readiness.mic)}
      ${renderSignalLane("system", "Computer", status.spk_level, readiness.system)}
    </span>
  `;
}

export function renderMobileCaptureHero(
  status: RecordingStatus,
  startupState: RecordingStartupRenderState | null,
): string {
  const mic = captureSignalReadiness(status, startupState).mic;
  // Thumb-reachable pause/discard live with the waveform on mobile; the
  // duplicate footer pause and header discard are hidden via CSS.
  const pausing = status.capture_phase === "pausing";
  const paused = status.paused ?? false;
  const pauseOrResume = paused
    ? `<button class="icon-button mobile-capture-resume" aria-label="Resume capture" title="Resume capture" onclick="window.__resumeRecording()">${iconSvg(Play, "ui-icon button-icon")}</button>`
    : `<button class="icon-button mobile-capture-pause" aria-label="Pause capture" title="Pause capture" onclick="window.__pauseRecording()">${iconSvg(Pause, "ui-icon button-icon mobile-capture-pause-icon")}${iconSvg(LoaderCircle, "ui-icon button-icon capture-pausing-spinner mobile-capture-pausing-icon")}</button>`;
  const hostedObserver = status.capture_phase === "interrupted"
    || status.capture_phase === "capturing_elsewhere"
    || status.capture_phase === "recording_unknown";
  const controls = (status.is_recording || paused) && status.capture_phase !== "finalizing" && !hostedObserver ? `
    <span class="mobile-capture-hero-controls">
      <button class="icon-button mobile-capture-discard" aria-label="Discard capture" title="Discard capture" onclick="window.__discardActiveRecording()">${iconSvg(Trash2, "ui-icon button-icon")}</button>
      ${pauseOrResume}
    </span>
  ` : "";
  return `
    <div class="mobile-capture-hero ${mic.state}${pausing ? " pausing" : ""}" id="mobile-capture-hero" aria-label="Microphone: ${esc(mic.label)}">
      <canvas id="mobile-capture-waveform" aria-hidden="true"></canvas>
      <div class="mobile-capture-hero-meta">
        <span class="mobile-capture-timer" id="mobile-capture-timer">${esc(formatElapsed(status.elapsed_secs))}</span>
        <span class="mobile-capture-state" id="mobile-capture-state">${esc(mic.label)}</span>
        ${controls}
      </div>
    </div>
  `;
}

export function renderRecordingView(ctx: RecordingRenderContext, chrome: { windowDragRegion: string; settingsOverlay: string }): string {
  const footerAction = renderCaptureFooterAction({
    startupState: ctx.startupState,
    capturePhase: ctx.recordingStatus.capture_phase,
    paused: ctx.recordingStatus.paused ?? false,
    finishLabel: "Finish",
  });
  const discardAction = renderCaptureDiscardAction(ctx);
  // Determine which rail to show: block-margin rail (clock-stopped) takes precedence over backchannel.
  const railHtml = ctx.clockStopped ? (ctx.blockMarginRailHtml ?? "") : (ctx.backchannelRailHtml ?? "");
  const editorHtml = `
    <div class="memo-editor" id="memo-editor">
      ${ctx.memoLines.map((line, i) => renderMemoLine(line, i, ctx.memoBackchannelButtonHtml, ctx.blockWallClock)).join("")}
    </div>
  `;
  const mobileHero = ctx.mobileCapture && (!ctx.clockStopped || (ctx.recordingStatus.paused ?? false))
    ? renderMobileCaptureHero(ctx.recordingStatus, ctx.startupState)
    : "";
  return `
    ${chrome.windowDragRegion}
    <div class="recording-view${ctx.clockStopped ? " clock-stopped-posture" : ""}${ctx.mobileCapture ? " mobile-capture" : ""}">
      <div class="recording-header quiet-recording-header">
        <div class="recording-help"><strong>${esc(ctx.recordingStatus.session_name || "Untitled")}</strong><span id="rec-status-text">${esc(ctx.statusLine)}</span></div>
        ${discardAction}
      </div>
      ${mobileHero}
      <div id="audio-health-slot">${renderAudioHealth(ctx)}</div>
      ${railHtml ? `
        <div class="live-backchannel-layout">
          ${editorHtml}
          ${railHtml}
        </div>
      ` : editorHtml}
      <div class="recording-footer">
        ${ctx.clockStopped ? renderClockStoppedFooter(ctx) : `
          ${renderCaptureSignalStrip(ctx.recordingStatus, ctx.startupState)}
          <select id="device-select" disabled title="Change input device in setup before capture">
            ${ctx.devices.filter(d => !isTapDevice(d.name)).map(d => `<option value="${esc(d.uid)}" ${ctx.selectedDeviceName === d.name ? "selected" : ""}>${esc(d.name)}</option>`).join("")}
          </select>
          <div class="spacer"></div>
          ${footerAction}
        `}
      </div>
    </div>
    ${chrome.settingsOverlay}
  `;
}

function renderClockStoppedFooter(ctx: RecordingRenderContext): string {
  return `<div class="spacer"></div>${renderCaptureFooterAction({
    startupState: ctx.startupState,
    capturePhase: ctx.recordingStatus.capture_phase,
    paused: ctx.recordingStatus.paused ?? false,
    preStart: !ctx.recordingStatus.is_recording && !ctx.startupState,
    autoStartCountdownText: ctx.autoStartCountdownText,
    finishLabel: "Finish",
  })}`;
}

export function renderCaptureDiscardAction(ctx: RecordingRenderContext): string {
  if (ctx.startupState?.phase === "starting") return "";
  if (ctx.startupState?.phase === "failed") return "";
  if (!ctx.recordingStatus.is_recording) return "";
  if (ctx.recordingStatus.capture_phase === "pausing" || ctx.recordingStatus.capture_phase === "finalizing") return "";
  if (["interrupted", "capturing_elsewhere", "recording_unknown"].includes(ctx.recordingStatus.capture_phase)) return "";
  return `
    <button class="icon-button capture-discard-action" aria-label="Discard capture" title="Discard capture" onclick="window.__discardActiveRecording()">
      ${iconSvg(Trash2, "ui-icon button-icon")}
    </button>
  `;
}

export function renderCaptureFooterAction(ctx: CaptureFooterActionContext): string {
  if (ctx.preStart) {
    if (ctx.prepStartBlockedReason) {
      return `
        <span class="clock-stopped-countdown needs-attention">${esc(ctx.prepStartBlockedReason)}</span>
        <button class="primary" onclick="window.__startRecordingFromPrep()">Try again</button>
        <button class="subtle" onclick="window.__openAudioSetup()">Setup</button>
      `;
    }
    if (ctx.autoStartCountdownText) {
      return `
        <span class="clock-stopped-countdown">${esc(ctx.autoStartCountdownText)}</span>
        <button class="primary capture-primary-action" onclick="window.__startRecordingFromPrep()">Start now</button>
        <button class="subtle" onclick="window.__holdOffAutoStart()">hold off</button>
      `;
    }
    return `<button class="primary capture-primary-action" onclick="window.__startRecordingFromPrep()">Start</button>`;
  }
  if (ctx.startupState?.phase === "starting") {
    if (ctx.startupState.waitingForPermission) {
      return `<span class="capture-pause-footer-copy">${esc(ctx.startupState.message || "Waiting for microphone permission…")}</span><button class="subtle" onclick="window.__cancelRecordingStartup()" ${ctx.startupState.cancelRequested ? "disabled" : ""}>${ctx.startupState.cancelRequested ? "Cancelling…" : "Cancel"}</button>`;
    }
    return `<button class="primary capture-primary-action action-with-icon" disabled><span class="inline-spinner" aria-hidden="true"></span><span>Starting…</span></button>`;
  }
  if (ctx.startupState?.phase === "failed") {
    const failure = startupFailurePresentation(ctx.startupState.webMicrophoneFailure);
    return `${failure?.retry === false ? `<button class="primary" onclick="window.__openAudioSetup()">Audio setup</button>` : `<button class="primary" onclick="window.__retryRecordingStartup()">Retry audio</button>`}<button class="subtle" onclick="window.__discardRecordingStartup()">Discard draft</button>`;
  }
  if (ctx.capturePhase === "interrupted") {
    return `
      <span class="capture-pause-footer-copy needs-attention">Browser capture owner stopped responding</span>
      <button class="primary" onclick="window.__takeOverInterruptedRecording()">Take control</button>
    `;
  }
  if (ctx.capturePhase === "capturing_elsewhere") {
    return `<span class="capture-pause-footer-copy">Capturing in another tab — controls stay with that tab</span>`;
  }
  if (ctx.capturePhase === "recording_unknown") {
    return `<span class="capture-pause-footer-copy needs-attention">Capture ownership is unknown — use the original tab</span>`;
  }
  if (ctx.capturePhase === "interrupted_recovery_empty") {
    return `<span class="capture-pause-footer-copy needs-attention">No durable audio was received — discard this partial capture</span>`;
  }
  if (ctx.capturePhase === "interrupted_recovery") {
    return `<span class="capture-pause-footer-copy needs-attention">Recovery control acquired — received audio is ready to finalize</span><button class="primary capture-finish-action" onclick="window.__finishRecording()">${esc(ctx.finishLabel || "Finish and save")}</button>`;
  }
  const posture = captureFooterPosture(ctx.capturePhase, ctx.paused);
  if (posture === "pausing") {
    return `<button class="subtle capture-pausing-action" disabled>${iconSvg(LoaderCircle, "ui-icon button-icon capture-pausing-spinner")}<span>Pausing…</span></button>`;
  }
  if (posture === "paused") {
    return `
      <span class="capture-pause-footer-copy">Paused — notes still land, untimed</span>
      <button class="primary capture-primary-action capture-resume-action" onclick="window.__resumeRecording()">Resume</button>
      <button class="subtle capture-finish-action" onclick="window.__finishRecording()">${esc(ctx.finishLabel || "Finish")}</button>
    `;
  }
  return `<button class="primary capture-primary-action capture-pause-action action-with-icon" onclick="window.__pauseRecording()">${iconSvg(Pause, "ui-icon button-icon")}<span>${esc(ctx.pauseLabel || "Pause")}</span></button>`;
}

export function renderAudioHealth(ctx: RecordingRenderContext): string {
  const otherRecoveries = (ctx.recordingStatus.web_recoveries || [])
    .filter(recovery => recovery.recording_id !== ctx.recordingStatus.web_recording_id);
  if (otherRecoveries.length > 0 && [
    "recording",
    "paused",
    "capturing_elsewhere",
    "interrupted",
    "interrupted_recovery",
    "interrupted_recovery_empty",
    "needs_attention",
  ].includes(ctx.recordingStatus.capture_phase)) {
    return `
      <div class="audio-health warning-only info hosted-recovery-list">
        <div class="audio-health-warning info-action">
          <strong>${otherRecoveries.length} earlier hosted capture${otherRecoveries.length === 1 ? "" : "s"} need recovery.</strong>
          <span>Each retained capture has separate control; opening one does not stop this recording.</span>
          ${otherRecoveries.map(recovery => `<button onclick="window.__selectHostedRecovery('${esc(recovery.recording_id)}')">Open ${esc(recovery.session_name)}</button>`).join("")}
        </div>
      </div>
    `;
  }
  if (ctx.startupState?.phase === "starting") {
    // Startup stays truthful in the header, signal lanes, and fixed footer
    // action. A second full-width status banner shifted the memo canvas when it
    // disappeared at the recording boundary and repeated internal setup copy.
    return "";
  }

  if (ctx.startupState?.phase === "failed") {
    const failure = startupFailurePresentation(ctx.startupState.webMicrophoneFailure);
    return `
      <div class="audio-health warning-only critical startup-failed">
        <div class="audio-health-warning critical-action">
          <strong>${esc(failure?.heading || "Audio not recording.")}</strong>
          <span>${esc(failure?.detail || ctx.startupState.error || ctx.startupState.message || "Your marks remain in this draft. Retry audio or open setup.")}</span>
          ${failure?.retry === false ? "" : `<button onclick="window.__retryRecordingStartup()">Retry audio</button>`}
          <button onclick="window.__openAudioSetup()">Audio setup</button>
        </div>
      </div>
    `;
  }

  if (!ctx.recordingStatus.is_recording) return "";
  if (ctx.recordingStatus.capture_phase === "capturing_elsewhere") {
    return `
      <div class="audio-health warning-only info hosted-capture-remote">
        <div class="audio-health-warning info-action">
          <strong>This recording is capturing in another browser tab.</strong>
          <span>${esc(ctx.recordingStatus.web_capture_warning || "Return to the owner tab to pause or finish it.")}</span>
        </div>
      </div>
    `;
  }
  if (ctx.recordingStatus.capture_phase === "recording_unknown") {
    return `
      <div class="audio-health warning-only info hosted-capture-unknown">
        <div class="audio-health-warning info-action">
          <strong>Capture ownership is unavailable.</strong>
          <span>${esc(ctx.recordingStatus.web_capture_warning || "Use the original recording tab.")}</span>
        </div>
      </div>
    `;
  }
  if (ctx.recordingStatus.capture_phase === "interrupted") {
    return `
      <div class="audio-health warning-only critical hosted-capture-interrupted">
        <div class="audio-health-warning critical-action">
          <strong>The browser capture owner stopped responding.</strong>
          <span>${esc(ctx.recordingStatus.web_capture_warning || "Take control before changing this partial capture.")}</span>
          <button class="primary" onclick="window.__takeOverInterruptedRecording()">Take control</button>
        </div>
      </div>
    `;
  }
  if (ctx.recordingStatus.capture_phase?.startsWith("interrupted_recovery")) {
    const hasAudio = ctx.recordingStatus.capture_phase === "interrupted_recovery";
    return `
      <div class="audio-health warning-only critical hosted-capture-recovery">
        <div class="audio-health-warning critical-action">
          <strong>${hasAudio ? "Recovery control acquired." : "No durable browser audio was received."}</strong>
          <span>${esc(ctx.recordingStatus.web_capture_warning || (hasAudio ? "Finish preserves the received WebM. If finalization fails, retry or explicitly discard it." : "This empty partial capture cannot be finished; discard it to start again."))}</span>
          ${hasAudio ? `<button class="primary" onclick="window.__finishRecording()">Finish and save received audio</button>` : ""}
        </div>
      </div>
    `;
  }
  // A user pause owns the presentation even if the last mic state was Holding.
  // The paused segment has no live capture path to recover until Resume.
  if (ctx.recordingStatus.paused) return "";
  if (ctx.recordingStatus.web_capture_warning) {
    return `
      <div class="audio-health warning-only critical hosted-durable-audio-warning">
        <div class="audio-health-warning critical-action">
          <strong>Durable browser audio needs attention.</strong>
          <span>${esc(ctx.recordingStatus.web_capture_warning)}</span>
        </div>
      </div>
    `;
  }
  if (ctx.recordingStatus.web_live_pcm_warning) {
    return `
      <div class="audio-health warning-only info hosted-live-pcm-warning">
        <div class="audio-health-warning info-action">
          <strong>Live transcript microphone feed needs attention.</strong>
          <span>${esc(ctx.recordingStatus.web_live_pcm_warning)}</span>
        </div>
      </div>
    `;
  }
  const captureDevice = ctx.recordingStatus.capture_device;
  if (captureDevice?.state === "holding") {
    return `
      <div class="audio-health warning-only critical capture-device-holding">
        <div class="audio-health-warning critical-action capture-device-action">
          <div class="audio-health-text">
            <strong>No microphone — recording computer audio only until one connects.</strong>
            <div class="holding-mic-meter-group" aria-label="Microphone level: silent">
              <span>Microphone</span>
              <div class="level-bar holding-mic-meter" aria-hidden="true">${levelBars(0, 12)}</div>
            </div>
          </div>
          <div class="audio-health-actions">
            <button class="primary" onclick="window.__chooseCaptureDevice()">Choose microphone</button>
            <button onclick="window.__pauseRecording()">Pause</button>
          </div>
        </div>
      </div>
    `;
  }
  if (captureDevice?.state === "switching") {
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning info-warning capture-device-switching">
          <span class="inline-spinner" aria-hidden="true"></span>
          <strong>Switching to ${esc(captureDevice.to)}… recording continues.</strong>
        </div>
      </div>
    `;
  }
  const tapStatus = ctx.recordingStatus.tap_status || "ok";
  const sessionName = ctx.recordingStatus.session_name || "";
  const micWarning = microphoneQuietWarning(ctx.recordingStatus.mic_level);
  const micQuiet = micWarning !== null;
  const connectingDismissed = tapStatus === "connecting" && ctx.dismissedTapNoticeSession === sessionName;
  const micDiarized = ctx.recordingStatus.live_transcription_mode === "mic_diarized";
  // Recovery and switch confirmations are transient. Never let one replace an
  // actionable computer-audio state in the shared health slot.
  if (ctx.captureDeviceSwitchToast && shouldShowCaptureDeviceToast(tapStatus, ctx.recordingStatus.tap_warning)) {
    const toast = ctx.captureDeviceSwitchToast;
    const message = toast.kind === "fallback"
      ? `${toast.pinnedName ? esc(toast.pinnedName) : "Your pinned mic"} isn't available — recording on ${esc(toast.deviceName)}, back to it next recording.`
      : toast.kind === "switch"
        ? `Switched to ${esc(toast.deviceName)} to follow your Mac's default.`
        : toast.kind === "recovery"
          ? toast.deviceName
            ? `Microphone briefly stopped — recovered on ${esc(toast.deviceName)}.`
            : "Microphone briefly stopped — capture recovered."
          : toast.kind === "switch_failed"
            ? `Couldn't switch to ${toast.deviceName ? esc(toast.deviceName) : "the microphone"} — still recording on ${toast.previousDevice ? esc(toast.previousDevice) : "the previous microphone"}.`
            : esc(toast.message);
    const hasAction = toast.kind === "switch" || (toast.kind === "switch_failed" && toast.retryDevice);
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning ${toast.kind === "fallback" || toast.kind === "switch_failed" || toast.kind === "preference_error" ? "info-warning" : "success"} capture-device-toast ${hasAction ? "has-action" : ""}">
          <strong>${message}</strong>
          ${toast.kind === "switch" ? `<div class="audio-health-actions">
            <button class="link-button" onclick="window.__usePreviousCaptureDevice()">Use ${esc(toast.previousDevice)} instead</button>
          </div>` : toast.kind === "switch_failed" && toast.retryDevice ? `<div class="audio-health-actions">
            <button class="link-button" onclick="window.__retryCaptureDeviceSwitch()">Try again</button>
          </div>` : ""}
        </div>
      </div>
    `;
  }

  if ((micDiarized || tapStatus === "not_expected") && !micQuiet) {
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning success">
          <strong>Using mic diarization to split speakers from the microphone.</strong>
          <button onclick="window.__openAudioSetup()">Audio setup</button>
        </div>
      </div>
    `;
  }

  if (ctx.tapRecoveryState === "recovering" && ctx.tapRecoverySession === sessionName) {
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning info-warning">
          <strong>Rearming computer audio; mic is still recording.</strong>
        </div>
      </div>
    `;
  }

  if (ctx.tapRecoveryState === "recovered" && ctx.tapRecoverySession === sessionName && tapStatus === "ok") {
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning success">
          <strong>Computer audio recovered; recording continued.</strong>
        </div>
      </div>
    `;
  }

  if (tapStatus === "ok" && !micQuiet && !ctx.recordingStatus.tap_warning) return "";
  if (connectingDismissed && !micQuiet) return "";

  if (tapStatus === "blocked") {
    return `
      <div class="audio-health warning-only info">
        <div class="audio-health-warning info-action">
          <strong>Computer audio is blocked; mic is still recording.</strong>
          <div class="audio-health-actions">
            <button class="primary" onclick="window.__openPrivacyPane('system-audio')">Open System Settings</button>
            <button onclick="window.__useMicDiarization()">Label speakers from mic</button>
            <button class="link-button" onclick="window.__openAudioSetup()">Audio setup</button>
          </div>
        </div>
      </div>
    `;
  }

  if (tapStatus === "connecting" && !connectingDismissed) {
    return `
      <div class="audio-health warning-only">
        <div class="audio-health-warning compact-warning info-warning">
          <div class="audio-health-text">
            <strong>Recording mic; computer audio is connecting.</strong>
          </div>
          <div class="audio-health-actions">
            <button class="primary" onclick="window.__useMicDiarization()">Label speakers from mic</button>
            <button onclick="window.__openAudioSetup()">Audio setup</button>
            <button class="link-button" onclick="window.__dismissTapNotice()">Dismiss</button>
          </div>
        </div>
      </div>
    `;
  }

  const warning = micWarning
    ?? (tapStatus === "silent"
      ? "Computer audio is quiet — mic is still recording."
      : ctx.recordingStatus.tap_warning || "Audio needs attention.");

  return `
    <div class="audio-health warning-only">
      <div class="audio-health-warning compact-warning info-warning">
        <strong>${esc(warning)}</strong>
        <div class="audio-health-actions">
          ${tapStatus === "silent" || tapStatus === "quiet" ? `<button class="primary" onclick="window.__useMicDiarization()">Label speakers from mic</button>` : ""}
          ${tapStatus === "silent" ? `<button onclick="window.__restartSystemAudioCapture()">Restart computer audio</button>` : ""}
          <button class="link-button" onclick="window.__openAudioSetup()">Audio setup</button>
        </div>
      </div>
    </div>
  `;
}

function startupFailurePresentation(category?: WebMicrophoneFailureCategory | null): {
  heading: string;
  detail: string;
  retry?: boolean;
} | null {
  switch (category) {
    case "permission-denied":
      return { heading: "Microphone permission is blocked.", detail: "Open Audio setup for browser-specific steps to allow microphone access.", retry: false };
    case "policy-blocked":
      return { heading: "This site cannot use the microphone.", detail: "A browser or organization policy blocked microphone capture. Open Audio setup to review the connection and site permissions.", retry: false };
    case "no-device":
      return { heading: "No microphone was found.", detail: "Connect or enable a microphone, then retry audio." };
    case "selected-device-unavailable":
      return { heading: "The selected microphone is unavailable.", detail: "Open Audio setup and choose another microphone." };
    case "hardware-unavailable":
      return { heading: "The microphone could not start.", detail: "Another app or a hardware problem may be preventing access. Close other audio apps or choose another microphone, then retry." };
    case "aborted":
      return { heading: "Microphone startup was cancelled.", detail: "Your marks remain in this draft. Retry whenever you're ready." };
    case "unsupported":
      return { heading: "This browser cannot record audio.", detail: "Use a current browser with microphone and MediaRecorder support.", retry: false };
    case "insecure-context":
      return { heading: "Microphone recording needs a secure connection.", detail: "Open Margins over HTTPS (or localhost) and try again.", retry: false };
    default:
      return null;
  }
}

export function renderMemoLine(
  line: MemoLine,
  index: number,
  backchannelButtonHtml?: (index: number) => string,
  blockWallClock?: (ordinal: number) => string | undefined,
): string {
  const isBlock = line.block_ordinal != null;
  const lineClass = isBlock
    ? `memo-line memo-line-block${line.block_ordinal === 0 ? " memo-line-block-prep" : ""}`
    : "memo-line";
  return `<div class="${lineClass}">${renderMemoLineInner(line, index, backchannelButtonHtml, blockWallClock)}</div>`;
}

export function renderMemoLineInner(
  line: MemoLine,
  index: number,
  backchannelButtonHtml?: (index: number) => string,
  blockWallClock?: (ordinal: number) => string | undefined,
): string {
  const isBlock = line.block_ordinal != null;
  let gutterContent: string;
  let gutterClass: string;
  if (isBlock) {
    // Block (clock-stopped) line: pause glyph, no ticking timestamp
    gutterClass = "memo-gutter block-gutter";
    // ordinal > 0 = mid-meeting pause: show "paused · H:MM PM" wall-clock label stored at block entry
    const ordinal = line.block_ordinal ?? 0;
    const wallClock = ordinal > 0 ? blockWallClock?.(ordinal) : undefined;
    const blockLabel = wallClock
      ? `<span class="memo-block-label">paused · ${esc(wallClock)}</span>`
      : "";
    gutterContent = `⏸${blockLabel}`;
  } else {
    const isEdited = line.edited_secs !== null;
    gutterClass = isEdited ? "memo-gutter edited" : "memo-gutter";
    const displayTs = formatElapsed(isEdited ? line.edited_secs! : line.created_secs);
    const preLive = line.audio_pending_at_mark
      ? `<span class="memo-audio-pending" title="Marked before audio capture was confirmed">audio pending</span>`
      : "";
    gutterContent = `${displayTs}${preLive}`;
  }

  return `
    <div class="${gutterClass}">${gutterContent}</div>
    <div class="memo-text">
      <textarea id="memo-input-${index}" rows="1" spellcheck="false" autocomplete="off">${esc(line.text)}</textarea>
    </div>
    ${backchannelButtonHtml ? backchannelButtonHtml(index) : ""}
  `;
}
