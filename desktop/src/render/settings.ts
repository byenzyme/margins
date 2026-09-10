import type {
  AiReadiness,
  AiStatus,
  AudioTestResult,
  CaptureDeviceEvent,
  DeviceInfo,
  ResolutionPreview,
  Settings,
  SystemAudioTestResult,
} from "../lib/tauri";
import { levelBars } from "../lib/format";
import { esc } from "../lib/html";
import type { CueTier } from "../lib/cue-tier";
import { quickAssistsSectionKind, roleSummaryInner } from "../lib/ai-preview";
import { DEFAULT_VAULT_PATH, EDITOR_OPTIONS, activeProject } from "../state/defaults";
import { SIDEBAR_DATE_FORMAT_OPTIONS, normalizeSidebarDateFormat } from "../lib/date-format";
import { isTapDevice } from "../lib/tap-device";
import {
  webMicrophonePolicyBlockedGuidance,
  webMicrophoneRecoveryGuidance,
  type WebMicrophoneEnvironment,
  type WebMicrophoneFailureCategory,
  type WebMicrophonePermissionState,
} from "../lib/web-microphone-permission";

export interface SettingsRenderContext {
  settings: Settings;
  aiStatus: AiStatus;
  devices: DeviceInfo[];
  audioTestRunning: boolean;
  audioTestError: string | null;
  audioTestResult: AudioTestResult | null;
  systemAudioTestRunning: boolean;
  systemAudioTestResult: SystemAudioTestResult | null;
  systemAudioTestError: string | null;
  audioCheckPhase: "microphone" | "system" | null;
  speechModelDownloadInProgress: boolean;
  notesFolderReady: boolean;
  currentAiMode: string;
  aiReady: boolean;
  includedAiReady: boolean;
  aiReadinessText: string;
  /** All-modes readiness from get_ai_readiness; null if unavailable. */
  aiReadiness: AiReadiness | null;
  /** Per-activity resolution preview for the (unsaved) settings; null until
   *  fetched or if the command failed. */
  aiPreview: ResolutionPreview | null;
  /** Selected quick-assists speed tier (may be an ephemeral selection ahead of a
   *  persisted model, e.g. Custom before typing). */
  cueTier: CueTier;
  selectedEditorValue: string;
  selectedDeviceName: string | null;
  liveCaptureActive: boolean;
  captureDeviceState: CaptureDeviceEvent | null;
  audioTestMessage: string;
  audioSetupReady: boolean;
  audioResumeState: "resumed" | "checking" | "confirmed" | "blocked" | null;
  settingsReadyFromState: boolean;
  settingsReadinessMessage: string;
  notesDestinationPath: string;
  activeSection: string;
  /** Hosted web app (Rust HTTP backend). Native audio device/permission/
   *  system-audio controls are hidden; the browser handles mic access. */
  hosted: boolean;
  webMicrophonePermissionState: WebMicrophonePermissionState;
  webMicrophoneFailureCategory: WebMicrophoneFailureCategory | null;
  webMicrophoneEnvironment: WebMicrophoneEnvironment;
  webMicrophoneDeviceId: string | null;
  webMicrophoneRequesting: boolean;
  webMicrophoneError: string | null;
}

export function renderReadinessRow(label: string, detail: string, tone: "ok" | "warn" | "muted"): string {
  return `
    <div class="readiness-row ${tone}">
      <span class="health-dot"></span>
      <div><strong>${esc(label)}</strong><span>${esc(detail)}</span></div>
    </div>
  `;
}

type SettingTier = "required" | "recommended" | "optional";
function requiredBadge(tier: SettingTier = "required"): string {
  const label = tier === "required" ? "Required" : tier === "recommended" ? "Recommended" : "Optional";
  return `<span class="required-badge ${tier}">${label}</span>`;
}

function renderEditorOptions(ctx: SettingsRenderContext): string {
  return EDITOR_OPTIONS.map(option => `
    <option value="${esc(option.value)}" ${ctx.selectedEditorValue === option.value ? "selected" : ""}>${esc(option.label)}</option>
  `).join("");
}

function systemAudioPermissionPassed(ctx: SettingsRenderContext): boolean {
  return ctx.systemAudioTestResult?.status === "ok";
}

function systemAudioInlineMessage(ctx: SettingsRenderContext): string {
  if (ctx.audioCheckPhase === "microphone") return "Waiting for microphone…";
  if (ctx.systemAudioTestRunning) return "Checking…";
  if (ctx.systemAudioTestError) return "Check permission";
  if (ctx.systemAudioTestResult?.status === "ok") return "Ready";
  if (ctx.systemAudioTestResult?.status === "blocked") return "Needs setup";
  if (ctx.systemAudioTestResult?.status === "unsupported") return "Not available";
  if (ctx.systemAudioTestResult?.status === "quiet") return ctx.systemAudioTestResult.message;
  if (ctx.systemAudioTestResult) return "Try again";
  if (ctx.settings.system_audio_ready) return "Previously checked";
  return "Not checked yet";
}

function systemAudioInlineTone(ctx: SettingsRenderContext): string {
  if (ctx.systemAudioTestError) return "err";
  if (ctx.systemAudioTestResult?.status === "blocked") return "info";
  if (ctx.systemAudioTestResult?.status === "unsupported") return "warn";
  if (systemAudioPermissionPassed(ctx)) return "ok";
  if (ctx.systemAudioTestRunning) return "muted";
  if (ctx.systemAudioTestResult) return "warn";
  return "muted";
}

function isWindowsPlatform(): boolean {
  if (typeof navigator === "undefined") return false;
  return navigator.platform.toLowerCase().includes("win");
}

export function renderNotesSettingsSection(ctx: SettingsRenderContext): string {
  const project = activeProject(ctx.settings);
  const destination = ctx.notesDestinationPath;
  return `
    <div class="settings-group required-group ${project.path?.trim() ? "" : "missing"}" id="notes-folder-group">
      <label>Active project ${requiredBadge("required")}</label>
      <input type="hidden" id="project-path" value="${esc(project.path || DEFAULT_VAULT_PATH)}" aria-required="true" />
      <div class="vault-picker-row">
        <div id="project-path-display" class="vault-path-display">${esc(project.path || DEFAULT_VAULT_PATH)}</div>
        <button type="button" onclick="window.__selectVaultPath()">Choose folder...</button>
      </div>
      <div class="note-folder-summary">
        <span>Captures save to</span>
        <strong id="meeting-notes-destination">${esc(destination)}</strong>
      </div>
      <div id="vault-validation"></div>
      <div class="hint">Each project keeps its own captures, destination folder, and background context.</div>
    </div>
  `;
}

function renderNotesFolderSection(ctx: SettingsRenderContext): string {
  const project = activeProject(ctx.settings);
  const destination = ctx.notesDestinationPath;
  return `
    <div class="settings-group">
      <label>Project folder</label>
      <div class="vault-picker-row">
        <div id="project-path-display" class="vault-path-display">${esc(project.path || DEFAULT_VAULT_PATH)}</div>
        <button type="button" onclick="window.__settingsChangeVaultPath()">Change…</button>
      </div>
      <input type="hidden" id="project-path" value="${esc(project.path || DEFAULT_VAULT_PATH)}" />
      <div class="hint">Notes save to ${esc(destination)}.</div>
      <div id="vault-validation"></div>
    </div>
  `;
}

export function renderEditorPrefSection(ctx: SettingsRenderContext): string {
  return `
    <div class="settings-group">
      <label>Preferred editor ${requiredBadge("optional")}</label>
      <select id="editor-command" onchange="window.__updateSettingsSaveState()">
        ${renderEditorOptions(ctx)}
      </select>
      <div class="hint">Choose what should open finished notes. Obsidian is optional; any project folder works.</div>
    </div>
  `;
}

// Whether a mode would resolve successfully right now. Prefers the single
// get_ai_readiness endpoint; falls back to settings-derived checks when it is
// unavailable. Included is treated as always usable (it provisions its key
// lazily before the first note) so setup can complete offline.
function modeReady(ctx: SettingsRenderContext, mode: "included" | "chatgpt" | "api"): boolean {
  if (mode === "included") return true;
  const modes = ctx.aiReadiness?.modes;
  if (modes) {
    if (mode === "chatgpt") return modes.chatgpt.ready;
    return modes.api.ready;
  }
  if (mode === "chatgpt") return ctx.aiStatus.chatgpt_authenticated;
  return Boolean(ctx.settings.api_key?.trim());
}

function renderRoleSummary(ctx: SettingsRenderContext): string {
  // Always emit the container so main.ts can refresh it in place without a full
  // re-render (which would disrupt the input the user is typing into).
  return `<div id="ai-role-summary" class="ai-role-summary" aria-live="polite">${roleSummaryInner(ctx.aiPreview)}</div>`;
}

interface AiModeCard {
  mode: "included" | "chatgpt" | "api";
  title: string;
  desc: string;
  recommended: boolean;
  ready: boolean;
  expandedBody: string;
}

function aiModeCards(ctx: SettingsRenderContext): AiModeCard[] {
  const includedBody = `
    <div class="model-prep-card">
      <div>
        <strong>${ctx.includedAiReady ? "Included note-making is ready" : "Included note-making"}</strong>
        <p>Works without an account. Audio stays on this Mac.</p>
      </div>
    </div>
    ${ctx.includedAiReady ? "" : `<div class="hint">${esc(ctx.aiReadinessText)}</div>`}
  `;
  const chatgptReady = modeReady(ctx, "chatgpt");
  const chatgptMessage = ctx.aiReadiness?.modes.chatgpt.reason ?? ctx.aiStatus.chatgpt_message;
  const chatgptBody = `
    <div class="model-prep-card">
      <div>
        <strong>${chatgptReady ? "Note-making AI is connected" : "Use your ChatGPT subscription"}</strong>
        <p>${esc(chatgptMessage)}</p>
      </div>
      <button onclick="window.__signInChatgpt()">${chatgptReady ? "Reconnect" : "Sign in"}</button>
    </div>
    <input type="text" id="ai-model-chatgpt" list="chatgpt-model-options" value="${esc(ctx.settings.chatgpt_model || "")}" placeholder="Note-writing model (optional, default GPT-5.5)" oninput="window.__updateSettingsSaveState()" />
    <datalist id="chatgpt-model-options">
      <option value="gpt-5.1-codex"></option>
      <option value="gpt-5.5"></option>
    </datalist>
    <div class="hint">Favor the most capable model your plan allows. Leave blank for the default.</div>
  `;
  const apiBody = ctx.hosted ? `
    <div class="model-prep-card">
      <div>
        <strong>${modeReady(ctx, "api") ? "Server API provider is ready" : "Server API provider needs configuration"}</strong>
        <p>Credentials are managed by the Margins server environment and cannot be entered or stored in this browser.</p>
      </div>
    </div>
    <input
      type="text"
      id="ai-base-url"
      value="${esc(ctx.settings.ai_base_url || "")}"
      placeholder="Base URL (optional, default OpenAI)"
      oninput="window.__updateSettingsSaveState()"
    />
    <input type="text" id="ai-model" value="${esc(ctx.settings.ai_model || "")}" placeholder="Note-writing model (optional, default GPT-5.5)" oninput="window.__updateSettingsSaveState()" />
    <div class="hint">Ask the deployment operator to configure the provider credential on the server.</div>
  ` : `
    <input
      type="text"
      id="ai-base-url"
      value="${esc(ctx.settings.ai_base_url || "")}"
      placeholder="Base URL (optional, default OpenAI)"
      oninput="window.__updateSettingsSaveState()"
    />
    <input
      type="password"
      id="api-key"
      value="${esc(ctx.settings.api_key || "")}"
      placeholder="API key - required"
      aria-required="true"
      oninput="window.__updateSettingsSaveState()"
    />
    <input type="text" id="ai-model" value="${esc(ctx.settings.ai_model || "")}" placeholder="Note-writing model (optional, default GPT-5.5)" oninput="window.__updateSettingsSaveState()" />
    <div class="hint">Your key stays on this Mac.</div>
  `;
  return [
    {
      mode: "included",
      title: "Included",
      desc: "Works out of the box — no account or API key needed.",
      recommended: true,
      ready: true,
      expandedBody: includedBody,
    },
    {
      mode: "chatgpt",
      title: "ChatGPT subscription",
      desc: "Sign in with your ChatGPT Plus or Pro subscription.",
      recommended: false,
      ready: modeReady(ctx, "chatgpt"),
      expandedBody: chatgptBody,
    },
    {
      mode: "api",
      title: "Your own API key",
      desc: "Bring a key from OpenAI or any compatible provider.",
      recommended: false,
      ready: modeReady(ctx, "api"),
      expandedBody: apiBody,
    },
  ];
}

function renderAiOptionCard(ctx: SettingsRenderContext, card: AiModeCard): string {
  const selected = ctx.currentAiMode === card.mode;
  // Status chip: always show "Ready" when the mode is set up. When not ready,
  // only warn on the selected card so unchosen paths stay quiet.
  const statusChip = card.ready
    ? `<span class="ai-option-status ok">Ready</span>`
    : selected
      ? `<span class="ai-option-status warn">Set up needed</span>`
      : "";
  return `
    <label class="ai-option-card ${selected ? "selected" : ""}">
      <input type="radio" name="ai-mode" value="${card.mode}" ${selected ? "checked" : ""} onchange="window.__setAiMode('${card.mode}')" />
      <div class="ai-option-card-body">
        <div class="ai-option-card-head">
          <strong>${esc(card.title)}</strong>
          ${card.recommended ? `<span class="recommended-chip">Recommended</span>` : ""}
          ${statusChip}
        </div>
        <p class="ai-option-desc">${esc(card.desc)}</p>
        ${selected ? `<div class="ai-option-card-expanded">${card.expandedBody}${renderRoleSummary(ctx)}</div>` : ""}
      </div>
    </label>
  `;
}

export function renderAiSettingsSection(ctx: SettingsRenderContext): string {
  return `
    <div class="settings-group required-group ${ctx.aiReady ? "" : "missing"}" id="ai-notes-group">
      <label>Note-making AI ${requiredBadge("required")}</label>
      <div class="ai-option-cards" role="radiogroup" aria-label="Note-making AI">
        ${aiModeCards(ctx).map(card => renderAiOptionCard(ctx, card)).join("")}
      </div>
    </div>
  `;
}

function separateCueKeyNotice(): string {
  return `<div class="hint">A separate quick-assists provider is configured in settings — managed outside this screen.</div>`;
}

function cueTierRadio(tier: string, current: string, label: string): string {
  return `
    <label class="cue-tier-option ${tier === current ? "selected" : ""}">
      <input type="radio" name="cue-tier" value="${tier}" ${tier === current ? "checked" : ""} onchange="window.__setCueTier('${tier}')" />
      <span>${esc(label)}</span>
    </label>
  `;
}

export function renderBackchannelSettingsSection(ctx: SettingsRenderContext): string {
  const kind = quickAssistsSectionKind(ctx.currentAiMode, ctx.settings);
  const separateKeyConfigured = Boolean(ctx.settings.backchannel_api_key?.trim());

  // ChatGPT cues always follow the subscription model — a speed tier is not
  // meaningful, so the control is hidden entirely (less code than a disabled
  // state, and there is nothing actionable behind it).
  if (kind === "chatgpt") {
    return `
      <div class="settings-group" id="backchannel-ai-group">
        <label>Quick assists</label>
        <p class="backchannel-auto-line">Live cues and note re-runs use your ChatGPT subscription model.</p>
        ${separateKeyConfigured ? separateCueKeyNotice() : ""}
      </div>
    `;
  }

  // An externally-configured separate cue key is managed outside this screen; we
  // surface a notice rather than a tier control so we never clobber it.
  if (kind === "external-key") {
    return `
      <div class="settings-group" id="backchannel-ai-group">
        <label>Quick assists</label>
        <p class="backchannel-auto-line">Live cues and note re-runs use a separate, faster model.</p>
        ${separateCueKeyNotice()}
      </div>
    `;
  }

  const tier = ctx.cueTier;
  const customModel = tier === "custom" ? (ctx.settings.backchannel_model || "") : "";
  return `
    <div class="settings-group" id="backchannel-ai-group">
      <label>Quick assists</label>
      <p class="backchannel-auto-line">Choose how fast live cues and note re-runs should respond.</p>
      <div class="cue-tier-options" role="radiogroup" aria-label="Quick-assists speed">
        ${cueTierRadio("balanced", tier, "Balanced")}
        ${cueTierRadio("fastest", tier, "Fastest")}
        ${cueTierRadio("custom", tier, "Custom model…")}
      </div>
      ${tier === "custom" ? `
        <input
          type="text"
          id="backchannel-model"
          value="${esc(customModel)}"
          placeholder="Model for quick assists (e.g. google/gemini-3.1-flash-lite)"
          oninput="window.__updateSettingsSaveState()"
        />
        <div class="hint">Runs live cues and note re-runs. Uses your note-making key.</div>
      ` : `
        <div class="hint">${tier === "fastest"
          ? "Uses a small, fast model for the snappiest cues."
          : "Cues use the same model that writes your notes."}</div>
      `}
    </div>
  `;
}

export function renderSpeechSettingsSection(ctx: SettingsRenderContext): string {
  const ready = Boolean(ctx.settings.parakeet_model_dir);
  const downloading = ctx.speechModelDownloadInProgress;
  return `
    <div class="settings-group" id="speech-model-group">
      <label>Local transcription ${requiredBadge("required")}</label>
      <div class="model-prep-card">
        <div>
          <strong>${ready ? "Local transcription is ready" : "Local transcription isn't installed yet"}</strong>
          <p>${ready
            ? "Transcribes your captures on this Mac. Nothing leaves the device."
            : "A one-time download lets Margins transcribe captures on this Mac, privately."}</p>
        </div>
        <div class="model-action-buttons">
          <button id="speech-model-download-btn" class="${ready ? "" : "primary"}" onclick="window.__prepareSpeechModels()" ${downloading ? "disabled" : ""}>${ready ? "Check again" : "Download"}</button>
          <button id="speech-model-cancel-btn" class="danger subtle ${downloading ? "" : "hidden"}" onclick="window.__cancelSpeechModelDownload()">Cancel</button>
          ${ready ? `<button id="speech-model-clear-btn" class="subtle" onclick="window.__clearSpeechModels()">Remove</button>` : ""}
        </div>
      </div>
      <div id="speech-model-status" class="model-status ${downloading ? "" : ready ? "ok" : "muted"}">${downloading ? "Download in progress…" : ready ? "Ready to transcribe on this Mac." : "Not installed yet."}</div>
      <details class="advanced-setting">
        <summary>Advanced</summary>
        <input
          type="text"
          id="parakeet-model-dir"
          value="${esc(ctx.settings.parakeet_model_dir || "")}"
          placeholder="Custom model folder — leave blank for automatic setup"
          oninput="window.__updateSettingsSaveState()"
        />
        <div class="hint">Point to an existing local transcription model folder, or leave blank to use the standard location.</div>
      </details>
    </div>
  `;
}

// Post-restart confirmation card: shown after the user enables Screen & System
// Audio Recording and relaunches, so the loop closes without another manual Test.
function renderAudioResumeCard(ctx: SettingsRenderContext): string {
  if (ctx.audioResumeState === "resumed") {
    // Landed here right after the permission restart. We intentionally have NOT
    // probed the tap yet (probing touches ScreenCapture TCC and launch stays
    // TCC-clean); Confirm runs the check on this explicit press.
    return `
      <div class="audio-resume-card resumed">
        <div>
          <strong>Enabled computer audio? Confirm it's working</strong>
          <p>We'll do a quick computer-audio check when you're ready.</p>
        </div>
        <button type="button" class="primary" onclick="window.__confirmSystemAudioAfterRestart()">Confirm computer audio</button>
      </div>
    `;
  }
  if (ctx.audioResumeState === "checking") {
    return `
      <div class="audio-resume-card checking">
        <span class="inline-spinner" aria-hidden="true"></span>
        <span>Checking computer audio…</span>
      </div>
    `;
  }
  if (ctx.audioResumeState === "confirmed") {
    return `
      <div class="audio-resume-card confirmed">
        <div>
          <strong>Computer audio is on — you're all set</strong>
          <p>Microphone and computer audio are ready.</p>
        </div>
        <button type="button" class="primary" onclick="window.__startNew()">Start capture</button>
      </div>
    `;
  }
  return "";
}

function renderImportSpeakerSeparation(ctx: SettingsRenderContext): string {
  const count = Math.max(1, Math.min(4, ctx.settings.import_speaker_count ?? 1));
  const label = count >= 4 ? "4+ speakers" : count === 1 ? "Single voice" : `${count} speakers`;
  return `
      <label>Speakers for imports</label>
      <input type="hidden" id="import-speaker-count" value="${count}" />
      <div class="capture-speaker-control" aria-label="Speaker separation for imports">
        <button type="button" class="capture-speaker-step" ${count <= 1 ? "disabled" : ""} aria-label="Fewer speakers" onclick="window.__adjustImportSpeakerCount(-1)">−</button>
        <span class="capture-speaker-value">${esc(label)}</span>
        <button type="button" class="capture-speaker-step" ${count >= 4 ? "disabled" : ""} aria-label="More speakers" onclick="window.__adjustImportSpeakerCount(1)">+</button>
      </div>
      <div class="hint">Imported audio is treated as a single voice by default. Increase this to separate speakers; 4+ detects up to 8.</div>`;
}

export function renderAudioSettingsSection(ctx: SettingsRenderContext): string {
  if (ctx.hosted) {
    const state = ctx.webMicrophonePermissionState;
    const selectedDeviceMissing = Boolean(
      ctx.webMicrophoneDeviceId
      && !ctx.devices.some(device => device.uid === ctx.webMicrophoneDeviceId),
    );
    const deviceControls = state === "granted" ? `
      <div class="settings-row">
        <select id="input-device-name" onchange="window.__onInputDeviceChange()">
          <option value="" ${ctx.webMicrophoneDeviceId ? "" : "selected"}>Browser default microphone</option>
          ${selectedDeviceMissing ? `<option value="${esc(ctx.webMicrophoneDeviceId!)}" selected>Previously selected microphone (unavailable)</option>` : ""}
          ${ctx.devices.map(device => `<option value="${esc(device.uid)}" ${ctx.webMicrophoneDeviceId === device.uid ? "selected" : ""}>${esc(device.name)}</option>`).join("")}
        </select>
        <button type="button" onclick="window.__refreshDevices()" ${ctx.webMicrophoneRequesting ? "disabled" : ""}>Refresh</button>
      </div>
      <div class="hint" id="input-device-policy-hint">${ctx.webMicrophoneDeviceId ? "Margins will request this exact browser microphone for new captures." : "Margins follows the browser's default microphone."}</div>
    ` : "";
    const status = ctx.webMicrophoneFailureCategory === "policy-blocked"
      ? `<div class="restart-callout">
          <strong>Microphone access is blocked by browser policy</strong>
          <p>${esc(webMicrophonePolicyBlockedGuidance(ctx.webMicrophoneEnvironment))}</p>
          <div class="restart-callout-actions"><button type="button" onclick="window.__refreshWebMicrophonePermission()">Check again</button></div>
        </div>`
      : state === "denied"
      ? `<div class="restart-callout">
          <strong>Microphone is blocked for this site</strong>
          <p>${esc(webMicrophoneRecoveryGuidance(ctx.webMicrophoneEnvironment))}</p>
          <div class="restart-callout-actions"><button type="button" onclick="window.__refreshWebMicrophonePermission()">Check again</button></div>
        </div>`
      : state === "unsupported"
        ? `<div class="restart-callout"><strong>Microphone capture is unavailable</strong><p>Use Margins over HTTPS in a browser that supports microphone capture and MediaRecorder.</p></div>`
        : state === "prompt"
          ? `<div class="audio-test-card">
              <strong>Allow microphone access</strong>
              <p>Your browser will ask before Margins starts listening.</p>
              <div class="model-action-buttons">
                <button type="button" class="primary" onclick="window.__requestWebMicrophonePermission()" ${ctx.webMicrophoneRequesting ? "disabled" : ""}>${ctx.webMicrophoneRequesting ? "Waiting for permission…" : "Allow microphone"}</button>
                <button type="button" onclick="window.__testAudioInput()" ${ctx.webMicrophoneRequesting ? "disabled" : ""}>Test microphone</button>
              </div>
            </div>`
          : state === "granted"
            ? `${deviceControls}
              <div class="audio-test-card">
                <div class="audio-test-row">
                  <button type="button" class="primary" onclick="window.__testAudioInput()" ${ctx.audioTestRunning ? "disabled" : ""}>${ctx.audioTestRunning ? "Listening…" : "Test microphone"}</button>
                  <div class="level-bar audio-test-meter" aria-label="Microphone test level">${levelBars(ctx.audioTestResult?.peak || 0, 12)}</div>
                </div>
                <div class="audio-test-state ${ctx.audioTestError ? "err" : ctx.audioTestResult?.ok ? "ok" : "muted"}"><span>Microphone: ${esc(ctx.audioTestError || (ctx.audioTestResult?.ok ? "Ready" : "Permission allowed"))}</span></div>
              </div>`
            : `<div class="audio-test-card">
                <strong>Check microphone access</strong>
                <p>This browser does not expose microphone permission state. A capability test is the reliable check, especially on iOS.</p>
                <div class="model-action-buttons">
                  <button type="button" class="primary" onclick="window.__requestWebMicrophonePermission()" ${ctx.webMicrophoneRequesting ? "disabled" : ""}>${ctx.webMicrophoneRequesting ? "Waiting for permission…" : "Test microphone"}</button>
                  <button type="button" onclick="window.__refreshWebMicrophonePermission()">Retry status check</button>
                </div>
              </div>`;
    return `
    <div class="settings-group" id="audio-permissions-group">
      <label>Audio ${requiredBadge("required")}</label>
      <p class="hint">Margins records your microphone in this browser. Computer (system) audio capture is available in the desktop app only.</p>
      ${status}
      ${ctx.webMicrophoneError ? `<div class="hint input-device-fallback-truth">${esc(ctx.webMicrophoneError)}</div>` : ""}
      ${renderImportSpeakerSeparation(ctx)}
    </div>
  `;
  }

  const windows = isWindowsPlatform();
  const restartCopy = windows
    ? "Check Windows microphone access and default sound output, then restart Margins."
    : "Turn on Margins under System Audio Recording Only, then restart.";
  const blocked = Boolean(ctx.systemAudioTestResult?.restart_recommended) || ctx.audioResumeState === "blocked";
  const blockedHeadline = ctx.audioResumeState === "blocked"
    ? `Still not detected; ${restartCopy}`
    : `Computer audio needs permission; ${restartCopy}`;
  const micTone = ctx.audioTestError
    ? "err"
    : ctx.audioTestResult?.ok || (ctx.settings.audio_input_ready && !ctx.audioTestResult)
      ? "ok"
      : "muted";
  const checking = ctx.audioTestRunning || ctx.systemAudioTestRunning || ctx.audioResumeState === "checking";
  const inputDeviceMode = ctx.settings.input_device_mode
    ?? (ctx.settings.input_device_uid ? "pinned" : "follow_default");
  // Exclude the app's own virtual tap device and any aggregate loopback the
  // app creates. These are capture-only infrastructure and must never appear
  // as selectable microphone inputs. This is defense-in-depth: the Rust layer
  // already filters them from the enumerated list.
  const pickerDevices = ctx.devices.filter(d => !isTapDevice(d.name));
  const defaultName = pickerDevices.find(device => device.is_default)?.name
    ?? ctx.devices.find(device => device.is_default)?.name ?? null;
  const pinnedDevice = ctx.settings.input_device_uid
    ? pickerDevices.find(device => device.uid === ctx.settings.input_device_uid)
    : null;
  const pinnedName = pinnedDevice?.name || ctx.settings.input_device_name || null;
  const activeDeviceName = ctx.captureDeviceState?.state === "active"
    ? ctx.captureDeviceState.device_name
    : ctx.captureDeviceState?.state === "fallback"
      ? ctx.captureDeviceState.device_name
      : ctx.captureDeviceState?.state === "switching"
        ? ctx.captureDeviceState.from
        : null;
  const pinnedFallback = Boolean(
    ctx.liveCaptureActive
    && inputDeviceMode === "pinned"
    && activeDeviceName
    && (!pinnedName || activeDeviceName !== pinnedName),
  );
  const policyHint = inputDeviceMode === "follow_default"
    ? "Records from whatever mic your Mac uses, and follows when you change it."
    : `Always records from ${pinnedName || "your pinned mic"}. If it's unplugged, Margins uses your Mac's default and switches back next recording.`;
  const missingPinnedOption = inputDeviceMode === "pinned"
    && ctx.settings.input_device_uid
    && !pinnedDevice
    ? `<option value="${esc(ctx.settings.input_device_uid)}" selected>${esc(pinnedName || "Your pinned mic")}</option>`
    : "";
  return `
    <div class="settings-group" id="audio-permissions-group">
      <label>Audio ${requiredBadge("required")}</label>
      ${renderAudioResumeCard(ctx)}
      <p class="hint">Your microphone captures your voice; computer audio is captured automatically. Run one check to confirm both.</p>
      <div class="settings-row">
        <select id="input-device-name" onchange="window.__onInputDeviceChange()">
          <option value="" ${inputDeviceMode === "follow_default" ? "selected" : ""}>${defaultName ? `Follow system default (now: ${esc(defaultName)})` : "Follow system default"}</option>
          ${missingPinnedOption}
          ${pickerDevices.map(d => `<option value="${esc(d.uid)}" ${inputDeviceMode === "pinned" && ctx.settings.input_device_uid === d.uid ? "selected" : ""}>${esc(d.name)}</option>`).join("")}
        </select>
        <button onclick="window.__refreshDevices()">Refresh</button>
      </div>
      <div class="hint" id="input-device-policy-hint">${esc(policyHint)}</div>
      ${ctx.liveCaptureActive ? `<div class="hint">Switches the active recording right away, and is used for new captures.</div>` : ""}
      ${pinnedFallback ? `<div class="hint input-device-fallback-truth">${pinnedName ? esc(pinnedName) : "Your pinned mic"} isn't connected — recording on ${esc(activeDeviceName!)} now.</div>` : ""}
      <div class="audio-test-card">
        <div class="audio-test-row">
          <button type="button" class="primary" onclick="window.__runAudioCheck()" ${checking ? "disabled" : ""}>Test audio</button>
          ${ctx.audioCheckPhase ? `<span class="inline-spinner" aria-label="Testing audio"></span>` : ""}
          <div class="level-bar audio-test-meter" aria-label="Microphone test level">${levelBars(ctx.audioTestResult?.peak || 0, 12)}</div>
        </div>
        <div class="audio-test-state ${micTone}" id="microphone-test-state">
          ${ctx.audioTestRunning && !ctx.audioCheckPhase ? `<span class="inline-spinner" aria-hidden="true"></span>` : ""}
          <span>Microphone: ${esc(ctx.audioTestMessage)}</span>
        </div>
        <div class="audio-test-state ${systemAudioInlineTone(ctx)}" id="system-audio-test-state">
          ${ctx.systemAudioTestRunning && !ctx.audioCheckPhase ? `<span class="inline-spinner" aria-hidden="true"></span>` : ""}
          <span>Computer audio: ${esc(systemAudioInlineMessage(ctx))}</span>
        </div>
      </div>
      ${blocked ? `
        <div class="restart-callout">
          <strong>${esc(blockedHeadline)}</strong>
          <div class="restart-callout-actions">
            <button type="button" onclick="window.__openPrivacyPane('system-audio')">Open System Settings</button>
            <button type="button" class="primary" onclick="window.__restartMarginsToAudioSetup()">Restart and finish</button>
          </div>
        </div>
      ` : ""}
      ${renderImportSpeakerSeparation(ctx)}
    </div>
  `;
}

function renderCalendarSettingsSection(ctx: SettingsRenderContext): string {
  return `
    <div class="settings-group" id="calendar-settings-group">
      <label>Calendar ${requiredBadge("optional")}</label>
      <label class="checkbox-row">
        <input type="checkbox" id="auto-start-from-calendar" ${ctx.settings.auto_start_from_calendar !== false ? "checked" : ""} onchange="window.__updateSettingsSaveState()" />
        Auto-start captures at event time
      </label>
      <div class="model-prep-card">
        <div>
          <strong>Workspace-managed Calendar</strong>
          <p>Calendar suggestions come from the active workspace's refreshed Google Calendar binding. Connection and collection settings are managed through Margins setup.</p>
        </div>
      </div>
    </div>
  `;
}

function renderNoteFormatSettingsSection(ctx: SettingsRenderContext): string {
  return `
    <div class="settings-group" id="note-format-group">
      <label>How notes are written</label>
      <div class="hint">Guidance Margins follows when turning your marks and transcript into a note.</div>
      <textarea id="distill-instructions" rows="7" oninput="window.__updateSettingsSaveState()">${esc(ctx.settings.distill_instructions || "")}</textarea>
      <label>Sidebar date format</label>
      <select id="sidebar-date-format" onchange="window.__updateSettingsSaveState()">
        ${(() => {
          const selected = normalizeSidebarDateFormat(ctx.settings.sidebar_date_format);
          return SIDEBAR_DATE_FORMAT_OPTIONS.map(o =>
            `<option value="${o.value}" ${o.value === selected ? "selected" : ""}>${esc(o.label)} — ${esc(o.example)}</option>`,
          ).join("");
        })()}
      </select>
      <div class="hint">How each capture's date and time reads in the sidebar. Words you add to a title show in a brighter color alongside it.</div>
      <details class="advanced-setting">
        <summary>Customize templates</summary>
        <label>People folder</label>
        <input type="text" id="people-folder" value="${esc(ctx.settings.people_folder || "people")}" oninput="window.__updateSettingsSaveState()" />
        <div class="hint">Where imported contacts are filed. Used when importing from Granola; a default is chosen automatically.</div>
        <label>Created date format</label>
        <input type="text" id="created-date-format" value="${esc(ctx.settings.created_date_format || "[[%Y-%m-%d]]")}" oninput="window.__updateSettingsSaveState()" />
        <label>Note filename template</label>
        <input type="text" id="note-filename-template" value="${esc(ctx.settings.note_filename_template || "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}")}" oninput="window.__updateSettingsSaveState()" />
        <label>People note template</label>
        <textarea id="person-note-template" rows="3" oninput="window.__updateSettingsSaveState()">${esc(ctx.settings.person_note_template || "# {{name}}\n")}</textarea>
        <div class="hint">Template tokens: {{date:...}}, {{event_title}}, {{name}}. Finished and people notes stay inside the active project.</div>
      </details>
    </div>
  `;
}

function renderAdvancedSettingsSection(ctx: SettingsRenderContext): string {
  return `
    <div class="settings-group secondary-setting">
      <label>Audio retention</label>
      <div class="radio-group">
        <label><input type="radio" name="cleanup" value="immediate" ${ctx.settings.cleanup_policy === "immediate" ? "checked" : ""} onchange="window.__updateSettingsSaveState()" /> Delete audio after the note is made</label>
        <label><input type="radio" name="cleanup" value="7days" ${ctx.settings.cleanup_policy === "7days" ? "checked" : ""} onchange="window.__updateSettingsSaveState()" /> Keep for 7 days</label>
        <label><input type="radio" name="cleanup" value="keep" ${ctx.settings.cleanup_policy === "keep" ? "checked" : ""} onchange="window.__updateSettingsSaveState()" /> Keep forever</label>
      </div>
    </div>
  `;
}

interface SettingsSection {
  id: string;
  group: "setup" | "prefs";
  label: string;
  subtitle: string;
  // Setup sections carry a readiness status that surfaces as a nav dot; prefs
  // sections never block capture, so they omit it.
  status?: "ok" | "warn";
  body: string;
}

// Single source of truth for the settings page: the nav rail, readiness dots,
// progress counter, and content panes all derive from this one list, so they
// can never drift between hand-maintained copies.
function settingsSections(ctx: SettingsRenderContext): SettingsSection[] {
  const audioReady = ctx.audioSetupReady && Boolean(ctx.settings.parakeet_model_dir);
  return [
    {
      id: "ai",
      group: "setup",
      label: "Note-making",
      subtitle: "Turns your marks and transcript into a note",
      status: ctx.aiReady ? "ok" : "warn",
      body: `${renderAiSettingsSection(ctx)}${renderBackchannelSettingsSection(ctx)}`,
    },
    {
      id: "audio",
      group: "setup",
      label: "Audio",
      subtitle: "Microphone, computer audio, and local transcription",
      status: audioReady ? "ok" : "warn",
      body: `${renderAudioSettingsSection(ctx)}${renderSpeechSettingsSection(ctx)}`,
    },
    {
      id: "notes",
      group: "prefs",
      label: "Notes",
      subtitle: "How finished notes open and read",
      body: `${renderNotesFolderSection(ctx)}${renderEditorPrefSection(ctx)}${renderNoteFormatSettingsSection(ctx)}`,
    },
    {
      id: "calendar",
      group: "prefs",
      label: "Calendar",
      subtitle: "Suggest capture titles and people",
      body: renderCalendarSettingsSection(ctx),
    },
    {
      id: "advanced",
      group: "prefs",
      label: "Advanced",
      subtitle: "How long recordings are kept",
      body: renderAdvancedSettingsSection(ctx),
    },
  ];
}

function renderSettingsNavItem(section: SettingsSection, active: boolean): string {
  return `
    <button type="button" class="settings-nav-item ${active ? "active" : ""}" data-section="${section.id}" onclick="window.__settingsNav('${section.id}')">
      <span>${esc(section.label)}</span>
      ${section.status ? `<span class="status-dot ${section.status}" aria-hidden="true"></span>` : ""}
    </button>`;
}

export function renderSettingsOverlay(ctx: SettingsRenderContext): string {
  const sections = settingsSections(ctx);
  const active = sections.find(s => s.id === ctx.activeSection) ?? sections[0];
  const setup = sections.filter(s => s.group === "setup");
  const prefs = sections.filter(s => s.group === "prefs");
  const readyCount = setup.filter(s => s.status === "ok").length;
  const allReady = readyCount === setup.length;

  return `
    <div class="settings-overlay settings-page-overlay" id="settings-dialog" role="dialog" aria-modal="true" aria-label="Settings" tabindex="-1">
      <div class="window-drag-region" data-tauri-drag-region aria-hidden="true"></div>
      <div class="settings-page">
        <aside class="settings-nav">
          <div class="settings-nav-title">Settings</div>
          <div class="settings-nav-group">
            <div class="settings-nav-group-label">
              <span>Setup</span>
              <small class="${allReady ? "ok" : "warn"}">${allReady ? "Ready" : `${readyCount}/${setup.length}`}</small>
            </div>
            ${setup.map(s => renderSettingsNavItem(s, s.id === active.id)).join("")}
          </div>
          <div class="settings-nav-group">
            <div class="settings-nav-group-label"><span>Preferences</span></div>
            ${prefs.map(s => renderSettingsNavItem(s, s.id === active.id)).join("")}
          </div>
        </aside>
        <div class="settings-content">
          <header class="settings-content-head">
            <div>
              <h2 id="settings-pane-title">${esc(active.label)}</h2>
              <p id="settings-pane-subtitle">${esc(active.subtitle)}</p>
            </div>
            <button type="button" class="icon-button settings-close" aria-label="Close settings" onclick="window.__closeSettings()">✕</button>
          </header>
          <div class="settings-panes">
            ${sections.map(s => `
              <section class="settings-pane ${s.id === active.id ? "active" : ""}" data-pane="${s.id}" data-title="${esc(s.label)}" data-subtitle="${esc(s.subtitle)}">
                ${s.body}
              </section>`).join("")}
          </div>
          <div class="settings-save-row">
            <button id="settings-save-btn" class="primary" ${ctx.settingsReadyFromState ? "" : "disabled"} onclick="window.__saveSettings()">Save</button>
            <span id="settings-save-hint" class="settings-save-hint">${esc(ctx.settingsReadinessMessage)}</span>
          </div>
        </div>
      </div>
    </div>
  `;
}
