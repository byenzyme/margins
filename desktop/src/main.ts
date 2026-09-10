import MarkdownIt from "markdown-it";
import { Menu, MessageCircle, Moon, PanelLeft, Plus, Settings as SettingsIcon, Sun, Trash2 } from "lucide";
import { speakerCountToMax } from "./lib/speaker-count";
import { settingsSaveErrorMessage } from "./lib/settings-save-error";
import {
  getSettings,
  updateSettings,
  updateAudioSettings,
  registerProject,
  updateProjectReadiness,
  getProjectFilesFingerprint,
  getAiStatus,
  getIncludedAiStatus,
  getAiReadiness,
  previewAiResolution,
  prepareIncludedAi,
  signInChatgpt,
  validateVault,
  countVaultNotes,
  indexVault,
  selectVaultFolder,
  selectProjectSubfolder,
  getCalendarEventSuggestion,
  updateSessionPeople,
  updateSessionTitle,
  prepareSpeechModels,
  probeSpeechModels,
  cancelSpeechModelDownload,
  clearSpeechModels,
  listDevices,
  refreshDevices,
  testAudioInput,
  isHostedWeb,
  listSessions,
  deleteSession,
  reconcileProjectNotes,
  startRecording,
  cancelRecordingStartup,
  stopRecording,
  consumeHostedFinalizationWarning,
  discardRecording,
  switchRecordingDevice,
  pauseRecording,
  resumeRecording,
  restartSystemAudioCapture,
  setLiveTranscriptionMode,
  getRecordingStatus,
  getHostedRecordingStatus,
  negotiateHostedCaptureProtocol,
  listHostedRecordingRecoveries,
  syncMemo,
  hydrateRecordingMemo,
  claimInterruptedRecording,
  checkpointMemoLine,
  requestBackchannelForMemo,
  steerBackchannelForMemo,
  hydratePrepSketch,
  steerPrepHydration,
  onPrepHydration,
  processSession,
  retrySession,
  cancelProcessSession,
  clearSessionNoteError,
  onRecallIndexing,
  onSettingsChanged,
  refineSession,
  reprocessSessionWithPeople,
  DISTILL_CANCELLED_SENTINEL,
  getAlignedContent,
  getSessionMemo,
  getVaultNote,
  getSessionGrounding,
  getDistillTrace,
  openNote,
  openNoteInObsidian,
  openNoteTargetInObsidian,
  onProcessingProgress,
  onSpeechModelProgress,
  onBackchannelSuggestion,
  onBackchannelError,
  onCaptureDeviceChanged,
  onLiveTranscriptReady,
  onLiveTranscriptDegraded,
  onDevicesChanged,
  onFileDrop,
  importAudioFile,
  surveyGranolaImport,
  importGranolaFiles,
  getGranolaImportStatus,
  authorizeGranolaImport,
  importGranolaMcp,
  onGranolaImportProgress,
  installCliTool,
  ensureCliTools,
  installAvailableAppUpdate,
  IMPORT_AUDIO_EXTENSIONS,
  GRANOLA_IMPORT_EXTENSIONS,
  pickAudioFiles,
  openPrivacyPane,
  restartApp,
  testSystemAudioTap,
  type FileDropPayload,
  type Settings,
  type ProjectSource,
  type AiStatus,
  type IncludedAiStatus,
  type AiReadiness,
  type ResolutionPreview,
  type CalendarEventSuggestion,
  type CalendarSuggestionResult,
  type EvidenceFreshness,
  type SessionInfo,
  type DeviceInfo,
  type AudioTestResult,
  type SystemAudioTestResult,
  type MemoLine,
  type PrepHydrationEvent,
  type PrepHydrationMarginalia,
  type BackchannelSuggestionEvent,
  type ProcessingEvent,
  type TranscriptEntry,
  type RecordingStatus,
  type WebRecordingRecoveryStatus,
  type CaptureDeviceState,
  type CaptureDeviceEvent,
  type SpeechModelProgressEvent,
  type GranolaImportStatus,
  type InstallCliResult,
} from "./lib/tauri";
import { formatDuration, formatElapsed } from "./lib/format";
import { startRollingWaveform, type RollingWaveformHandle } from "./lib/rolling-waveform";
import { webMicrophoneRms } from "./lib/http-backend";
import { HostedRecoveryBrowserState } from "./lib/hosted-recovery-browser-state";
import {
  executeHostedRecoveryMutation,
  hostedMemoSyncAllowed,
  HostedRecoveryWorkspaceStore,
} from "./lib/hosted-recovery-workspace";
import { FASTEST_CUE_MODEL, backchannelFieldsForTier, cueTierFromSettings, type CueBackchannelFields, type CueTier } from "./lib/cue-tier";
import { systemAudioReadinessAfterRecordingStatus } from "./lib/audio-readiness";
import {
  getPreferredWebMicrophoneDeviceId,
  isWebMicrophoneFailure,
  setPreferredWebMicrophoneDeviceId,
  subscribeWebMicrophoneDevices,
  webMicrophoneEnvironment,
  webMicrophonePermission,
  type WebMicrophoneFailureCategory,
  type WebMicrophonePermissionState,
} from "./lib/web-microphone-permission";
import { esc, iconSvg, js } from "./lib/html";
import {
  applyTheme,
  loadSidebarWidth,
  loadThemeMode,
  type ThemeMode,
} from "./lib/preferences";
import {
  DEFAULT_VAULT_PATH,
  SETUP_RESUME_SECTION_KEY,
  activeProject,
  defaultSettings,
  normalizeProjects,
  normalizeSettings,
  projectIdFromPath,
  projectNameFromPath,
  resolveEditorCommand,
  stageProjectPathChange,
} from "./state/defaults";
import { polishRenderedMarkdown, prepareMarkdownForRender } from "./lib/markdown";
import {
  appendProcessingTrace,
  defaultDistillTrace,
  traceStatusText,
  visiblePiTrace,
  type DistillTraceEvent,
} from "./lib/distill-trace";
import {
  deriveSessionJobState,
  newSessionLifecycleState,
  reduceSessionLifecycle,
  sessionLifecycleFromRecording,
  type SessionLifecycleState,
} from "./lib/lifecycle-state";
import {
  cleanVisibleMarkdown,
  consumeNoteStreamChunkInState,
  groundedStateFromMarkdown,
  mergeGroundingUsesIntoState,
  mergeVaultRefsIntoState,
  newGroundedNoteState,
  stripMarginsMarkers,
  type MarginsGroundingUse,
  type GroundedNoteState,
} from "./lib/grounding";
import {
  captureCta,
  defaultCollapsedSidebarKeys,
  emptySessionFilters,
  filterSessionsForSidebar,
  isCaptureNote,
  sessionDescription,
  sessionFrontmatterPeople,
  sessionTitle,
  sessionTitleParts,
  sortSessionsForSidebar,
  type SessionFilters,
} from "./lib/session-model";
import { normalizeSidebarDateFormat } from "./lib/date-format";
import { renderHomeIntro, renderSidebar, type SidebarRenderContext } from "./render/sidebar";
import {
  renderAudioHealth,
  renderMemoLine,
  renderMemoLineInner,
  renderRecordingView,
  type RecordingStartupRenderState,
  type RecordingRenderContext,
  type TapRecoveryState,
  type CaptureDeviceToast,
} from "./render/recording";
import {
  renderAiSettingsSection,
  renderAudioSettingsSection,
  renderSettingsOverlay,
  renderSpeechSettingsSection,
  type SettingsRenderContext,
} from "./render/settings";
import { roleSummaryInner } from "./lib/ai-preview";
import {
  renderJobBanner as renderJobBannerView,
  renderLiveBackchannel,
  renderSessionStatus,
  renderSessionWorkspace as renderSessionWorkspaceView,
  sessionStatusLabel,
  type JobBannerRenderContext,
  type LiveBackchannelRenderContext,
  type SessionWorkspaceRenderContext,
} from "./render/session-workspace";
import {
  groundedNoteVisibleMarkdown,
  renderDistillTab as renderDistillTabView,
  type DistillTabRenderContext,
} from "./render/distill";
import {
  appendNewMemoInput as appendNewMemoInputController,
  attachMemoEditHandlers as attachMemoEditHandlersController,
  updateRecordingHeader as updateRecordingHeaderController,
  type MemoEditorContext,
} from "./controllers/memo-editor";
import {
  renderProcessingView,
  renderReviewView,
  renderSummaryView,
} from "./render/flow";
import { renderModelNotice, type ModelNoticeData } from "./render/model-notice";
import { registerNavigationActions, type NavigationActionsContext } from "./actions/navigation";
import type { SessionTab, View } from "./actions/types";
import { installCaptureShortcuts, captureShortcutHints } from "./lib/keyboard";
import {
  pendingMemoDraftForSession,
  type PendingMemoDraft,
} from "./lib/memo-ownership";
import { mergeRefreshedSessions } from "./lib/live-capture-row";
import { appendPulledMarginLine } from "./lib/prep-margin-pull";
import {
  isMobileNavigationViewport,
  MOBILE_NAVIGATION_QUERY,
  sidebarToggleLabel,
} from "./lib/mobile-navigation";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

let currentView: View = "home";
let settingsOverlayOpen = false;
// Which settings panel the page is showing. Module state so it survives the
// frequent background re-renders that rebuild the overlay from scratch.
let settingsActiveSection = "ai";
const mobileNavigationQuery = typeof window !== "undefined" && window.matchMedia
  ? window.matchMedia(MOBILE_NAVIGATION_QUERY)
  : null;
let sidebarCollapsed = isMobileNavigationViewport();
let sidebarWidth = loadSidebarWidth();
let themeMode: ThemeMode = loadThemeMode();
applyTheme(themeMode);
let activeSessionName: string | null = null;
let activeSessionTab: SessionTab = "backchannel";
let openSessionRequestSeq = 0;
let sidebarFilterOpen = false;
let sessionFilters: SessionFilters = emptySessionFilters();
// Active-project session search: when active, the sidebar swaps the project tree
// for a search view scoped to the loaded project.
let globalSearchActive = false;
let globalSearchQuery = "";
let sidebarCollapsedSections = new Set<string>(defaultCollapsedSidebarKeys());

function closeMobileNavigation(): void {
  if (!isMobileNavigationViewport() || sidebarCollapsed) return;
  sidebarCollapsed = true;
  render();
}

let projectSessionsById: Record<string, SessionInfo[]> = {};
let artifactCache: Record<string, { aligned?: string; note?: string; loading?: boolean }> = {};
let projectFilesFingerprint: string | null = null;
let projectFilesRefreshInterval: number | null = null;
let projectFilesRefreshInFlight = false;
let lastProjectFilesRefreshAt = 0;
let cliInstallStatus: { state: "idle" | "installing" | "installed" | "error"; message: string | null; result?: InstallCliResult | null } = {
  state: "idle",
  message: null,
  result: null,
};

let memoLinesBySession: Record<string, MemoLine[]> = {};
let groundedNoteBySession: Record<string, GroundedNoteState> = {};

// Granola plan-gated transcript hint: shown once after an import reveals plan gating,
// dismissed via the × button and remembered across restarts via localStorage.
const GRANOLA_PLAN_CALLOUT_DISMISSED_KEY = "margins.granolaPlanCalloutDismissed";
let granolaPlanCalloutDismissed: boolean = (() => {
  try { return window.localStorage.getItem(GRANOLA_PLAN_CALLOUT_DISMISSED_KEY) === "1"; }
  catch { return false; }
})();
// Set to true after an import result with transcripts_plan_gated; cleared on dismiss.
let granolaPlanCalloutVisible = false;

// People each note was last written/reprocessed with, so "Reprocess with
// <name>" is only offered for people added AFTER the note existed. Persisted
// across launches, along with per-session dismissals of the suggestion.
const REPROCESS_BASELINE_STORAGE_KEY = "margins.reprocessPeopleBaseline";
const REPROCESS_DISMISSED_STORAGE_KEY = "margins.reprocessPeopleDismissed";
const FIRST_CAPTURE_NOTE_CHOICE_KEY = "margins.firstCaptureNoteChoice";
const FIRST_NOTE_CELEBRATED_KEY = "margins.firstNoteCelebrated";

// True only on the very first completed note payoff (cleared from localStorage).
let firstNoteCelebrated = false;
let reprocessSuggestionStateLoaded = false;
let reprocessPeopleBaselineBySession: Record<string, string[]> = {};
let reprocessDismissedKeyBySession: Record<string, string> = {};
let firstCaptureNoteChoice: "write" | "capture" | null = readFirstCaptureNoteChoice();

// Read lazily rather than at module init so the browser test harness (loaded
// via dynamic import) gets a chance to seed scenario state first.
function ensureReprocessSuggestionState() {
  if (reprocessSuggestionStateLoaded) return;
  reprocessSuggestionStateLoaded = true;
  reprocessPeopleBaselineBySession = readStoredJson(REPROCESS_BASELINE_STORAGE_KEY) || {};
  reprocessDismissedKeyBySession = readStoredJson(REPROCESS_DISMISSED_STORAGE_KEY) || {};
}

function readStoredJson<T>(key: string): T | null {
  try {
    const raw = window.localStorage.getItem(key);
    return raw ? JSON.parse(raw) as T : null;
  } catch {
    return null;
  }
}

function readFirstCaptureNoteChoice(): "write" | "capture" | null {
  try {
    const raw = window.localStorage.getItem(FIRST_CAPTURE_NOTE_CHOICE_KEY);
    return raw === "write" || raw === "capture" ? raw : null;
  } catch {
    return null;
  }
}

function setFirstCaptureNoteChoice(choice: "write" | "capture" | null) {
  firstCaptureNoteChoice = choice;
  try {
    if (choice) window.localStorage.setItem(FIRST_CAPTURE_NOTE_CHOICE_KEY, choice);
    else window.localStorage.removeItem(FIRST_CAPTURE_NOTE_CHOICE_KEY);
  } catch {
    // Local storage is non-critical; the visible choice still applies in-memory.
  }
}

function writeStoredJson(key: string, value: unknown) {
  try {
    window.localStorage.setItem(key, JSON.stringify(value));
  } catch {}
}

function normalizedPersonKey(raw: string): string {
  return raw.trim().replace(/^\[\[/, "").replace(/\]\]$/, "").toLowerCase();
}

function peopleSetKey(people: string[]): string {
  return people.map(normalizedPersonKey).sort().join("|");
}

function rememberReprocessPeopleBaseline(name: string, people: string[]) {
  ensureReprocessSuggestionState();
  reprocessPeopleBaselineBySession[name] = [...people];
  delete reprocessDismissedKeyBySession[name];
  writeStoredJson(REPROCESS_BASELINE_STORAGE_KEY, reprocessPeopleBaselineBySession);
  writeStoredJson(REPROCESS_DISMISSED_STORAGE_KEY, reprocessDismissedKeyBySession);
}

// People added to the session after its note was written. Empty when the note
// isn't saved yet, nothing changed, or the user dismissed the suggestion.
function reprocessCandidatesForSession(s: SessionInfo): string[] {
  if (!artifactCacheForSession(s).note) return [];
  ensureReprocessSuggestionState();
  const people = sessionFrontmatterPeople(s);
  const baseline = reprocessPeopleBaselineBySession[s.name];
  if (!baseline) {
    // First time this note is seen (including notes that predate this
    // feature): whoever is on it now counts as distill-time people.
    rememberReprocessPeopleBaseline(s.name, people);
    return [];
  }
  const known = new Set(baseline.map(normalizedPersonKey));
  const added = people.filter(p => !known.has(normalizedPersonKey(p)));
  if (!added.length) return [];
  if (reprocessDismissedKeyBySession[s.name] === peopleSetKey(added)) return [];
  return added;
}
let settings: Settings = defaultSettings();
let aiStatus: AiStatus = {
  chatgpt_authenticated: false,
  chatgpt_message: "Sign in with ChatGPT Plus/Pro, or use an API key below.",
};
let includedAiStatus: IncludedAiStatus = {
  included_ready: false,
  message: "Included note-making will create a usage-limited key before your first note.",
};
let includedAiPreparePromise: Promise<void> | null = null;
// Single-shot all-modes readiness + a debounced resolution preview from the
// Rust primitives. Null until first fetch (or if the command fails — the
// settings UI then falls back to settings-derived checks rather than crashing).
let aiReadiness: AiReadiness | null = null;
let aiPreview: ResolutionPreview | null = null;
let aiPreviewDebounce: ReturnType<typeof setTimeout> | null = null;
// Ephemeral quick-assists tier selection. Lets "Custom model…" stay selected
// before the user has typed a model (which is indistinguishable from Balanced in
// the persisted fields alone). Null → derive from settings. Reset on mode switch
// and when the settings view opens.
let cueTierSelection: CueTier | null = null;
let sessions: SessionInfo[] = [];
let devices: DeviceInfo[] = [];
let webMicrophonePermissionState: WebMicrophonePermissionState = "unknown";
let webMicrophoneFailureCategory: WebMicrophoneFailureCategory | null = null;
let webMicrophoneDeviceId: string | null = getPreferredWebMicrophoneDeviceId();
let webMicrophoneRequesting = false;
let webMicrophoneError: string | null = null;
let audioTestRunning = false;
let audioTestResult: AudioTestResult | null = null;
let audioTestError: string | null = null;
let systemAudioTestRunning = false;
let systemAudioTestResult: SystemAudioTestResult | null = null;
let systemAudioTestError: string | null = null;
let audioCheckPhase: "microphone" | "system" | null = null;
let systemAudioPrivacyPaneOpened = false;
let resumedAfterPermissionRestart = false;
// Drives the post-restart confirm flow: after the user enables System Audio
// Recording Only and relaunches, we land them on the Audio pane ("resumed") with
// a one-press Confirm. We deliberately do NOT probe the tap at launch — creating
// the system-audio tap touches ScreenCapture TCC, and launch must stay
// TCC-clean (all permission activity happens on an explicit audio action). The
// probe runs only when the user presses Confirm or starts a capture.
// null = not resuming.
let audioResumeState: "resumed" | "checking" | "confirmed" | "blocked" | null = null;
let recordingStatus: RecordingStatus = {
  is_recording: false,
  paused: false,
  session_name: null,
  elapsed_secs: 0,
  input_device_name: null,
  capture_device: { state: "active", device_name: "System Default" },
  mic_level: 0,
  mic_audio_frame_count: 0,
  spk_level: 0,
  mic_drop_count: 0,
  spk_drop_count: 0,
  mic_gap_ms_total: 0,
  mic_switch_count: 0,
  timeline_reusable: true,
  speaker_silence_secs: 0,
  system_audio_expected: false,
  system_audio_frame_count: 0,
  system_audio_observed: false,
  system_audio_seen: false,
  tap_status: "not_expected",
  tap_warning: null,
  live_transcription_mode: "stereo_split",
  capture_phase: "idle",
};
const hostedRecoveryState = new HostedRecoveryBrowserState();
let hostedActiveRecordingStatus: RecordingStatus | null = null;
type RecordingStartupState = RecordingStartupRenderState & {
  id: number;
  projectId: string | null;
  requestedName: string;
  sessionName: string;
};
let recordingStartup: RecordingStartupState | null = null;
let recordingStartupSeq = 0;
let recordingStartPromise: Promise<void> | null = null;
let queuedMemoCheckpointIndices = new Set<number>();
let queuedBackchannelRequestIndices = new Set<number>();
const memoCheckpointInFlight = new Map<number, Promise<void>>();
let dismissedTapNoticeSession: string | null = null;
let tapRecoverySession: string | null = null;
let tapRecoveryState: TapRecoveryState = "idle";
let tapRecoveryError: string | null = null;
let lastCaptureDeviceState: CaptureDeviceEvent | null = null;
let pendingCaptureDeviceSwitchFrom: string | null = null;
let pendingCaptureDeviceSwitchTo: string | null = null;
let manualCaptureDeviceSwitchPending = false;
let manualCaptureDeviceSwitchTarget: string | null = null;
let captureDeviceSwitchToast: CaptureDeviceToast | null = null;
let captureDeviceSwitchToastTimer: number | null = null;
let holdingRecoveryDeviceName: string | null = null;
let stopInFlight = false;
// A Finish action can arrive from a shortcut or a stale pre-render button while
// Pause is durably sealing the current segment. Preserve the intent instead of
// silently requiring a second action once the pause command resolves.
let finishQueuedAfterPause = false;
let captureDiscardDialog: { sessionName: string } | null = null;
let captureSpeakerCount = 0;
// Per-session live ASR warmup / degraded state. Set when recording starts,
// cleared by live-transcript-ready / live-transcript-degraded events or on stop.
let transcriptWarming = false;
let transcriptDegraded = false;
let finalizingCaptureSessionName: string | null = null;
let speechModelDownloadInProgress = false;
// The first capture kicks the model download off inline. Attempt that at most
// once per app run so a user who cancels the download isn't re-nagged on their
// next capture — the passive notice still lets them start it by hand.
let autoModelDownloadAttempted = false;
let modelProvisionNotice: ModelNoticeData | null = null;
// Unsaved device picker selection that must survive model-download re-renders.
// Set in __onInputDeviceChange (non-recording path); cleared when the overlay
// closes. Overlaid onto `settings` in settingsRenderContext() so the picker
// shows the right value even after reloadSettingsAndRender() reloads from disk.
let pendingDeviceSelection: { mode: "pinned" | "follow_default"; uid: string | null; name: string | null } | null = null;
// Unsaved Notes folder ("Change…") selection that must survive re-renders. The
// native folder chooser blurs the window; returning fires the focus handler
// which calls reloadSettingsAndRender() and overwrites `settings` from disk. We
// therefore stage the pick here (not in `settings`) and overlay it in
// settingsRenderContext(), so the pane renders the chosen path from state rather
// than DOM patching. Committed on Save; cleared when the overlay closes.
let pendingVaultPathChange: string | null = null;
// Track the previous open state so render() can detect the close transition.
let prevSettingsOverlayOpen = false;
let modelNoticeDismissTimer: number | null = null;
let obsidianVaultReady = false;
let notesFolderReady = false;
const indexingProjectIds = new Set<string>();
const recallIndexingProjectIds = new Set<string>();
const refreshingProjectIds = new Set<string>();
let copiedProjectPrompt: { projectId: string; mode: "assess" | "fix" | "setup" } | null = null;
let copiedProjectPromptTimer: number | null = null;
let agentSetupWaitingProjectId: string | null = null;
const vaultNoteCounts = new Map<string, number>();
// Ensure-CLI-tools step that runs before the Setup-with-agent prompt is copied.
// CLI verification runs in its own click; the clipboard write happens on a later
// click so no async await ever sits between the user gesture and the copy
// (WKWebView drops transient activation across async verification). The "ready"
// state caches the verified CLI path so the copy click needs no await.
let agentSetupCliInstall: {
  state: "idle" | "installing" | "ready" | "error";
  projectId: string | null;
  message: string | null;
} = {
  state: "idle",
  projectId: null,
  message: null,
};
let agentSetupPollTimer: number | null = null;
const dismissedProjectSetupHints = new Set<string>();
let calendarSuggestion: CalendarEventSuggestion | null = null;
let calendarSuggestionFreshness: EvidenceFreshness = {
  status: "not_applicable",
  stale: false,
};

function acceptCalendarSuggestionResult(result: CalendarSuggestionResult): CalendarEventSuggestion | null {
  calendarSuggestion = result.suggestion;
  calendarSuggestionFreshness = result.freshness;
  return calendarSuggestion;
}

// Audio-import drag state. While a drag is active over the window we toggle
// classes on the live `.sidebar` element and DEFER any background render so a
// processing-progress tick cannot destroy the overlay mid-gesture.
let dragInProgress = false;
let renderPendingFromDrag = false;
let lastDropValid = true;
// Which kind of files the active drag holds, so the drop box copy can flash
// "Granola export" for .json/.jsonl drags vs the generic audio copy.
let lastDropKind: "audio" | "granola" | null = null;
// Aria-live announcement text for imports; rendered into #sidebar-import-status
// so it survives full re-renders and only re-announces when the text changes.
let importAnnouncement = "";
// Session name to flash with the success highlight after an import reconciles.
let flashImportSessionName: string | null = null;
// Session name to flash with the success highlight after distillation completes.
let flashDistilledSessionName: string | null = null;
let flashDistilledTimer: number | null = null;
const deletedSessionNames = new Set<string>();
// A dropped/browsed import awaiting the configure step (speaker count + confirm).
// While set, the top of the session list shows a configure card instead of
// firing the single-shot import immediately.
let pendingImport: { paths: string[]; speakerCount: number; projectId: string | null } | null = null;
// Ephemeral Granola import authorization/progress for the sidebar action.
let granolaStatus: GranolaImportStatus = { authorized: false, account: null, accounts: [], message: "Not authorized", retryable: false };
let granolaBusy: "authorizing" | "importing" | null = null;
let granolaImportStage: string | null = null;

const distillTraceBySession: Record<string, DistillTraceEvent[]> = {};
const lifecycleBySession: Record<string, SessionLifecycleState> = {};
const followupChatBySession: Record<string, { role: "user" | "assistant"; text: string }[]> = {};
const noteJobErrorBySession: Record<string, string> = {};
const noteCancelledBySession: Record<string, boolean> = {};
function noteErrorForSession(sessionOrName: SessionInfo | string): string | undefined {
  const name = typeof sessionOrName === "string" ? sessionOrName : sessionOrName.name;
  const session = typeof sessionOrName === "string" ? sessions.find(s => s.name === name) : sessionOrName;
  if (processingSessionName === name || session?.status === "processing") return noteJobErrorBySession[name];
  return noteJobErrorBySession[name] || session?.failure_message || (session?.status === "failed" ? "Margins could not finish the note." : undefined);
}
function noteErrorNames(): Set<string> {
  const names = new Set(Object.keys(noteJobErrorBySession));
  for (const session of sessions) {
    if (noteErrorForSession(session)) names.add(session.name);
  }
  return names;
}

type CaptureHealthEvent = { atSecs: number; kind: "tap_dropped" | "tap_waiting" | "tap_recovered" | "mic_diarized"; label: string };
let captureHealthBySession: Record<string, CaptureHealthEvent[]> = {};
let lastTapStatusBySession: Record<string, string> = {};
let lastLiveModeBySession: Record<string, string> = {};

// Prep/pause session context (clock-stopped surface)
let prepSessionName = "";
let prepEventTitle: string | null = null;
let prepPeople: string[] = [];
// One-view/two-postures state machine: tracks which clock-stopped block we're in.
// 0 = initial prep (before first recording start); incremented on each pause.
let currentBlockOrdinal = 0;
// Auto-start from calendar
let armedFromCalendar = false;
// True while calendar prep owns the main workspace before audio starts.
let preStartPrepOpen = false;
// Project ownership prevents a prepared draft from starting in a project the
// user switched to after opening the calendar card.
let prepProjectId: string | null = null;
let prepStartBlockedReason: string | null = null;
let autoStartDeadline: number | null = null;
let autoStartTimer: ReturnType<typeof setInterval> | null = null;
// Wall-clock ms at the moment the capture was paused. While set, the memo clock
// is frozen (pause is not recording time); on resume we advance memoStartTime by
// the paused wall-duration so that memoStartTime tracks RECORDING time, not raw
// wall time. This keeps timed lines' created_secs aligned with the contiguous
// audio/transcript timeline (which has no gap for the pause).
let pausedAtWallMs: number | null = null;
// Wall-clock string captured when entering each block (for gutter label on pause lines)
const blockWallClockByOrdinal = new Map<number, string>();

// Block-scoped margin regions (replaces flat prepMarginalia / index-keyed sets)
type BlockMargin = {
  marginalia: PrepHydrationMarginalia[];
  state: PrepHydrationEvent["state"] | null;
  reason: string | null;
  hint: string | null;
  consumed: Set<string>;   // content-hash keys
  expanded: Set<string>;   // content-hash keys
  pulledTexts: string[];
};
const blockMargins = new Map<number, BlockMargin>();

function getOrCreateBlockMargin(ordinal: number): BlockMargin {
  if (!blockMargins.has(ordinal)) {
    blockMargins.set(ordinal, { marginalia: [], state: null, reason: null, hint: null, consumed: new Set(), expanded: new Set(), pulledTexts: [] });
  }
  return blockMargins.get(ordinal)!;
}

/** FNV-1a-style content hash for marginalia identity (anchor + " " + text). */
function marginaliaHash(item: PrepHydrationMarginalia): string {
  return `${item.anchor ?? ""}\x00${item.text}`;
}

/** Committed (non-empty) memo lines belonging to a clock-stopped block. */
function committedLinesInBlock(ordinal: number): MemoLine[] {
  return memoLines.filter(l => l.block_ordinal === ordinal && l.text.trim().length > 0);
}

/** Committed (non-empty) TIMED memo lines — the meeting's notes so far. */
function timedMemoLines(): MemoLine[] {
  return memoLines.filter(l => l.block_ordinal == null && l.text.trim().length > 0);
}

/**
 * What a "Look over this" hydration works from for a clock-stopped block, or
 * null when there is genuinely nothing to look over (so the affordance hides).
 * - Block 0 (pre-start prep): the prep lines themselves.
 * - Block N>0 (mid-meeting pause): the pause-block notes if the user jotted any,
 *   with the meeting's timed notes as positioned context; otherwise — at a fresh
 *   pause with no new notes — look over the timed notes so far directly, so the
 *   user isn't forced to type a new line before the affordance appears.
 */
function hydrationMaterial(ordinal: number): { sketch: MemoLine[]; meetingSoFar: string | null } | null {
  const blockLines = committedLinesInBlock(ordinal);
  if (ordinal === 0) {
    return blockLines.length > 0 ? { sketch: blockLines, meetingSoFar: null } : null;
  }
  if (blockLines.length > 0) {
    const meetingSoFar = timedMemoLines().map(l => l.text).join("\n") || null;
    return { sketch: blockLines, meetingSoFar };
  }
  const priorTimed = timedMemoLines();
  return priorTimed.length > 0 ? { sketch: priorTimed, meetingSoFar: null } : null;
}

/**
 * Reset all per-capture prep-hydration block state to block 0. Single source of
 * truth for the reset so new state fields can't be forgotten at one of the
 * several entry points (calendar open, cancel) — a class of bug we've already
 * hit. Does NOT touch armedFromCalendar/autoStart timers (caller owns those) or
 * seeded block margins (begin-from-prep preserves block 0 deliberately).
 */
function resetPrepBlockState() {
  currentBlockOrdinal = 0;
  blockMargins.clear();
  blockWallClockByOrdinal.clear();
  prepSteerDraft = "";
  prepSteerActive = false;
  pausedAtWallMs = null;
}

// Steer input for prep (still single-draft; per-block on submit)
let prepSteerDraft = "";
let prepSteerActive = false;

// Memo editor state
let memoLines: MemoLine[] = [];
let memoStartTime = 0; // epoch ms
type BackchannelCard = BackchannelSuggestionEvent & { collapsed?: boolean; received_at: number; placeholder?: boolean; steering?: boolean };
const backchannelCardsByMemo: Record<number, BackchannelCard> = {};
const backchannelSteerDraftByMemo: Record<number, string> = {};
const backchannelErrorByMemo: Record<number, { memo_time: string; received_at: number }> = {};
const backchannelTerminalByMemo: Record<number, { status: "quiet" | "warming" | "unavailable" | "error"; memo_time?: string; received_at: number; reason?: string; hint?: string }> = {};
let activeBackchannelMemoIndex: number | null = null;
// Backchannel (live cue) state is keyed by memo_index, so it must be wiped
// whenever a fresh capture reuses those indices — otherwise a stale "drafting"
// card from a deleted note reappears on the new note's line at the same index.
function clearBackchannelState() {
  for (const k of Object.keys(backchannelCardsByMemo)) delete backchannelCardsByMemo[Number(k)];
  for (const k of Object.keys(backchannelSteerDraftByMemo)) delete backchannelSteerDraftByMemo[Number(k)];
  for (const k of Object.keys(backchannelErrorByMemo)) delete backchannelErrorByMemo[Number(k)];
  for (const k of Object.keys(backchannelTerminalByMemo)) delete backchannelTerminalByMemo[Number(k)];
  activeBackchannelMemoIndex = null;
}
// Summary state (shown after recording ends)
let summarySessionName = "";
let summaryDuration = 0;

// Processing state
let processingSteps: ProcessingEvent[] = [];
let processingSessionName = "";
let currentProgress = 0;
let processingRunId = 0;
let currentProcessingUnlisten: (() => void) | null = null;
let transcriptEntries: TranscriptEntry[] = [];
let streamingPanelScrollTop = 0;
// Whether the streaming note should stay glued to the bottom. True until the
// user scrolls up to read earlier content; flips back once they return to the
// bottom. Lets the note anchor at the bottom without trapping the user there.
let streamingStickToBottom = true;
// How close to the bottom (px) still counts as "pinned". Generous enough to
// survive a tick's worth of newly-landed content between capture and re-anchor.
const STREAM_STICK_THRESHOLD_PX = 48;
const streamingBlockSizesBySession: Record<string, Record<string, number>> = {};
const streamingFirstParagraphPinnedBySession: Record<string, boolean> = {};

// Geometry of each "not yet woven in" memo line, measured against the top of the
// memo stack (not the viewport or note body) so the offsets only change when a
// memo is actually consumed — note growth and bottom-anchor scrolling don't
// perturb them. Captured off the *old* DOM before each innerHTML rebuild so the
// post-rebuild hydrate can FLIP survivors up into the gap a consumed memo left.
type MemoLineRect = { offset: number; height: number; html: string };
const streamingMemoRectsBySession: Record<string, Record<string, MemoLineRect>> = {};

const reduceMotionQuery = typeof window !== "undefined" && window.matchMedia
  ? window.matchMedia("(prefers-reduced-motion: reduce)")
  : null;
function prefersReducedMotion(): boolean {
  return Boolean(reduceMotionQuery?.matches);
}

// Snapshot the remaining-marks stack from the live DOM, keyed by memo id, with
// each line's offset measured relative to the stack's own top edge. Run this
// *before* the innerHTML wipe — it reads the element that's about to be replaced.
function snapshotMemoStack(): Record<string, MemoLineRect> {
  const out: Record<string, MemoLineRect> = {};
  const stack = document.querySelector(".primary-session-panel .note-memo-source-stack") as HTMLElement | null;
  if (!stack) return out;
  const stackTop = stack.getBoundingClientRect().top;
  for (const line of Array.from(stack.querySelectorAll<HTMLElement>(".note-buffer-memo-line"))) {
    const id = line.dataset.memoId;
    if (!id) continue;
    // getBoundingClientRect reflects any in-flight transform, so a FLIP that's
    // still settling when the next tick rebuilds resumes from where it visually
    // is rather than snapping — the recede stays continuous across rebuilds.
    out[id] = {
      offset: line.getBoundingClientRect().top - stackTop,
      height: line.offsetHeight,
      html: line.outerHTML,
    };
  }
  return out;
}

// Review state
let reviewContent = "";
let reviewSessionName = "";

// Polling intervals
let statusInterval: number | null = null;
let memoSyncInterval: number | null = null;

// ---------------------------------------------------------------------------
// Render engine
// ---------------------------------------------------------------------------

const app = document.getElementById("app")!;

// A clock-stopped capture can receive async prep results while WebKit is still
// composing text (IME, dictation, or inline autocomplete). Replacing the DOM in
// that window makes the acceptance keystroke act on a fresh textarea. Hold the
// render until composition has cleanly ended.
let memoCompositionActive = false;
let memoRenderPendingAfterComposition = false;
app.addEventListener("compositionstart", event => {
  if ((event.target as Element | null)?.matches?.(".memo-editor textarea")) {
    memoCompositionActive = true;
  }
}, true);
app.addEventListener("compositionend", event => {
  if (!(event.target as Element | null)?.matches?.(".memo-editor textarea")) return;
  memoCompositionActive = false;
  if (memoRenderPendingAfterComposition) {
    memoRenderPendingAfterComposition = false;
    queueMicrotask(() => render());
  }
}, true);

// The settings overlay and capture-discard dialog are full-screen fixed modals
// that live in their own persistent host rather than inside `app`. Rebuilding
// `app.innerHTML` on every background render (polls, progress ticks) would wipe
// these modals' DOM nodes, resetting CSS `:hover`/focus and making the nav rail
// flicker under the cursor. Keeping them here lets us re-render the modal only
// when its content actually changes. See syncSettingsHost().
const settingsHost = (() => {
  const el = document.createElement("div");
  el.id = "settings-host";
  app.after(el);
  return el;
})();
let lastSettingsHostHtml = "";

app.addEventListener("click", event => {
  const link = (event.target as Element | null)?.closest<HTMLAnchorElement>("a.internal-note-link");
  if (!link) return;
  const href = link.getAttribute("href") || "";
  if (!href.startsWith("#note:")) return;
  event.preventDefault();
  const target = decodeURIComponent(href.slice("#note:".length)).trim();
  if (target) void window.__openNoteTargetInVault(target);
});

const markdown = new MarkdownIt({
  html: false,
  linkify: false,
  typographer: true,
  breaks: false,
});

const defaultLinkOpenRenderer = markdown.renderer.rules.link_open;
markdown.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  const href = tokens[idx].attrGet("href") || "";
  if (href.startsWith("#note:")) {
    tokens[idx].attrSet("href", href);
    tokens[idx].attrJoin("class", "internal-note-link");
    tokens[idx].attrSet("title", decodeURIComponent(href.slice("#note:".length)));
  } else {
    tokens[idx].attrJoin("class", "markdown-link");
  }
  return defaultLinkOpenRenderer
    ? defaultLinkOpenRenderer(tokens, idx, options, env, self)
    : self.renderToken(tokens, idx, options);
};

function renderWindowDragRegion(): string {
  return `<div class="window-drag-region" data-tauri-drag-region aria-hidden="true"></div>`;
}

function renderWorkspaceTopbar(): string {
  const mobile = isMobileNavigationViewport();
  const sidebarToggleIcon = iconSvg(mobile ? Menu : PanelLeft, "ui-icon topbar-icon");
  const toggleLabel = sidebarToggleLabel(sidebarCollapsed, mobile);
  const active = activeSessionName ? sessions.find(session => session.name === activeSessionName) : null;
  const mobileTitle = active ? sessionTitle(active) : activeProject(settings).name || "Margins";
  const project = activeProject(settings);
  const cta = captureCta({
    captureReady: captureReadyFromState(),
    projectConfigured: Boolean(project.path?.trim()),
    projectName: project.name || "Margins",
  });
  const mobileNewButton = `<button class="icon-button mobile-topbar-new" title="${esc(cta.title)}" aria-label="${esc(cta.ariaLabel)}" ${cta.onclick ? `onclick="${cta.onclick}"` : ""} ${cta.disabled ? "disabled" : ""}>${iconSvg(Plus, "ui-icon topbar-icon")}</button>`;
  return `
    <div class="window-topbar" data-tauri-drag-region>
      <div class="window-topbar-leading">
        <button class="icon-button window-topbar-toggle" title="${toggleLabel}" aria-label="${toggleLabel}" aria-expanded="${!sidebarCollapsed}" aria-controls="workspace-sidebar" onclick="window.__toggleSidebar()">${sidebarToggleIcon}</button>
        <span class="mobile-topbar-title">${esc(mobileTitle)}</span>
        ${mobileNewButton}
      </div>
    </div>
  `;
}

function renderGlobalHeaderActions(): string {
  const nextTheme = themeMode === "dark" ? "light" : "dark";
  const themeIcon = iconSvg(themeMode === "dark" ? Sun : Moon, "ui-icon topbar-icon");
  const settingsIcon = iconSvg(SettingsIcon, "ui-icon topbar-icon");
  let captureBtn = "";
  if (sidebarCollapsed) {
    const project = activeProject(settings);
    const cta = captureCta({
      captureReady: captureReadyFromState(),
      projectConfigured: Boolean(project.path?.trim()),
      projectName: project.name || "Margins",
    });
    const plusIcon = iconSvg(Plus, "ui-icon topbar-icon");
    captureBtn = `<button class="icon-button global-header-new" title="${esc(cta.title)}" aria-label="${esc(cta.ariaLabel)}" ${cta.onclick ? `onclick="${cta.onclick}"` : ""} ${cta.disabled ? "disabled" : ""}>${plusIcon}</button>`;
  }
  return `
    <div class="global-header-actions">
      <button class="icon-button" title="Switch to ${nextTheme} mode" aria-label="Switch to ${nextTheme} mode" onclick="window.__toggleTheme()">${themeIcon}</button>
      <button class="icon-button" title="Setup" aria-label="Setup" onclick="window.__nav('settings')">${settingsIcon}</button>
      ${captureBtn}
    </div>
  `;
}

// The overlay/dialog markup now mounts into the persistent `settingsHost`, not
// inline in each view's innerHTML — see syncSettingsHost(). View templates keep
// a `settingsOverlay` slot for layout compatibility; it stays empty.
function renderSettingsOverlayIfOpen(): string {
  return "";
}

// Re-render the settings modal host only when its markup changes. On an
// unchanged render (the common background-poll case) we leave the live DOM
// untouched, so hover, focus, and scroll survive. When it does change, we
// re-assert focus/scroll/section state exactly as a full render used to.
function syncSettingsHost(sectionState: Record<string, boolean> | null, modalScrollTop: number | null): void {
  const html = `${settingsOverlayOpen ? renderSettingsOverlay(settingsRenderContext()) : ""}${renderCaptureDiscardDialog()}`;
  if (html === lastSettingsHostHtml) return;
  lastSettingsHostHtml = html;
  settingsHost.innerHTML = html;
  if (settingsOverlayOpen) {
    hydrateSettingsOverlay();
    restoreSettingsSectionState(sectionState);
    restoreSettingsModalScroll(modalScrollTop);
  }
}

// Refresh only the settings modal — not the whole app. Settings-scoped handlers
// (test audio, toggles) mutate state that lives entirely inside the overlay; the
// home view underneath is hidden by the modal, so rebuilding its sidebar +
// session workspace on every keystroke/click just adds latency and makes the
// buttons feel sluggish. This patches the modal host in place instead.
function renderSettings() {
  syncSettingsHost(captureSettingsSectionState(), captureSettingsModalScroll());
}

function renderCaptureDiscardDialog(): string {
  if (!captureDiscardDialog) return "";
  return `
    <div class="app-confirm-overlay" role="dialog" aria-modal="true" aria-labelledby="discard-capture-title" onclick="if (event.target === this) window.__cancelDiscardActiveRecording()">
      <div class="app-confirm-dialog destructive" onclick="event.stopPropagation()">
        <div class="app-confirm-icon">${iconSvg(Trash2, "ui-icon confirm-icon")}</div>
        <div class="app-confirm-copy">
          <h2 id="discard-capture-title">Discard this capture?</h2>
          <p>This stops capture now and deletes the local audio, transcript, and marks.</p>
        </div>
        <div class="app-confirm-actions">
          <button class="subtle" onclick="window.__cancelDiscardActiveRecording()">Cancel</button>
          <button class="danger" onclick="window.__confirmDiscardActiveRecording()">Discard</button>
        </div>
      </div>
    </div>
  `;
}

let sidebarSearchWasFocused = false;
function captureSidebarFocus() {
  sidebarSearchWasFocused = document.activeElement?.id === "sidebar-global-search-input";
}

function captureSettingsModalScroll(): number | null {
  if (!settingsOverlayOpen) return null;
  const modal = document.querySelector(".settings-content") as HTMLElement | null;
  return modal ? modal.scrollTop : null;
}

function captureSettingsSectionState(): Record<string, boolean> | null {
  if (!settingsOverlayOpen) return null;
  const sections = Array.from(document.querySelectorAll<HTMLDetailsElement>(".settings-modal details[id]"));
  if (sections.length === 0) return null;
  return Object.fromEntries(sections.map(section => [section.id, section.open]));
}

function restoreSettingsSectionState(state: Record<string, boolean> | null) {
  if (!state) return;
  for (const [id, open] of Object.entries(state)) {
    const section = document.getElementById(id) as HTMLDetailsElement | null;
    if (section?.tagName === "DETAILS") section.open = open;
  }
}

function restoreSettingsModalScroll(scrollTop: number | null) {
  if (scrollTop === null) return;
  const modal = document.querySelector(".settings-content") as HTMLElement | null;
  if (modal) modal.scrollTop = scrollTop;
}

let processingRenderFrame: number | null = null;
function renderProcessingProgress(stage: string, shouldRender: boolean) {
  if (!shouldRender) return;
  if (stage === "note_stream") {
    // Several completed lines can arrive in one model/network burst. Consume
    // them immediately but rebuild the application DOM at most once per paint.
    if (processingRenderFrame === null) {
      processingRenderFrame = requestAnimationFrame(() => {
        processingRenderFrame = null;
        render();
      });
    }
    return;
  }
  // Lifecycle, error, cancellation, and saved states must not wait behind a
  // queued streaming frame.
  if (processingRenderFrame !== null) {
    cancelAnimationFrame(processingRenderFrame);
    processingRenderFrame = null;
  }
  render();
}

function render() {
  // Never rebuild the DOM mid-drag: it would wipe the overlay class state on
  // `.sidebar`. Background callers (polling, processing-progress) get deferred.
  if (dragInProgress) {
    renderPendingFromDrag = true;
    return;
  }
  if (memoCompositionActive) {
    memoRenderPendingAfterComposition = true;
    return;
  }
  // When the settings overlay just closed, discard any unsaved device pick so
  // the next open starts from the persisted state rather than a stale choice.
  if (!settingsOverlayOpen && prevSettingsOverlayOpen) {
    pendingDeviceSelection = null;
    pendingVaultPathChange = null;
  }
  prevSettingsOverlayOpen = settingsOverlayOpen;
  capturePendingDraftForNextRender();
  captureSidebarFocus();
  const settingsModalScrollTop = captureSettingsModalScroll();
  const settingsSectionState = captureSettingsSectionState();
  const activeBeforeRender = activeSessionName ? sessions.find(s => s.name === activeSessionName) ?? null : null;
  const preserveStreamingScroll = Boolean(activeBeforeRender && isSessionStreaming(activeBeforeRender));
  if (preserveStreamingScroll) {
    // Capture the live panel's position *before* innerHTML wipes it. Measuring
    // the old element (not the rebuilt one) is the only honest way to know
    // whether the user was sitting at the bottom or had scrolled up to read.
    const livePanel = document.querySelector(".primary-session-panel") as HTMLElement | null;
    if (livePanel) {
      streamingPanelScrollTop = livePanel.scrollTop;
      const distanceFromBottom = livePanel.scrollHeight - livePanel.clientHeight - livePanel.scrollTop;
      streamingStickToBottom = distanceFromBottom <= STREAM_STICK_THRESHOLD_PX;
    }
    if (activeBeforeRender) {
      streamingMemoRectsBySession[activeBeforeRender.name] = snapshotMemoStack();
    }
  }
  switch (currentView) {
    case "home":
      renderHome();
      break;
    case "recording":
      renderRecording();
      break;
    case "summary":
      renderSummary();
      break;
    case "processing":
      renderProcessing();
      break;
    case "review":
      renderReview();
      break;
    case "settings":
      settingsOverlayOpen = true;
      cueTierSelection = null;
      currentView = "home";
      renderHome();
      break;
  }

  syncSettingsHost(settingsSectionState, settingsModalScrollTop);
  mountMobileCaptureWaveform();
  hydrateStreamingNoteMotion(preserveStreamingScroll ? streamingPanelScrollTop : null);
}

// ---------------------------------------------------------------------------
// Home view
// ---------------------------------------------------------------------------

// Keep a single place to derive sidebar section state before rendering. This
// remains separate from the raw set so future one-off visibility rules don't
// leak into the rest of the app state.
function effectiveSidebarCollapsed(): Set<string> {
  return new Set(sidebarCollapsedSections);
}

function sidebarRenderContext(
  visibleSessions: SessionInfo[],
  projectSessions: Record<string, SessionInfo[]>,
  visibleProjectSessions: Record<string, SessionInfo[]>,
): SidebarRenderContext {
  return {
    sidebarCollapsed,
    sessions,
    visibleSessions,
    projectSessions,
    visibleProjectSessions,
    activeSessionName,
    activeSessionStatus: activeSessionName ? sessions.find(s => s.name === activeSessionName)?.status ?? null : null,
    sidebarFilterOpen,
    sessionFilters,
    globalSearchActive,
    globalSearchQuery,
    sidebarCollapsedSections: effectiveSidebarCollapsed(),
    settings,
    captureReady: captureReadyFromState(),
    calendarSuggestion,
    calendarSuggestionFreshness,
    sessionStatusLabel,
    renderSessionStatus,
    failedStartupSessionName: recordingStartup?.phase === "failed" ? recordingStartup.sessionName : null,
    noteErrorNames: noteErrorNames(),
    importAnnouncement,
    flashImportSessionName,
    flashDistilledSessionName,
    pendingImport,
    agentSetupCliInstall,
    agentSetupWaitingProjectId,
    recallIndexingProjectIds,
    refreshingProjectIds,
    copiedProjectPrompt,
    dismissedProjectSetupHints,
    vaultNoteCounts: new Map(vaultNoteCounts),
    granolaStatus,
    granolaBusy,
    granolaImportStage,
    showGranolaPlanCallout: granolaPlanCalloutVisible && !granolaPlanCalloutDismissed,
  };
}

function ensureMemoClockForRecordingSession(session: SessionInfo | null) {
  if (!session || session.status !== "recording" || memoStartTime > 0) return;
  const activeRecordingElapsed = recordingStatus.is_recording && (!recordingStatus.session_name || recordingStatus.session_name === session.name)
    ? recordingStatus.elapsed_secs
    : 0;
  const elapsedSecs = Math.max(0, activeRecordingElapsed || session.duration_secs || 0);
  if (elapsedSecs > 0) {
    memoStartTime = Date.now() - elapsedSecs * 1000;
    return;
  }
  const startedAt = Date.parse(session.start_time);
  memoStartTime = Number.isFinite(startedAt) ? startedAt : Date.now();
}

function activeStartupState(): RecordingStartupState | null {
  if (!recordingStartup) return null;
  if (activeSessionName && recordingStartup.sessionName !== activeSessionName) return null;
  return recordingStartup;
}

function displayElapsedSecs(): number {
  // Frozen while paused: pause is not recording time, so the clock must not tick.
  // Keyed on the synchronous pause intent (pausedAtWallMs), not the racy status flag.
  if (memoStartTime > 0 && pausedAtWallMs != null) {
    return Math.max(0, (pausedAtWallMs - memoStartTime) / 1000);
  }
  if (memoStartTime > 0 && (recordingStatus.is_recording || recordingStartup)) {
    return Math.max(0, (Date.now() - memoStartTime) / 1000);
  }
  return Math.max(0, recordingStatus.elapsed_secs || 0);
}

function recordingStatusLine(): string {
  const elapsed = formatDuration(displayElapsedSecs());
  const startup = activeStartupState();
  if (startup?.phase === "starting") return `Starting…`;
  if (startup?.phase === "failed") return `Not recording · draft kept open`;
  if (recordingStatus.capture_phase === "pausing") return `Saving audio · ${elapsed} · pause requested`;
  if (recordingStatus.capture_phase === "capturing_elsewhere") return `Capturing in another tab · ${elapsed}`;
  if (recordingStatus.capture_phase === "recording_unknown") return `Capture ownership unknown · ${elapsed}`;
  if (recordingStatus.capture_phase === "interrupted") return `Interrupted · ${elapsed} · take control to recover`;
  if (recordingStatus.capture_phase === "interrupted_recovery_empty") return `Interrupted · ${elapsed} · no durable audio received`;
  if (recordingStatus.capture_phase === "interrupted_recovery") return `Recovery control · ${elapsed} · received audio ready to finish`;
  if (recordingStatus.capture_phase === "needs_attention") return `Needs attention · ${elapsed} · audio was not fully saved`;
  if (recordingStatus.paused) return `Paused · ${elapsed} · saved locally`;
  if (transcriptDegraded) return `Recording · ${elapsed} · live transcript unavailable — audio still recording`;
  if (recordingStatus.web_live_pcm_status === "missing" || recordingStatus.web_live_pcm_status === "stale" || recordingStatus.web_live_pcm_status === "failed") {
    return `Recording · ${elapsed} · live transcript mic feed unavailable — audio still recording`;
  }
  if (transcriptWarming) return `Recording · ${elapsed} · transcript warming up`;
  return `Capturing · ${elapsed} · saved locally`;
}

function captureTransitionActive(): boolean {
  // Only block on a live in-progress pause (capture_phase "pausing").
  // WAV finalizing runs in the background and must NOT block a new capture.
  return Boolean(
    recordingStatus.capture_phase === "pausing"
  );
}

function capturePendingDraftForNextRender() {
  const textarea = document.getElementById("memo-input-new") as HTMLTextAreaElement | null;
  const editor = document.getElementById("memo-editor");
  const active = document.activeElement instanceof HTMLTextAreaElement
    && document.activeElement.closest("#memo-editor")
    ? document.activeElement
    : null;
  memoEditorRestore = editor ? {
    activeId: active?.id ?? null,
    selectionStart: active?.selectionStart ?? null,
    selectionEnd: active?.selectionEnd ?? null,
    selectionDirection: active?.selectionDirection ?? "none",
    scrollTop: editor.scrollTop,
  } : null;
  if (!textarea) return;
  // Pre-start prep has no recordingStatus session yet; its draft belongs to
  // prepSessionName. Using recording-only ownership here silently dropped the
  // draft on any prep hydration render.
  const sessionName = memoCaptureSessionName() || prepSessionName;
  if (!sessionName) return;
  pendingDraftRestore = textarea.value
    ? { sessionName, text: textarea.value, startSecs: pendingLineStartSecs }
    : null;
}

function restorePendingDraftAfterRender() {
  const textarea = document.getElementById("memo-input-new") as HTMLTextAreaElement | null;
  if (textarea && pendingDraftRestore) {
    const draft = pendingMemoDraftForSession(pendingDraftRestore, memoCaptureSessionName() || prepSessionName);
    pendingDraftRestore = null;
    if (draft) {
      textarea.value = draft.text;
      textarea.style.height = "auto";
      textarea.style.height = `${textarea.scrollHeight}px`;
      pendingLineStartSecs = draft.startSecs ?? (draft.text.trim() ? (Date.now() - memoStartTime) / 1000 : null);
      const gutterEl = document.getElementById("memo-gutter-pending");
      if (gutterEl && !isClockStopped()) {
        gutterEl.textContent = pendingLineStartSecs !== null ? formatElapsed(pendingLineStartSecs) : "";
      }
    }
  }

  const restore = memoEditorRestore;
  memoEditorRestore = null;
  if (!restore) return;
  const editor = document.getElementById("memo-editor");
  if (editor) editor.scrollTop = restore.scrollTop;
  if (!restore.activeId) return;
  const active = document.getElementById(restore.activeId) as HTMLTextAreaElement | null;
  if (!active) return;
  active.focus({ preventScroll: true });
  if (restore.selectionStart !== null && restore.selectionEnd !== null) {
    const max = active.value.length;
    try {
      active.setSelectionRange(
        Math.min(restore.selectionStart, max),
        Math.min(restore.selectionEnd, max),
        restore.selectionDirection,
      );
    } catch { /* ignore */ }
  }
}

function memoCaptureSessionName(): string {
  return (recordingStatus.session_name || recordingStartup?.sessionName || "").trim();
}

/**
 * Session that owns the current clock-stopped block's prep hydration. During a
 * real recording (mid-meeting pause), that's the active capture session — NOT
 * prepSessionName, which is only seeded on the pre-start calendar/prep path and
 * is empty for immediate-record captures. Pre-start prep falls back to prepSessionName.
 */
function activePrepSessionName(): string {
  const active = memoCaptureSessionName();
  return active || prepSessionName;
}

function clearPendingMemoDraft(sessionName?: string) {
  if (!sessionName || pendingDraftRestore?.sessionName === sessionName) {
    pendingDraftRestore = null;
  }
  if (sessionName && memoCaptureSessionName() !== sessionName) return;
  const textarea = document.getElementById("memo-input-new") as HTMLTextAreaElement | null;
  if (textarea) textarea.value = "";
  pendingLineStartSecs = null;
}

function renderHome() {
  const sortedSessions = sortSessionsForSidebar(sessions);
  const currentProjectId = activeProjectId();
  if (currentProjectId) projectSessionsById[currentProjectId] = sessions;
  if (!activeSessionName && sortedSessions.length > 0) {
    activeSessionName = sortedSessions[0].name;
    activeSessionTab = defaultTabForSession(sortedSessions[0]);
  }

  const active = activeSessionName ? sessions.find(s => s.name === activeSessionName) ?? null : null;
  ensureMemoClockForRecordingSession(active);
  const visibleSessions = filterSessionsForSidebar(sortedSessions, sessionFilters);
  const projectSessions = Object.fromEntries(
    normalizeProjects(settings).map(project => {
      const raw = project.id === currentProjectId ? sessions : projectSessionsById[project.id] || [];
      return [project.id, sortSessionsForSidebar(raw)];
    })
  ) as Record<string, SessionInfo[]>;
  const visibleProjectSessions = Object.fromEntries(
    Object.entries(projectSessions).map(([projectId, projectItems]) => [
      projectId,
      filterSessionsForSidebar(projectItems, sessionFilters),
    ])
  ) as Record<string, SessionInfo[]>;

  app.innerHTML = `
    <div class="workspace-shell ${sidebarCollapsed ? "sidebar-collapsed" : ""}" style="--sidebar-width: ${sidebarWidth}px">
      ${renderWorkspaceTopbar()}
      ${renderSidebar(sidebarRenderContext(visibleSessions, projectSessions, visibleProjectSessions))}
      <button class="sidebar-scrim" type="button" aria-label="Close navigation" onclick="window.__toggleSidebar()"></button>

      <main class="workspace-main">
        ${isPreStartPrepOpen() ? renderPreStartPrepWorkspace() : active ? renderSessionWorkspace(active) : renderHomeEmptyState()}
      </main>
      ${renderJobBanner()}
    </div>
    ${renderSettingsOverlayIfOpen()}
    ${renderModelNotice(modelProvisionNotice)}
  `;

  restoreSidebarSearchFocus();

  setTimeout(() => {
    // Don't mount/focus the recording memo editor behind an open settings
    // modal — the modal owns focus and the editor re-mounts when it closes.
    if (settingsOverlayOpen) return;
    if (isPreStartPrepOpen() || (active?.status === "recording" && activeSessionTab === "backchannel")) {
      recordingMounted = true;
      attachMemoEditHandlers();
      appendNewMemoInput();
      restorePendingDraftAfterRender();
      updateRecordingHeader();
    }
  }, 0);
}

function isPreStartPrepOpen(): boolean {
  return currentView === "home"
    && preStartPrepOpen
    && !hostedCaptureBlocksStart()
    && !recordingStartup;
}

function preStartCountdownText(): string | null {
  if (!armedFromCalendar || !autoStartDeadline || settings.auto_start_from_calendar === false) return null;
  const remaining = autoStartDeadline - Date.now();
  if (remaining <= 0) return null;
  const totalSecs = Math.ceil(remaining / 1000);
  const mins = Math.floor(totalSecs / 60);
  const secs = totalSecs % 60;
  return `Starts when the meeting does · ${mins}:${String(secs).padStart(2, "0")}`;
}

function abandonPreStartPrep() {
  clearAutoStartTimer();
  armedFromCalendar = false;
  preStartPrepOpen = false;
  prepProjectId = null;
  prepStartBlockedReason = null;
  autoStartDeadline = null;
  resetPrepBlockState();
  clearPendingMemoDraft();
  memoLines = [];
  memoStartTime = 0;
  recordingMounted = false;
}

function renderPreStartPrepWorkspace(): string {
  return `
    <div class="session-workspace recording-workspace prep-workspace">
      <header class="session-header">
        <div class="session-header-main">
          <div class="session-kicker">Upcoming meeting</div>
          <div class="session-title-display">${esc(prepEventTitle || prepSessionName || "Untitled")}</div>
          <div class="session-title-meta">Ready to prepare</div>
        </div>
        <div class="session-header-actions">${renderGlobalHeaderActions()}</div>
      </header>
      <div class="session-tab-panel primary-session-panel">
        ${renderLiveBackchannel(liveBackchannelRenderContext(true))}
      </div>
    </div>
  `;
}

function renderHomeEmptyState(): string {
  const project = activeProject(settings);
  // The empty home has no session workspace, so its header actions (theme +
  // Settings gear) would otherwise be missing here — leaving no way to reach
  // Settings on a clean first run. Render them top-right so Settings is always
  // one click away (⌘, also opens it).
  return `
    <div class="home-empty-actions">${renderGlobalHeaderActions()}</div>
    ${renderHomeIntro({
      captureReady: captureReadyFromState(),
      noteChoiceMade: Boolean(firstCaptureNoteChoice),
      noteChoice: firstCaptureNoteChoice,
      contextReady: project.readiness === "ready",
      projectName: project.name || "Margins",
    })}`;
}

function renderProjectSetupGate(project: ProjectSource): string {
  const quickRunning = indexingProjectIds.has(project.id) || project.readiness === "updating";
  const waitingForAgent = agentSetupWaitingProjectId === project.id;
  const copiedSetup = copiedProjectPrompt?.projectId === project.id && copiedProjectPrompt.mode === "setup";
  const copiedFix = copiedProjectPrompt?.projectId === project.id && copiedProjectPrompt.mode === "fix";
  const hasFailed = project.readiness === "error";
  const heading = hasFailed ? "Note memory needs attention" : `Help Margins learn "${project.name}"`;
  const body = hasFailed
    ? "Your notes weren't changed. Try quick setup again, or copy a prompt to diagnose note memory with your agent."
    : "Margins can learn your notes privately on this Mac so future writeups remember the right people, decisions, and follow-ups.";
  const eyebrow = hasFailed ? "Note memory" : "Optional note memory";
  const installingCli = agentSetupCliInstall.state === "installing" && agentSetupCliInstall.projectId === project.id;
  const readyToCopy = agentSetupCliInstall.state === "ready" && agentSetupCliInstall.projectId === project.id;
  const cliInstallError = agentSetupCliInstall.state === "error" && agentSetupCliInstall.projectId === project.id
    ? agentSetupCliInstall.message
    : null;
  const agentLabel = installingCli
    ? "Installing CLI…"
    : copiedSetup
      ? "Copied — paste into your agent to finish setup"
      : readyToCopy
        ? "Copy setup prompt — paste into your agent"
        : "Set up with your agent";
  const fixLabel = copiedFix ? "Copied — paste into your agent" : "Copy prompt to fix setup";
  return `
    <div class="project-setup-panel">
      <div class="project-setup-header">
        <div class="eyebrow">${esc(eyebrow)}</div>
        <h1>${esc(heading)}</h1>
        <p>${esc(body)}</p>
      </div>
      ${quickRunning ? `
        <div class="project-setup-status">
          <span class="project-setup-spinner" aria-hidden="true"></span>
          <span>Learning your notes... this stays on your device.</span>
        </div>
      ` : waitingForAgent ? `
        <div class="project-setup-status">
          <span class="project-setup-spinner" aria-hidden="true"></span>
          <span>Waiting for your agent — Margins will detect when it's done.</span>
        </div>
      ` : ""}
      <div class="project-setup-options">
        <button class="project-setup-option primary-option" title="Copies a prompt you paste into your AI agent. Margins first verifies the CLI, then the prompt tells your agent to read the embedded setup guide and initialize Margins search. Your notes aren't changed." onclick="window.__copyProjectHelpPrompt(${js(project.id)}, 'setup')" ${quickRunning || installingCli ? "disabled" : ""}>
          <span class="project-setup-option-title">
            ${installingCli ? `<span class="project-setup-spinner" aria-hidden="true"></span>` : ""}
            <span>${esc(agentLabel)}</span>
            <span class="project-setup-badge">Recommended</span>
          </span>
          <span class="project-setup-option-copy">Reviews how your notes are organized, sets up Margins search to match, and can explain any suggested structure.</span>
        </button>
        ${cliInstallError ? `<p class="project-setup-note project-setup-error" role="alert">Could not prepare the setup prompt: ${esc(cliInstallError)}</p>` : ""}
        <button class="project-setup-option secondary-option" title="Margins initializes search directly using its defaults — the fastest way to start. You can run the agent review anytime afterward." onclick="window.__quickSetupProject(${js(project.id)})" ${quickRunning || installingCli ? "disabled" : ""}>
          <span class="project-setup-option-title">Quick setup</span>
          <span class="project-setup-option-copy">Set up right now with sensible defaults. No agent needed.</span>
        </button>
      </div>
      ${hasFailed ? `
        <div class="project-setup-fix">
          <button class="subtle" onclick="window.__copyProjectHelpPrompt(${js(project.id)}, 'fix')">${esc(fixLabel)}</button>
        </div>
      ` : ""}
      <div class="project-setup-footer">
        <span>Capture can work before note memory is ready.</span>
        <details>
          <summary>What's the difference?</summary>
          <p>Quick setup gets you capturing in seconds with a general-purpose index. Agent setup does the same, plus it looks at your actual folders and naming so search fits your workspace — and it can explain what it did. You can switch to the agent review later either way.</p>
        </details>
      </div>
    </div>
  `;
}

function restoreSidebarSearchFocus() {
  // Keep focus in the cross-project search box across re-renders.
  if (sidebarSearchWasFocused) {
    const el = document.getElementById("sidebar-global-search-input") as HTMLInputElement | null;
    if (el && document.activeElement !== el) {
      el.focus();
      const len = el.value.length;
      try { el.setSelectionRange(len, len); } catch { /* ignore */ }
    }
  }
  // When the global search view first opens, move focus into the input.
  if (globalSearchActive && !sidebarSearchWasFocused) {
    const el = document.getElementById("sidebar-global-search-input") as HTMLInputElement | null;
    if (el && document.activeElement?.tagName !== "INPUT") el.focus();
  }
}

function sessionNameFromSuggestion(suggestion: CalendarEventSuggestion): string {
  // Always prefix with the capture (creation) time, not the event's start time,
  // so re-capturing the same calendar event never collides on filename and the
  // sidebar orders by when the note was actually taken. The event title becomes
  // the descriptive suffix; `calendar_event_title` drives the primary-color label.
  const base = `${timestampName()} ${suggestion.title}`.replace(/\.md$/i, "");
  return slugifyName(base);
}

function defaultMeetingName(): string {
  return timestampName();
}

function timestampName(date = new Date()): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  const h = String(date.getHours()).padStart(2, "0");
  const min = String(date.getMinutes()).padStart(2, "0");
  return `${y}-${m}-${d}-${h}-${min}`;
}

function sessionWorkspaceRenderContext(s: SessionInfo): SessionWorkspaceRenderContext {
  const captureNote = isCaptureNote(s);
  const { date, description } = sessionTitleParts(s, normalizeSidebarDateFormat(settings.sidebar_date_format));
  const processing = !noteErrorForSession(s) && (s.status === "processing" || processingSessionName === s.name);
  const settledNote = Boolean(artifactCacheForSession(s).note || (s.status === "synthesized" && groundedNoteForSession(s).blocks.length)) && !processing;
  return {
    session: s,
    title: sessionTitle(s),
    titleDescription: description,
    dateLabel: date,
    captureNote,
    savedNoteTitleMeta: friendlyVaultPath(s.vault_note_path, s.name).replace(/^Saved to\s+/, ""),
    obsidianVaultReady,
    settledNote,
    liveCaptureMeta: s.status === "recording" && recordingStatus.session_name === s.name
      ? recordingStatus.capture_phase === "capturing_elsewhere"
        ? "Capturing in another browser tab"
        : recordingStatus.capture_phase === "recording_unknown"
          ? "Capture ownership unavailable — use the original tab"
          : recordingStatus.capture_phase === "interrupted"
            ? "Browser capture interrupted — take control to recover"
            : recordingStatus.capture_phase?.startsWith("interrupted_recovery")
              ? "Browser capture interrupted — recovery control acquired"
        : recordingStatus.web_live_pcm_status === "missing" || recordingStatus.web_live_pcm_status === "stale" || recordingStatus.web_live_pcm_status === "failed"
                ? "Audio recording · live transcript feed unavailable"
                : "Capturing in this browser"
      : undefined,
    peopleSuggestions: peopleCandidatesForSession(s.name),
    globalActionsHtml: renderGlobalHeaderActions(),
    primaryPanelHtml: s.status === "recording" ? renderLiveBackchannel(liveBackchannelRenderContext()) : renderDistillTab(s),
  };
}

function peopleCandidatesForSession(sessionName: string): string[] {
  const activeSession = sessions.find(s => s.name === sessionName);
  const activePeople = new Set((activeSession ? sessionFrontmatterPeople(activeSession) : []).map(normalizePersonLookupKey));
  const candidates: string[] = [];
  for (const session of sessions) {
    for (const person of [...(session.people || []), ...sessionFrontmatterPeople(session)]) {
      const cleaned = person.trim();
      if (!cleaned || activePeople.has(normalizePersonLookupKey(cleaned))) continue;
      if (!candidates.some(existing => normalizePersonLookupKey(existing) === normalizePersonLookupKey(cleaned))) {
        candidates.push(cleaned);
      }
      if (candidates.length >= 50) return candidates;
    }
  }
  return candidates;
}

function normalizePersonLookupKey(person: string): string {
  return person
    .trim()
    .normalize("NFKD")
    .replace(/\p{Diacritic}/gu, "")
    .toLowerCase();
}

function peopleForSessionEdit(session: SessionInfo | undefined): string[] {
  return session ? sessionFrontmatterPeople(session) : [];
}

function uniquePeople(people: string[]): string[] {
  const seen = new Set<string>();
  const cleaned: string[] = [];
  for (const person of people) {
    const trimmed = person.trim();
    const key = normalizePersonLookupKey(trimmed);
    if (!key || seen.has(key)) continue;
    seen.add(key);
    cleaned.push(trimmed);
  }
  return cleaned;
}

function resolvePeopleCandidate(sessionName: string, raw: string): string {
  const typed = raw.trim();
  const typedKey = normalizePersonLookupKey(typed);
  if (!typedKey) return typed;
  const candidates = peopleCandidatesForSession(sessionName);
  const exact = candidates.find(candidate => normalizePersonLookupKey(candidate) === typedKey);
  if (exact) return exact;
  const prefix = candidates.find(candidate => normalizePersonLookupKey(candidate).startsWith(typedKey));
  if (prefix) return prefix;
  const close = candidates.find(candidate => isOneEditAway(typedKey, normalizePersonLookupKey(candidate)));
  return close || typed;
}

function isOneEditAway(a: string, b: string): boolean {
  if (a === b) return true;
  if (Math.abs(a.length - b.length) > 1) return false;
  let edits = 0;
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      i += 1;
      j += 1;
    } else {
      edits += 1;
      if (edits > 1) return false;
      if (a.length > b.length) i += 1;
      else if (b.length > a.length) j += 1;
      else {
        i += 1;
        j += 1;
      }
    }
  }
  return edits + (i < a.length ? 1 : 0) + (j < b.length ? 1 : 0) <= 1;
}

function liveBackchannelRenderContext(preStart = false): LiveBackchannelRenderContext {
  const settingsReady = settingsReadyFromState();
  return {
    elapsedLabel: formatDuration(displayElapsedSecs()),
    statusLine: preStart ? "Ready · audio not started" : recordingStatusLine(),
    capturePhase: recordingStatus.capture_phase,
    paused: recordingStatus.paused ?? false,
    settingsReady,
    currentAiMode: currentAiMode(),
    stopLabel: "Pause",
    stopHint: settingsReady
      ? "Finish from the paused state when the conversation ends."
      : "Capture will be saved. Sign in to the note-making AI before writing the note.",
    stopShortcutHint: captureShortcutHints().stop,
    speakerCount: captureSpeakerCount,
    audioHealthHtml: renderAudioHealth(recordingRenderContext()),
    recordingStatus,
    startupState: activeStartupState(),
    memoLines,
    renderMemoLine: (line, index) => renderMemoLine(line, index, renderMemoBackchannelButton, (o) => blockWallClockByOrdinal.get(o)),
    backchannelRailHtml: renderBackchannelAssistRail(),
    clockStopped: isClockStopped(),
    blockMarginRailHtml: isClockStopped() ? blockMarginRailHtml(currentBlockOrdinal) : undefined,
    preStart,
    autoStartCountdownText: preStart ? preStartCountdownText() : null,
    prepStartBlockedReason: preStart ? prepStartBlockedReason : null,
    mobileCapture: isMobileCaptureView(),
  };
}

function renderSessionWorkspace(s: SessionInfo): string {
  return renderSessionWorkspaceView(sessionWorkspaceRenderContext(s));
}

function canOpenNotePanel(s: SessionInfo): boolean {
  return Boolean(artifactCacheForSession(s).note || groundedNoteForSession(s).blocks.length || s.vault_note_path || s.status === "synthesized" || s.status === "processing" || s.status === "failed" || processingSessionName === s.name);
}

function jobBannerRenderContext(): JobBannerRenderContext | null {
  const jobName = processingSessionName || sessions.find(s => s.status === "processing")?.name || Object.keys(noteJobErrorBySession)[0] || sessions.find(s => noteErrorForSession(s))?.name || "";
  if (!jobName) return null;
  if (currentView === "home" && activeSessionName === jobName && !noteErrorForSession(jobName)) return null;
  const storedError = noteErrorForSession(jobName);
  const session = sessions.find(s => s.name === jobName);
  const latestEvent = processingSteps[processingSteps.length - 1];
  const errorEvent = storedError ? { message: storedError } : latestEvent?.stage === "error" ? latestEvent : null;
  const latest = processingSteps[processingSteps.length - 1];
  const lifecycleJob = deriveSessionJobState(lifecycleBySession[jobName]);
  const progress = errorEvent ? currentProgress : Math.max(currentProgress || 0.08, sessions.find(s => s.name === jobName)?.status === "processing" ? 0.38 : 0);
  const message = errorEvent
    ? (storedError || errorEvent.message).replace(/^Error:\s*(Error:\s*)?/, "")
    : lifecycleJob.message || (latest?.stage === "note_stream" ? "Writing note…" : latest?.message || "Writing a note from the recording and marks...");
  return { jobName, error: Boolean(errorEvent), progress, message, errorTitle: errorEvent ? noteFailureHeadlineForSession(session, message) : undefined };
}

function noteFailureHeadlineForSession(session: SessionInfo | undefined, message: string): string {
  const timedOut = /timed?\s*out|timeout/i.test(message);
  if (session?.failed_stage === "transcribe") return timedOut ? "Transcription timed out" : "Transcription failed";
  if (session?.failed_stage === "distill") return timedOut ? "Note generation timed out" : "Note generation failed";
  if (session?.failed_stage === "save") return "Note save failed";
  return "Couldn’t finish the note";
}

function renderJobBanner(): string {
  return renderJobBannerView(jobBannerRenderContext());
}

function distillTabRenderContext(s: SessionInfo): DistillTabRenderContext {
  return {
    session: s,
    cache: artifactCacheForSession(s),
    processing: !noteErrorForSession(s) && (s.status === "processing" || processingSessionName === s.name),
    captureNote: isCaptureNote(s),
    settingsReady: settingsReadyFromState(),
    currentAiMode: currentAiMode(),
    friendlyVaultPath: friendlyVaultPath(s.vault_note_path, s.name),
    obsidianVaultReady,
    contextReady: activeProject(settings).readiness === "ready",
    currentProgress,
    groundedState: groundedNoteForSession(s),
    memoLines: memoLinesForSession(s.name),
    // Raw trace (may be empty): the distill view substitutes the all-"done"
    // default only for a settled note's provenance, never while streaming —
    // where an empty trace must read as in-progress, not as finished.
    trace: distillTraceForSessionInfo(s),
    followupChat: followupChatBySession[s.name] || [],
    renderMarkdown,
    noteError: noteErrorForSession(s),
    cancelled: Boolean(noteCancelledBySession[s.name]),
    finalizingCapture: finalizingCaptureSessionName === s.name,
    jobState: deriveSessionJobState(lifecycleBySession[s.name]),
    captureHealth: (captureHealthBySession[s.name] || []).map(e => ({ atSecs: e.atSecs, label: e.label })),
    reprocessCandidates: reprocessCandidatesForSession(s),
    showFirstNotePayoff: firstNoteCelebrated,
  };
}

function renderDistillTab(s: SessionInfo): string {
  const stateKey = sessionStateKeyForSession(s);
  const cache = artifactCache[stateKey] || {};
  const processing = !noteErrorForSession(s) && (s.status === "processing" || processingSessionName === s.name);
  if (cache.note && !processing) {
    const parsed = groundedStateFromMarkdown(cache.note);
    const existingUses = (groundedNoteBySession[stateKey]?.blocks || []).flatMap(block => block.uses);
    groundedNoteBySession[stateKey] = existingUses.length && !parsed.blocks.some(block => block.uses.length)
      ? mergeGroundingUsesIntoState(parsed, existingUses)
      : parsed;
    groundedNoteBySession[stateKey] = mergeVaultRefsIntoState(
      groundedNoteBySession[stateKey],
      vaultRefsFromTrace(distillTraceBySession[stateKey] || []),
    );
    reviewContent = cleanVisibleMarkdown(cache.note);
    reviewSessionName = s.name;
  } else if (processing || groundedNoteBySession[stateKey]?.blocks.length) {
    const state = ensureGroundedNoteState(s.name);
    reviewSessionName = s.name;
    reviewContent = groundedNoteVisibleMarkdown(state);
  }

  return renderDistillTabView(distillTabRenderContext(s));
}

function traceForSession(sessionName: string): DistillTraceEvent[] {
  const trace = distillTraceBySession[sessionStateKey(sessionName)];
  return trace?.length ? trace : defaultDistillTrace();
}

function rememberProcessingTrace(sessionName: string, event: ProcessingEvent) {
  rememberProcessingTraceForStateKey(sessionStateKey(sessionName), event);
}

function rememberProcessingTraceForStateKey(stateKey: string, event: ProcessingEvent) {
  distillTraceBySession[stateKey] = appendProcessingTrace(distillTraceBySession[stateKey] || [], event);
}

function rememberLifecycleEvent(sessionName: string, event: ProcessingEvent) {
  lifecycleBySession[sessionName] = reduceSessionLifecycle(lifecycleBySession[sessionName], event, sessionName);
}

function rememberNoteWritingTrace(sessionName: string, progress: number | null) {
  rememberNoteWritingTraceForStateKey(sessionStateKey(sessionName), sessionName, progress);
}

function rememberNoteWritingTraceForStateKey(stateKey: string, sessionName: string, progress: number | null) {
  const trace = distillTraceBySession[stateKey] || [];
  if (trace.some(event => event.label === "Write note")) return;
  distillTraceBySession[stateKey] = appendProcessingTrace(trace, {
    stage: "note_stream",
    message: "Writing the connected note into the document.",
    progress,
    session: sessionName,
    track: "note",
    phase: "writing",
  });
}

let recordingMounted = false;
let pendingLineStartSecs: number | null = null;
let pendingDraftRestore: PendingMemoDraft | null = null;
interface MemoEditorRestore {
  activeId: string | null;
  selectionStart: number | null;
  selectionEnd: number | null;
  selectionDirection: "forward" | "backward" | "none";
  scrollTop: number;
}
let memoEditorRestore: MemoEditorRestore | null = null;

interface HostedActiveWorkspaceSnapshot {
  status: RecordingStatus;
  memoLines: MemoLine[];
  pendingDraft: PendingMemoDraft | null;
  editorRestore: MemoEditorRestore | null;
  activeSessionName: string | null;
  activeSessionTab: SessionTab;
  currentView: View;
  session: SessionInfo | null;
  lifecycle: SessionLifecycleState | null;
  currentBlockOrdinal: number;
  memoStartTime: number;
  pausedAtWallMs: number | null;
  pendingLineStartSecs: number | null;
}

interface HostedFinishedArtifact {
  recordingId: string;
  sessionName: string;
  projectId: string | null;
  memoLines: MemoLine[];
}

const hostedRecoveryWorkspace = new HostedRecoveryWorkspaceStore<
  HostedActiveWorkspaceSnapshot,
  HostedFinishedArtifact
>();

function snapshotHostedActiveWorkspaceBeforeRecovery(recoveryRecordingId: string): void {
  const active = hostedActiveRecordingStatus;
  const activeRecordingId = active?.web_recording_id;
  if (!activeRecordingId
    || activeRecordingId === recoveryRecordingId
    || recordingStatus.web_recording_id !== activeRecordingId) return;

  capturePendingDraftForNextRender();
  hostedRecoveryWorkspace.rememberActive(activeRecordingId, {
    status: { ...active },
    memoLines: memoLines.map(line => ({ ...line })),
    pendingDraft: pendingDraftRestore ? { ...pendingDraftRestore } : null,
    editorRestore: memoEditorRestore ? { ...memoEditorRestore } : null,
    activeSessionName,
    activeSessionTab,
    currentView,
    session: active.session_name
      ? sessions.find(session => session.name === active.session_name) ?? null
      : null,
    lifecycle: active.session_name ? lifecycleBySession[active.session_name] ?? null : null,
    currentBlockOrdinal,
    memoStartTime,
    pausedAtWallMs,
    pendingLineStartSecs,
  });
  // These restorers now belong to B's exact-ID snapshot. Leaving them global
  // would inject B's draft/focus into A on the selection render.
  pendingDraftRestore = null;
  memoEditorRestore = null;
}

function recordCaptureHealth() {
  if (!recordingStatus.is_recording || recordingStatus.paused) return;
  const name = recordingStatus.session_name;
  if (!name) return;
  const atSecs = recordingStatus.elapsed_secs || 0;
  const tap = recordingStatus.tap_status || "ok";
  const prevTap = lastTapStatusBySession[name];
  if (tap !== prevTap) {
    lastTapStatusBySession[name] = tap;
    const log = (captureHealthBySession[name] ??= []);
    if (tap === "dead") log.push({ atSecs, kind: "tap_dropped", label: "Computer audio dropped" });
    else if ((tap === "connecting" || tap === "blocked") && prevTap !== undefined) log.push({ atSecs, kind: "tap_waiting", label: tap === "blocked" ? "Computer audio blocked" : "Waited for computer audio" });
    else if (tap === "ok" && (prevTap === "dead" || prevTap === "silent")) log.push({ atSecs, kind: "tap_recovered", label: "Computer audio recovered" });
  }
  const mode = recordingStatus.live_transcription_mode || "stereo_split";
  const prevMode = lastLiveModeBySession[name];
  if (mode !== prevMode) {
    lastLiveModeBySession[name] = mode;
    if (mode === "mic_diarized" && prevMode !== undefined) (captureHealthBySession[name] ??= []).push({ atSecs, kind: "mic_diarized", label: "Switched to mic audio" });
  }
}

function recordingRenderContext(): RecordingRenderContext {
  const clockStopped = isClockStopped();
  // Build countdown text for auto-start footer
  let autoStartCountdownText: string | null = null;
  if (clockStopped && armedFromCalendar && autoStartDeadline && settings.auto_start_from_calendar !== false) {
    const remaining = autoStartDeadline - Date.now();
    if (remaining > 0) {
      const totalSecs = Math.ceil(remaining / 1000);
      const mins = Math.floor(totalSecs / 60);
      const secs = totalSecs % 60;
      const mmss = `${mins}:${String(secs).padStart(2, "0")}`;
      autoStartCountdownText = `Starts when the meeting does · ${mmss}`;
    }
  }
  // Clock-stopped pre-start (not yet recording): seed a display title from the
  // calendar/prep event so the header isn't "Untitled", and show a stopped 0:00.
  const preStart = clockStopped && !hostedCaptureBlocksStart();
  const statusLine = preStart ? "Ready · 0:00 · clock stopped" : recordingStatusLine();
  const displayStatus = preStart
    ? { ...recordingStatus, session_name: recordingStatus.session_name || prepEventTitle || prepSessionName || "Untitled" }
    : recordingStatus;
  return {
    devices,
    recordingStatus: displayStatus,
    statusLine,
    startupState: activeStartupState(),
    memoLines,
    selectedDeviceName: selectedDeviceName(),
    settingsReady: settingsReadyFromState(),
    dismissedTapNoticeSession,
    tapRecoverySession,
    tapRecoveryState,
    tapRecoveryError,
    captureDeviceSwitchToast,
    backchannelRailHtml: clockStopped ? undefined : renderBackchannelAssistRail(),
    memoBackchannelButtonHtml: clockStopped ? undefined : renderMemoBackchannelButton,
    clockStopped,
    autoStartCountdownText,
    blockMarginRailHtml: clockStopped ? blockMarginRailHtml(currentBlockOrdinal) : undefined,
    blockWallClock: (ordinal) => blockWallClockByOrdinal.get(ordinal),
    mobileCapture: isMobileCaptureView(),
  };
}

function renderMemoBackchannelButton(index: number): string {
  if (backchannelErrorByMemo[index]) {
    return `<button class="memo-comment-button error" title="Cue unavailable — retry" onclick="window.__retryBackchannelForMemo(${index})">⚠</button>`;
  }
  const card = backchannelCardsByMemo[index];
  if (!card) return "";
  const label = card.collapsed ? "Show suggested cue for this mark" : "Hide suggested cue";
  return `<button class="memo-comment-button ${card.collapsed ? "collapsed" : "active"}" title="${label}" aria-label="${label}" onclick="window.__showBackchannelSuggestion(${index})">${iconSvg(MessageCircle, "ui-icon button-icon")}</button>`;
}

function renderBackchannelAssistRail(): string {
  const cards = visibleBackchannelCards();
  if (cards.length > 0) {
    const stackClass = cards.length > 1 ? "stacked" : "single";
    return `
      <margins class="backchannel-assist-rail" aria-label="Backchannel assist">
        <ol class="backchannel-assist-stack ${stackClass}">
          ${cards.map(renderBackchannelAssistCard).join("")}
        </ol>
      </margins>
    `;
  }
  const errIdx = latestBackchannelErrorIndex();
  if (errIdx !== null) {
    const err = backchannelErrorByMemo[errIdx];
    return `
      <margins class="backchannel-assist-rail" aria-label="Backchannel assist">
        <div class="backchannel-assist-card error">
          <div class="backchannel-assist-anchor">${esc(err.memo_time)} mark</div>
          <div class="backchannel-assist-title">Cue unavailable</div>
          <div class="backchannel-assist-suggestion">Margins could not fetch a live cue. Your mark is saved.</div>
          <div class="backchannel-assist-actions">
            <button onclick="window.__retryBackchannelForMemo(${errIdx})">Retry</button>
            <button onclick="window.__dismissBackchannelError(${errIdx})">Dismiss</button>
          </div>
        </div>
      </margins>
    `;
  }
  const termIdx = latestBackchannelTerminalIndex();
  if (termIdx !== null) {
    const term = backchannelTerminalByMemo[termIdx];
    // A cue was heard but produced no card. Never leave it silent: one calm,
    // jargon-free line so the user knows the mark landed. Warming = self-heal in
    // progress and retriable; quiet = deliberately nothing worth surfacing.
    const anchor = term.memo_time ? `${esc(term.memo_time)} mark` : "Your mark";
    const line = term.status === "warming"
      ? "Getting set up — ask again in a moment."
      : term.status === "unavailable"
        ? `${esc(term.reason ?? "Recall unavailable")} — ${esc(term.hint ?? "run margins setup")}. Your marks are still saved.`
        : "Heard. Nothing worth pulling you out for.";
    return `
      <margins class="backchannel-assist-rail" aria-label="Backchannel assist">
        <div class="backchannel-assist-card ${term.status}" aria-live="polite">
          <div class="backchannel-assist-anchor">${anchor}</div>
          <div class="backchannel-assist-status">${line}</div>
        </div>
      </margins>
    `;
  }
  return "";
}

function visibleBackchannelCards(): BackchannelCard[] {
  return Object.values(backchannelCardsByMemo)
    .filter(card => !card.collapsed)
    .sort((a, b) => (b.received_at || 0) - (a.received_at || 0))
    .slice(0, 3);
}

function latestBackchannelCardIndex(): number | null {
  const indices = Object.keys(backchannelCardsByMemo).map(Number).filter(n => Number.isFinite(n));
  if (!indices.length) return null;
  return indices.sort((a, b) => (backchannelCardsByMemo[b].received_at || 0) - (backchannelCardsByMemo[a].received_at || 0))[0];
}

function latestBackchannelErrorIndex(): number | null {
  const indices = Object.keys(backchannelErrorByMemo).map(Number).filter(n => Number.isFinite(n));
  if (!indices.length) return null;
  return indices.sort((a, b) => (backchannelErrorByMemo[b].received_at || 0) - (backchannelErrorByMemo[a].received_at || 0))[0];
}

// Latest heard-but-cardless outcome (warming or quiet) so the rail can show a
// calm one-line acknowledgment instead of going silent.
function latestBackchannelTerminalIndex(): number | null {
  const indices = Object.keys(backchannelTerminalByMemo).map(Number).filter(n => Number.isFinite(n));
  if (!indices.length) return null;
  return indices.sort((a, b) => (backchannelTerminalByMemo[b].received_at || 0) - (backchannelTerminalByMemo[a].received_at || 0))[0];
}

function renderBackchannelAssistCard(card: BackchannelCard): string {
  const pending = card.status === "pending";
  const isPlaceholder = card.placeholder && !card.suggestion && !card.title;
  if (isPlaceholder) {
    return `
      <li class="backchannel-assist-card pending placeholder">
        <div class="backchannel-assist-anchor">${esc(card.memo_time)} mark · drafting</div>
        <div class="backchannel-skeleton" aria-hidden="true">
          <span class="skeleton-line skeleton-line-title"></span>
          <span class="skeleton-line"></span>
          <span class="skeleton-line skeleton-line-short"></span>
        </div>
        <div class="visually-hidden" aria-live="polite">Drafting a live cue…</div>
      </li>
    `;
  }
  const direction = card.direction || card.title || (pending ? "finding the angle" : suggestionTypeLabel(card.kind || card.state));
  const question = card.suggestion || "Cue available.";
  const steering = Boolean(card.steering);
  const canSteer = !pending && Boolean(card.suggestion);
  const draft = backchannelSteerDraftByMemo[card.memo_index] || "";
  return `
    <li class="backchannel-assist-card ${pending ? "pending" : ""} ${steering ? "steering" : ""}">
      <div class="backchannel-assist-anchor">${esc(card.memo_time)} mark${pending ? " · drafting" : ""}${steering ? " · steering" : ""}</div>
      <div class="backchannel-assist-direction">${esc(direction)}</div>
      <div class="backchannel-assist-question">${esc(question)}</div>
      <div class="backchannel-assist-actions">
        ${pending ? "" : `<button onclick="window.__copyBackchannelSuggestion(${card.memo_index})">Copy</button>`}
        <button onclick="window.__collapseBackchannelSuggestion(${card.memo_index})">Dismiss</button>
      </div>
      ${canSteer ? `
      <form class="backchannel-steer" onsubmit="return window.__steerBackchannelForMemo(${card.memo_index})">
        <input class="backchannel-steer-input" type="text" autocomplete="off"
          placeholder="${steering ? "Regenerating…" : "Steer this cue — e.g. less salesy, ask about timeline"}"
          value="${esc(draft)}" ${steering ? "disabled" : ""}
          oninput="window.__updateBackchannelSteerDraft(${card.memo_index}, this.value)" />
        <button type="submit" ${steering ? "disabled" : ""}>${steering ? "…" : "Steer"}</button>
      </form>
      ` : ""}
    </li>
  `;
}

function suggestionTypeLabel(kind: string): string {
  switch (kind) {
    case "unresolved_thread": return "unresolved thread";
    case "proof_debt": return "proof over claim";
    case "relationship_obligation": return "relationship debt";
    case "decision_pressure": return "decision pressure";
    case "risk": return "risk to name";
    case "pattern_to_test": return "pattern to test";
    case "ask_specifics": return "specifics over story";
    case "clarify_next_step": return "toward next step";
    default: return "Cue";
  }
}

// ---------------------------------------------------------------------------
// Clock-stopped surface (prep block 0 + mid-meeting pauses)
// One view, two postures. All prep/pause logic lives here.
// ---------------------------------------------------------------------------

/**
 * True when the capture surface (memo editor + rail) is on screen — either the
 * standalone recording view (pre-start prep) or the active-recording surface
 * embedded in the home session workspace. Used to decide whether async updates
 * (e.g. hydration results) should trigger a re-render.
 */
function isCaptureSurfaceVisible(): boolean {
  return currentView === "recording" || isPreStartPrepOpen() || (currentView === "home" && activeSessionTab === "backchannel");
}

/** True when the clock is NOT running (pre-start prep or paused recording). */
function isClockStopped(): boolean {
  // Trust the synchronous pause INTENT over the async backend echo. Once the
  // user pauses, pausedAtWallMs is set (and stays set until resume), so a
  // status event taken mid-seal that still carries paused:false can't flip the
  // clock-stopped decision at the instant a line is committed — which was
  // causing pause-time notes to be mis-stamped as timed timeline marks.
  if (pausedAtWallMs != null) return true;
  if (isHostedWeb() && hostedRecoveryState.selectedRecordingId()) return true;
  return !(recordingStatus.is_recording && !recordingStatus.paused);
}

/** Shared slugifier — T9 fix: "/" and other non-alnum chars → "-". */
function slugifyName(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 90);
}

/** Build the block-margin rail HTML for the given block ordinal. */
function blockMarginRailHtml(ordinal: number): string {
  const bm = blockMargins.get(ordinal);
  const state = bm?.state ?? null;
  const marginalia = bm?.marginalia ?? [];
  const consumed = bm?.consumed ?? new Set<string>();
  const expanded = bm?.expanded ?? new Set<string>();

  // Affordance button: "Look over this" / "Look again". Show whenever there is
  // something to look over (prep lines, pause-block notes, or the meeting so far).
  if (!hydrationMaterial(ordinal)) return "";

  const hasResult = state !== null && state !== "warming";
  const affordanceLabel = hasResult ? "Look again" : "Look over this";
  const affordanceBtn = `<button class="prep-look-btn" onclick="window.__firePrepHydration()">${esc(affordanceLabel)}</button>`;

  if (state === null) {
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        <div class="backchannel-assist-card prep muted look-affordance">
          ${affordanceBtn}
        </div>
      </margins>
    `;
  }

  if (state === "warming") {
    const rehydratingWithFragments = hasResult && marginalia.length > 0;
    if (!rehydratingWithFragments) {
      return `
        <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
          <div class="backchannel-assist-card prep pending placeholder">
            <div class="backchannel-assist-anchor">Getting your notes ready…</div>
          </div>
        </margins>
      `;
    }
    // Re-hydrating: keep fragments visible, add "Looking again…" line above them
  }

  // "thin" means the model held little — but it can still return real, sourced
  // marginalia. Only show the bare "not much" placeholder when there are genuinely
  // no cards; otherwise fall through and render whatever the backend returned.
  // (The real backend routinely emits "thin" with 1-2 good fragments; suppressing
  // them here was why hydrated cards never appeared in the app while the mock —
  // which returns "hydrated" — rendered fine.)
  if (state === "thin" && marginalia.length === 0) {
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        <div class="backchannel-assist-card prep muted">
          <div class="backchannel-assist-anchor">Not much held on this one</div>
        </div>
        ${affordanceBtn}
      </margins>
    `;
  }

  if (state === "quiet") {
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        ${affordanceBtn}
      </margins>
    `;
  }

  if (state === "unavailable") {
    const detail = bm?.reason
      ? `Recall unavailable — ${esc(bm.hint ?? "run margins setup")}.`
      : "Prep context unavailable";
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        <div class="backchannel-assist-card prep muted">
          <div class="backchannel-assist-anchor">${detail}</div>
        </div>
      </margins>
    `;
  }

  if (state === "error") {
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        <div class="backchannel-assist-card prep error">
          <div class="backchannel-assist-anchor">Couldn't load prep notes</div>
          <div class="backchannel-assist-actions">
            <button onclick="window.__firePrepHydration()">Retry</button>
          </div>
        </div>
      </margins>
    `;
  }

  // Any state that reaches here with cards (hydrated, thin-with-cards, or
  // re-hydrating warming with prior fragments) should render them; the
  // card-suppressing states (null/quiet/unavailable/error, empty thin) already
  // returned above. No cards → just the affordance button.
  if (marginalia.length === 0) {
    return `
      <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
        ${affordanceBtn}
      </margins>
    `;
  }

  const cards = marginalia.map((item, i) => {
    const hash = marginaliaHash(item);
    const isConsumed = consumed.has(hash);
    const isExpanded = expanded.has(hash);
    const kindLabel = item.kind === "carried" ? "carried"
      : item.kind === "blind_spot" ? "not on your list"
      : item.kind === "sharpen" ? "sharpen"
      : item.kind === "counterevidence" ? "receipt"
      : "context";
    const anchorHtml = item.anchor
      ? `<div class="prep-marginalia-anchor">"${esc(item.anchor.slice(0, 60))}"</div>`
      : `<div class="prep-marginalia-anchor not-on-list">not on your list</div>`;
    const firstSource = item.sources[0] ?? null;
    const sourceBasename = firstSource ? firstSource.split("/").pop()?.replace(/\.md$/, "") ?? firstSource : null;
    const sourceDateMatch = sourceBasename?.match(/\d{4}-\d{2}-\d{2}/);
    const sourceChipLabel = sourceBasename
      ? (sourceDateMatch ? `${sourceBasename.replace(sourceDateMatch[0] + "-", "")} · ${sourceDateMatch[0]}` : sourceBasename).slice(0, 40)
      : null;
    const sourcesHtml = item.sources.length > 0 ? `
      <span class="prep-source-chip" onclick="window.__togglePrepMarginaliaExpand(${i})" title="Toggle source">
        ${esc(sourceChipLabel ?? item.sources[0])}
      </span>
      ${isExpanded ? `
        <div class="prep-sources-expanded">
          ${item.sources.map(s => `<div class="prep-source-path">${esc(s)}</div>`).join("")}
        </div>
      ` : ""}
    ` : "";
    const pullAffordance = !isConsumed ? `
      <button class="prep-pull-btn" title="Pull into notes (rephrase as your own)" onclick="window.__pullPrepMarginalia(${i})">←</button>
    ` : "";
    return `
      <li class="backchannel-assist-card prep ${isConsumed ? "prep-consumed" : ""}" data-kind="${esc(item.kind)}">
        <div class="prep-marginalia-kind">${esc(kindLabel)}</div>
        ${anchorHtml}
        <div class="backchannel-assist-suggestion">${esc(item.text)}</div>
        <div class="prep-marginalia-footer">
          ${sourcesHtml}
          ${pullAffordance}
        </div>
      </li>
    `;
  });

  // eslint-disable-next-line @typescript-eslint/no-unnecessary-type-assertion
  const rehydratingLine = (state as string) === "warming" ? `
    <div class="backchannel-assist-card prep pending placeholder">
      <div class="backchannel-assist-anchor">Looking again…</div>
    </div>
  ` : "";

  const steerSection = prepSteerActive ? `
    <form class="backchannel-steer" onsubmit="window.__steerPrep(); return false;">
      <input class="backchannel-steer-input" type="text" autocomplete="off"
        placeholder="Steer the margin…"
        value="${esc(prepSteerDraft)}"
        oninput="window.__updatePrepSteerDraft(this.value)"
        onkeydown="if(event.key==='Enter'){window.__steerPrep();return false;}"
      />
      <button type="submit">Send</button>
    </form>
  ` : "";

  return `
    <margins class="backchannel-assist-rail prep-hydration-rail" aria-label="Prep notes">
      ${rehydratingLine}
      <ol class="backchannel-assist-stack single">
        ${cards.join("")}
      </ol>
      ${affordanceBtn}
      ${steerSection}
    </margins>
  `;
}

/** Fire hydration for the current block. */
window.__firePrepHydration = () => {
  firePrepHydration();
};

function firePrepHydration() {
  if (!isClockStopped()) return;
  // Guard BEFORE mutating state: firing with nothing to look over (Cmd+Enter
  // alias, Retry) must not strand the rail in "warming" with no request in flight.
  const material = hydrationMaterial(currentBlockOrdinal);
  if (!material) return;
  const bm = getOrCreateBlockMargin(currentBlockOrdinal);
  bm.state = "warming";
  render();
  void hydratePrepSketch({
    lines: material.sketch,
    sessionName: activePrepSessionName(),
    people: prepPeople,
    eventTitle: prepEventTitle,
    blockOrdinal: currentBlockOrdinal,
    pulledTexts: bm.pulledTexts,
    meetingSoFar: material.meetingSoFar,
  }).catch(() => {});
}

/** Open capture surface clock-stopped (pre-start from calendar card). */
window.__openCaptureFromCalendar = () => {
  if (hostedCaptureBlocksStart() || recordingStartup?.phase === "starting") return;
  // Seed from calendarSuggestion if available
  prepEventTitle = calendarSuggestion?.title ?? null;
  prepPeople = calendarSuggestion?.people ?? [];
  const rawName = prepEventTitle ?? "prep";
  prepSessionName = slugifyName(rawName) || "prep-session";
  resetPrepBlockState();
  armedFromCalendar = true;
  preStartPrepOpen = true;
  prepProjectId = activeProjectId();
  prepStartBlockedReason = null;
  // Set up auto-start countdown if calendar event time is in the future
  clearAutoStartTimer();
  if (
    settings.auto_start_from_calendar !== false
    && !calendarSuggestionFreshness.stale
    && calendarSuggestion?.start
  ) {
    const deadline = Date.parse(calendarSuggestion.start);
    if (Number.isFinite(deadline) && deadline > Date.now()) {
      autoStartDeadline = deadline;
      autoStartTimer = setInterval(() => {
        if (!armedFromCalendar || !(settings.auto_start_from_calendar !== false)) {
          clearAutoStartTimer();
          render();
          return;
        }
        if (!autoStartDeadline) { clearAutoStartTimer(); return; }
        const remaining = autoStartDeadline - Date.now();
        if (remaining <= 0) {
          clearAutoStartTimer();
          if (!hostedCaptureBlocksStart()) {
            void window.__startRecordingFromPrep();
          }
        } else {
          render(); // tick countdown
        }
      }, 1_000);
    }
  }
  // Set up memo editor surface without audio
  clearPendingMemoDraft();
  memoLines = [];
  pendingLineStartSecs = null;
  memoStartTime = Date.now();
  recordingMounted = false;
  // Stay in the main workspace and show its embedded clock-stopped capture posture.
  currentView = "home";
  activeSessionTab = "backchannel";
  render();
};

// Keep old __enterPrepPhase as alias for sidebar card (will be pointed at __openCaptureFromCalendar below)
window.__enterPrepPhase = () => {
  window.__openCaptureFromCalendar();
};

window.__cancelPrepPhase = () => {
  abandonPreStartPrep();
  currentView = "home";
  render();
};

function clearAutoStartTimer() {
  if (autoStartTimer !== null) { clearInterval(autoStartTimer); autoStartTimer = null; }
}

window.__holdOffAutoStart = () => {
  clearAutoStartTimer();
  autoStartDeadline = null;
  armedFromCalendar = false;
  render();
};

window.__startRecordingFromPrep = async () => {
  if (prepProjectId && prepProjectId !== activeProjectId()) {
    clearAutoStartTimer();
    armedFromCalendar = false;
    autoStartDeadline = null;
    prepStartBlockedReason = "This prepared meeting belongs to another project. Open Prepare again from that project.";
    render();
    return;
  }
  if (!captureReadyFromState()) {
    clearAutoStartTimer();
    armedFromCalendar = false;
    autoStartDeadline = null;
    prepStartBlockedReason = activeProject(settings).path?.trim()
      ? "Choose a writable notes folder before capture can start."
      : "Choose a project before capture can start.";
    render();
    return;
  }
  clearAutoStartTimer();
  autoStartDeadline = null;
  prepStartBlockedReason = null;
  const seedLines = [...memoLines];
  // Compact un-pulled fragments for block 0 before leaving clock-stopped
  compactBlockMargin(currentBlockOrdinal);
  armedFromCalendar = false;
  const name = prepSessionName || defaultMeetingName();
  await startNamedMeetingFromPrep(name, seedLines);
};

/** Compact un-pulled marginalia for a block — freeze them as quiet read-only lines. */
function compactBlockMargin(ordinal: number) {
  // Currently we just leave the block margin state; fragments are no longer rendered
  // since the block is not the current block after recording starts.
  // Future: persist compacted lines in vault. For now: mark state so we don't re-render them live.
  const bm = blockMargins.get(ordinal);
  if (bm && bm.state === "warming") {
    // In-flight hydration at clock start: ignore when it lands (guard in onPrepHydration)
  }
}

async function startNamedMeetingFromPrep(name: string, seedLines: MemoLine[]) {
  const sanitized = slugifyName(name);
  if (!sanitized) return;
  beginOptimisticRecordingFromPrep(sanitized, seedLines);
}

function beginOptimisticRecordingFromPrep(requestedName: string, seedLines: MemoLine[]) {
  preStartPrepOpen = false;
  prepProjectId = null;
  prepStartBlockedReason = null;
  // Pass seed lines into beginOptimisticRecording so they are applied before the first render()
  // call inside that function. This ensures sketch lines are visible immediately during startup.
  //
  // Anchor the clock at THIS moment (Start), not at prep-open time. The clock only
  // runs while recording, so the first timed line and the displayed elapsed must be
  // relative to capture start — not inflated by however long the user spent jotting
  // in the clock-stopped prep posture. Seed lines are block-0 (untimed, excluded from
  // alignment), so re-anchoring memoStartTime does not corrupt their marks.
  const seedMemoState = seedLines.length > 0 ? { lines: seedLines, startTime: Date.now() } : undefined;
  beginOptimisticRecording(requestedName, seedMemoState);
}

window.__pullPrepMarginalia = (index: number) => {
  const bm = blockMargins.get(currentBlockOrdinal);
  if (!bm) return;
  const item = bm.marginalia[index];
  if (!item) return;
  const hash = marginaliaHash(item);
  if (bm.consumed.has(hash)) return;
  // Preserve anything the user was already drafting before rebuilding the
  // embedded workspace. A pulled margin is committed immediately so the click
  // has visible, durable memo semantics; committed lines remain directly
  // editable for the user's rephrasing pass.
  capturePendingDraftForNextRender();
  const committedIndex = appendPulledMarginLine(
    memoLines,
    item.text,
    (Date.now() - memoStartTime) / 1000,
    currentBlockOrdinal,
  );
  if (committedIndex === null) return;
  bm.consumed.add(hash);
  bm.pulledTexts.push(item.text);
  render();

  const sessionName = memoCaptureSessionName();
  if (sessionName) {
    const durableLines = memoLines.map(line => ({ ...line }));
    void syncMemo(durableLines, sessionName, recordingStatus.web_recording_id).catch(error => {
      console.warn("Could not persist pulled prep margin", error);
    });
  }

  // renderHome mounts textarea handlers on its own zero-delay task. Queue focus
  // after that task so the pulled line is visibly selected for immediate edits.
  window.setTimeout(() => {
    const textarea = document.getElementById(`memo-input-${committedIndex}`) as HTMLTextAreaElement | null;
    textarea?.focus();
    textarea?.setSelectionRange(textarea.value.length, textarea.value.length);
  }, 0);
};

window.__togglePrepMarginaliaExpand = (index: number) => {
  const bm = blockMargins.get(currentBlockOrdinal);
  if (!bm) return;
  const item = bm.marginalia[index];
  if (!item) return;
  const hash = marginaliaHash(item);
  if (bm.expanded.has(hash)) {
    bm.expanded.delete(hash);
  } else {
    bm.expanded.add(hash);
  }
  render();
};

window.__updatePrepSteerDraft = (value: string) => {
  prepSteerDraft = value;
};

window.__steerPrep = () => {
  if (!prepSteerDraft.trim()) return;
  const instruction = prepSteerDraft;
  prepSteerDraft = "";
  prepSteerActive = false;
  const bm = getOrCreateBlockMargin(currentBlockOrdinal);
  bm.state = "warming";
  render();
  void steerPrepHydration(activePrepSessionName(), currentBlockOrdinal, instruction).catch(() => {});
};

window.__rehydratePrep = () => {
  firePrepHydration();
};

/** Mobile capture posture: hosted web on a phone-sized screen. Desktop and the
 *  Tauri app keep the spacious meeting view. */
function isMobileCaptureView(): boolean {
  return isHostedWeb()
    && typeof window !== "undefined"
    && window.matchMedia?.("(max-width: 720px)").matches === true;
}

let mobileCaptureWaveform: { handle: RollingWaveformHandle; canvas: HTMLCanvasElement } | null = null;

function mountMobileCaptureWaveform() {
  const canvas = document.getElementById("mobile-capture-waveform") as HTMLCanvasElement | null;
  if (canvas === mobileCaptureWaveform?.canvas) return;
  // A full innerHTML render replaced the canvas: seed the successor with the
  // outgoing trail so the waveform doesn't visibly restart.
  const seed = mobileCaptureWaveform?.handle.bars();
  mobileCaptureWaveform?.handle.stop();
  mobileCaptureWaveform = null;
  if (!canvas) return;
  // Prefer the live browser meter (rAF-fresh) over the 250ms status poll.
  // The waveform expects RMS energy; scale the peak-based status fallback
  // down so mock/native levels land in the same visual range.
  const handle = startRollingWaveform(
    canvas,
    () => webMicrophoneRms()
      ?? (recordingStatus.mic_level != null ? recordingStatus.mic_level * 0.35 : null),
    seed,
  );
  mobileCaptureWaveform = { handle, canvas };
}

function renderRecording() {
  // If already mounted, just patch the header - don't touch the editor.
  // Exception: clock-stopped posture (prep/pause) has a live footer countdown,
  // a block-margin rail, and a "Look over this" affordance that all live OUTSIDE
  // the header — the header-only fast path would strand them stale (hold-off
  // inert, countdown frozen, affordance never appearing). Force a full re-render
  // there; the pending-input draft is preserved via capture/restore.
  if (recordingMounted && currentView === "recording" && !settingsOverlayOpen && !captureDiscardDialog && !isClockStopped()) {
    updateRecordingHeader();
    return;
  }

  recordingMounted = true;
  app.innerHTML = renderRecordingView(recordingRenderContext(), {
    windowDragRegion: renderWindowDragRegion(),
    settingsOverlay: renderSettingsOverlayIfOpen(),
  });

  // Attach edit handlers to any pre-existing committed lines
  attachMemoEditHandlers();
  appendNewMemoInput();
  restorePendingDraftAfterRender();

  // Cmd+Enter on an EMPTY pending line while clock-stopped = fire/re-fire hydration.
  if (isClockStopped()) {
    const editorEl = document.getElementById("memo-editor");
    editorEl?.addEventListener("keydown", (e) => {
      if (e.key !== "Enter" || !(e.metaKey || e.ctrlKey)) return;
      const target = e.target as HTMLElement | null;
      if (!(target instanceof HTMLTextAreaElement)) return;
      if (target.value.trim().length !== 0) return;
      if (!hydrationMaterial(currentBlockOrdinal)) return;
      e.preventDefault();
      firePrepHydration();
    });
  }
}

function memoEditorContext(): MemoEditorContext {
  return {
    getMemoLines: () => memoLines,
    getMemoStartTime: () => memoStartTime,
    getPendingLineStartSecs: () => pendingLineStartSecs,
    setPendingLineStartSecs: value => { pendingLineStartSecs = value; },
    getRecordingStatus: () => ({ ...recordingStatus, elapsed_secs: displayElapsedSecs() }),
    getRecordingStartupState: activeStartupState,
    getRecordingStatusLine: recordingStatusLine,
    isFullRecordingView: () => currentView === "recording",
    renderMemoLineInner: (line, index) => renderMemoLineInner(line, index, renderMemoBackchannelButton, (o) => blockWallClockByOrdinal.get(o)),
    renderAudioHealth: () => renderAudioHealth(recordingRenderContext()),
    isAudioPendingForNewMarks: () => recordingStartup?.phase === "starting",
    // Consult live clock state: during a real recording pause the clock is
    // stopped, so lines committed while paused must be stamped with the current
    // block ordinal (untimed) instead of a ticking timeline timestamp.
    getClockStopped: () => isClockStopped(),
    getCurrentBlockOrdinal: () => currentBlockOrdinal,
    getBlockWallClock: (ordinal) => blockWallClockByOrdinal.get(ordinal),
    checkpointMemoLine: scheduleMemoCheckpoint,
    requestBackchannelForMemo: scheduleBackchannelRequest,
    collapseBackchannelCards: collapsePreviousBackchannelCards,
  };
}

/** Clock-stopped memo editor context: stamps block_ordinal, no checkpoint/backchannel. */
function clockStoppedMemoEditorContext(): MemoEditorContext {
  return {
    getMemoLines: () => memoLines,
    getMemoStartTime: () => memoStartTime,
    getPendingLineStartSecs: () => null, // no pending time for clock-stopped
    setPendingLineStartSecs: _value => { /* clock stopped — don't set pendingLineStartSecs */ },
    getRecordingStatus: () => ({
      ...recordingStatus,
      is_recording: false,
      paused: false,
      elapsed_secs: (Date.now() - memoStartTime) / 1000,
    }),
    getRecordingStartupState: () => null,
    getRecordingStatusLine: () => "",
    isFullRecordingView: () => currentView === "recording",
    renderMemoLineInner: (line, index) => renderMemoLineInner(line, index, () => "", (o) => blockWallClockByOrdinal.get(o)),
    renderAudioHealth: () => "",
    isAudioPendingForNewMarks: () => false,
    getClockStopped: () => true,
    getCurrentBlockOrdinal: () => currentBlockOrdinal,
    getBlockWallClock: (ordinal) => blockWallClockByOrdinal.get(ordinal),
    checkpointMemoLine: (_committedIndex: number) => {
      // Clock-stopped: persist memo but no transcript checkpoint
      const recordingId = recordingStatus.web_recording_id;
      if (!isHostedWeb() || hostedMemoSyncAllowed({
        recordingId,
        selectedRecoveryId: hostedRecoveryState.selectedRecordingId(),
        selectedRecoveryHydrated: hostedRecoveryState.selectedMemoReady(recordingId),
        locallyOwned: currentTabOwnsHostedCapture(),
      })) {
        void syncMemo(memoLines, activePrepSessionName(), recordingId).catch(() => {});
      }
      // Re-render so the block-margin rail + "Look over this" affordance appears
      // once the first line in this block is committed (render preserves the
      // pending input via capture/restore; commit already moved focus to it).
      render();
    },
    // Cmd+Enter in clock-stopped → fire hydration (backchannel callback repurposed)
    requestBackchannelForMemo: (_committedIndex: number) => {
      firePrepHydration();
    },
    collapseBackchannelCards: () => {},
  };
}

// The clock-stopped memo context applies whenever the clock is stopped — pre-start
// prep (standalone recording view) AND a mid-meeting pause (embedded in the home
// session workspace, where currentView === "home"). Gating on the view name here
// was the reason pause-time editing fell back to the live/timed context: no render
// after commit (so the block rail never appeared) and Cmd+Enter fired a backchannel
// instead of hydration.
function attachMemoEditHandlers() {
  attachMemoEditHandlersController(isClockStopped() ? clockStoppedMemoEditorContext() : memoEditorContext());
}

function updateRecordingHeader() {
  updateRecordingHeaderController(isClockStopped() ? clockStoppedMemoEditorContext() : memoEditorContext());
}

function appendNewMemoInput() {
  appendNewMemoInputController(isClockStopped() ? clockStoppedMemoEditorContext() : memoEditorContext());
}

function selectedDeviceName(): string | null {
  return captureDeviceSelection()?.name ?? null;
}

function captureDeviceSelection(): DeviceInfo | undefined {
  if (inputDeviceMode() === "pinned") {
    return preferredCaptureDevice(devices, settings.input_device_uid, settings.input_device_name);
  }
  return defaultCaptureDevice(devices);
}

function inputDeviceMode(): "follow_default" | "pinned" {
  return settings.input_device_mode ?? (settings.input_device_uid ? "pinned" : "follow_default");
}

function captureDeviceUidForStart(): string | null {
  if (isHostedWeb()) return webMicrophoneDeviceId;
  return inputDeviceMode() === "pinned" ? captureDeviceSelection()?.uid ?? settings.input_device_uid ?? null : null;
}

function deviceUidForName(deviceName: string | null): string | undefined {
  if (!deviceName) return undefined;
  return devices.find(d => d.name === deviceName)?.uid;
}

function liveCaptureDeviceName(): string | null {
  const captureDevice = recordingStatus.capture_device;
  if (captureDevice?.state === "active") return captureDevice.device_name;
  if (captureDevice?.state === "switching") return captureDevice.to;
  if (captureDevice?.state === "holding") return captureDevice.last_good;
  return recordingStatus.input_device_name || selectedDeviceName();
}

function fallbackCaptureDeviceState(status: RecordingStatus): CaptureDeviceState {
  const captureDevice = (status as RecordingStatus & { capture_device?: CaptureDeviceState | null }).capture_device;
  if (captureDevice) return captureDevice;
  const deviceName = status.input_device_name || selectedDeviceName();
  return { state: "active", device_name: deviceName || "System Default" };
}

function normalizeRecordingStatus(status: RecordingStatus): RecordingStatus {
  return { ...status, capture_device: fallbackCaptureDeviceState(status) };
}

function clearCaptureDeviceSwitchToast() {
  captureDeviceSwitchToast = null;
  if (captureDeviceSwitchToastTimer !== null) {
    window.clearTimeout(captureDeviceSwitchToastTimer);
    captureDeviceSwitchToastTimer = null;
  }
}

function showCaptureDeviceToast(toast: CaptureDeviceToast) {
  captureDeviceSwitchToast = toast;
  if (captureDeviceSwitchToastTimer !== null) window.clearTimeout(captureDeviceSwitchToastTimer);
  captureDeviceSwitchToastTimer = window.setTimeout(() => {
    captureDeviceSwitchToast = null;
    captureDeviceSwitchToastTimer = null;
    renderRecordingStatusChange();
  }, 8000);
}

function reconcileCaptureDeviceState(next: CaptureDeviceEvent | null) {
  const previous = lastCaptureDeviceState;
  if (!next) {
    lastCaptureDeviceState = null;
    pendingCaptureDeviceSwitchFrom = null;
    pendingCaptureDeviceSwitchTo = null;
    manualCaptureDeviceSwitchPending = false;
    manualCaptureDeviceSwitchTarget = null;
    holdingRecoveryDeviceName = null;
    clearCaptureDeviceSwitchToast();
    return;
  }

  if (next.state === "fallback") {
    // Holding recovery already has its own confirmation. The backend may emit
    // an additive fallback fact immediately afterward while PinFallback stays
    // latched; do not replace the recovery confirmation with a second notice.
    if (holdingRecoveryDeviceName !== next.device_name) {
      const pinnedName = next.requested_uid
        ? devices.find(device => device.uid === next.requested_uid)?.name ?? settings.input_device_name ?? null
        : settings.input_device_name ?? null;
      showCaptureDeviceToast({ kind: "fallback", deviceName: next.device_name, pinnedName, retryDevice: null });
    }
    return;
  }

  if (next.state === "recovered") {
    holdingRecoveryDeviceName = next.device_name;
    showCaptureDeviceToast({ kind: "recovery", deviceName: next.device_name, retryDevice: null });
    return;
  }

  if (next.state === "switch_failed") {
    const target = manualCaptureDeviceSwitchTarget
      ?? pendingCaptureDeviceSwitchTo
      ?? (lastCaptureDeviceState?.state === "switching" ? lastCaptureDeviceState.to : null);
    showCaptureDeviceToast({
      kind: "switch_failed",
      deviceName: target,
      previousDevice: next.device_name || (lastCaptureDeviceState?.state === "switching" ? lastCaptureDeviceState.from : null),
      retryDevice: target,
    });
    pendingCaptureDeviceSwitchFrom = null;
    pendingCaptureDeviceSwitchTo = null;
    manualCaptureDeviceSwitchPending = false;
    manualCaptureDeviceSwitchTarget = null;
    return;
  }

  if (next.state === "switching") {
    pendingCaptureDeviceSwitchFrom = next.from;
    pendingCaptureDeviceSwitchTo = next.to;
    holdingRecoveryDeviceName = null;
    clearCaptureDeviceSwitchToast();
  } else if (next.state === "holding") {
    pendingCaptureDeviceSwitchFrom = null;
    pendingCaptureDeviceSwitchTo = null;
    manualCaptureDeviceSwitchPending = false;
    manualCaptureDeviceSwitchTarget = null;
    holdingRecoveryDeviceName = null;
    clearCaptureDeviceSwitchToast();
    if (previous?.state !== "holding") {
      // Keep Audio Setup current while the persistent banner offers device choice.
      void listDevices().then(list => { devices = list; renderRecordingStatusChange(); }).catch(() => {});
    }
  } else if (next.state === "active") {
    const previousDevice = previous?.state === "switching"
      ? previous.from
      : pendingCaptureDeviceSwitchFrom;
    const isManualSwitch = manualCaptureDeviceSwitchPending
      && (!manualCaptureDeviceSwitchTarget || manualCaptureDeviceSwitchTarget === next.device_name);
    if (previous?.state === "holding") {
      holdingRecoveryDeviceName = next.device_name;
      showCaptureDeviceToast({ kind: "recovery", deviceName: next.device_name || null, retryDevice: null });
    } else if (previousDevice && previousDevice !== next.device_name && !isManualSwitch) {
      showCaptureDeviceToast({ kind: "switch", deviceName: next.device_name, previousDevice, retryDevice: null });
    }
    if (isManualSwitch || manualCaptureDeviceSwitchTarget === next.device_name) {
      manualCaptureDeviceSwitchPending = false;
      manualCaptureDeviceSwitchTarget = null;
    }
    const returnedToOldDevice = previous?.state === "switching" && next.device_name === previous.from;
    if (!returnedToOldDevice) {
      pendingCaptureDeviceSwitchFrom = null;
      pendingCaptureDeviceSwitchTo = null;
    }
  }

  lastCaptureDeviceState = next;
}

// Bumped on every applied status so the poll can detect that a newer snapshot
// (typically a pause/resume/stop command's own response) landed while its
// fetch was in flight, and drop the stale one instead of reverting the UI.
let recordingStatusApplyCount = 0;

function applyRecordingStatus(next: RecordingStatus): RecordingStatus {
  recordingStatusApplyCount += 1;
  recordingStatus = normalizeRecordingStatus({
    ...next,
    web_recoveries: hostedRecoveryState.list(),
  });
  reconcileCaptureDeviceState(recordingStatus.capture_device ?? null);
  reconcilePersistedSystemAudioReadiness(recordingStatus);
  return recordingStatus;
}

async function refreshHostedRecoveryDiscovery(): Promise<WebRecordingRecoveryStatus[]> {
  if (!isHostedWeb()) return [];
  const recoveries = await listHostedRecordingRecoveries();
  hostedRecoveryState.discover(recoveries);
  recordingStatus = { ...recordingStatus, web_recoveries: hostedRecoveryState.list() };
  return recoveries;
}

function hostedCaptureBlocksStart(): boolean {
  return isHostedWeb()
    ? Boolean(hostedActiveRecordingStatus?.is_recording)
    : recordingStatus.is_recording;
}

async function restoreHostedActiveAfterRecoveryCompletion(
  completedRecordingId: string,
): Promise<RecordingStatus | null> {
  if (!isHostedWeb()) return null;
  const recoveries = await listHostedRecordingRecoveries().catch(() =>
    hostedRecoveryState.list().filter(recovery => recovery.recording_id !== completedRecordingId));
  const globalStatus = await getRecordingStatus().catch(() => hostedActiveRecordingStatus);
  if (!globalStatus) {
    hostedRecoveryState.clearSelection(completedRecordingId);
    hostedRecoveryState.discover(recoveries);
    return null;
  }
  const active = hostedRecoveryState.completeRecovery(
    completedRecordingId,
    recoveries,
    globalStatus,
  );
  hostedActiveRecordingStatus = active;
  if (!active) return null;

  const activeRecordingId = active.web_recording_id;
  const snapshot = activeRecordingId
    ? hostedRecoveryWorkspace.takeActive(activeRecordingId)
    : null;
  applyRecordingStatus(active);
  if (snapshot) {
    memoLines = snapshot.memoLines.map(line => ({ ...line }));
    pendingDraftRestore = snapshot.pendingDraft ? { ...snapshot.pendingDraft } : null;
    memoEditorRestore = snapshot.editorRestore ? { ...snapshot.editorRestore } : null;
    activeSessionName = snapshot.activeSessionName || active.session_name || activeSessionName;
    activeSessionTab = snapshot.activeSessionTab;
    currentView = snapshot.currentView;
    if (snapshot.session) {
      sessions = [
        snapshot.session,
        ...sessions.filter(session => session.name !== snapshot.session!.name),
      ];
      if (snapshot.lifecycle) lifecycleBySession[snapshot.session.name] = snapshot.lifecycle;
    }
    currentBlockOrdinal = snapshot.currentBlockOrdinal;
    memoStartTime = snapshot.memoStartTime;
    pausedAtWallMs = snapshot.pausedAtWallMs;
    pendingLineStartSecs = snapshot.pendingLineStartSecs;
  } else {
    // Never leave A's memo in the shared editor while B is the status target.
    // A tab without a local snapshot may rehydrate B if it owns that exact ID;
    // otherwise the empty editor remains read-only under server authorization.
    memoLines = [];
    pendingDraftRestore = null;
    memoEditorRestore = null;
    if (active.session_name && activeRecordingId && active.web_capture_owner === "local") {
      const hydrated = await hydrateRecordingMemo(active.session_name, activeRecordingId).catch(() => null);
      if (hydrated) memoLines = hydrated.map(line => ({ ...line }));
    }
    activeSessionName = active.session_name || activeSessionName;
    activeSessionTab = "backchannel";
    currentView = "home";
  }
  if (active.session_name) {
    memoLinesBySession[sessionStateKey(active.session_name)] = memoLines.map(line => ({ ...line }));
  }
  recordingMounted = false;
  startPolling();
  return active;
}

function reconcilePersistedSystemAudioReadiness(status: RecordingStatus) {
  const ready = systemAudioReadinessAfterRecordingStatus(
    Boolean(settings.system_audio_ready),
    status,
  );
  if (ready === Boolean(settings.system_audio_ready)) return;

  settings = { ...settings, system_audio_ready: ready };
  if (!ready) {
    systemAudioTestResult = {
      peak: status.spk_level || 0,
      drop_count: status.spk_drop_count || 0,
      silent_secs: status.speaker_silence_secs || 0,
      frame_count: status.system_audio_frame_count || 0,
      status: "blocked",
      message: "Computer audio delivered no frames; restore System Audio Recording Only permission and restart Margins.",
      restart_recommended: true,
    };
  }
  void updateSettings(settings).catch((error) => {
    console.warn("Could not reconcile system-audio readiness", error);
  });
}

function renderRecordingStatusChange() {
  if ((currentView === "recording" || (currentView === "home" && activeSessionTab === "backchannel")) && recordingMounted) {
    updateRecordingHeader();
    return;
  }
  render();
}

async function switchLiveCaptureDevice(deviceUidOrName: string | null): Promise<boolean> {
  let device = deviceUidOrName
    ? devices.find(d => d.uid === deviceUidOrName || d.name === deviceUidOrName)
    : undefined;
  if (deviceUidOrName && !device) {
    devices = await refreshDevices().catch(() => devices);
    device = devices.find(d => d.uid === deviceUidOrName || d.name === deviceUidOrName);
  }
  if (deviceUidOrName && !device) {
    alert(`Could not find microphone: ${deviceUidOrName}. Choose another device.`);
    return false;
  }

  manualCaptureDeviceSwitchPending = true;
  manualCaptureDeviceSwitchTarget = device?.name ?? null;
  clearCaptureDeviceSwitchToast();
  try {
    applyRecordingStatus(await switchRecordingDevice(device?.uid ?? null));
    renderRecordingStatusChange();
    return true;
  } catch (e: any) {
    const message = String(e?.message || e);
    const continuedWith = message.match(/\. Continued recording with (.+)\.$/)?.[1] ?? null;
    if (message.startsWith("Microphone switched to ")) {
      showCaptureDeviceToast({ kind: "preference_error", message, retryDevice: null });
    } else if (captureDeviceSwitchToast?.kind !== "switch_failed") {
      showCaptureDeviceToast({
        kind: "switch_failed",
        deviceName: device?.name ?? null,
        previousDevice: continuedWith ?? liveCaptureDeviceName(),
        retryDevice: device?.uid ?? null,
      });
    }
    manualCaptureDeviceSwitchPending = false;
    manualCaptureDeviceSwitchTarget = null;
    applyRecordingStatus(await getRecordingStatus().catch(() => recordingStatus));
    renderRecordingStatusChange();
    return message.startsWith("Microphone switched to ");
  }
}

function preferredCaptureDevice(deviceList: DeviceInfo[], savedUid?: string | null, savedName?: string | null): DeviceInfo | undefined {
  return savedUid
    ? deviceList.find(d => d.uid === savedUid)
    : savedName ? deviceList.find(d => d.name === savedName) : undefined;
}

function defaultCaptureDevice(deviceList: DeviceInfo[]): DeviceInfo | undefined {
  return deviceList.find(d => d.is_default);
}

function audioTestMessage(): string {
  if (audioTestError) return "Microphone check failed";
  if (audioTestRunning) return "Listening";
  if (settings.audio_input_ready && !audioTestResult) return "Ready";
  if (!audioTestResult) return "Speak normally during the check";
  if (audioTestResult.ok) return "Ready";
  return `Try another mic or unmute ${audioTestResult.device_name}.`;
}

function readHarnessMemoLines(): MemoLine[] | null {
  if (typeof window === "undefined") return null;
  const memo = (window as any).__marginsMock?.getState?.()?.memo;
  if (!Array.isArray(memo)) return null;
  return memo.map((line: MemoLine) => ({ ...line }));
}

function defaultTabForStatus(status: SessionInfo["status"]): SessionTab {
  return status === "synthesized" || status === "processing" || status === "failed" ? "distill" : "backchannel";
}

function defaultTabForSession(s: SessionInfo): SessionTab {
  return isCaptureNote(s) ? "distill" : defaultTabForStatus(s.status);
}

function notesDestinationPath(vaultPath = settings.vault_path, inboxFolder = settings.inbox_folder || ""): string {
  const project = activeProject(settings);
  const root = (vaultPath?.trim() || project.path || DEFAULT_VAULT_PATH).replace(/\/+$/, "");
  const folder = (inboxFolder || "").trim().replace(/^\/+|\/+$/g, "");
  return folder ? `${root}/${folder}` : root;
}

function activeProjectDestinationPath(): string {
  const project = activeProject(settings);
  return notesDestinationPath(project.path, project.inbox_folder || "");
}

function activeProjectId(): string | null {
  return activeProject(settings).id || settings.active_project_id || null;
}

function projectIdForSession(name: string): string | null {
  return sessions.find(s => s.name === name)?.project_id || activeProjectId();
}

function sessionStateKeyForProjectId(name: string, projectId: string | null | undefined): string {
  return `${projectId || "active"}:${name}`;
}

function sessionStateKey(name: string): string {
  return sessionStateKeyForProjectId(name, projectIdForSession(name));
}

function sessionStateKeyForSession(session: SessionInfo): string {
  return sessionStateKeyForProjectId(session.name, session.project_id || activeProjectId());
}

function artifactCacheForSession(session: SessionInfo): { aligned?: string; note?: string; loading?: boolean } {
  return artifactCache[sessionStateKeyForSession(session)] || {};
}

function groundedNoteForSession(session: SessionInfo): GroundedNoteState {
  return groundedNoteBySession[sessionStateKeyForSession(session)] || newGroundedNoteState();
}

function distillTraceForSessionInfo(session: SessionInfo): DistillTraceEvent[] {
  return distillTraceBySession[sessionStateKeyForSession(session)] || [];
}

function friendlyVaultPath(path: string | null, sessionName: string): string {
  if (path) return `Saved to ${friendlySavedNotePath(path)}`;
  const project = activeProject(settings);
  if (project.path) return `Ready in ${activeProjectDestinationPath()}/${sessionName}.md`;
  return "Saved as a local note";
}

function friendlySavedNotePath(path: string): string {
  const project = activeProject(settings);
  const vaultRoot = project.path?.replace(/\/+$/, "");
  if (vaultRoot && path === vaultRoot) return project.name || "vault";
  if (vaultRoot && path.startsWith(`${vaultRoot}/`)) {
    return path.slice(vaultRoot.length + 1);
  }
  return path.replace(/^.*?(Obsidian|second-brain|notes)/i, "$1");
}

// ---------------------------------------------------------------------------
// Summary, processing, review views
// ---------------------------------------------------------------------------

function renderSummary() {
  stopPolling();
  app.innerHTML = renderSummaryView({
    sessionName: summarySessionName,
    durationSecs: summaryDuration,
    memoLines,
    windowDragRegion: renderWindowDragRegion(),
    settingsOverlay: renderSettingsOverlayIfOpen(),
  });
}

function renderProcessing() {
  app.innerHTML = renderProcessingView({
    sessionName: processingSessionName,
    steps: processingSteps,
    trace: traceForSession(processingSessionName),
    currentProgress,
    transcriptEntries,
    windowDragRegion: renderWindowDragRegion(),
    settingsOverlay: renderSettingsOverlayIfOpen(),
  });

  // Auto-scroll transcript to bottom
  const scrollEl = document.getElementById("transcript-scroll");
  if (scrollEl) scrollEl.scrollTop = scrollEl.scrollHeight;
}

function renderReview() {
  app.innerHTML = renderReviewView({
    sessionName: reviewSessionName,
    content: reviewContent,
    obsidianVaultReady,
    vaultPath: settings.vault_path,
    renderedMarkdown: renderMarkdown(reviewContent),
    windowDragRegion: renderWindowDragRegion(),
    settingsOverlay: renderSettingsOverlayIfOpen(),
  });
}

function renderMarkdown(md: string): string {
  const prepared = prepareMarkdownForRender(stripMarginsMarkers(md).cleanMarkdown);
  return polishRenderedMarkdown(markdown.render(prepared));
}

function isSessionStreaming(s: SessionInfo): boolean {
  return !noteErrorForSession(s) && (s.status === "processing" || processingSessionName === s.name);
}

function hydrateStreamingNoteMotion(previousScrollTop: number | null) {
  if (currentView !== "home") return;
  const active = activeSessionName ? sessions.find(s => s.name === activeSessionName) : null;
  if (!active || !isSessionStreaming(active)) return;

  const panel = document.querySelector(".primary-session-panel") as HTMLElement | null;

  // Keep the right-rail thread pinned to its live tail while a turn runs, so the
  // "writing"/"revising" pulse stays in view as provenance steps land. This runs
  // even during a refine's dimmed/reflow phase (no `.grounded-note.streaming`
  // yet), so do it before the note-specific early return below. The thread is a
  // fresh element each tick, so re-anchor synchronously.
  const thread = panel?.querySelector("[data-note-thread]") as HTMLElement | null;
  if (thread) thread.scrollTop = thread.scrollHeight;

  const note = panel?.querySelector(".grounded-note.streaming") as HTMLElement | null;
  if (!panel || !note) return;

  // Fade in only blocks that are brand new this tick, so a freshly-inserted
  // section blurs in once while already-present sections — including the one
  // still accumulating lines — stay put instead of re-blurring as they grow.
  const seen = streamingBlockSizesBySession[active.name] || {};
  const next: Record<string, number> = {};
  for (const block of Array.from(note.querySelectorAll<HTMLElement>("[data-stream-block]"))) {
    // Baseline blocks (the pre-reprocess note held on screen) never fade in —
    // they were already there; only their replacements animate.
    if (block.dataset.streamBaseline) continue;
    const key = block.dataset.streamBlock || "";
    const size = Number(block.dataset.streamSize || 0);
    next[key] = size;
    if (!Number.isFinite(size)) continue;
    if (seen[key] === undefined) block.classList.add("stream-fade-in");
  }
  const firstRealBlockLanded = Object.keys(seen).length === 0 && Object.keys(next).length > 0;
  if (firstRealBlockLanded) streamingFirstParagraphPinnedBySession[active.name] = true;
  streamingBlockSizesBySession[active.name] = next;

  applyMemoStackRecede(panel, active.name);

  // The panel is a brand-new element every tick (innerHTML rebuild), so its
  // scrollTop is 0 here. Re-anchor it synchronously — before the browser
  // paints — or it flashes to the top of the note for a frame. Bypass the
  // panel's `scroll-behavior: smooth` too: an animated re-anchor on every tick
  // is exactly the drifting/jitter we're trying to kill. The new paragraph's
  // own fade-in supplies the only motion the eye should see.
  // At narrower widths the provenance rail stacks *below* the note. Using the
  // whole panel's scrollHeight would pin the viewport to provenance and hide
  // the prose that just arrived. Cap auto-follow at the note panel's bottom;
  // the provenance thread already maintains its own live-tail scroll.
  const notePanel = panel.querySelector(".grounded-note-main") as HTMLElement | null;
  const noteBottom = notePanel ? notePanel.offsetTop + notePanel.offsetHeight : panel.scrollHeight;
  const maxScroll = Math.max(0, Math.min(
    panel.scrollHeight - panel.clientHeight,
    noteBottom - panel.clientHeight,
  ));
  const firstParagraph = streamingFirstParagraphPinnedBySession[active.name]
    ? Array.from(note.querySelectorAll<HTMLElement>("p")).find(paragraph =>
        // A short date/attendee byline is metadata, not the first reading
        // payoff. Pin the first substantive paragraph once it is complete.
        (paragraph.textContent || "").trim().length >= 80,
      ) || null
    : null;
  const firstParagraphTarget = firstParagraph
    ? Math.max(0, firstParagraph.getBoundingClientRect().top - panel.getBoundingClientRect().top + panel.scrollTop - 16)
    : null;
  const target = firstParagraphTarget !== null
    ? Math.min(firstParagraphTarget, maxScroll)
    : streamingStickToBottom
      ? maxScroll
      : Math.min(previousScrollTop ?? maxScroll, maxScroll);
  const priorBehavior = panel.style.scrollBehavior;
  panel.style.scrollBehavior = "auto";
  panel.scrollTop = target;
  panel.style.scrollBehavior = priorBehavior;
  // Keep the literal first paragraph stable while stream ticks rebuild the
  // document. Release the pin as soon as the user expresses navigation intent;
  // after that the existing bottom/proportional preservation logic takes over.
  const releaseFirstParagraph = () => {
    streamingFirstParagraphPinnedBySession[active.name] = false;
  };
  panel.addEventListener("wheel", releaseFirstParagraph, { once: true, passive: true });
  panel.addEventListener("touchstart", releaseFirstParagraph, { once: true, passive: true });
  panel.addEventListener("pointerdown", releaseFirstParagraph, { once: true, passive: true });
  panel.addEventListener("keydown", releaseFirstParagraph, { once: true });
}

// As marks get woven into the note they drop out of the "not yet woven in"
// stack. A plain innerHTML rebuild makes the woven-in line vanish and the rest
// snap up to close the gap — a hard jump. This FLIPs the survivors: each line
// that persisted starts the frame at its *previous* position (the layout that
// still included the departed line) and glides up to its new resting spot, so
// the list recedes upward smoothly. The departed line itself fades out in place
// via a throwaway clone, so a mark reads as "absorbed" rather than "deleted".
function applyMemoStackRecede(panel: HTMLElement, sessionName: string) {
  const prev = streamingMemoRectsBySession[sessionName];
  const stack = panel.querySelector(".note-memo-source-stack") as HTMLElement | null;
  if (!stack || prefersReducedMotion()) return;
  if (!prev || Object.keys(prev).length === 0) return;

  const stackTop = stack.getBoundingClientRect().top;
  const present = new Set<string>();

  // Survivors: FLIP from their old offset within the stack to the new one.
  for (const line of Array.from(stack.querySelectorAll<HTMLElement>(".note-buffer-memo-line"))) {
    const id = line.dataset.memoId;
    if (!id) continue;
    present.add(id);
    const old = prev[id];
    if (!old) continue;
    const newOffset = line.getBoundingClientRect().top - stackTop;
    const delta = old.offset - newOffset;
    if (Math.abs(delta) < 0.5) continue;
    line.style.transition = "none";
    line.style.transform = `translateY(${delta}px)`;
    // Force the start frame to commit before swapping in the eased transition,
    // so the browser interpolates the whole travel synchronously within this
    // hydrate (no rAF that a fast rebuild could outrun and drop).
    void line.offsetHeight;
    line.style.transition = "transform 460ms cubic-bezier(0.22, 0.61, 0.36, 1)";
    line.style.transform = "";
  }

  // Departed marks: ghost each one at the slot it just left and fade it out so
  // the absorption is visible instead of an instant pop.
  if (getComputedStyle(stack).position === "static") stack.style.position = "relative";
  for (const id of Object.keys(prev)) {
    if (present.has(id)) continue;
    const old = prev[id];
    const ghost = document.createElement("div");
    ghost.innerHTML = old.html;
    const node = ghost.firstElementChild as HTMLElement | null;
    if (!node) continue;
    node.removeAttribute("data-memo-id");
    node.setAttribute("aria-hidden", "true");
    node.style.position = "absolute";
    node.style.left = "0";
    node.style.right = "0";
    node.style.top = `${old.offset}px`;
    node.style.height = `${old.height}px`;
    node.style.pointerEvents = "none";
    node.style.margin = "0";
    node.classList.add("memo-line-absorbing");
    stack.appendChild(node);
    node.addEventListener("animationend", () => node.remove(), { once: true });
    // Safety net: if a rebuild detaches the node mid-animation the listener
    // never fires, so the next snapshot just won't see it — nothing leaks.
  }
}

function ensureGroundedNoteState(sessionName: string): GroundedNoteState {
  const stateKey = sessionStateKey(sessionName);
  groundedNoteBySession[stateKey] ??= newGroundedNoteState();
  return groundedNoteBySession[stateKey];
}

function memoLinesForSession(sessionName: string): MemoLine[] {
  const stateKey = sessionStateKey(sessionName);
  const stored = memoLinesBySession[stateKey];
  if (stored) return stored;
  const harness = readHarnessMemoLines();
  if (harness) {
    memoLinesBySession[stateKey] = harness.map(line => ({ ...line }));
    return memoLinesBySession[stateKey];
  }
  return [];
}

function consumeNoteStreamChunk(sessionName: string, chunk: string): boolean {
  return consumeNoteStreamChunkInState(ensureGroundedNoteState(sessionName), chunk);
}

function consumeNoteStreamChunkForStateKey(stateKey: string, chunk: string): boolean {
  groundedNoteBySession[stateKey] ??= newGroundedNoteState();
  return consumeNoteStreamChunkInState(groundedNoteBySession[stateKey], chunk);
}

// ---------------------------------------------------------------------------
// Settings view
// ---------------------------------------------------------------------------

function currentAiMode(): string {
  const fallback = settings.api_key?.trim() ? "api" : "included";
  switch (settings.ai_mode?.trim().toLowerCase()) {
    case "included":
    case "margins":
    case "hosted":
      return "included";
    case "chatgpt":
    case "codex":
      return "chatgpt";
    case "api":
    case "api_key":
    case "custom":
      return "api";
    case "auto":
    default:
      return fallback;
  }
}

// Per-mode readiness from the single get_ai_readiness endpoint, or null if the
// call has not landed / failed (callers then fall back to settings-derived
// checks so the UI never crashes on a missing endpoint).
function modeReadiness(mode: string): { ready: boolean; reason: string } | null {
  if (!aiReadiness) return null;
  if (mode === "included") return aiReadiness.modes.included;
  if (mode === "chatgpt") return aiReadiness.modes.chatgpt;
  if (mode === "api") return aiReadiness.modes.api;
  return null;
}

function aiReady(): boolean {
  const mode = currentAiMode();
  // Included provisions its key lazily before the first note, so it is always
  // usable-enough to complete setup — the card still surfaces the live key
  // state separately.
  if (mode === "included") return true;
  const readiness = modeReadiness(mode);
  if (readiness) return readiness.ready;
  // Fallback: readiness endpoint unavailable.
  if (mode === "chatgpt") return aiStatus.chatgpt_authenticated;
  return Boolean(settings.api_key?.trim());
}

function includedReadyFromState(): boolean {
  return modeReadiness("included")?.ready ?? includedAiStatus.included_ready;
}

function aiReadinessText(): string {
  const mode = currentAiMode();
  const readiness = modeReadiness(mode);
  if (readiness) return readiness.reason;
  // Fallback text when the endpoint is unavailable.
  if (mode === "included") return includedAiStatus.message;
  if (mode === "chatgpt") return "Note-making AI connected";
  return `Note-making AI ready via ${settings.ai_base_url || "OpenAI"}`;
}

function settingsReadyFromState(): boolean {
  return captureReadyFromState() && aiReady();
}

// "Can start a capture" — a writable project exists. Recall/context setup makes
// notes richer, but capture and transcript-first cues must not wait on it.
function captureReadyFromState(): boolean {
  const project = activeProject(settings);
  return Boolean(project.path?.trim()) && notesFolderReady;
}

function audioSetupReady(): boolean {
  const micReady = audioTestResult ? audioTestResult.ok : Boolean(settings.audio_input_ready);
  const tapReady = systemAudioTestResult
    ? systemAudioTestResult.status === "ok" || systemAudioTestResult.status === "unsupported"
    : Boolean(settings.system_audio_ready);
  return micReady && tapReady;
}

async function persistAudioReadinessPatch(patch: Partial<Pick<Settings, "audio_input_ready" | "system_audio_ready" | "input_device_mode" | "input_device_uid" | "input_device_name">>) {
  settings = { ...settings, ...patch };
  try {
    await updateAudioSettings({
      audio_input_ready: settings.audio_input_ready,
      system_audio_ready: settings.system_audio_ready,
      input_device_mode: settings.input_device_mode,
      input_device_uid: settings.input_device_uid,
      input_device_name: settings.input_device_name,
    });
  } catch (e) {
    console.warn("Could not persist audio readiness", e);
  }
}

function selectedEditorValue(): string {
  const raw = settings.editor_command?.trim().toLowerCase() || "system";
  if (raw === "obsidian") return "obsidian";
  if (raw === "vscode" || raw === "code" || raw.startsWith("code ")) return "vscode";
  if (raw === "textedit" || raw.includes("textedit")) return "textedit";
  return "system";
}

function settingsReadinessMessageFromDom(): string {
  const notesPath = (document.getElementById("project-path") as HTMLInputElement | null)?.value.trim() || activeProject(settings).path || "";
  const mode = (document.querySelector('input[name="ai-mode"]:checked') as HTMLInputElement | null)?.value || currentAiMode();
  const apiKey = (document.getElementById("api-key") as HTMLInputElement | null)?.value.trim() || "";
  const missing: string[] = [];

  if (!notesPath) {
    missing.push("add a project");
  } else if (!notesFolderReady) {
    missing.push("choose an existing project folder");
  }

  if (mode === "chatgpt" && !aiStatus.chatgpt_authenticated) {
    missing.push("sign in to ChatGPT or choose Included");
  } else if (mode === "api" && !(isHostedWeb() ? modeReadiness("api")?.ready : Boolean(apiKey))) {
    missing.push(isHostedWeb() ? "configure the API credential on the server" : "enter an API key");
  }

  return missing.length === 0
    ? "Ready to save. Capture works now; connected note context can be added later."
    : `Required before first capture: ${missing.join("; ")}.`;
}

function settingsReadyFromDom(): boolean {
  return !settingsReadinessMessageFromDom().startsWith("Required");
}

function settingsRenderContext(): SettingsRenderContext {
  // If the user changed the device picker without saving, overlay the pending
  // selection so re-renders (e.g. from model download completing) never revert
  // the picker to the on-disk value. The pick is only committed to `settings`
  // when the user clicks Save; cleared when the overlay closes.
  let effectiveSettings: Settings = pendingDeviceSelection !== null ? {
    ...settings,
    input_device_mode: pendingDeviceSelection.mode,
    input_device_uid: pendingDeviceSelection.uid,
    input_device_name: pendingDeviceSelection.name,
  } : settings;
  // Overlay an unsaved Notes folder change the same way, so the pane renders the
  // staged path even after a disk reload clobbers `settings`.
  if (pendingVaultPathChange !== null) {
    effectiveSettings = stageProjectPathChange(
      effectiveSettings,
      activeProject(effectiveSettings).id,
      pendingVaultPathChange,
    );
  }
  return {
    settings: effectiveSettings,
    aiStatus,
    devices,
    audioTestRunning,
    audioTestError,
    audioTestResult,
    systemAudioTestRunning,
    systemAudioTestResult,
    systemAudioTestError,
    audioCheckPhase,
    speechModelDownloadInProgress,
    notesFolderReady,
    currentAiMode: currentAiMode(),
    aiReady: aiReady(),
    includedAiReady: includedReadyFromState(),
    aiReadinessText: aiReadinessText(),
    aiReadiness,
    aiPreview,
    cueTier: cueTierSelection ?? cueTierFromSettings(settings),
    selectedEditorValue: selectedEditorValue(),
    selectedDeviceName: selectedDeviceName(),
    liveCaptureActive: hostedCaptureBlocksStart() && !(hostedActiveRecordingStatus ?? recordingStatus).paused,
    captureDeviceState: recordingStatus.capture_device ?? null,
    audioTestMessage: audioTestMessage(),
    audioSetupReady: audioSetupReady(),
    audioResumeState,
    settingsReadyFromState: settingsReadyFromState(),
    settingsReadinessMessage: settingsReadinessMessageFromDom(),
    notesDestinationPath: (() => {
      const p = activeProject(effectiveSettings);
      return notesDestinationPath(p.path, p.inbox_folder || "");
    })(),
    activeSection: settingsActiveSection,
    hosted: isHostedWeb(),
    webMicrophonePermissionState,
    webMicrophoneFailureCategory,
    webMicrophoneEnvironment: webMicrophoneEnvironment(),
    webMicrophoneDeviceId,
    webMicrophoneRequesting,
    webMicrophoneError,
  };
}

function hydrateSettingsOverlay() {
  const vaultPath = activeProject(settings).path || settings.vault_path || DEFAULT_VAULT_PATH;
  if (vaultPath) {
    window.__validateVaultPath(vaultPath);
  } else {
    window.__updateSettingsSaveState();
  }
  // Keep keyboard focus inside the dialog so Escape/Tab operate on the modal
  // rather than the workspace behind it. Re-assert focus only when it is not
  // already inside the dialog — this pulls focus in on open and re-captures it
  // after the frequent background re-renders (which rebuild the overlay and
  // drop focus to <body>), while never yanking focus out of an input the user
  // is actively typing into.
  const dialog = document.getElementById("settings-dialog");
  if (dialog) {
    const active = document.activeElement;
    const focusInside = active != null && active !== document.body && dialog.contains(active);
    if (!focusInside) {
      const target = dialog.querySelector<HTMLElement>(".settings-nav-item.active") ?? dialog;
      target.focus({ preventScroll: true });
    }
  }
}

// Collect the visible, focusable controls inside the settings dialog, in DOM
// order, for the Tab focus trap.
function settingsFocusables(dialog: HTMLElement): HTMLElement[] {
  const selector = 'button, [href], input, select, textarea, [tabindex]:not([tabindex="-1"])';
  return Array.from(dialog.querySelectorAll<HTMLElement>(selector)).filter(
    el => !el.hasAttribute("disabled") && el.offsetParent !== null,
  );
}

// Close the settings overlay from a keyboard gesture and return focus to the
// control that opens it, so keyboard users are not stranded on <body>.
function closeSettingsAndRestoreFocus() {
  window.__closeSettings();
  window.setTimeout(() => {
    const gear = document.querySelector<HTMLElement>('.global-header-actions button[aria-label="Setup"]');
    gear?.focus({ preventScroll: true });
  }, 0);
}

// ---------------------------------------------------------------------------
// Actions (exposed on window for onclick handlers)
// ---------------------------------------------------------------------------

function navigationActionsContext(): NavigationActionsContext {
  return {
    get currentView() { return currentView; },
    set currentView(value) { currentView = value; },
    get settingsOverlayOpen() { return settingsOverlayOpen; },
    set settingsOverlayOpen(value) { settingsOverlayOpen = value; },
    get devices() { return devices; },
    set devices(value) { devices = value; },
    get aiStatus() { return aiStatus; },
    set aiStatus(value) { aiStatus = value; },
    get sessions() { return sessions; },
    set sessions(value) { sessions = value; },
    get calendarSuggestion() { return calendarSuggestion; },
    set calendarSuggestion(value) { calendarSuggestion = value; },
    get calendarSuggestionFreshness() { return calendarSuggestionFreshness; },
    set calendarSuggestionFreshness(value) { calendarSuggestionFreshness = value; },
    get recordingMounted() { return recordingMounted; },
    set recordingMounted(value) { recordingMounted = value; },
    get sidebarCollapsed() { return sidebarCollapsed; },
    set sidebarCollapsed(value) { sidebarCollapsed = value; },
    get sidebarWidth() { return sidebarWidth; },
    set sidebarWidth(value) { sidebarWidth = value; },
    get themeMode() { return themeMode; },
    set themeMode(value) { themeMode = value; },
    get sidebarFilterOpen() { return sidebarFilterOpen; },
    set sidebarFilterOpen(value) { sidebarFilterOpen = value; },
    get sessionFilters() { return sessionFilters; },
    set sessionFilters(value) { sessionFilters = value; },
    get globalSearchActive() { return globalSearchActive; },
    set globalSearchActive(value) { globalSearchActive = value; },
    get globalSearchQuery() { return globalSearchQuery; },
    set globalSearchQuery(value) { globalSearchQuery = value; },
    get sidebarCollapsedSections() { return sidebarCollapsedSections; },
    set sidebarCollapsedSections(value) { sidebarCollapsedSections = value; },
    beginPendingImport,
    exitTransientPrep: () => {
      if (preStartPrepOpen) abandonPreStartPrep();
    },
    render,
  };
}

registerNavigationActions(navigationActionsContext());

async function openSettingsSection(section: string) {
  settingsActiveSection = section;
  await window.__nav("settings");
}

window.__openAudioSetup = async () => {
  await openSettingsSection("audio");
};

window.__openPrivacyPane = async (pane: "microphone" | "system-audio") => {
  try {
    if (pane === "system-audio") systemAudioPrivacyPaneOpened = true;
    await openPrivacyPane(pane);
    render();
  } catch (e: any) {
    alert(`Could not open System Settings: ${e}`);
  }
};

window.__restartMarginsToAudioSetup = async () => {
  try {
    window.localStorage.setItem(SETUP_RESUME_SECTION_KEY, "audio");
  } catch {
    // Ignore storage errors; restart still works, but cannot restore context.
  }
  await restartApp();
};

// Runs the system-audio tap probe, persists readiness, and clears the restart
// resume intent on success. Shared by the manual Test button and the post-restart
// auto-confirm flow. Returns true when the tap is receiving signal.
async function runSystemAudioTest(): Promise<boolean> {
  systemAudioTestRunning = true;
  systemAudioTestError = null;
  systemAudioTestResult = null;
  render();
  try {
    systemAudioTestResult = await testSystemAudioTap();
    const ok = systemAudioTestResult.status === "ok";
    await persistAudioReadinessPatch({ system_audio_ready: ok });
    if (ok) {
      systemAudioPrivacyPaneOpened = false;
      resumedAfterPermissionRestart = false;
      clearAudioSetupResumeKey();
    }
    return ok;
  } catch (e: any) {
    systemAudioTestError = String(e);
    await persistAudioReadinessPatch({ system_audio_ready: false });
    return false;
  } finally {
    systemAudioTestRunning = false;
    render();
  }
}

function clearAudioSetupResumeKey() {
  try {
    window.localStorage.removeItem(SETUP_RESUME_SECTION_KEY);
  } catch {
    // Ignore storage errors.
  }
}

window.__testSystemAudioTap = async () => {
  audioResumeState = null;
  await runSystemAudioTest();
};

// Post-restart confirm: re-check the tap so the loop closes with a single press
// instead of hunting for the Test button. This runs the tap probe, so it must
// only be invoked from an explicit user gesture (the "resumed" card's Confirm
// button) — never at launch, which must stay ScreenCapture-TCC-clean. On success
// we show a confirmation with a Start capture affordance; otherwise we fall back
// to the blocked/restart callout.
async function confirmSystemAudioAfterRestart() {
  audioResumeState = "checking";
  render();
  const ok = await runSystemAudioTest();
  audioResumeState = ok ? "confirmed" : "blocked";
  render();
}

window.__confirmSystemAudioAfterRestart = async () => {
  await confirmSystemAudioAfterRestart();
};

window.__startNew = async () => {
  const suggestion = calendarSuggestion;
  void refreshCalendarSuggestionForStart();
  await startNamedMeeting(suggestion ? sessionNameFromSuggestion(suggestion) : defaultMeetingName());
};

window.__startDefaultMeeting = async () => {
  if (!firstCaptureNoteChoice) {
    setFirstCaptureNoteChoice("write");
  }
  const suggestion = await refreshCalendarSuggestionForStart();
  await startNamedMeeting(suggestion ? sessionNameFromSuggestion(suggestion) : defaultMeetingName());
};

window.__chooseFirstCaptureMode = async (choice: "write" | "capture") => {
  setFirstCaptureNoteChoice(choice);
  render();
  await window.__startDefaultMeeting();
};

window.__resetFirstCaptureChoice = () => {
  setFirstCaptureNoteChoice(null);
  render();
};

window.__startNewFrom = async (inputId: string) => {
  await startNewFromInput(inputId);
};

async function startNewFromInput(inputId: string) {
  const input = document.getElementById(inputId) as HTMLInputElement;
  const suggestion = input?.value.trim()
    ? calendarSuggestion
    : await refreshCalendarSuggestionForStart();
  const name = input?.value.trim() || input?.dataset.suggestedName || (suggestion ? sessionNameFromSuggestion(suggestion) : defaultMeetingName());
  await startNamedMeeting(name);
}

async function refreshCalendarSuggestionForStart(): Promise<CalendarEventSuggestion | null> {
  return getCalendarEventSuggestion()
    .then(acceptCalendarSuggestionResult)
    .catch(() => calendarSuggestion);
}

async function startNamedMeeting(name: string) {
  if (!captureReadyFromState()) {
    if (!activeProject(settings).path?.trim()) {
      await window.__addProject();
    } else {
      currentView = "home";
      render();
    }
    return;
  }
  if (!name) return;

  // First-run: do NOT pre-gate capture on audio setup. Pressing capture is
  // exactly what should trigger the permission prompts inline — native mic +
  // system-audio prompts on macOS, the browser's own getUserMedia prompt on
  // hosted web. The user never has to visit Settings first; startup failures
  // land in the failed-capture posture with per-category recovery guidance,
  // and a missing transcription model degrades the live transcript instead of
  // blocking the recording.
  const sanitized = slugifyName(name);
  if (!sanitized) return;

  if (hostedCaptureBlocksStart() || recordingStartup?.phase === "starting" || captureTransitionActive()) {
    activeSessionName = hostedActiveRecordingStatus?.session_name || recordingStatus.session_name || recordingStartup?.sessionName || activeSessionName;
    activeSessionTab = "backchannel";
    currentView = "home";
    render();
    return;
  }

  beginOptimisticRecording(sanitized);
}

function beginOptimisticRecording(requestedName: string, seedMemoState?: { lines: MemoLine[]; startTime: number }) {
  hostedRecoveryState.clearSelection();
  const optimisticName = uniqueOptimisticSessionName(requestedName);
  forgetDeletedSession(optimisticName);
  const projectId = activeProjectId();
  const startedAt = new Date();
  clearPendingMemoDraft();
  memoLines = seedMemoState ? seedMemoState.lines : [];
  pendingLineStartSecs = null;
  memoStartTime = seedMemoState ? seedMemoState.startTime : Date.now();
  // Non-prep recordings: clear block margin state
  if (!seedMemoState) {
    blockMargins.clear();
    blockWallClockByOrdinal.clear();
  }
  // A previous session may still be finalizing its WAV in the background. Clear the
  // target so the deferred-navigation callback knows the user has moved on.
  finalizingCaptureSessionName = null;
  dismissedTapNoticeSession = null;
  tapRecoverySession = null;
  tapRecoveryState = "idle";
  tapRecoveryError = null;
  lastCaptureDeviceState = null;
  holdingRecoveryDeviceName = null;
  pendingCaptureDeviceSwitchFrom = null;
  pendingCaptureDeviceSwitchTo = null;
  manualCaptureDeviceSwitchPending = false;
  manualCaptureDeviceSwitchTarget = null;
  clearCaptureDeviceSwitchToast();
  captureSpeakerCount = 0;
  queuedMemoCheckpointIndices.clear();
  queuedBackchannelRequestIndices.clear();
  memoCheckpointInFlight.clear();
  clearBackchannelState();
  // Reset block ordinal for a fresh capture (prep already set block 0; non-prep starts at 0)
  // Don't reset blockMargins here — beginOptimisticRecordingFromPrep seeds with existing block 0
  currentBlockOrdinal = 0;
  // A fresh capture starts with the clock running — clear any stale pause freeze.
  pausedAtWallMs = null;
  activeSessionName = optimisticName;
  activeSessionTab = "backchannel";
  currentView = "home";
  recordingMounted = false;
  recordingStatus = {
    is_recording: false,
    paused: false,
    session_name: optimisticName,
    elapsed_secs: 0,
    input_device_name: null,
    capture_device: { state: "active", device_name: "System Default" },
    mic_level: 0,
    mic_audio_frame_count: 0,
    spk_level: 0,
    mic_drop_count: 0,
    spk_drop_count: 0,
    mic_gap_ms_total: 0,
    mic_switch_count: 0,
    timeline_reusable: true,
    speaker_silence_secs: 0,
    system_audio_expected: false,
    system_audio_frame_count: 0,
    system_audio_observed: false,
    system_audio_seen: false,
    tap_status: "not_expected",
    tap_warning: null,
    live_transcription_mode: "stereo_split",
    capture_phase: "idle",
  };
  recordingStartup = {
    id: ++recordingStartupSeq,
    projectId,
    phase: "starting",
    requestedName,
    sessionName: optimisticName,
    message: "Preparing microphone and computer audio…",
    error: null,
    waitingForPermission: false,
    webMicrophoneFailure: null,
  };
  sessions = [optimisticSessionInfo(optimisticName, startedAt, projectId), ...sessions.filter(s => s.name !== optimisticName)];
  startPolling();
  render();
  recordingStartPromise = finishRecordingStart(recordingStartup.id, requestedName, optimisticName);
}

function optimisticSessionInfo(name: string, startedAt = new Date(), projectId = activeProjectId()): SessionInfo {
  return {
    name,
    project_id: projectId,
    start_time: startedAt.toISOString(),
    notes_path: `${name}.md`,
    title: null,
    segment_count: 0,
    duration_secs: 0,
    memo_line_count: memoLines.length,
    status: "recording",
    vault_note_path: null,
    people: [],
  };
}

function rememberDeletedSession(name: string) {
  if (name.trim()) deletedSessionNames.add(name);
}

function forgetDeletedSession(name: string) {
  deletedSessionNames.delete(name);
}

function visibleSessions(items: SessionInfo[]): SessionInfo[] {
  if (deletedSessionNames.size === 0) return items;
  return items.filter(s => !deletedSessionNames.has(s.name));
}

function uniqueOptimisticSessionName(base: string): string {
  const existing = new Set(sessions.map(s => s.name));
  const cleanBase = base.replace(/^-+|-+$/g, "") || "meeting";
  for (let ordinal = 1; ordinal < 1000; ordinal++) {
    const suffix = ordinal <= 1 ? "" : `-${ordinal}`;
    const stem = cleanBase.slice(0, Math.max(1, 90 - suffix.length)).replace(/-+$/g, "") || "meeting";
    const candidate = `${stem}${suffix}`;
    if (!existing.has(candidate)) return candidate;
  }
  return `${cleanBase.slice(0, 78).replace(/-+$/g, "")}-${Date.now()}`;
}

function updateRecordingStartup(id: number, patch: Partial<Pick<RecordingStartupState, "phase" | "message" | "error" | "sessionName" | "cancelRequested" | "waitingForPermission" | "webMicrophoneFailure">>) {
  if (!recordingStartup || recordingStartup.id !== id) return false;
  recordingStartup = { ...recordingStartup, ...patch };
  return true;
}

async function finishRecordingStart(id: number, requestedName: string, optimisticName: string) {
  try {
    updateRecordingStartup(id, { message: "Finding your microphone…" });
    updateRecordingHeader();
    const latestDevices = await listDevices().catch(() => devices);
    if (recordingStartup?.id !== id) return;
    devices = latestDevices;
    if (recordingStartup.cancelRequested) {
      throw new Error("Recording startup cancelled.");
    }
    updateRecordingStartup(id, isHostedWeb()
      ? { message: "Waiting for microphone permission…", waitingForPermission: true }
      : { message: "Starting microphone and computer audio…", waitingForPermission: false });
    updateRecordingHeader();
    const actualName = await startRecording(requestedName, captureDeviceUidForStart(), recordingStartup?.projectId ?? activeProjectId());
    if (recordingStartup?.id !== id) return;
    if (recordingStartup.cancelRequested) {
      // The route may have crossed the finish line at the same instant as the
      // cancel click. Retire that just-opened recorder instead of presenting a
      // capture the user explicitly ended.
      await discardRecording(recordingStatus.web_recording_id).catch(() => {});
      throw new Error("Recording startup cancelled.");
    }
    if (isHostedWeb()) {
      // Device labels are usually revealed only after the first successful
      // grant. Refresh now so Audio setup has the real browser-native list.
      devices = await listDevices().catch(() => devices);
    }
    if (recordingStartup?.id !== id) return;
    if (recordingStartup.cancelRequested) {
      await discardRecording(recordingStatus.web_recording_id).catch(() => {});
      throw new Error("Recording startup cancelled.");
    }
    forgetDeletedSession(actualName);

    const pendingMemo = memoLines.map(line => ({ ...line }));
    const listedSessions = await listSessions().then(visibleSessions).catch(() => sessions);
    sessions = mergeStartedSession(listedSessions, optimisticName, actualName);
    activeSessionName = actualName;
    activeSessionTab = "backchannel";
    recordingStartup = null;
    transcriptWarming = true;
    transcriptDegraded = false;
    const startedStatus = await getRecordingStatus();
    hostedActiveRecordingStatus = isHostedWeb()
      ? hostedRecoveryState.activeStatus(startedStatus)
      : null;
    applyRecordingStatus(startedStatus);
    capturePendingDraftForNextRender();
    recordingMounted = false;
    currentView = "home";
    render();

    await flushQueuedMemoAfterRecordingStart(pendingMemo);

    // Fire-and-forget: probe speech models and, on the first capture that finds
    // them missing, drive the download inline so the user never had to set it up
    // in Settings first. Recording is already underway; it transcribes once the
    // model is warm.
    void triggerModelProvisionCheck({ autoDownload: true });
  } catch (e: any) {
    if (!recordingStartup || recordingStartup.id !== id) return;
    const message = String(e?.message || e);
    const webMicrophoneFailure = isWebMicrophoneFailure(e) ? e.category : null;
    recordingStartup = {
      ...recordingStartup,
      phase: "failed",
      cancelRequested: false,
      message: /startup cancelled/i.test(message)
        ? "Audio startup cancelled."
        : "Audio setup needs attention.",
      error: message,
      waitingForPermission: false,
      webMicrophoneFailure,
    };
    recordingStatus = {
      ...recordingStatus,
      is_recording: false,
      session_name: optimisticName,
      elapsed_secs: displayElapsedSecs(),
      capture_device: { state: "active", device_name: "System Default" },
      mic_level: 0,
      mic_audio_frame_count: 0,
      spk_level: 0,
      capture_phase: "idle",
    };
    capturePendingDraftForNextRender();
    recordingMounted = false;
    stopPolling();
    render();
  } finally {
    if (!recordingStartup || recordingStartup.id === id) recordingStartPromise = null;
  }
}

// Default model path as reported by the backend when parakeet_model_dir is null.
// Used to decide whether a "ready" probe result is at an unexpected/custom path.
const DEFAULT_PARAKEET_DIR_SUFFIX = "margins/models/parakeet-tdt-0.6b-v3-int8";

async function triggerModelProvisionCheck(opts: { autoDownload?: boolean } = {}): Promise<void> {
  try {
    const probe = await probeSpeechModels();
    if (probe.transcription === "unsupported") return;

    if (probe.transcription === "missing") {
      // State A: need to download transcription model
      if (!modelProvisionNotice || modelProvisionNotice.state === "hidden") {
        modelProvisionNotice = {
          state: "A",
          downloadBytes: probe.download_bytes,
          foundPath: null,
          progress: null,
          message: "",
        };
        render();
      }
      // Capture-driven inline download: the first capture drives the download
      // itself so the user never has to detour through Settings. Guarded to run
      // at most once per app run and not while a download is already going.
      if (
        opts.autoDownload &&
        !autoModelDownloadAttempted &&
        !speechModelDownloadInProgress &&
        modelProvisionNotice?.state === "A"
      ) {
        autoModelDownloadAttempted = true;
        void window.__modelNoticeDownload();
      }
      return;
    }

    // transcription === "ready"
    const path = probe.transcription_path || "";
    const savedDir = settings.parakeet_model_dir || "";
    const isDefaultPath = path.endsWith(DEFAULT_PARAKEET_DIR_SUFFIX);
    const isAlreadySaved = savedDir && path && (path === savedDir || path.startsWith(savedDir));

    if (!isAlreadySaved && !isDefaultPath && path) {
      // State C: found at unexpected path — offer reuse
      if (!modelProvisionNotice || modelProvisionNotice.state === "hidden") {
        modelProvisionNotice = {
          state: "C",
          downloadBytes: null,
          foundPath: path,
          progress: null,
          message: "",
        };
        render();
      }
      return;
    }

    if (probe.diarization === "missing") {
      // State B: transcription ready, speaker labels still loading
      if (!modelProvisionNotice || modelProvisionNotice.state === "hidden") {
        modelProvisionNotice = {
          state: "B",
          downloadBytes: null,
          foundPath: null,
          progress: null,
          message: "",
        };
        render();
      }
    }
    // Both ready — no notice needed.
  } catch (e) {
    // probe failures are non-critical; silently ignore
    console.warn("[model-provision] probe failed:", e);
  }
}

// Session name of the capture that must stay a live `recording` row through any
// wholesale session-list replacement: the one actively recording, or the one
// whose audio is still warming up. Backend list_sessions only reports a session
// as recording once startup fully completes, so an unguarded refresh landing in
// that window downgrades the live workspace to the settled-note view.
function liveCaptureRowName(): string | null {
  if (isHostedWeb()) return hostedActiveRecordingStatus?.session_name || null;
  if (recordingStatus.is_recording) return recordingStatus.session_name || null;
  if (recordingStartup?.phase === "starting") return recordingStartup.sessionName;
  return null;
}

function mergeRefreshedSessionList(listed: SessionInfo[]): SessionInfo[] {
  const name = liveCaptureRowName();
  const fallback = name
    ? sessions.find(s => s.name === name)
      ?? optimisticSessionInfo(name, new Date(), recordingStartup?.projectId ?? activeProjectId())
    : null;
  // A capture belongs to the project it was started in — never inject its row
  // into another project's freshly listed sessions.
  const captureName = name && (listed.some(s => s.name === name) || (fallback?.project_id ?? null) === (activeProjectId() ?? null))
    ? name
    : null;
  return mergeRefreshedSessions(listed, sessions, {
    captureName,
    captureFallback: fallback,
    memoLineCount: memoLines.length,
    processingName: processingSessionName || null,
  });
}

function mergeStartedSession(listedSessions: SessionInfo[], optimisticName: string, actualName: string): SessionInfo[] {
  const actual = listedSessions.find(s => s.name === actualName);
  const optimistic = sessions.find(s => s.name === optimisticName) || optimisticSessionInfo(actualName);
  const started = {
    ...(actual || optimistic),
    name: actualName,
    status: "recording" as const,
    memo_line_count: memoLines.length,
  };
  return [started, ...listedSessions.filter(s => s.name !== actualName), ...sessions.filter(s => s.name !== optimisticName && s.name !== actualName && !listedSessions.some(listed => listed.name === s.name))];
}

async function flushQueuedMemoAfterRecordingStart(lines: MemoLine[]) {
  const sessionName = memoCaptureSessionName();
  if (!recordingStatus.is_recording || !sessionName) return;
  try {
    await syncMemo(lines, sessionName, recordingStatus.web_recording_id);
  } catch (e) {
    console.warn("Initial memo sync failed", e);
  }
  const checkpointIndices = [...queuedMemoCheckpointIndices].sort((a, b) => a - b);
  queuedMemoCheckpointIndices.clear();
  for (const index of checkpointIndices) {
    runMemoCheckpoint(index);
  }
  const backchannelIndices = [...queuedBackchannelRequestIndices].sort((a, b) => a - b);
  queuedBackchannelRequestIndices.clear();
  for (const index of backchannelIndices) {
    fireBackchannelRequestAfterCheckpoint(index);
  }
}

// ---------------------------------------------------------------------------
// Audio import (drag-and-drop + browse fallback)
// ---------------------------------------------------------------------------

function isAcceptedAudioPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return (IMPORT_AUDIO_EXTENSIONS as readonly string[]).includes(ext);
}

function audioFileStem(path: string): string {
  const base = path.split(/[\\/]+/).filter(Boolean).pop() ?? path;
  return base.replace(/\.[^.]+$/, "");
}

function importTitleFromStem(stem: string): string {
  const words = stem.replace(/[-_]+/g, " ").trim();
  return words ? words.replace(/\b\w/g, c => c.toUpperCase()) : "Imported audio";
}

function importSpeakerMax(): number | null {
  return speakerCountToMax(settings.import_speaker_count ?? 1);
}

function announceImportStatus(text: string) {
  importAnnouncement = text;
}

// Map a processing-progress stage to the coarse import phase shown in the row.
function importPhaseForStage(stage: string): "transcribing" | "note" {
  return ["align", "synthesize", "note_stream", "cleanup", "complete"].includes(stage) ? "note" : "transcribing";
}

function optimisticImportSessionInfo(name: string, title: string, path: string, startedAt: Date, status: SessionInfo["import_status"] = "transcribing", projectId = activeProjectId()): SessionInfo {
  return {
    name,
    project_id: projectId,
    start_time: startedAt.toISOString(),
    notes_path: `${name}.md`,
    title,
    segment_count: 0,
    duration_secs: 0,
    memo_line_count: 0,
    status: "processing",
    vault_note_path: null,
    people: [],
    source: "session",
    import_status: status,
    import_source_path: path,
  };
}

// A drop (or browse pick) opens the configure step instead of firing the
// single-shot import. The speaker count is locked when import_audio_file runs
// (diarization is mid-transcription, no re-run), so the count must be chosen
// before Transcribe — hence the pending card rather than an immediate import.
function beginPendingImport(paths: string[]) {
  const audio = paths.filter(isAcceptedAudioPath);
  if (audio.length === 0) {
    if (paths.length > 0) {
      announceImportStatus("Audio files only — wav · m4a · mp3 · flac · aac");
      render();
    }
    return;
  }
  pendingImport = { paths: audio, speakerCount: settings.import_speaker_count ?? 1, projectId: activeProjectId() };
  announceImportStatus(`Ready to transcribe ${audio.length === 1 ? importTitleFromStem(audioFileStem(audio[0])) : `${audio.length} files`}`);
  render();
}

function isAcceptedGranolaImportPath(path: string): boolean {
  const ext = path.split(".").pop()?.toLowerCase() ?? "";
  return (GRANOLA_IMPORT_EXTENSIONS as readonly string[]).includes(ext);
}

// Dropped Granola exports are surveyed in memory, staged, and committed as
// native notes. Re-imports intentionally allocate new collision-safe filenames.
async function beginGranolaImport(paths: string[]) {
  const granolaPaths = paths.filter(isAcceptedGranolaImportPath);
  if (granolaPaths.length === 0) {
    announceImportStatus("Granola exports should be .json, .jsonl, or .csv");
    render();
    return;
  }
  const projectId = activeProjectId();
  settingsOverlayOpen = false;
  currentView = "home";
  announceImportStatus(`Importing ${granolaPaths.length === 1 ? "Granola export" : `${granolaPaths.length} Granola exports`}`);
  render();
  try {
    const survey = await surveyGranolaImport(granolaPaths, projectId);
    const result = await importGranolaFiles(granolaPaths, {
      notes_folder: survey.suggested_notes_folder || activeProject(settings).inbox_folder || "meetings",
      people_folder: survey.suggested_people_folder || activeProject(settings).people_folder || "people",
      organizations_folder: survey.suggested_organizations_folder || "organizations",
    }, projectId);
    if (result.imported_count > 0 && projectId) {
      await setProjectReadiness(projectId, "ready").catch(() => {});
    }
    const summary = granolaResultSummary(result.imported_count) ?? "No Granola meetings found";
    announceImportStatus(result.warnings.length ? `${summary} — ${result.warnings.length} warning${result.warnings.length === 1 ? "" : "s"}` : summary);
  } catch (e: any) {
    announceImportStatus(String(e?.message ?? e ?? "Granola import failed."));
  }
  render();
}

async function importAudioPaths(paths: string[], maxSpeakers: number | null, projectId = activeProjectId()) {
  const audio = paths.filter(isAcceptedAudioPath);
  if (audio.length === 0) {
    if (paths.length > 0) {
      announceImportStatus("Audio files only — wav · m4a · mp3 · flac · aac");
      render();
    }
    return;
  }
  // One session per file. Sequential because the shared processing-progress
  // channel carries no session name, so concurrent imports would interleave.
  // For multi-file drops, eagerly insert a 'queued' row for each file beyond
  // the first so the sidebar shows waiting state instead of an idle-looking row.
  if (audio.length > 1) {
    const now = new Date();
    for (let i = 1; i < audio.length; i++) {
      const path = audio[i];
      const stem = audioFileStem(path);
      const sanitized = slugifyName(stem);
      const optimisticName = uniqueOptimisticSessionName(sanitized || "imported-audio");
      const title = importTitleFromStem(stem);
      sessions = [optimisticImportSessionInfo(optimisticName, title, path, now, "queued", projectId), ...sessions.filter(s => s.name !== optimisticName)];
    }
    render();
  }
  for (const path of audio) {
    await importOneAudioFile(path, maxSpeakers, projectId);
  }
}

async function importOneAudioFile(path: string, maxSpeakers: number | null, projectId: string | null) {
  // Remove any queued placeholder row for this exact path so it doesn't
  // collide with uniqueOptimisticSessionName and leaves no ghost row.
  sessions = sessions.filter(s => !(s.import_status === "queued" && s.import_source_path === path));
  const stem = audioFileStem(path);
  const sanitized = slugifyName(stem);
  const optimisticName = uniqueOptimisticSessionName(sanitized || "imported-audio");
  const title = importTitleFromStem(stem);

  sessions = [optimisticImportSessionInfo(optimisticName, title, path, new Date(), "transcribing", projectId), ...sessions.filter(s => s.name !== optimisticName)];
  activeSessionName = optimisticName;
  activeSessionTab = "distill";
  currentView = "home";
  settingsOverlayOpen = false;
  processingSessionName = optimisticName;
  processingSteps = [];
  transcriptEntries = [];
  currentProgress = 0.04;
  const optimisticStateKey = sessionStateKeyForProjectId(optimisticName, projectId);
  distillTraceBySession[optimisticStateKey] = [];
  lifecycleBySession[optimisticName] = newSessionLifecycleState(optimisticName);
  groundedNoteBySession[optimisticStateKey] = newGroundedNoteState();
  announceImportStatus(`Importing ${title}`);
  render();

  processingRunId += 1;
  const runId = processingRunId;
  if (currentProcessingUnlisten) {
    try { currentProcessingUnlisten(); } catch {}
    currentProcessingUnlisten = null;
  }

  const unlisten = await onProcessingProgress((event: ProcessingEvent) => {
    if (runId !== processingRunId) return;
    rememberLifecycleEvent(optimisticName, event);
    let shouldRender = event.stage !== "note_stream";
    if (event.stage === "transcript" && event.entry) {
      transcriptEntries.push(event.entry);
    } else if (event.stage === "note_stream") {
      shouldRender = consumeNoteStreamChunkForStateKey(optimisticStateKey, event.message);
      rememberNoteWritingTraceForStateKey(optimisticStateKey, optimisticName, event.progress);
      processingSteps.push(event);
    } else {
      processingSteps.push(event);
      rememberProcessingTraceForStateKey(optimisticStateKey, event);
    }
    if (event.progress !== null) currentProgress = event.progress;
    const phase = importPhaseForStage(event.stage);
    sessions = sessions.map(s => (s.name === optimisticName && s.import_status && s.import_status !== phase ? { ...s, import_status: phase } : s));
    renderProcessingProgress(event.stage, shouldRender);
  });
  currentProcessingUnlisten = unlisten;
  const detach = () => {
    if (currentProcessingUnlisten === unlisten) currentProcessingUnlisten = null;
    try { unlisten(); } catch {}
  };

  try {
    const realName = await importAudioFile(path, maxSpeakers, projectId);
    if (runId !== processingRunId) { detach(); return; }
    detach();
    // Clear the processing marker before refreshing: the refresh merge keeps an
    // in-flight import's progress badge alive keyed on processingSessionName,
    // and this import is no longer in flight.
    processingSessionName = "";
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    // The backend slugs its own name from the file stem; drop the optimistic
    // placeholder if it differs so we don't leave a duplicate row.
    if (realName !== optimisticName) sessions = sessions.filter(s => s.name !== optimisticName);
    currentProgress = 1;
    activeSessionName = realName;
    activeSessionTab = "distill";
    flashImportSessionName = realName;
    announceImportStatus(`Note ready for ${title}`);
    render();
    await window.__loadArtifacts(realName);
    window.setTimeout(() => {
      if (flashImportSessionName === realName) {
        flashImportSessionName = null;
        render();
      }
    }, 1300);
  } catch (e: any) {
    detach();
    if (runId !== processingRunId) return;
    const message = String(e?.message ?? e ?? "").replace(/^Error:\s*(Error:\s*)?/, "").trim() || "Margins could not import this audio file.";
    // Reconcile the optimistic row into a visible error/retry state rather than
    // leaving a silent dangling row (Phase-1 orphan caveat). We keep the
    // client-side placeholder as the single visible artifact.
    processingSessionName = "";
    currentProgress = 0;
    sessions = sessions.map(s => (s.name === optimisticName
      ? { ...s, status: "unprocessed", import_status: "error", import_error: message }
      : s));
    activeSessionName = optimisticName;
    announceImportStatus(`Import failed for ${title}`);
    render();
  }
}

window.__setImportSpeakerCount = async (count: number) => {
  const clamped = Math.max(1, Math.min(4, Math.round(count)));
  if ((settings.import_speaker_count ?? 1) === clamped) return;
  settings = { ...settings, import_speaker_count: clamped };
  render();
  try {
    await updateSettings(settings);
  } catch (e) {
    console.warn("Could not save import speaker default", e);
  }
};

window.__adjustImportSpeakerCount = (delta: number) => {
  void window.__setImportSpeakerCount((settings.import_speaker_count ?? 1) + delta);
};

// Pending configure card: adjusts only the in-flight pending import's count,
// never the saved Settings default (that's __setImportSpeakerCount).
window.__setPendingSpeakerCount = (count: number) => {
  if (!pendingImport) return;
  pendingImport = { ...pendingImport, speakerCount: Math.max(1, Math.min(4, Math.round(count))) };
  render();
};

window.__confirmPendingImport = async () => {
  const pending = pendingImport;
  if (!pending) return;
  pendingImport = null;
  render();
  await importAudioPaths(pending.paths, speakerCountToMax(pending.speakerCount), pending.projectId);
};

window.__cancelPendingImport = () => {
  pendingImport = null;
  announceImportStatus("");
  render();
};

window.__importAudioBrowse = async () => {
  const paths = await pickAudioFiles();
  if (paths.length) beginPendingImport(paths);
};

function granolaResultSummary(imported: number): string | null {
  return imported > 0
    ? `Imported ${imported} Granola meeting${imported === 1 ? "" : "s"}`
    : null;
}

// The sidebar action authorizes OAuth when needed, then imports one bounded
// snapshot and refreshes product recall.
window.__granolaImport = async () => {
  if (granolaBusy) return;
  try {
    if (!granolaStatus.authorized) {
      granolaBusy = "authorizing";
      announceImportStatus("Authorizing Granola import");
      render();
      granolaStatus = await authorizeGranolaImport();
      if (!granolaStatus.authorized) {
        announceImportStatus(granolaStatus.message || "Granola authorization failed");
        return;
      }
    }
    granolaBusy = "importing";
    granolaImportStage = "Importing Granola meetings";
    announceImportStatus("Importing Granola meetings");
    render();
    const projectId = activeProjectId();
    const result = await importGranolaMcp(projectId);
    granolaStatus = await getGranolaImportStatus().catch(() => granolaStatus);
    if (result.imported_count > 0 && projectId) {
      await setProjectReadiness(projectId, "ready").catch(() => {});
    }
    const planGatedSuffix = result.transcripts_plan_gated && result.imported_count > 0 ? " — no transcripts on this plan" : "";
    const summary = (granolaResultSummary(result.imported_count) ?? "No Granola meetings available to import") + planGatedSuffix;
    announceImportStatus(result.warnings.length ? `${summary} — ${result.warnings.length} warning${result.warnings.length === 1 ? "" : "s"}` : summary);
    if (result.transcripts_plan_gated && !granolaPlanCalloutDismissed) {
      granolaPlanCalloutVisible = true;
    }
  } catch (e: any) {
    announceImportStatus(String(e?.message ?? e ?? "Granola import failed."));
  } finally {
    granolaBusy = null;
    granolaImportStage = null;
    render();
  }
};

window.__dismissGranolaPlanCallout = () => {
  granolaPlanCalloutVisible = false;
  granolaPlanCalloutDismissed = true;
  try { window.localStorage.setItem(GRANOLA_PLAN_CALLOUT_DISMISSED_KEY, "1"); } catch {}
  render();
};

window.__retryImport = async (name: string) => {
  const session = sessions.find(s => s.name === name);
  const path = session?.import_source_path ?? null;
  sessions = sessions.filter(s => s.name !== name);
  if (activeSessionName === name) activeSessionName = null;
  render();
  if (path) await importOneAudioFile(path, importSpeakerMax(), session?.project_id || activeProjectId());
};

window.__openSession = async (name: string, status: string, projectId?: string | null) => {
  closeMobileNavigation();
  const requestSeq = ++openSessionRequestSeq;
  // Sidebar navigation is an explicit exit from transient prep. In particular,
  // disarm its timer before any asynchronous project switch so a hidden draft
  // can never begin recording behind the session the user chose.
  if (preStartPrepOpen) abandonPreStartPrep();

  if (projectId && projectId !== activeProjectId()) {
    const nextSettings = normalizeSettings({ ...settings, active_project_id: projectId });
    const nextSessions = await listSessions(projectId).then(visibleSessions).catch(() => projectSessionsById[projectId] || []);
    if (requestSeq !== openSessionRequestSeq) return;

    // Persist the active-project switch only once this is confirmed the latest
    // selection, so a superseded rapid switch can't win the on-disk setting.
    await updateSettings(nextSettings).catch(() => undefined);
    if (requestSeq !== openSessionRequestSeq) return;

    settings = nextSettings;
    sessions = mergeRefreshedSessionList(nextSessions);
    projectSessionsById[projectId] = nextSessions;
  }
  activeSessionName = name;
  const session = sessions.find(s => s.name === name);
  activeSessionTab = session ? defaultTabForSession(session) : defaultTabForStatus(status as SessionInfo["status"]);

  if (recordingStartup?.sessionName === name) {
    currentView = "home";
    activeSessionTab = "backchannel";
    render();
    return;
  }

  if (status === "recording") {
    devices = await listDevices();
    applyRecordingStatus(await getRecordingStatus().catch(() => recordingStatus));
    memoLines = readHarnessMemoLines() ?? memoLines;
    ensureMemoClockForRecordingSession(session ?? null);
    currentView = "home";
    activeSessionTab = "backchannel";
    startPolling();
    render();
    return;
  }

  currentView = "home";
  render();
  await window.__loadArtifacts(name);
};

window.__saveSessionTitle = async (name: string, title: string) => {
  const session = sessions.find(s => s.name === name);
  if (!session) return;

  const currentTitle = sessionDescription(session).trim();
  const fallbackTitle = sessionTitle(session);
  const cleaned = title.trim();
  const titleForSave = cleaned === fallbackTitle && !currentTitle && !session.vault_note_path ? "" : cleaned;
  if (titleForSave === currentTitle) return;

  sessions = sessions.map(s => s.name === name ? { ...s, title: titleForSave || null, frontmatter_title: s.vault_note_path ? titleForSave || null : s.frontmatter_title } : s);
  render();

  try {
    const saved = await updateSessionTitle(name, titleForSave);
    sessions = sessions.map(s => s.name === name ? {
      ...s,
      title: saved.title,
      frontmatter_title: s.vault_note_path ? saved.title : s.frontmatter_title,
      vault_note_path: saved.vault_note_path || s.vault_note_path,
      notes_path: saved.vault_note_path || s.notes_path,
    } : s);
  } catch (e: any) {
    alert(`Failed to update title: ${e}`);
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
  }
  render();
};

window.__sessionTitleKeydown = (event: KeyboardEvent, name: string) => {
  const input = event.currentTarget as HTMLInputElement | null;
  if (!input) return;
  if (event.key === "Enter") {
    event.preventDefault();
    input.blur();
  } else if (event.key === "Escape") {
    event.preventDefault();
    const session = sessions.find(s => s.name === name);
    if (session) input.value = sessionDescription(session);
    input.blur();
  }
};

window.__setSessionTab = (tab: SessionTab) => {
  const active = activeSessionName ? sessions.find(s => s.name === activeSessionName) : null;
  if (active && isCaptureNote(active) && tab === "backchannel") return;
  if (tab === "distill") {
    if (active && !canOpenNotePanel(active)) return;
  }
  activeSessionTab = tab;
  render();
  if (activeSessionName) window.__loadArtifacts(activeSessionName);
};

async function loadArtifacts(name: string, options: { renderLoading?: boolean } = {}) {
  const renderLoading = options.renderLoading ?? true;
  const projectId = projectIdForSession(name);
  const stateKey = sessionStateKeyForProjectId(name, projectId);
  artifactCache[stateKey] = { ...(artifactCache[stateKey] || {}), loading: true };
  if (renderLoading && currentView === "home" && activeSessionName === name) render();
  try {
    const [alignedResult, memoResult, noteResult, groundingResult, traceResult] = await Promise.allSettled([
      getAlignedContent(name, projectId),
      getSessionMemo(name, projectId),
      getVaultNote(name, projectId),
      getSessionGrounding(name, projectId),
      getDistillTrace(name, projectId),
    ]);
    // This read may have started on the captured-note screen and settled after
    // processing initialized. Never let that stale snapshot replace a live
    // stream with an empty/old note or trace.
    const processingNow = processingSessionName === name
      || sessions.some(s => s.name === name && s.status === "processing");
    const next = { ...(artifactCache[stateKey] || {}), loading: false };
    if (alignedResult.status === "fulfilled") next.aligned = alignedResult.value;
    if (memoResult.status === "fulfilled") {
      const parsed = parseSessionMemo(memoResult.value);
      if (parsed.length > 0 || !memoLinesBySession[stateKey]) memoLinesBySession[stateKey] = parsed;
    }
    if (!processingNow && noteResult.status === "fulfilled") {
      next.note = noteResult.value;
      groundedNoteBySession[stateKey] = groundedStateFromMarkdown(noteResult.value);
      reviewContent = cleanVisibleMarkdown(noteResult.value);
      reviewSessionName = name;
    } else if (!processingNow && noteResult.status !== "fulfilled") {
      delete next.note;
      groundedNoteBySession[stateKey] = newGroundedNoteState();
      if (reviewSessionName === name) {
        reviewContent = "";
        reviewSessionName = "";
      }
    }
    if (!processingNow && groundingResult.status === "fulfilled") {
      const uses = groundingUsesFromValue(groundingResult.value);
      if (uses.length) {
        groundedNoteBySession[stateKey] = mergeGroundingUsesIntoState(
          groundedNoteBySession[stateKey] || newGroundedNoteState(),
          uses,
        );
      }
    }
    if (!processingNow && traceResult.status === "fulfilled" && traceResult.value.length > 0) {
      const noteAlreadyLoaded = noteResult.status === "fulfilled";
      distillTraceBySession[stateKey] = [];
      if (!groundedNoteBySession[stateKey]) groundedNoteBySession[stateKey] = newGroundedNoteState();
      for (const event of traceResult.value) {
        if (event.stage === "note_stream") {
          if (!noteAlreadyLoaded) consumeNoteStreamChunkForStateKey(stateKey, event.message);
          rememberNoteWritingTraceForStateKey(stateKey, name, event.progress);
        } else {
          distillTraceBySession[stateKey] = appendProcessingTrace(distillTraceBySession[stateKey] || [], event);
        }
      }
      mergeTraceVaultRefsIntoGroundingForStateKey(stateKey);
    }
    artifactCache[stateKey] = next;
  } catch {
    artifactCache[stateKey] = { ...(artifactCache[stateKey] || {}), loading: false };
  }
  if (currentView === "home" && activeSessionName === name) render();
}

window.__loadArtifacts = async (name: string) => {
  await loadArtifacts(name, { renderLoading: true });
};

function invalidateSessionArtifacts(name: string) {
  const stateKey = sessionStateKey(name);
  delete artifactCache[stateKey];
  delete groundedNoteBySession[stateKey];
  delete distillTraceBySession[stateKey];
  delete artifactCache[name];
  delete groundedNoteBySession[name];
  delete distillTraceBySession[name];
  if (reviewSessionName === name) {
    reviewContent = "";
    reviewSessionName = "";
  }
}

function invalidateChangedSessionArtifacts(previous: SessionInfo[], next: SessionInfo[]) {
  const nextByName = new Map(next.map(session => [session.name, session]));
  for (const oldSession of previous) {
    const updated = nextByName.get(oldSession.name);
    if (!updated) {
      invalidateSessionArtifacts(oldSession.name);
      continue;
    }
    if (
      oldSession.vault_note_path !== updated.vault_note_path
      || oldSession.status !== updated.status
      || oldSession.source !== updated.source
      || oldSession.frontmatter_title !== updated.frontmatter_title
      || oldSession.frontmatter_created !== updated.frontmatter_created
      || oldSession.frontmatter_reflection_type !== updated.frontmatter_reflection_type
      || (oldSession.frontmatter_tags || []).join("\u0000") !== (updated.frontmatter_tags || []).join("\u0000")
      || (oldSession.frontmatter_people || []).join("\u0000") !== (updated.frontmatter_people || []).join("\u0000")
    ) {
      invalidateSessionArtifacts(oldSession.name);
    }
  }
}

async function refreshProjectFilesState(options: { force?: boolean; renderIfChanged?: boolean } = {}) {
  const now = Date.now();
  if (projectFilesRefreshInFlight) return;
  if (!options.force && now - lastProjectFilesRefreshAt < 5000) return;
  lastProjectFilesRefreshAt = now;
  projectFilesRefreshInFlight = true;
  try {
    const fingerprint = await getProjectFilesFingerprint(activeProjectId()).catch(() => null);
    if (!fingerprint) return;
    if (projectFilesFingerprint === null) {
      projectFilesFingerprint = fingerprint.fingerprint;
      return;
    }
    if (fingerprint.fingerprint === projectFilesFingerprint) return;
    projectFilesFingerprint = fingerprint.fingerprint;

    const previous = sessions;
    const refreshed = await listSessions().then(visibleSessions).catch(() => null);
    if (!refreshed) return;
    invalidateChangedSessionArtifacts(previous, refreshed);
    sessions = mergeRefreshedSessionList(refreshed);

    const active = activeSessionName ? sessions.find(s => s.name === activeSessionName) : null;
    if (!active && sessions.length > 0) {
      activeSessionName = sessions[0].name;
      activeSessionTab = defaultTabForSession(sessions[0]);
    } else if (active && activeSessionTab === "distill" && !canOpenNotePanel(active)) {
      activeSessionTab = defaultTabForSession(active);
    }

    if (options.renderIfChanged ?? true) render();
    if (activeSessionName) {
      await loadArtifacts(activeSessionName, { renderLoading: false });
    }
  } finally {
    projectFilesRefreshInFlight = false;
  }
}

/**
 * Re-fetch settings from the backend, reconcile active project readiness with
 * the live filesystem, and re-render. Called both from the backend settings-
 * changed watcher listener and as a focus/visibility fallback so that a
 * CLI-added project (written into settings.json) is reflected even if the
 * notify watcher event was missed.
 */
async function reloadSettingsAndRender(): Promise<void> {
  try {
    settings = normalizeSettings(await getSettings());
    const active = activeProject(settings);
    if (active?.path) {
      const v = await validateVault(active.path).catch(() => null);
      if (v) {
        const newReadiness = !v.exists ? "error" : v.has_recall_index ? "ready" : "needs_setup";
        // Only persist if the computed readiness actually differs from what is
        // already stored. This prevents a write→watch→write feedback loop:
        // update_project_readiness always writes settings.json, which fires the
        // notify watcher, which calls reloadSettingsAndRender again. Skipping
        // the persist when nothing changed breaks the cycle while still updating
        // the in-memory `settings` object so the UI stays consistent.
        const alreadyCurrent = active.readiness === newReadiness;
        await setProjectReadiness(active.id, newReadiness, /* persist= */ !alreadyCurrent);
      }
    }
  } catch {
    // Non-fatal: stale settings are better than a crash.
  }
  render();
}

function startProjectFilesRefreshLoop() {
  if (projectFilesRefreshInterval !== null) return;
  projectFilesRefreshInterval = window.setInterval(() => {
    if (document.visibilityState === "visible") {
      void refreshProjectFilesState();
    }
  }, 15000);
  window.addEventListener("focus", () => {
    void refreshProjectFilesState({ force: true });
    // Fallback: also re-fetch settings in case a CLI write was missed by the
    // backend watcher while the window was out of focus.
    void reloadSettingsAndRender();
  });
  document.addEventListener("visibilitychange", () => {
    if (document.visibilityState === "visible") {
      void refreshProjectFilesState({ force: true });
      // Same fallback as focus handler above.
      void reloadSettingsAndRender();
    }
  });
}

function groundingUsesFromValue(value: unknown): MarginsGroundingUse[] {
  if (!value || typeof value !== "object") return [];
  const uses = (value as { uses?: unknown }).uses;
  if (!Array.isArray(uses)) return [];
  return uses.filter((use): use is MarginsGroundingUse => Boolean(use) && typeof use === "object");
}

function mergeTraceVaultRefsIntoGrounding(name: string) {
  mergeTraceVaultRefsIntoGroundingForStateKey(sessionStateKey(name));
}

function mergeTraceVaultRefsIntoGroundingForStateKey(stateKey: string) {
  const refs = vaultRefsFromTrace(distillTraceBySession[stateKey] || []);
  if (!refs.length) return;
  const state = groundedNoteBySession[stateKey];
  if (!state?.blocks.length) return;
  groundedNoteBySession[stateKey] = mergeVaultRefsIntoState(state, refs);
}

function vaultRefsFromTrace(trace: DistillTraceEvent[]): string[] {
  const refs = new Set<string>();
  for (const event of trace) {
    const text = `${event.label} ${event.detail}`;
    const connected = text.match(/connected to\s+(.+)$/i)?.[1];
    if (connected) {
      for (const item of connected.split(/\s*,\s*/)) {
        const clean = item.replace(/\.$/, "").trim();
        if (clean) refs.add(clean);
      }
    }
    for (const match of text.matchAll(/\bread:\s*([^—]+?\.md)\b/gi)) {
      const title = match[1].trim().replace(/\.md$/i, "");
      if (title) refs.add(title);
    }
  }
  return [...refs].slice(0, 6);
}

function projectIdForRecallIndexingEvent(event: { project_id?: string | null; path?: string | null }): string | null {
  const explicit = event.project_id?.trim();
  if (explicit) return explicit;
  const eventPath = (event.path || "").replace(/\/+$/g, "");
  if (!eventPath) return null;
  return normalizeProjects(settings).find(project => project.path.replace(/\/+$/g, "") === eventPath)?.id ?? null;
}

function parseSessionMemo(markdown: string): MemoLine[] {
  const lines: MemoLine[] = [];
  for (const rawLine of markdown.split(/\r?\n/)) {
    const match = rawLine.match(/^\[(\d{2}:\d{2}(?::\d{2})?)(?:\s+~(\d{2}:\d{2}(?::\d{2})?))?\]\s+(.*)$/);
    if (!match) continue;
    const rawText = match[3].trim();
    const audioPending = rawText.startsWith("(audio not live yet)");
    const text = audioPending ? rawText.replace(/^\(audio not live yet\)\s*/, "").trim() : rawText;
    if (!text) continue;
    lines.push({
      text,
      created_secs: parseMemoTimestamp(match[1]),
      edited_secs: match[2] ? parseMemoTimestamp(match[2]) : null,
      audio_pending_at_mark: audioPending || undefined,
    });
  }
  return lines;
}

function parseMemoTimestamp(value: string): number {
  const parts = value.split(":").map(part => Number(part));
  if (parts.some(part => !Number.isFinite(part))) return 0;
  if (parts.length === 3) return parts[0] * 3600 + parts[1] * 60 + parts[2];
  return parts[0] * 60 + parts[1];
}

window.__deleteSession = async (name: string) => {
  try {
    clearPendingMemoDraft(name);
    if (activeSessionName === name) clearBackchannelState();
    rememberDeletedSession(name);
    processingRunId += 1;
    if (currentProcessingUnlisten) {
      try { currentProcessingUnlisten(); } catch {}
      currentProcessingUnlisten = null;
    }
    if (processingSessionName === name || sessions.some(s => s.name === name && s.status === "processing")) {
      processingSessionName = "";
      currentProgress = 0;
      noteCancelledBySession[name] = true;
      try { await cancelProcessSession(name, projectIdForSession(name)); } catch {}
    }
    // If we're deleting the session that's currently open, pre-compute the
    // neighbour to fall back to so we land on the most recent adjacent note
    // instead of the empty home screen.
    let nextActive: string | null = activeSessionName;
    if (activeSessionName === name) {
      const order = sortSessionsForSidebar(sessions);
      const idx = order.findIndex(s => s.name === name);
      const neighbour = order[idx + 1] ?? order[idx - 1] ?? null;
      nextActive = neighbour ? neighbour.name : null;
    }
    await deleteSession(name, projectIdForSession(name));
    sessions = mergeRefreshedSessionList(await listSessions()
      .then(visibleSessions)
      .catch(() => visibleSessions(sessions.filter(s => s.name !== name))));
    // The backend has confirmed the delete and the refresh above reflects it,
    // so release the name — keeping it remembered forever would silently hide
    // a future session that legitimately reuses it.
    forgetDeletedSession(name);
    if (nextActive && sessions.some(s => s.name === nextActive)) {
      activeSessionName = nextActive;
      const next = sessions.find(s => s.name === nextActive);
      if (next) activeSessionTab = defaultTabForSession(next);
    } else {
      activeSessionName = null;
    }
    render();
  } catch (e: any) {
    forgetDeletedSession(name);
    alert(`Failed to delete: ${e}`);
  }
};

window.__processSession = async (name: string, overwriteExistingNote = false, maxSpeakers: number | null = null, forceTranscribe = false, retry = false) => {
  // Each invocation gets a fresh run id so a stale callback (from a cancelled
  // run that is still winding down on the backend) cannot leak into the UI.
  processingRunId += 1;
  const runId = processingRunId;
  processingSessionName = name;
  delete noteJobErrorBySession[name];
  delete noteCancelledBySession[name];
  processingSteps = [];
  transcriptEntries = [];
  currentProgress = 0.02;
  const stateKey = sessionStateKey(name);
  distillTraceBySession[stateKey] = [];
  lifecycleBySession[name] = newSessionLifecycleState(name);
  activeSessionName = name;
  activeSessionTab = "distill";
  currentView = "home";
  groundedNoteBySession[stateKey] = newGroundedNoteState();
  streamingBlockSizesBySession[name] = {};
  streamingFirstParagraphPinnedBySession[name] = false;
  memoLinesBySession[stateKey] = memoLinesBySession[stateKey] || memoLines.map(line => ({ ...line }));
  sessions = sessions.map(s => s.name === name ? { ...s, status: "processing" } : s);
  render();

  if (currentProcessingUnlisten) {
    try { currentProcessingUnlisten(); } catch {}
    currentProcessingUnlisten = null;
  }

  const unlisten = await onProcessingProgress((event: ProcessingEvent) => {
    if (runId !== processingRunId) return;
    rememberLifecycleEvent(name, event);
    let shouldRender = event.stage !== "note_stream";
    if (event.stage === "transcript" && event.entry) {
      transcriptEntries.push(event.entry);
    } else if (event.stage === "note_stream") {
      shouldRender = consumeNoteStreamChunkForStateKey(stateKey, event.message);
      rememberNoteWritingTraceForStateKey(stateKey, name, event.progress);
      processingSteps.push(event);
    } else {
      processingSteps.push(event);
      rememberProcessingTraceForStateKey(stateKey, event);
    }
    if (event.progress !== null) currentProgress = event.progress;
    renderProcessingProgress(event.stage, shouldRender);
  });
  currentProcessingUnlisten = unlisten;

  const detach = () => {
    if (currentProcessingUnlisten === unlisten) currentProcessingUnlisten = null;
    try { unlisten(); } catch {}
  };

  try {
    if (retry) {
      await retrySession(name, projectIdForSession(name));
    } else {
      await processSession(name, projectIdForSession(name), overwriteExistingNote, maxSpeakers, forceTranscribe);
    }
    if (runId !== processingRunId) { detach(); return; }
    detach();
    const shouldSelectFinishedSession = currentView === "home" && activeSessionName === name;
    processingSessionName = "";
    currentProgress = 1;
    sessions = mergeRefreshedSessionList(await listSessions()
      .then(visibleSessions)
      .catch(() => visibleSessions(sessions.map(s => s.name === name ? { ...s, status: "synthesized" } : s))));
    if (deletedSessionNames.has(name)) return;
    if (shouldSelectFinishedSession) {
      activeSessionName = name;
      activeSessionTab = "distill";
    }
    await loadArtifacts(name, { renderLoading: false });
    mergeTraceVaultRefsIntoGrounding(name);
    {
      const finished = sessions.find(s => s.name === name);
      rememberReprocessPeopleBaseline(name, finished ? sessionFrontmatterPeople(finished) : []);
    }
    // First-note payoff: show once after the very first completed note.
    if (!firstNoteCelebrated) {
      try {
        const alreadyCelebrated = window.localStorage.getItem(FIRST_NOTE_CELEBRATED_KEY);
        if (!alreadyCelebrated) {
          firstNoteCelebrated = true;
          window.localStorage.setItem(FIRST_NOTE_CELEBRATED_KEY, "1");
        }
      } catch {
        // localStorage unavailable; treat as already celebrated so the
        // callout never loops.
      }
    }
    // Best-effort diarization may have landed the speaker model during this
    // run — clear the "Adding speaker labels…" pill if so.
    void refreshModelNoticeDiarization();
    flashDistilledSessionName = name;
    if (flashDistilledTimer !== null) window.clearTimeout(flashDistilledTimer);
    render();
    flashDistilledTimer = window.setTimeout(() => {
      flashDistilledTimer = null;
      if (flashDistilledSessionName === name) {
        flashDistilledSessionName = null;
        render();
      }
    }, 2600);
  } catch (e: any) {
    detach();
    // If a newer run has started (or cancel rolled this run back), drop silently.
    if (runId !== processingRunId) return;
    const message = String(e?.message ?? e ?? "");
    if (message.includes(DISTILL_CANCELLED_SENTINEL)) {
      processingSessionName = "";
      currentProgress = 0;
      noteCancelledBySession[name] = true;
      sessions = visibleSessions(sessions.map(s => s.name === name && s.status === "processing" ? { ...s, status: "unprocessed" } : s));
      if (deletedSessionNames.has(name)) {
        render();
        return;
      }
      activeSessionName = name;
      activeSessionTab = "distill";
      render();
      return;
    }
    const cleanMessage = message.replace(/^Error:\s*(Error:\s*)?/, "").trim() || "Margins could not finish the note.";
    const event = {
      stage: "error",
      message: `Error: ${e}`,
      progress: null,
    };
    processingSteps.push(event);
    rememberProcessingTraceForStateKey(stateKey, event);
    noteJobErrorBySession[name] = cleanMessage;
    processingSessionName = "";
    groundedNoteBySession[stateKey] = groundedNoteBySession[stateKey]?.blocks.length ? groundedNoteBySession[stateKey] : newGroundedNoteState();
    const shouldSelectFailedSession = currentView === "home" && activeSessionName === name;
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    if (shouldSelectFailedSession) {
      currentView = "home";
      activeSessionName = name;
      activeSessionTab = "distill";
    }
    render();
  }
};

window.__retrySession = async (name: string) => {
  await window.__processSession(name, false, null, false, true);
};

window.__adjustCaptureSpeakerCount = (delta: number) => {
  captureSpeakerCount = Math.max(0, Math.min(4, Math.round(captureSpeakerCount + delta)));
  render();
};

window.__cycleCaptureSpeakerCount = () => {
  captureSpeakerCount = captureSpeakerCount >= 4 ? 0 : captureSpeakerCount + 1;
  render();
};

window.__cancelProcessing = async (name: string) => {
  // Optimistic UI: stop showing the job immediately, but leave an explicit
  // cancelled outcome instead of silently rolling back to marks.
  if (processingSessionName && processingSessionName !== name) return;
  processingRunId += 1;
  if (currentProcessingUnlisten) {
    try { currentProcessingUnlisten(); } catch {}
    currentProcessingUnlisten = null;
  }
  processingSteps = [];
  currentProgress = 0;
  processingSessionName = "";
  delete noteJobErrorBySession[name];
  noteCancelledBySession[name] = true;
  groundedNoteBySession[sessionStateKey(name)] = newGroundedNoteState();
  sessions = sessions.map(s => s.name === name && s.status === "processing" ? { ...s, status: "unprocessed" } : s);
  activeSessionName = name;
  activeSessionTab = "distill";
  render();
  try {
    await cancelProcessSession(name);
  } catch {
    // Best-effort: the backend may already have wound down or never registered.
  }
};

window.__dismissFirstNotePayoff = () => {
  firstNoteCelebrated = false;
  render();
};

window.__dismissCancelledNote = (name: string) => {
  delete noteCancelledBySession[name];
  render();
};

window.__dismissNoteError = async (name: string) => {
  delete noteJobErrorBySession[name];
  sessions = sessions.map(s => s.name === name && s.status === "failed" ? { ...s, status: "unprocessed", failure_message: null } : s);
  render();
  try {
    await clearSessionNoteError(name, projectIdForSession(name));
  } catch {
    // Best-effort: the local UI dismissal should not be blocked by storage repair.
  }
};

window.__reviewSession = async (name: string) => {
  reviewSessionName = name;
  try {
    reviewContent = await getVaultNote(name, projectIdForSession(name));
  } catch {
    try {
      reviewContent = await getAlignedContent(name, projectIdForSession(name));
    } catch {
      reviewContent = "(No content available)";
    }
  }
  currentView = "review";
  render();
};

let rerunArmTimer: ReturnType<typeof setTimeout> | null = null;

function disarmRerun() {
  if (rerunArmTimer) {
    clearTimeout(rerunArmTimer);
    rerunArmTimer = null;
  }
  const btn = document.getElementById("review-rerun") as HTMLButtonElement | null;
  if (btn && btn.classList.contains("armed")) {
    btn.classList.remove("armed");
    btn.textContent = btn.dataset.label || "Re-run processing";
  }
}

window.__reprocessSession = async (name: string) => {
  const btn = document.getElementById("review-rerun") as HTMLButtonElement | null;
  // Inline arm-then-fire on the review screen's red control (no modal).
  if (btn) {
    if (!btn.classList.contains("armed")) {
      btn.dataset.label = btn.textContent || "Re-run processing";
      btn.classList.add("armed");
      btn.textContent = "Confirm re-run? — replaces this note";
      if (rerunArmTimer) clearTimeout(rerunArmTimer);
      rerunArmTimer = setTimeout(disarmRerun, 4000);
      return;
    }
    disarmRerun();
    await window.__processSession(name, true);
    return;
  }
  // Fallback for other call sites (distill.ts) that have no #review-rerun control.
  if (!confirm("Rewrite the note? This will overwrite the current note.")) return;
  await window.__processSession(name, true);
};

window.__reprocessWithPeople = async (name: string) => {
  const session = sessions.find(s => s.name === name);
  const people = session ? sessionFrontmatterPeople(session) : [];
  if (people.length === 0) return;

  // Put the note into streaming mode. The existing note is kept as a baseline
  // so it stays on screen while streaming, with each paragraph swapped for its
  // rewritten version as that paragraph completes — instead of clearing the
  // note and rebuilding it from a blank slate.
  const stateKey = sessionStateKey(name);
  const priorState = groundedNoteBySession[stateKey];
  let priorBlocks = priorState?.blocks?.length
    ? priorState.blocks
    : artifactCache[stateKey]?.note
      ? groundedStateFromMarkdown(artifactCache[stateKey].note!).blocks
      : [];
  if (!priorBlocks.length) {
    // Artifacts may not have loaded yet (e.g. reprocess triggered right after
    // opening the session) — fetch the saved note so the baseline swap still works.
    try {
      priorBlocks = groundedStateFromMarkdown(await getVaultNote(name, projectIdForSession(name))).blocks;
    } catch {}
  }
  processingRunId += 1;
  const runId = processingRunId;
  processingSessionName = name;
  delete noteJobErrorBySession[name];
  currentProgress = 0.02;
  lifecycleBySession[name] = newSessionLifecycleState(name);
  const freshState = newGroundedNoteState();
  if (priorBlocks.length) {
    freshState.baseline = priorBlocks.map(b => ({ ...b, uses: [...b.uses] }));
  }
  groundedNoteBySession[stateKey] = freshState;
  streamingBlockSizesBySession[name] = {};
  streamingFirstParagraphPinnedBySession[name] = false;
  sessions = sessions.map(s => s.name === name ? { ...s, status: "processing" } : s);
  activeSessionName = name;
  activeSessionTab = "distill";
  currentView = "home";
  render();

  if (currentProcessingUnlisten) {
    try { currentProcessingUnlisten(); } catch {}
    currentProcessingUnlisten = null;
  }

  const unlisten = await onProcessingProgress((event: ProcessingEvent) => {
    if (runId !== processingRunId) return;
    rememberLifecycleEvent(name, event);
    let shouldRender = event.stage !== "note_stream";
    if (event.stage === "note_stream") {
      shouldRender = consumeNoteStreamChunkForStateKey(stateKey, event.message);
      rememberNoteWritingTraceForStateKey(stateKey, name, event.progress);
      processingSteps.push(event);
    } else {
      processingSteps.push(event);
      rememberProcessingTraceForStateKey(stateKey, event);
    }
    if (event.progress !== null) currentProgress = event.progress;
    renderProcessingProgress(event.stage, shouldRender);
  });
  currentProcessingUnlisten = unlisten;

  const detach = () => {
    if (currentProcessingUnlisten === unlisten) currentProcessingUnlisten = null;
    try { unlisten(); } catch {}
  };

  try {
    await reprocessSessionWithPeople(name, people, projectIdForSession(name));
    if (runId !== processingRunId) { detach(); return; }
    detach();
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => visibleSessions(sessions.map(s => s.name === name ? { ...s, status: "synthesized" } : s))));
    processingSessionName = "";
    currentProgress = 1;
    activeSessionName = name;
    activeSessionTab = "distill";
    await loadArtifacts(name, { renderLoading: false });
    mergeTraceVaultRefsIntoGrounding(name);
    rememberReprocessPeopleBaseline(name, people);
    render();
  } catch (e: any) {
    detach();
    if (runId !== processingRunId) return;
    const message = String(e?.message ?? e ?? "");
    const cleanMessage = message.replace(/^Error:\s*(Error:\s*)?/, "").trim() || "Reprocess with people failed.";
    noteJobErrorBySession[name] = cleanMessage;
    processingSessionName = "";
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    activeSessionName = name;
    activeSessionTab = "distill";
    render();
  }
};

window.__dismissReprocessPeople = (name: string) => {
  const session = sessions.find(s => s.name === name);
  if (!session) return;
  ensureReprocessSuggestionState();
  const baseline = reprocessPeopleBaselineBySession[name] || [];
  const known = new Set(baseline.map(normalizedPersonKey));
  const added = sessionFrontmatterPeople(session).filter(p => !known.has(normalizedPersonKey(p)));
  reprocessDismissedKeyBySession[name] = peopleSetKey(added);
  writeStoredJson(REPROCESS_DISMISSED_STORAGE_KEY, reprocessDismissedKeyBySession);
  render();
};

window.__dismissTapNotice = () => {
  dismissedTapNoticeSession = recordingStatus.session_name || activeSessionName;
  render();
};

window.__restartSystemAudioCapture = async () => {
  if (!recordingStatus.is_recording) return;
  const sessionName = recordingStatus.session_name || activeSessionName;
  tapRecoverySession = sessionName;
  tapRecoveryState = "recovering";
  tapRecoveryError = null;
  render();
  try {
    applyRecordingStatus(await restartSystemAudioCapture());
    tapRecoverySession = recordingStatus.session_name || sessionName;
    tapRecoveryState = "recovered";
    window.setTimeout(() => {
      if (tapRecoveryState === "recovered" && tapRecoverySession === (recordingStatus.session_name || activeSessionName)) {
        tapRecoveryState = "idle";
        render();
      }
    }, 4000);
  } catch (e: any) {
    tapRecoveryState = "failed";
    tapRecoveryError = String(e?.message || e);
  }
  render();
};

window.__useMicDiarization = async () => {
  if (!recordingStatus.is_recording) return;
  const previousStatus = recordingStatus;
  recordingStatus = { ...recordingStatus, live_transcription_mode: "mic_diarized" };
  dismissedTapNoticeSession = recordingStatus.session_name || activeSessionName;
  tapRecoveryState = "idle";
  tapRecoveryError = null;
  render();
  try {
    applyRecordingStatus(await setLiveTranscriptionMode("mic_diarized"));
    dismissedTapNoticeSession = recordingStatus.session_name || activeSessionName;
    tapRecoveryState = "idle";
    tapRecoveryError = null;
    render();
  } catch (e: any) {
    recordingStatus = previousStatus;
    render();
    alert(`Could not switch to mic diarization: ${e?.message || e}`);
  }
};

window.__retryRecordingStartup = () => {
  if (!recordingStartup || recordingStartup.phase === "starting") return;
  const previous = recordingStartup;
  recordingStartup = {
    ...previous,
    id: ++recordingStartupSeq,
    phase: "starting",
    message: "Retrying audio setup…",
    error: null,
    cancelRequested: false,
    waitingForPermission: false,
    webMicrophoneFailure: null,
  };
  recordingStatus = {
    ...recordingStatus,
    is_recording: false,
    session_name: previous.sessionName,
    capture_device: { state: "active", device_name: "System Default" },
    mic_level: 0,
    mic_audio_frame_count: 0,
    spk_level: 0,
    capture_phase: "idle",
  };
  activeSessionName = previous.sessionName;
  activeSessionTab = "backchannel";
  currentView = "home";
  capturePendingDraftForNextRender();
  recordingMounted = false;
  startPolling();
  render();
  recordingStartPromise = finishRecordingStart(recordingStartup.id, previous.requestedName, previous.sessionName);
};

window.__cancelRecordingStartup = async () => {
  if (!recordingStartup || recordingStartup.phase !== "starting" || recordingStartup.cancelRequested) return;
  const id = recordingStartup.id;
  updateRecordingStartup(id, {
    cancelRequested: true,
    message: "Cancelling audio startup…",
  });
  capturePendingDraftForNextRender();
  recordingMounted = false;
  render();
  try {
    const cancelledLocally = await cancelRecordingStartup();
    if (isHostedWeb() && cancelledLocally && recordingStartup?.id === id) {
      recordingStartup = {
        ...recordingStartup,
        id: ++recordingStartupSeq,
        phase: "failed",
        cancelRequested: false,
        waitingForPermission: false,
        message: "Audio startup cancelled.",
        error: null,
        webMicrophoneFailure: "aborted",
      };
      recordingStartPromise = null;
      recordingMounted = false;
      stopPolling();
      render();
    }
  } catch (error: any) {
    if (!recordingStartup || recordingStartup.id !== id) return;
    recordingStartup = {
      ...recordingStartup,
      phase: "failed",
      cancelRequested: false,
      message: "Could not cancel audio startup.",
      error: String(error?.message || error),
    };
    recordingMounted = false;
    render();
  }
};

window.__discardRecordingStartup = async () => {
  if (!recordingStartup || recordingStartup.phase !== "failed") return;
  const previous = recordingStartup;
  clearPendingMemoDraft(previous.sessionName);
  // Hosted startup owns no server session until microphone setup succeeds,
  // and startRecording scopes cleanup to the exact name it allocated. Calling
  // an unscoped stop here after a failed retry could finalize a different,
  // reload-interrupted capture. Native startup retains its legacy best-effort
  // cleanup because it has a single in-process recorder.
  const hostedActive = isHostedWeb()
    ? await getRecordingStatus().catch(() => null)
    : null;
  if (!isHostedWeb()) {
    try { await stopRecording(recordingStatus.web_recording_id); } catch {}
  }
  if (previous.sessionName && hostedActive?.session_name !== previous.sessionName) {
    try { await deleteSession(previous.sessionName, projectIdForSession(previous.sessionName)); } catch {}
  }
  recordingStartup = null;
  recordingMounted = false;
  if (hostedActive?.is_recording && hostedActive.session_name) {
    applyRecordingStatus(hostedActive);
    activeSessionName = hostedActive.session_name;
    activeSessionTab = "backchannel";
    currentView = "home";
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    startPolling();
    render();
    return;
  }
  stopPolling();
  recordingStatus = {
    ...recordingStatus,
    is_recording: false,
    session_name: "",
    capture_device: { state: "active", device_name: "System Default" },
    mic_level: 0,
    mic_audio_frame_count: 0,
    spk_level: 0,
    capture_phase: "idle",
  };
  activeSessionName = "";
  currentView = "home";
  sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => visibleSessions(sessions.filter(s => s.name !== previous.sessionName))));
  render();
};

function clearDiscardedCaptureUi(name: string) {
  clearPendingMemoDraft(name);
  stopPolling();
  recordingStartup = null;
  recordingMounted = false;
  finalizingCaptureSessionName = null;
  dismissedTapNoticeSession = null;
  tapRecoverySession = null;
  tapRecoveryState = "idle";
  tapRecoveryError = null;
  lastCaptureDeviceState = null;
  holdingRecoveryDeviceName = null;
  pendingCaptureDeviceSwitchFrom = null;
  pendingCaptureDeviceSwitchTo = null;
  manualCaptureDeviceSwitchPending = false;
  manualCaptureDeviceSwitchTarget = null;
  clearCaptureDeviceSwitchToast();
  delete lastTapStatusBySession[name];
  delete lastLiveModeBySession[name];
  delete captureHealthBySession[name];
  delete lifecycleBySession[name];
  delete memoLinesBySession[sessionStateKey(name)];
  delete memoLinesBySession[name];
  delete artifactCache[sessionStateKey(name)];
  delete artifactCache[name];
  sessions = sessions.filter(s => s.name !== name);
  recordingStatus = {
    ...recordingStatus,
    is_recording: false,
    paused: false,
    session_name: "",
    capture_device: { state: "active", device_name: "System Default" },
    mic_level: 0,
    mic_audio_frame_count: 0,
    spk_level: 0,
    capture_phase: "idle",
  };
  if (!activeSessionName || activeSessionName === name || !sessions.some(s => s.name === activeSessionName)) {
    activeSessionName = sessions[0]?.name || null;
  }
  if (activeSessionName) {
    const next = sessions.find(s => s.name === activeSessionName);
    if (next) activeSessionTab = defaultTabForSession(next);
  } else {
    activeSessionTab = "distill";
  }
  currentView = "home";
}

function currentTabOwnsHostedCapture(): boolean {
  if (!isHostedWeb()) return true;
  if (recordingStatus.web_capture_owner === "local") return true;
  if (recordingStatus.web_capture_owner !== "recovery") return false;
  const selectedId = hostedRecoveryState.selectedRecordingId();
  return hostedRecoveryState.selectedMemoReady(selectedId)
    && selectedId === recordingStatus.web_recording_id;
}

window.__discardActiveRecording = async () => {
  if (recordingStartup?.phase === "starting") {
    alert("Audio is still starting. You can discard this draft if audio setup fails.");
    return;
  }
  if (recordingStartup?.phase === "failed") {
    await window.__discardRecordingStartup();
    return;
  }
  if (stopInFlight || !recordingStatus.is_recording) return;
  if (!currentTabOwnsHostedCapture()) {
    alert("This recording is controlled by another browser tab. Take control only after its capture lease expires.");
    return;
  }
  const sessionName = recordingStatus.session_name || activeSessionName;
  if (!sessionName) return;
  captureDiscardDialog = { sessionName };
  render();
};

window.__cancelDiscardActiveRecording = () => {
  captureDiscardDialog = null;
  render();
};

window.__confirmDiscardActiveRecording = async () => {
  const dialog = captureDiscardDialog;
  if (!dialog || stopInFlight) return;
  const sessionName = dialog.sessionName;
  if (!currentTabOwnsHostedCapture()) return;
  captureDiscardDialog = null;

  stopInFlight = true;
  const statusBeforeDiscard = recordingStatus;
  const discardRecordingId = statusBeforeDiscard.web_recording_id;
  const activeAlongsideRecovery = Boolean(
    discardRecordingId
      && hostedActiveRecordingStatus?.is_recording
      && hostedActiveRecordingStatus.web_recording_id !== discardRecordingId,
  );
  recordingStatus = { ...recordingStatus, paused: true, capture_phase: "pausing" };
  if (!activeAlongsideRecovery) clearDiscardedCaptureUi(sessionName);
  render();
  try {
    const completion = discardRecordingId
      ? await executeHostedRecoveryMutation({
        completedRecordingId: discardRecordingId,
        mutate: () => discardRecording(discardRecordingId),
        restoreActive: restoreHostedActiveAfterRecoveryCompletion,
      })
      : { result: await discardRecording(discardRecordingId), active: null };
    const deleteName = completion.result;
    if (!activeAlongsideRecovery) clearDiscardedCaptureUi(deleteName);
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() =>
      visibleSessions(activeAlongsideRecovery ? sessions : sessions.filter(s => s.name !== deleteName))));
    if (!activeAlongsideRecovery) clearDiscardedCaptureUi(deleteName);
    render();
  } catch (e: any) {
    const reconciled = discardRecordingId
      ? await getHostedRecordingStatus(discardRecordingId).catch(() => null)
      : await getRecordingStatus().catch(() => null);
    if (discardRecordingId) {
      await refreshHostedRecoveryDiscovery().catch(() => hostedRecoveryState.list());
      if (hostedRecoveryState.isRecovery(discardRecordingId)) {
        hostedRecoveryState.select(discardRecordingId);
        hostedRecoveryState.markHydrated(discardRecordingId);
      }
    }
    applyRecordingStatus(reconciled ?? ({
      ...statusBeforeDiscard,
      is_recording: true,
      paused: false,
      capture_phase: "needs_attention",
    }));
    if (recordingStatus.is_recording) {
      sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
      activeSessionName = recordingStatus.session_name || activeSessionName;
      activeSessionTab = "backchannel";
      currentView = "home";
      startPolling();
    } else {
      stopPolling();
      recordingStartup = null;
      recordingMounted = false;
      sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
      currentView = "home";
    }
    alert(`Failed to discard capture: ${e?.message || e}`);
    render();
  } finally {
    stopInFlight = false;
  }
};

window.__pauseRecording = async () => {
  if (recordingStartup?.phase === "starting") {
    alert("Audio is still starting. You can pause once audio is live.");
    return;
  }
  if (recordingStartup?.phase === "failed") {
    alert("Audio has not started yet. Retry audio or open Audio setup.");
    return;
  }
  if (stopInFlight || pausedAtWallMs != null || recordingStatus.paused) return;
  if (!currentTabOwnsHostedCapture()) return;
  const memoSessionName = memoCaptureSessionName();
  if (!memoSessionName) return;
  stopInFlight = true;
  const statusBeforePause = recordingStatus;
  const ordinalBeforePause = currentBlockOrdinal;
  // Enter the clock-stopped block SYNCHRONOUSLY with the pause intent, before any
  // await. Freezing the clock (pausedAtWallMs) and bumping the block ordinal up
  // front means notes committed during the seal window are stamped as untimed
  // lines in the new block — not timed marks — regardless of when the backend
  // status echo lands. Rolled back below if the pause command fails.
  pausedAtWallMs = Date.now();
  currentBlockOrdinal = ordinalBeforePause + 1;
  const wallClock = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });
  blockWallClockByOrdinal.set(currentBlockOrdinal, wallClock);
  getOrCreateBlockMargin(currentBlockOrdinal);
  // Optimistically reflect the paused state so the footer swaps instantly.
  recordingStatus = { ...recordingStatus, paused: true, capture_phase: "pausing" };
  render();
  try {
    await syncMemo(memoLines, memoSessionName, statusBeforePause.web_recording_id);
    applyRecordingStatus(await pauseRecording());
  } catch (e: any) {
    // Pause failed — capture is still running. Roll back the clock-stopped block.
    pausedAtWallMs = null;
    blockWallClockByOrdinal.delete(currentBlockOrdinal);
    currentBlockOrdinal = ordinalBeforePause;
    await recoverRecordingStatusAfterCommandFailure(statusBeforePause.session_name, {
      ...statusBeforePause,
      capture_phase: "needs_attention",
    }, statusBeforePause.web_recording_id);
    alert(`Failed to pause capture: ${e}`);
  } finally {
    stopInFlight = false;
    render();
    if (finishQueuedAfterPause) {
      finishQueuedAfterPause = false;
      void window.__finishRecording();
    }
  }
};

window.__resumeRecording = async () => {
  if (stopInFlight || (pausedAtWallMs == null && !recordingStatus.paused)) return;
  if (!currentTabOwnsHostedCapture()) return;
  stopInFlight = true;
  const statusBeforeResume = recordingStatus;
  const pausedAtWallMsBefore = pausedAtWallMs;
  const memoStartTimeBefore = memoStartTime;
  const ordinalBeforeResume = currentBlockOrdinal;
  // Advance the clock anchor past the paused interval so it keeps measuring
  // RECORDING time. Timed lines committed after resume then get created_secs
  // that match the contiguous audio timeline (no pause gap), and the displayed
  // clock continues from the frozen value instead of jumping by the pause.
  if (pausedAtWallMs != null) {
    memoStartTime += Date.now() - pausedAtWallMs;
    pausedAtWallMs = null;
  }
  recordingStatus = { ...recordingStatus, paused: false, capture_phase: "recording" };
  // Compact the block being left: un-pulled fragments are frozen
  compactBlockMargin(currentBlockOrdinal);
  render();
  try {
    applyRecordingStatus(await resumeRecording());
  } catch (e: any) {
    // Resume failed — restore the full clock-stopped block state we optimistically left.
    pausedAtWallMs = pausedAtWallMsBefore;
    memoStartTime = memoStartTimeBefore;
    currentBlockOrdinal = ordinalBeforeResume;
    await recoverRecordingStatusAfterCommandFailure(statusBeforeResume.session_name, {
      ...statusBeforeResume,
      capture_phase: "needs_attention",
    }, statusBeforeResume.web_recording_id);
    alert(`Failed to resume capture: ${e}`);
  } finally {
    stopInFlight = false;
    render();
  }
};

// Finish ends the capture for good and writes the note. Available from
// the paused state (still an in-memory recording, so finalize first) — the
// post-capture page uses __processSession directly since its session is already
// finalized.
window.__finishRecording = async () => {
  if (stopInFlight) {
    if (recordingStatus.capture_phase === "pausing") {
      finishQueuedAfterPause = true;
    }
    return;
  }
  if (!currentTabOwnsHostedCapture()) {
    alert("This tab does not own the active capture. Return to the recording tab, or take control after it is interrupted.");
    return;
  }
  const canWriteNote = settingsReadyFromState();
  const recoveryFinish = recordingStatus.web_capture_owner === "recovery";
  if (!canWriteNote && !recoveryFinish) {
    window.__nav("settings");
    return;
  }
  const maxSpeakers = speakerCountToMax(captureSpeakerCount);
  const finishingRecordingId = recordingStatus.web_recording_id;
  void window.__stopRecording().then((finishedName) => {
    if (!finishedName || !canWriteNote) return;
    if (finishingRecordingId
      && hostedActiveRecordingStatus?.is_recording
      && hostedActiveRecordingStatus.web_recording_id !== finishingRecordingId) {
      // A is queued by recording ID in __stopRecording. Processing is
      // deliberately deferred so the name-based process route cannot navigate
      // away from or mutate a same-name active B.
      return;
    }
    window.setTimeout(() => {
      // Reuse a backend-qualified terminal live transcript when available.
      // process_session still builds the offline transcript automatically when
      // the checkpoint is absent or rejected, so correctness keeps its audio
      // fallback without forcing the slow path for every healthy capture.
      void window.__processSession(finishedName, false, maxSpeakers, false);
    }, 0);
  }).catch((e) => {
    console.warn("Finish capture failed", e);
  });
};

window.__takeOverInterruptedRecording = async () => {
  const sessionName = recordingStatus.session_name;
  const recordingId = recordingStatus.web_recording_id;
  if (!sessionName || !recordingId || recordingStatus.capture_phase !== "interrupted" || stopInFlight) return;
  stopInFlight = true;
  try {
    hostedRecoveryState.select(recordingId);
    const hydrated = await claimInterruptedRecording(sessionName, recordingId);
    memoLines = hydrated.map(line => ({ ...line }));
    memoLinesBySession[sessionStateKey(sessionName)] = memoLines.map(line => ({ ...line }));
    hostedRecoveryState.markHydrated(recordingId);
    const exact = await getHostedRecordingStatus(recordingId);
    if (hostedRecoveryState.acceptsSelectedPoll(recordingId, exact)) applyRecordingStatus(exact);
  } catch (error: any) {
    alert(`Could not take control of this capture: ${error?.message || error}`);
  } finally {
    stopInFlight = false;
    render();
  }
};

window.__selectHostedRecovery = async (recordingId: string) => {
  if (!recordingId || stopInFlight) return;
  stopInFlight = true;
  try {
    await refreshHostedRecoveryDiscovery();
    snapshotHostedActiveWorkspaceBeforeRecovery(recordingId);
    hostedRecoveryState.select(recordingId);
    let status = await getHostedRecordingStatus(recordingId);
    if (!status.session_name) throw new Error("The selected recovery has no session metadata.");
    const recoverySessionName = status.session_name;
    memoLines = [];
    applyRecordingStatus(status);
    activeSessionName = status.session_name;
    activeSessionTab = "backchannel";

    let hydrated: MemoLine[];
    if (status.web_capture_owner === "recovery" || status.web_capture_owner === "local") {
      hydrated = await hydrateRecordingMemo(recoverySessionName, recordingId) ?? [];
    } else {
      hydrated = await claimInterruptedRecording(recoverySessionName, recordingId);
      status = await getHostedRecordingStatus(recordingId);
    }
    if (!hostedRecoveryState.acceptsSelectedPoll(recordingId, status)) return;
    memoLines = hydrated.map(line => ({ ...line }));
    memoLinesBySession[sessionStateKey(recoverySessionName)] = memoLines.map(line => ({ ...line }));
    hostedRecoveryState.markHydrated(recordingId);
    applyRecordingStatus(status);
    if (status.session_name) {
      activeSessionName = status.session_name;
      activeSessionTab = "backchannel";
    }
  } catch (error: any) {
    if (hostedActiveRecordingStatus?.web_recording_id !== recordingId) {
      await restoreHostedActiveAfterRecoveryCompletion(recordingId).catch(() => null);
    }
    alert(`Could not open this hosted recovery: ${error?.message || error}`);
  } finally {
    stopInFlight = false;
    recordingMounted = false;
    render();
  }
};

function showCaptureFinalizingScreen(): string | null {
  const name = recordingStatus.session_name || activeSessionName;
  if (!name) return null;
  finalizingCaptureSessionName = name;
  stopPolling();
  recordingMounted = false;
  activeSessionName = name;
  activeSessionTab = "distill";
  currentView = "home";
  lifecycleBySession[name] = sessionLifecycleFromRecording(name, "finalizing", "Saving audio.");
  memoLinesBySession[sessionStateKey(name)] = memoLines.map(line => ({ ...line }));
  summarySessionName = name;
  summaryDuration = recordingStatus.elapsed_secs;
  recordingStatus = {
    ...recordingStatus,
    is_recording: false,
    paused: false,
    capture_phase: "finalizing",
    session_name: name,
  };
  sessions = sessions.map(s => s.name === name
    ? {
      ...s,
      status: "unprocessed",
      duration_secs: Math.max(s.duration_secs || 0, summaryDuration || 0),
      memo_line_count: Math.max(s.memo_line_count || 0, memoLines.length),
    }
    : s);
  render();
  return name;
}

function waitForFinalizingPaint(): Promise<void> {
  return new Promise((resolve) => {
    window.requestAnimationFrame(() => {
      window.setTimeout(resolve, 0);
    });
  });
}

async function recoverRecordingStatusAfterCommandFailure(
  name: string | null,
  fallback: Partial<RecordingStatus>,
  recordingId: string | null | undefined = recordingStatus.web_recording_id,
): Promise<void> {
  let next: RecordingStatus | null = null;
  try {
    next = normalizeRecordingStatus(recordingId
      ? await getHostedRecordingStatus(recordingId)
      : await getRecordingStatus());
  } catch {
    next = null;
  }

  const sessionName = next?.session_name || name || recordingStatus.session_name;
  applyRecordingStatus(next && (next.is_recording || next.session_name || next.capture_phase === "needs_attention")
    ? { ...next, session_name: sessionName || next.session_name }
    : {
      ...recordingStatus,
      ...fallback,
      is_recording: fallback.is_recording ?? true,
      session_name: sessionName || recordingStatus.session_name,
      capture_phase: fallback.capture_phase || "needs_attention",
    });

  finalizingCaptureSessionName = null;
  if (sessionName) {
    activeSessionName = sessionName;
    activeSessionTab = "backchannel";
    sessions = sessions.map(s => s.name === sessionName ? { ...s, status: "recording" } : s);
  }
  recordingMounted = false;
  startPolling();
}

function rollbackCaptureFinalizingScreen(name: string | null) {
  finalizingCaptureSessionName = null;
  if (name) {
    sessions = sessions.map(s => s.name === name ? { ...s, status: "recording" } : s);
    lifecycleBySession[name] = sessionLifecycleFromRecording(name, "recording", "Recording.");
    activeSessionName = name;
    activeSessionTab = "backchannel";
  }
  recordingStatus = {
    ...recordingStatus,
    is_recording: true,
    paused: false,
    capture_phase: "recording",
    session_name: name || recordingStatus.session_name,
  };
  recordingMounted = false;
  startPolling();
  render();
}

async function refreshSessionsAfterCaptureStop(name: string, preserveActiveRecordingId?: string | null) {
  const refreshed = await listSessions().then(visibleSessions).catch(() => null);
  if (!refreshed) return;
  if (deletedSessionNames.has(name)) return;
  sessions = mergeRefreshedSessionList(refreshed);
  const preservingHostedCapture = Boolean(
    preserveActiveRecordingId
      && hostedActiveRecordingStatus?.web_recording_id === preserveActiveRecordingId,
  );
  if (currentView === "home" && activeSessionName === name && !preservingHostedCapture) {
    activeSessionTab = "distill";
    render();
  }
}

window.__stopRecording = async () => {
  if (recordingStartup?.phase === "starting") {
    alert("Audio is still starting. Your memo is ready now; you can end the capture once audio is live.");
    return;
  }
  if (recordingStartup?.phase === "failed") {
    alert("Audio has not started yet. Retry audio or open Audio setup.");
    return;
  }
  if (stopInFlight) return;
  if (!currentTabOwnsHostedCapture()) return null;
  const memoSessionName = memoCaptureSessionName();
  if (!memoSessionName) return;
  const finishRecordingId = recordingStatus.web_recording_id;
  stopInFlight = true;
  // Synchronously acknowledge the click before any await — perceived-instant feedback.
  const stopBtn = document.querySelector(".recording-footer .capture-end-action") as HTMLButtonElement | null;
  if (stopBtn) {
    stopBtn.disabled = true;
    stopBtn.classList.add("capture-saving");
    stopBtn.textContent = "Saved · finishing…";
  }
  const optimisticName = showCaptureFinalizingScreen();
  // Release the stopInFlight lock immediately after the UI has transitioned to the
  // finalizing screen. WAV drain runs in the background (via the awaited stopRecording()
  // below) while the user is free to navigate or start a new capture right away.
  await waitForFinalizingPaint();
  stopInFlight = false;

  // Snapshot the view context so the deferred-navigation callback can tell whether
  // the user has already navigated away or started a new capture.
  const finishTargetName = optimisticName;
  const captureSessionAtFinish = recordingStatus.session_name;
  const savedMemoLines = memoLines.map(line => ({ ...line }));

  try {
    await syncMemo(memoLines, memoSessionName, finishRecordingId);
    const completion = finishRecordingId
      ? await executeHostedRecoveryMutation({
        completedRecordingId: finishRecordingId,
        mutate: () => stopRecording(finishRecordingId),
        restoreActive: restoreHostedActiveAfterRecoveryCompletion,
      })
      : { result: await stopRecording(finishRecordingId), active: null };
    const name = completion.result;
    const restoredActive = completion.active;
    const partialUploadWarning = await consumeHostedFinalizationWarning();
    recordingMounted = false;
    transcriptWarming = false;
    transcriptDegraded = false;
    dismissedTapNoticeSession = null;
    tapRecoverySession = null;
    tapRecoveryState = "idle";
    tapRecoveryError = null;
    const stoppedName = finishTargetName || captureSessionAtFinish;
    if (stoppedName && restoredActive?.session_name !== stoppedName) {
      delete lastTapStatusBySession[stoppedName];
      delete lastLiveModeBySession[stoppedName];
    }

    const collidesWithRestoredActive = Boolean(
      finishRecordingId
        && restoredActive?.web_recording_id
        && restoredActive.web_recording_id !== finishRecordingId
        && restoredActive.session_name === name,
    );
    if (finishRecordingId) {
      hostedRecoveryWorkspace.queueFinished(finishRecordingId, {
        recordingId: finishRecordingId,
        sessionName: name,
        projectId: projectIdForSession(name),
        memoLines: savedMemoLines.map(line => ({ ...line })),
      });
    }

    // Name-keyed state belongs to B when names collide. Keep A's completion
    // queued by recording ID rather than downgrading B's live row or memo.
    if (!collidesWithRestoredActive) {
      lifecycleBySession[name] = sessionLifecycleFromRecording(name, "captured", "Capture saved.");
      memoLinesBySession[sessionStateKey(name)] = savedMemoLines;
      summarySessionName = name;
      sessions = sessions.map(s => s.name === name ? { ...s, status: "unprocessed" } : s);
    }

    // Only navigate to the finished session's distill view if the user hasn't
    // moved away (e.g. started a new capture or navigated to another note).
    const userStillViewing =
      finalizingCaptureSessionName === name &&
      currentView === "home" &&
      activeSessionName === name &&
      !recordingStatus.is_recording &&
      recordingStartup === null;

    finalizingCaptureSessionName = null;

    if (userStillViewing) {
      activeSessionName = name;
      activeSessionTab = "distill";
      currentView = "home";
    }
    render();
    if (partialUploadWarning) alert(partialUploadWarning);
    void refreshSessionsAfterCaptureStop(name, restoredActive?.web_recording_id);
    if (restoredActive?.session_name !== name) void window.__loadArtifacts(name);
    return name;
  } catch (e: any) {
    await recoverRecordingStatusAfterCommandFailure(finishTargetName, {
      is_recording: true,
      paused: false,
      capture_phase: "needs_attention",
    }, finishRecordingId);
    alert(`Failed to stop recording: ${e}`);
    return null;
  }
};

window.__copyReview = () => {
  navigator.clipboard.writeText(reviewContent).then(() => {
    // Brief visual feedback — update only the label span so the ⌘C chip survives.
    const label = document.querySelector('.review-copy .review-copy-label') as HTMLElement | null;
    if (label) {
      const original = label.textContent;
      label.textContent = "Copied!";
      setTimeout(() => { label.textContent = original; }, 1500);
    }
  });
};

window.__openNote = async (name: string) => {
  try {
    await openNote(name, projectIdForSession(name));
  } catch (e: any) {
    alert(`Could not open note in editor: ${e}`);
  }
};

window.__openInVault = async (name?: string) => {
  const target = name || reviewSessionName || activeSessionName;
  if (!target) return;
  try {
    await openNoteInObsidian(target, projectIdForSession(target));
  } catch (e: any) {
    alert(`Could not open in Obsidian: ${e}`);
  }
};

window.__openNoteTargetInVault = async (target: string) => {
  const cleanTarget = target.trim();
  if (!cleanTarget) return;
  const projectId = activeSessionName ? projectIdForSession(activeSessionName) : activeProjectId();
  try {
    await openNoteTargetInObsidian(cleanTarget, projectId);
  } catch (e: any) {
    alert(`Could not open linked note in Obsidian: ${e}`);
  }
};

window.__sendFollowup = async (name: string) => {
  const input = document.getElementById(`followup-input-${name}`) as HTMLTextAreaElement | null;
  const text = input?.value.trim();
  if (!text) return;
  if (input) input.value = "";

  // Record the request in the thread and start a refine turn. A refine resumes
  // the saved Pi conversation and overwrites the note in place, so we drive the
  // same streaming UI as a first-pass distillation.
  const chat = followupChatBySession[name] || [];
  chat.push({ role: "user", text });
  followupChatBySession[name] = chat;

  processingRunId += 1;
  const runId = processingRunId;
  processingSessionName = name;
  delete noteJobErrorBySession[name];
  currentProgress = 0.05;
  lifecycleBySession[name] = newSessionLifecycleState(name);
  const stateKey = sessionStateKey(name);
  groundedNoteBySession[stateKey] = newGroundedNoteState();
  sessions = sessions.map(s => s.name === name ? { ...s, status: "processing" } : s);
  render();

  if (currentProcessingUnlisten) {
    try { currentProcessingUnlisten(); } catch {}
    currentProcessingUnlisten = null;
  }
  const unlisten = await onProcessingProgress((event: ProcessingEvent) => {
    if (runId !== processingRunId) return;
    rememberLifecycleEvent(name, event);
    let shouldRender = event.stage !== "note_stream";
    if (event.stage === "note_stream") {
      shouldRender = consumeNoteStreamChunkForStateKey(stateKey, event.message);
      rememberNoteWritingTraceForStateKey(stateKey, name, event.progress);
      processingSteps.push(event);
    } else {
      processingSteps.push(event);
      rememberProcessingTraceForStateKey(stateKey, event);
    }
    if (event.progress !== null) currentProgress = event.progress;
    renderProcessingProgress(event.stage, shouldRender);
  });
  currentProcessingUnlisten = unlisten;
  const detach = () => {
    if (currentProcessingUnlisten === unlisten) currentProcessingUnlisten = null;
    try { unlisten(); } catch {}
  };

  try {
    await refineSession(name, text, projectIdForSession(name));
    if (runId !== processingRunId) { detach(); return; }
    detach();
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    await window.__loadArtifacts(name);
    processingSessionName = "";
    currentProgress = 1;
    followupChatBySession[name] = [
      ...(followupChatBySession[name] || []),
      { role: "assistant", text: "Updated the note with that change." },
    ];
    render();
  } catch (e: any) {
    detach();
    if (runId !== processingRunId) return;
    const message = String(e?.message ?? e ?? "");
    processingSessionName = "";
    if (message.includes(DISTILL_CANCELLED_SENTINEL)) { render(); return; }
    // Refine failed: keep the existing note, surface the reason in the thread.
    try { window.__loadArtifacts(name); } catch {}
    const clean = message.replace(/^Error:\s*(Error:\s*)?/, "").trim() || "Couldn't apply that change.";
    followupChatBySession[name] = [
      ...(followupChatBySession[name] || []),
      { role: "assistant", text: clean },
    ];
    render();
  }
};

window.__copyTrace = (name: string) => {
  const trace = visiblePiTrace(traceForSession(name));
  const text = trace.map((event, index) => `${index + 1}. [${traceStatusText(event)}] ${event.label}: ${event.detail}`).join("\n");
  navigator.clipboard.writeText(text).catch(() => undefined);
};

window.__showBackchannelSuggestion = (memoIndex: number) => {
  const card = backchannelCardsByMemo[memoIndex];
  if (!card) return;
  card.collapsed = false;
  activeBackchannelMemoIndex = memoIndex;
  if (currentView === "recording") recordingMounted = false;
  render();
};

window.__collapseBackchannelSuggestion = (memoIndex: number) => {
  const card = backchannelCardsByMemo[memoIndex];
  if (!card) return;
  card.collapsed = true;
  if (activeBackchannelMemoIndex === memoIndex) activeBackchannelMemoIndex = null;
  if (currentView === "recording") recordingMounted = false;
  render();
};

window.__copyBackchannelSuggestion = (memoIndex: number) => {
  const text = backchannelCardsByMemo[memoIndex]?.suggestion || "";
  if (text) navigator.clipboard.writeText(text).catch(() => undefined);
};

window.__updateBackchannelSteerDraft = (memoIndex: number, value: string) => {
  backchannelSteerDraftByMemo[memoIndex] = value;
};

window.__steerBackchannelForMemo = (memoIndex: number): boolean => {
  const card = backchannelCardsByMemo[memoIndex];
  const draft = (backchannelSteerDraftByMemo[memoIndex] || "").trim();
  if (!card || card.steering || !card.suggestion || !draft) return false;
  card.steering = true;
  backchannelSteerDraftByMemo[memoIndex] = "";
  steerBackchannelForMemo(memoIndex, draft, card.suggestion, card.session_name, recordingStatus.web_recording_id).catch((e) => {
    console.warn("Steer backchannel failed", e);
    const current = backchannelCardsByMemo[memoIndex];
    if (current) current.steering = false;
    backchannelSteerDraftByMemo[memoIndex] = draft;
    if (currentView === "recording") recordingMounted = false;
    render();
  });
  if (currentView === "recording") recordingMounted = false;
  render();
  return false;
};

window.__retryBackchannelForMemo = (memoIndex: number) => {
  delete backchannelErrorByMemo[memoIndex];
  delete backchannelTerminalByMemo[memoIndex];
  fireBackchannelRequest(memoIndex);
};

window.__dismissBackchannelError = (memoIndex: number) => {
  delete backchannelErrorByMemo[memoIndex];
  if (currentView === "recording") recordingMounted = false;
  render();
};

function collapsePreviousBackchannelCards(currentIndex: number) {
  activeBackchannelMemoIndex = currentIndex;
}

function scheduleMemoCheckpoint(committedIndex: number) {
  if (!recordingStatus.is_recording) {
    queuedMemoCheckpointIndices.add(committedIndex);
    return;
  }
  void runMemoCheckpoint(committedIndex).catch((e) => {
    console.warn("Memo transcript checkpoint failed", e);
  });
}

function scheduleBackchannelRequest(committedIndex: number) {
  delete backchannelErrorByMemo[committedIndex];
  delete backchannelTerminalByMemo[committedIndex];
  if (!recordingStatus.is_recording) {
    queuedBackchannelRequestIndices.add(committedIndex);
    return;
  }
  fireBackchannelRequestAfterCheckpoint(committedIndex);
}

function runMemoCheckpoint(committedIndex: number): Promise<void> {
  const existing = memoCheckpointInFlight.get(committedIndex);
  if (existing) return existing;
  const sessionName = memoCaptureSessionName();
  if (!sessionName) return Promise.resolve();
  const run = checkpointMemoLine(memoLines, committedIndex, sessionName, recordingStatus.web_recording_id)
    .finally(() => {
      if (memoCheckpointInFlight.get(committedIndex) === run) {
        memoCheckpointInFlight.delete(committedIndex);
      }
    });
  memoCheckpointInFlight.set(committedIndex, run);
  return run;
}

function fireBackchannelRequestAfterCheckpoint(index: number) {
  const sessionName = memoCaptureSessionName();
  if (!sessionName) return;
  const checkpoint = memoCheckpointInFlight.get(index);
  if (checkpoint) {
    checkpoint.then(() => {
      if (memoCaptureSessionName() === sessionName) fireBackchannelRequest(index);
    }).catch((e) => {
      console.warn("Backchannel request suppressed after memo checkpoint rejection", e);
    });
    return;
  }
  fireBackchannelRequest(index);
}

// Open the assist rail the instant a mark fires: drop in an optimistic
// placeholder card so the cue box (with its shimmering skeleton) is on screen
// before the model's first token returns, then dispatch the real request.
function fireBackchannelRequest(index: number) {
  const existing = backchannelCardsByMemo[index];
  if (existing?.status === "pending" || existing?.status === "ready") return;
  if (backchannelTerminalByMemo[index] || backchannelErrorByMemo[index]) return;
  const sessionName = memoCaptureSessionName();
  if (!sessionName) return;
  showBackchannelPlaceholder(index);
  requestBackchannelForMemo(memoLines, index, sessionName, recordingStatus.web_recording_id).catch((e) => console.warn("Backchannel request failed", e));
}

function showBackchannelPlaceholder(index: number) {
  // Don't clobber a card that already has a real answer (or a terminal state)
  // when a duplicate/queued request fires for the same mark.
  const existing = backchannelCardsByMemo[index];
  if (existing && existing.status === "ready") return;
  if (backchannelTerminalByMemo[index] || backchannelErrorByMemo[index]) return;
  const line = memoLines[index];
  backchannelCardsByMemo[index] = {
    session_name: activeSessionName || "",
    memo_index: index,
    memo_time: line ? formatElapsed(line.created_secs) : "",
    status: "pending",
    state: "pending",
    suggestion: null,
    collapsed: false,
    placeholder: true,
    received_at: Date.now(),
  };
  activeBackchannelMemoIndex = index;
  if (currentView === "recording") recordingMounted = false;
  render();
}

window.__refreshDevices = async () => {
  devices = await refreshDevices().catch(() => devices);
  if (settingsOverlayOpen) render();
};

function webMicrophoneErrorMessage(error: unknown): string {
  const failure = isWebMicrophoneFailure(error) ? error : null;
  switch (failure?.category) {
    case "no-device": return "No microphone was found. Connect one and try again.";
    case "selected-device-unavailable": return "The selected microphone is unavailable. Choose another device.";
    case "hardware-unavailable": return "The microphone could not start. It may be in use by another app.";
    case "policy-blocked": return "A browser or organization policy blocked microphone access.";
    case "permission-denied": return "Microphone access remains blocked for this site.";
    case "insecure-context": return "Microphone access requires HTTPS or localhost.";
    case "unsupported": return "This browser does not support microphone recording.";
    case "aborted": return "The microphone check was cancelled.";
    default: return "The microphone check could not start. Try again.";
  }
}

window.__refreshWebMicrophonePermission = async () => {
  webMicrophoneError = null;
  webMicrophonePermissionState = await webMicrophonePermission.refresh();
  if (webMicrophonePermissionState !== "denied") webMicrophoneFailureCategory = null;
  if (webMicrophonePermissionState === "granted") {
    devices = await refreshDevices().catch(() => devices);
  }
  renderSettings();
};

window.__requestWebMicrophonePermission = async () => {
  if (webMicrophoneRequesting) return;
  webMicrophoneRequesting = true;
  webMicrophoneError = null;
  renderSettings();
  try {
    webMicrophonePermissionState = await webMicrophonePermission.request();
    webMicrophoneFailureCategory = webMicrophonePermission.failureCategory;
    if (webMicrophonePermissionState === "granted") {
      devices = await refreshDevices().catch(() => devices);
    }
  } catch (error) {
    webMicrophoneFailureCategory = isWebMicrophoneFailure(error) ? error.category : null;
    webMicrophoneError = webMicrophoneErrorMessage(error);
  } finally {
    webMicrophoneRequesting = false;
    renderSettings();
  }
};

// Selecting a microphone in audio setup applies to the LIVE session immediately
// when recording; otherwise it just stages a pending settings change for the
// next capture.
window.__onInputDeviceChange = async () => {
  const select = document.getElementById("input-device-name") as HTMLSelectElement | null;
  if (!select) return;
  const uid = select.value || null;
  const name = uid ? devices.find(device => device.uid === uid)?.name ?? null : null;
  if (isHostedWeb()) {
    webMicrophoneDeviceId = uid;
    setPreferredWebMicrophoneDeviceId(uid);
    audioTestResult = null;
    audioTestError = null;
    renderSettings();
    return;
  }
  const resolvedName = name ?? defaultCaptureDevice(devices)?.name ?? null;
  const nextMode = uid ? "pinned" : "follow_default";
  const policyChanged = nextMode !== inputDeviceMode()
    || (nextMode === "pinned" && uid !== settings.input_device_uid);
  const policyHint = document.getElementById("input-device-policy-hint");
  if (policyHint) {
    policyHint.textContent = uid
      ? `Always records from ${name || "your pinned mic"}. If it's unplugged, Margins uses your Mac's default and switches back next recording.`
      : "Records from whatever mic your Mac uses, and follows when you change it.";
  }
  audioTestResult = null;
  audioTestError = null;
  if (recordingStatus.is_recording && !recordingStatus.paused && (policyChanged || resolvedName !== liveCaptureDeviceName())) {
    const switched = await switchLiveCaptureDevice(uid);
    if (switched) {
      settings = {
        ...settings,
        input_device_mode: nextMode,
        input_device_uid: uid,
        input_device_name: name,
      };
      if (settingsOverlayOpen) render();
    }
    return;
  }
  // Stage the unsaved pick by stable UID so it survives model-download
  // re-renders that reload settings from disk (see pendingDeviceSelection).
  pendingDeviceSelection = { mode: nextMode, uid, name };
  window.__updateSettingsSaveState();
};

// Deterministic hook for driving the input-device picker from automation.
// The picker's staging runs off the <select>'s native 'change' event, which is
// NOT fired when an accessibility tool sets `.value` programmatically (AX
// SetValue / assigning element.value dispatches no 'change'). Real users click
// and fire 'change' normally, so the pin flow is sound for them — but it is
// otherwise un-automatable. This hook sets the value and invokes the same
// handler a real change would, so verifiers can exercise the explicit-pin path.
// Pass a device UID to pin it, or null/"" to follow the system default.
window.__setInputDevice = async (uid: string | null) => {
  const select = document.getElementById("input-device-name") as HTMLSelectElement | null;
  if (select) select.value = uid || "";
  await window.__onInputDeviceChange();
};

window.__chooseCaptureDevice = async () => {
  await window.__openAudioSetup();
  window.setTimeout(() => {
    const select = document.getElementById("input-device-name") as HTMLSelectElement | null;
    select?.focus();
  }, 0);
};

window.__usePreviousCaptureDevice = async () => {
  const previousDevice = captureDeviceSwitchToast?.kind === "switch"
    ? captureDeviceSwitchToast.previousDevice
    : null;
  if (!previousDevice) return;
  const switched = await switchLiveCaptureDevice(previousDevice);
  if (!switched) return;
  const pinned = devices.find(device => device.name === previousDevice);
  settings = {
    ...settings,
    input_device_mode: "pinned",
    input_device_uid: pinned?.uid ?? settings.input_device_uid ?? null,
    input_device_name: previousDevice,
  };
};

window.__retryCaptureDeviceSwitch = async () => {
  const retryDevice = captureDeviceSwitchToast?.kind === "switch_failed"
    ? captureDeviceSwitchToast.retryDevice
    : null;
  if (!retryDevice) return;
  await switchLiveCaptureDevice(retryDevice);
};

window.__selectVaultPath = async () => {
  const current = (document.getElementById("project-path") as HTMLInputElement | null)?.value || activeProject(settings).path || DEFAULT_VAULT_PATH;
  try {
    const selected = await selectVaultFolder(current);
    if (selected) {
      const project = activeProject(settings);
      const projects = normalizeProjects(settings).map(p => p.id === project.id ? { ...p, path: selected, name: p.name || projectNameFromPath(selected) } : p);
      settings = normalizeSettings({ ...settings, projects, active_project_id: project.id });
      await window.__validateVaultPath(selected);
    }
  } catch (e: any) {
    alert(`Could not choose project folder: ${e}`);
  }
};

// Staged folder change from Settings > Notes > Change….
// Updates settings in memory and the DOM display, but does NOT persist to disk.
// The caller's Save button commits the folder change alongside all other settings,
// preserving audio readiness, model dir, and every other field (merge-not-replace).
window.__settingsChangeVaultPath = async () => {
  const current = (document.getElementById("project-path") as HTMLInputElement | null)?.value
    || activeProject(settings).path || DEFAULT_VAULT_PATH;
  try {
    const selected = await selectVaultFolder(current);
    if (!selected) return;
    // Stage the pick in a survive-reload variable (not `settings`, which the
    // window-focus reload from returning to the app would clobber) and re-render
    // so the pane shows the staged path from state. Committed on Save.
    pendingVaultPathChange = selected;
    render();
    // Run vault validation for inline feedback. Patch the validation element
    // after the render so the async result lands in the freshly-rendered pane.
    const validationEl = document.getElementById("vault-validation");
    if (validationEl) {
      const projectId = activeProject(settings).id;
      try {
        const result = await validateVault(selected);
        notesFolderReady = result.exists;
        const checks: string[] = [];
        if (!result.exists) {
          checks.push('<div class="validation err">Can\'t find this folder. Reconnect it, or copy a prompt to fix setup.</div>');
        } else {
          checks.push('<div class="validation ok">Project folder connected.</div>');
          if (result.has_recall_index) checks.push('<div class="validation ok">Private search cache ready</div>');
          else checks.push(`<div class="validation warn">Note memory is not ready yet. <button type="button" class="inline-action" onclick="window.__quickSetupProject(${js(projectId)})">Quick setup</button></div>`);
        }
        // Re-resolve after the await in case a re-render replaced the element.
        const el = document.getElementById("vault-validation");
        if (el) el.innerHTML = checks.join("");
      } catch {
        notesFolderReady = false;
        const el = document.getElementById("vault-validation");
        if (el) el.innerHTML = '<div class="validation err">Can\'t find this folder.</div>';
      }
    }
    window.__updateSettingsSaveState();
  } catch (e: any) {
    alert(`Could not choose project folder: ${e}`);
  }
};

window.__addProject = async () => {
  const current = activeProject(settings).path || DEFAULT_VAULT_PATH;
  try {
    const selected = await selectVaultFolder(current);
    if (!selected) return;
    const projects = normalizeProjects(settings);
    let id = projectIdFromPath(selected);
    const existing = new Set(projects.map(p => p.id));
    for (let i = 2; existing.has(id); i++) id = `${projectIdFromPath(selected)}-${i}`;
    const nextProject = {
      id,
      name: projectNameFromPath(selected),
      path: selected,
      inbox_folder: "meetings",
      people_folder: settings.people_folder || "people",
      readiness: "needs_setup",
    };
    const result = await registerProject(settings, nextProject);
    settings = normalizeSettings(result.settings);
    obsidianVaultReady = result.validation.has_obsidian;
    notesFolderReady = result.validation.exists;
    const newProjectId = activeProject(settings)?.id ?? null;
    sessions = [];
    if (newProjectId) {
      vaultNoteCounts.delete(newProjectId);
      countVaultNotes(selected).then(count => { vaultNoteCounts.set(newProjectId, count); render(); }).catch(() => {});
    }
    render();
    sessions = mergeRefreshedSessionList(await listSessions(newProjectId).then(visibleSessions).catch(() => []));
    if (newProjectId) projectSessionsById[newProjectId] = sessions;
    activeSessionName = sessions[0]?.name || null;
    render();
  } catch (e: any) {
    alert(`Could not add project: ${e}`);
  }
};

// Repoint an existing project's root folder in place (same id, same sessions),
// baked into the sidebar's folder glyph. Reuses the add-project machinery so the
// new folder gets checked and re-runs agent setup
// when it isn't indexed yet — but keeps the project's id and any custom name.
window.__changeProjectFolder = async (projectId: string) => {
  const project = normalizeProjects(settings).find(p => p.id === projectId);
  if (!project) return;
  try {
    const selected = await selectVaultFolder(project.path || DEFAULT_VAULT_PATH);
    if (!selected || selected === project.path) return;
    // register_project bootstraps .margins at the picked folder and validates it,
    // but matches by id and preserves the stored path spelling — so we repoint the
    // path ourselves afterward and persist the full settings.
    const result = await registerProject(settings, {
      ...project,
      path: selected,
      name: project.name || projectNameFromPath(selected),
      readiness: "needs_setup",
    });
    const projects = normalizeProjects(result.settings).map(p =>
      p.id === projectId ? { ...p, path: selected } : p);
    settings = normalizeSettings({ ...result.settings, projects, active_project_id: projectId });
    await updateSettings(settings).catch(e => console.warn("Could not persist project folder change", e));
    const v = result.validation;
    if (projectId === activeProject(settings).id) {
      obsidianVaultReady = v.has_obsidian;
      notesFolderReady = v.exists;
    }
    sessions = [];
    vaultNoteCounts.delete(projectId);
    countVaultNotes(selected).then(count => { vaultNoteCounts.set(projectId, count); render(); }).catch(() => {});
    render();
    await setProjectReadiness(projectId, !v.exists ? "error" : v.has_recall_index ? "ready" : "needs_setup");
    sessions = mergeRefreshedSessionList(await listSessions(projectId).then(visibleSessions).catch(() => []));
    projectSessionsById[projectId] = sessions;
    activeSessionName = sessions[0]?.name || null;
    render();
  } catch (e: any) {
    alert(`Could not change project folder: ${e}`);
  }
};

window.__installCliTool = async () => {
  if (cliInstallStatus.state === "installing") return;
  cliInstallStatus = { state: "installing", message: "Adding terminal command...", result: null };
  render();
  try {
    const result = await installCliTool();
    cliInstallStatus = { state: "installed", message: result.message, result };
  } catch (e: any) {
    cliInstallStatus = { state: "error", message: String(e), result: null };
  }
  render();
};

window.__selectProject = async (id: string) => {
  closeMobileNavigation();
  if (id === settings.active_project_id) return;
  if (preStartPrepOpen) abandonPreStartPrep();
  settings = normalizeSettings({ ...settings, active_project_id: id });
  await updateSettings(settings).catch(() => undefined);
  await window.__validateVaultPath(activeProject(settings).path);
  sessions = mergeRefreshedSessionList(await listSessions(id).then(visibleSessions).catch(() => []));
  projectSessionsById[id] = sessions;
  activeSessionName = sessions[0]?.name || null;
  activeSessionTab = activeSessionName ? defaultTabForSession(sessions[0]) : "backchannel";
  render();
  if (activeSessionName) void loadArtifacts(activeSessionName, { renderLoading: true });
};

// Re-sync which notes a project recognizes: purge sessions whose note file was
// deleted (DB rows + recording), drop capture-notes whose file vanished, and
// refresh the sidebar. Renamed/moved notes are recovered, not purged.
window.__refreshProjectNotes = async (id: string) => {
  if (refreshingProjectIds.has(id)) return;
  refreshingProjectIds.add(id);
  render();
  try {
    const refreshed = visibleSessions(await reconcileProjectNotes(id));
    projectSessionsById[id] = refreshed;
    if (id === activeProjectId()) {
      const merged = mergeRefreshedSessionList(refreshed);
      sessions = merged;
      if (activeSessionName && !merged.some(s => s.name === activeSessionName)) {
        activeSessionName = merged[0]?.name || null;
        activeSessionTab = activeSessionName ? defaultTabForSession(merged[0]) : "backchannel";
      }
    }
  } catch (e: any) {
    announceImportStatus(String(e?.message ?? e ?? "Could not sync notes."));
  } finally {
    refreshingProjectIds.delete(id);
    render();
  }
};

// Start a capture in a specific project from the "New in …" dropdown: make that
// project active (if it isn't), then kick off the default capture there.
window.__startMeetingInProject = async (id: string) => {
  if (preStartPrepOpen) abandonPreStartPrep();
  if (id !== settings.active_project_id) {
    settings = normalizeSettings({ ...settings, active_project_id: id });
    await updateSettings(settings).catch(() => undefined);
    await window.__validateVaultPath(activeProject(settings).path);
  }
  await window.__startDefaultMeeting();
};

window.__pickProjectDestination = async (id: string) => {
  const project = normalizeProjects(settings).find(p => p.id === id);
  if (!project) return;
  try {
    const folder = await selectProjectSubfolder(project.path || DEFAULT_VAULT_PATH);
    if (folder === null) return; // cancelled
    const projects = normalizeProjects(settings).map(p => p.id === id ? { ...p, inbox_folder: folder } : p);
    settings = normalizeSettings({ ...settings, projects });
    await updateSettings(settings).catch((e) => alert(`Could not save project destination: ${e}`));
    sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    render();
  } catch (e: any) {
    alert(`Could not set capture destination: ${e}`);
  }
};

window.__copyProjectHelpPrompt = async (id: string, mode: "assess" | "fix" | "setup" = "assess") => {
  const project = normalizeProjects(settings).find(p => p.id === id);
  if (!project) return;
  if (mode === "setup") {
    const readyForProject = agentSetupCliInstall.state === "ready" && agentSetupCliInstall.projectId === id;
    if (!readyForProject) {
      // First click: verify the CLI tools, then stop. The clipboard
      // write must run on a fresh click with no preceding await — WKWebView drops
      // transient user activation across async verification, which is why retrying
      // in the same handler never copied. We cache the verified CLI path and
      // let the next click do the (await-free) copy.
      if (agentSetupCliInstall.state === "installing") return;
      agentSetupCliInstall = { state: "installing", projectId: id, message: null };
      render();
      try {
        await ensureCliTools();
        agentSetupCliInstall = { state: "ready", projectId: id, message: null };
      } catch (e: any) {
        agentSetupCliInstall = { state: "error", projectId: id, message: String(e) };
      }
      render();
      return;
    }
    // Second click: tools are ready. Copy synchronously in this gesture — the
    // margins-native prompt points the agent at the embedded CLI setup guide.
  }
  const prompt = projectSetupPrompt(project, mode);
  try {
    await navigator.clipboard.writeText(prompt);
  } catch {
    if (mode === "setup") {
      agentSetupCliInstall = { state: "error", projectId: id, message: "Could not copy the setup prompt to the clipboard. Please try again." };
      render();
      return;
    }
  }
  if (mode === "setup") {
    agentSetupCliInstall = { state: "idle", projectId: null, message: null };
  }
  copiedProjectPrompt = { projectId: id, mode };
  if (mode === "setup") {
    agentSetupWaitingProjectId = id;
    startAgentSetupPolling(id);
  }
  if (copiedProjectPromptTimer !== null) window.clearTimeout(copiedProjectPromptTimer);
  copiedProjectPromptTimer = window.setTimeout(() => {
    copiedProjectPrompt = null;
    copiedProjectPromptTimer = null;
    render();
  }, 2200);
  render();
};

function projectSetupPrompt(project: ProjectSource, mode: "assess" | "fix" | "setup"): string {
  if (mode === "setup") return projectSetupAgentPrompt(project);
  if (mode === "fix") return projectSetupFixPrompt(project);
  return projectSetupAssessPrompt(project);
}

function projectSetupAgentPrompt(project: ProjectSource): string {
  return [
    "Paste into your agent:",
    `Set up Margins in ${project.path}. Run margins guide workspace-setup and follow it end to end.`,
  ].join("\n");
}

function projectSetupAssessPrompt(project: ProjectSource): string {
  return [
    `Review the Margins workspace at: ${project.path}`,
    `Meeting notes are saved to the "${project.inbox_folder || "meetings"}" subfolder.`,
    "Run margins guide workspace-setup for the setup guide.",
    "Do not install any extra tools unless I explicitly ask for a standalone engine workflow.",
    "",
    "Please:",
    "1. Confirm the folder exists and the private search cache is set up and current.",
    "2. Assess how my notes are organized (folders, naming, meeting vs. people notes)",
    "   and tell me in plain terms whether the structure is working.",
    "3. Recommend concrete improvements, smallest first.",
    "",
    "Do not run setup, re-index, move, rename, or delete anything on your own.",
    "For any change that moves/renames/deletes files, describe it and ask me first.",
    "Never touch files outside this folder.",
  ].join("\n");
}

function projectSetupFixPrompt(project: ProjectSource): string {
  return [
    `Diagnose the Margins setup for this workspace: ${project.path}`,
    `Meeting notes are saved to the "${project.inbox_folder || "meetings"}" subfolder.`,
    "Run margins guide workspace-setup for the setup guide.",
    "Do not install any extra tools unless I explicitly ask for a standalone engine workflow.",
    "",
    "Please:",
    "1. Confirm the folder exists and can be used by Margins.",
    "2. Check whether the private search cache is set up and current.",
    "3. Initialize the private search cache if it is missing, then identify the smallest fix if setup still does not finish.",
    "",
    "Do not run setup, re-index, move, rename, or delete anything on your own.",
    "For any change that moves/renames/deletes files, describe it and ask me first.",
    "Never touch files outside this folder.",
  ].join("\n");
}

function startAgentSetupPolling(id: string) {
  if (agentSetupPollTimer !== null) window.clearInterval(agentSetupPollTimer);
  agentSetupPollTimer = window.setInterval(() => {
    if (agentSetupWaitingProjectId !== id) {
      stopAgentSetupPolling();
      return;
    }
    void window.__checkProjectSetup(id, { silent: true });
  }, 5000);
}

function stopAgentSetupPolling() {
  if (agentSetupPollTimer !== null) window.clearInterval(agentSetupPollTimer);
  agentSetupPollTimer = null;
}

window.__updateSettingsSaveState = () => {
  const save = document.getElementById("settings-save-btn") as HTMLButtonElement | null;
  const hint = document.getElementById("settings-save-hint");
  const notesInput = document.getElementById("project-path") as HTMLInputElement | null;
  const notesDisplay = document.getElementById("project-path-display");
  const apiInput = document.getElementById("api-key") as HTMLInputElement | null;
  const destination = document.getElementById("meeting-notes-destination");
  const notesGroup = document.getElementById("notes-folder-group");
  const aiGroup = document.getElementById("ai-notes-group");
  const mode = (document.querySelector('input[name="ai-mode"]:checked') as HTMLInputElement | null)?.value || currentAiMode();
  const notesPath = notesInput?.value.trim() || "";
  const apiKey = apiInput?.value.trim() || "";
  const notesReady = Boolean(notesPath) && notesFolderReady && activeProject(settings).readiness === "ready";
  const aiReadyNow = mode === "included"
    ? true
    : mode === "chatgpt"
      ? (modeReadiness("chatgpt")?.ready ?? aiStatus.chatgpt_authenticated)
      : Boolean(apiKey);

  if (save) save.disabled = false;
  if (hint) {
    hint.textContent = settingsReadinessMessageFromDom();
    hint.classList.remove("error");
    hint.removeAttribute("role");
  }
  if (destination) destination.textContent = notesDestinationPath(notesPath || activeProject(settings).path || DEFAULT_VAULT_PATH, activeProject(settings).inbox_folder || "");
  notesInput?.classList.toggle("required-empty", !notesReady);
  notesDisplay?.classList.toggle("required-empty", !notesReady);
  apiInput?.classList.toggle("required-empty", mode === "api" && !apiKey);
  notesGroup?.classList.toggle("missing", !notesReady);
  aiGroup?.classList.toggle("missing", !aiReadyNow);
  // Keep the per-activity model summary in sync with the just-edited fields.
  scheduleAiPreviewRefresh();
};

// Recompute the resolution preview from the current (unsaved) settings DOM and
// update the role summary in place. Debounced so it is safe to fire on every
// keystroke; the underlying command is pure (no network/keychain). `immediate`
// is used for discrete actions (mode/tier switch) that already re-render.
function refreshAiPreviewFromDom(immediate: boolean) {
  if (aiPreviewDebounce) {
    clearTimeout(aiPreviewDebounce);
    aiPreviewDebounce = null;
  }
  const run = () => {
    aiPreviewDebounce = null;
    if (!settingsOverlayOpen) return;
    let draft: Settings;
    try {
      draft = collectSettingsFromDom();
    } catch {
      return;
    }
    void previewAiResolution(draft)
      .then(preview => {
        aiPreview = preview;
        updateRoleSummaryDom();
      })
      .catch(() => undefined);
  };
  if (immediate) run();
  else aiPreviewDebounce = setTimeout(run, 250);
}

function scheduleAiPreviewRefresh() {
  if (!settingsOverlayOpen) return;
  refreshAiPreviewFromDom(false);
}

function updateRoleSummaryDom() {
  const el = document.getElementById("ai-role-summary");
  if (el) el.innerHTML = roleSummaryInner(aiPreview);
}

async function refreshAiReadiness() {
  aiReadiness = await getAiReadiness().catch(() => aiReadiness);
}

// Switch settings panels by toggling visibility in place rather than
// re-rendering, so any unsaved input in the panel being left is preserved.
window.__settingsNav = (id: string) => {
  settingsActiveSection = id;
  document.querySelectorAll<HTMLElement>(".settings-nav-item").forEach(item =>
    item.classList.toggle("active", item.dataset.section === id));
  let activePane: HTMLElement | null = null;
  document.querySelectorAll<HTMLElement>(".settings-pane").forEach(pane => {
    const isActive = pane.dataset.pane === id;
    pane.classList.toggle("active", isActive);
    if (isActive) activePane = pane;
  });
  const title = document.getElementById("settings-pane-title");
  const subtitle = document.getElementById("settings-pane-subtitle");
  if (activePane) {
    if (title) title.textContent = (activePane as HTMLElement).dataset.title || "";
    if (subtitle) subtitle.textContent = (activePane as HTMLElement).dataset.subtitle || "";
  }
  const content = document.querySelector(".settings-content") as HTMLElement | null;
  if (content) content.scrollTop = 0;
};

window.__validateVaultPath = async (path: string) => {
  const el = document.getElementById("vault-validation");
  const input = document.getElementById("project-path") as HTMLInputElement | null;
  const display = document.getElementById("project-path-display");
  if (input) input.value = path;
  if (display) display.textContent = path || "No folder selected";
  const activeId = activeProject(settings).id;
  if (!path) {
    obsidianVaultReady = false;
    notesFolderReady = false;
    await setProjectReadiness(activeId, "error");
    if (el) el.innerHTML = "";
    window.__updateSettingsSaveState();
    return;
  }

  try {
    const result = await validateVault(path);
    const checks: string[] = [];
    obsidianVaultReady = result.has_obsidian;
    notesFolderReady = result.exists;
    if (!result.exists) {
      obsidianVaultReady = false;
      notesFolderReady = false;
      await setProjectReadiness(activeId, "error");
      if (el) el.innerHTML = `<div class="validation err">Can't find this folder. Reconnect it, or copy a prompt to fix setup.</div>`;
      window.__updateSettingsSaveState();
      return;
    }
    await setProjectReadiness(activeId, result.has_recall_index ? "ready" : "needs_setup");
    countVaultNotes(path).then(count => { vaultNoteCounts.set(activeId, count); render(); }).catch(() => {});
    checks.push('<div class="validation ok">Project folder connected.</div>');
    if (result.has_recall_index) {
      checks.push('<div class="validation ok">Private search cache ready</div>');
    } else {
      checks.push(`<div class="validation warn">Note memory is not ready yet. <button type="button" class="inline-action" onclick="window.__quickSetupProject(${js(activeId)})">Quick setup</button></div>`);
    }
    if (result.has_obsidian) {
      checks.push('<div class="validation ok">Opens in Obsidian</div>');
    }
    if (el) el.innerHTML = checks.join("");
    window.__updateSettingsSaveState();
  } catch {
    obsidianVaultReady = false;
    notesFolderReady = false;
    await setProjectReadiness(activeId, "error");
    if (el) el.innerHTML = `<div class="validation err">Can't find this folder. Reconnect it, or copy a prompt to fix setup.</div>`;
    window.__updateSettingsSaveState();
  }
};

async function setProjectReadiness(id: string, readiness: "ready" | "needs_setup" | "error" | "updating", persist = true) {
  const projects = normalizeProjects(settings).map(project => project.id === id ? { ...project, readiness } : project);
  settings = normalizeSettings({ ...settings, projects, active_project_id: settings.active_project_id || id });
  if (persist) {
    settings = normalizeSettings(await updateProjectReadiness(id, readiness).catch(() => settings));
  }
}

window.__indexProject = async (id: string) => {
  if (indexingProjectIds.has(id)) return;
  const project = normalizeProjects(settings).find(p => p.id === id);
  if (!project?.path) return;
  indexingProjectIds.add(id);
  await setProjectReadiness(id, "updating");
  render();
  try {
    const result = await indexVault(project.path);
    await setProjectReadiness(id, result.has_recall_index ? "ready" : "needs_setup");
    if (result.has_recall_index && agentSetupWaitingProjectId === id) {
      agentSetupWaitingProjectId = null;
      stopAgentSetupPolling();
    }
    if (id === activeProject(settings).id) {
      obsidianVaultReady = result.has_obsidian;
      notesFolderReady = result.exists;
      sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => sessions));
    }
  } catch (e) {
    console.warn("Project indexing failed", e);
    await setProjectReadiness(id, "error");
  } finally {
    indexingProjectIds.delete(id);
    render();
  }
};

window.__quickSetupProject = async (id: string) => {
  agentSetupWaitingProjectId = null;
  stopAgentSetupPolling();
  dismissedProjectSetupHints.delete(id);
  await window.__indexProject(id);
};

window.__dismissProjectSetupHint = (id: string) => {
  dismissedProjectSetupHints.add(id);
  render();
};

window.__checkProjectSetup = async (id: string, options?: { silent?: boolean }) => {
  const project = normalizeProjects(settings).find(p => p.id === id);
  if (!project?.path) return;
  try {
    const result = await validateVault(project.path);
    await setProjectReadiness(id, result.has_recall_index ? "ready" : "needs_setup");
    if (id === activeProject(settings).id) {
      obsidianVaultReady = result.has_obsidian;
      notesFolderReady = result.exists;
    }
    if (result.has_recall_index) {
      agentSetupWaitingProjectId = null;
      stopAgentSetupPolling();
    }
    render();
  } catch (e) {
    if (!options?.silent) {
      console.warn("Project setup check failed", e);
      await setProjectReadiness(id, "error");
      render();
    }
  }
};

const showSpeechModelProgress = (event: SpeechModelProgressEvent) => {
  const status = document.getElementById("speech-model-status");
  if (!status) return;
  const pct = event.progress == null ? null : Math.max(0, Math.min(100, Math.round(event.progress * 100)));
  status.className = "model-status";
  status.innerHTML = `
    <div class="model-progress-top">
      <strong>${esc(event.message)}</strong>
      ${pct == null ? "" : `<span>${pct}%</span>`}
    </div>
    <div class="model-progress-track"><div style="width: ${pct ?? 8}%"></div></div>
  `;
};

window.__prepareSpeechModels = async () => {
  // A download from a previous visit to Settings may still be running. Re-entering
  // the screen renders fresh DOM, so avoid launching a second concurrent download
  // (the backend rejects it) and instead reflect that a download is already active.
  if (speechModelDownloadInProgress) {
    showSpeechModelProgress({ stage: "prepare", message: "Download already in progress...", progress: null });
    return;
  }

  const parakeetDir = (document.getElementById("parakeet-model-dir") as HTMLInputElement)?.value || null;
  const setButtonsBusy = (busy: boolean) => {
    const downloadButton = document.getElementById("speech-model-download-btn") as HTMLButtonElement | null;
    const cancelButton = document.getElementById("speech-model-cancel-btn") as HTMLButtonElement | null;
    const clearButton = document.getElementById("speech-model-clear-btn") as HTMLButtonElement | null;
    if (downloadButton) downloadButton.disabled = busy;
    if (clearButton) clearButton.disabled = busy;
    if (cancelButton) cancelButton.classList.toggle("hidden", !busy);
  };
  speechModelDownloadInProgress = true;
  setButtonsBusy(true);
  let unlisten: (() => void) | null = null;

  showSpeechModelProgress({ stage: "prepare", message: "Preparing local transcription…", progress: 0 });

  try {
    unlisten = await onSpeechModelProgress(showSpeechModelProgress);
    const result = await prepareSpeechModels(parakeetDir);
    if (result.parakeet_model_dir) {
      const input = document.getElementById("parakeet-model-dir") as HTMLInputElement | null;
      if (input) input.value = result.parakeet_model_dir;
      settings = { ...settings, parakeet_model_dir: result.parakeet_model_dir };
      await updateSettings(settings).catch(() => undefined);
    }
    const status = document.getElementById("speech-model-status");
    if (status) {
      const ok = result.parakeet_ready && result.diarization_ready;
      status.className = `model-status ${ok ? "ok" : "warn"}`;
      status.innerHTML = ok
        ? `<div><strong>Local transcription is ready.</strong> Captures transcribe on this Mac.</div>`
        : result.parakeet_ready
          ? `<div><strong>Ready to transcribe.</strong> Speaker labeling is still finishing — you can start capturing now.</div>`
          : `<div><strong>Setup didn't finish.</strong> ${esc(result.parakeet_message)}</div>`;
    }
  } catch (e: any) {
    const status = document.getElementById("speech-model-status");
    if (status) {
      status.className = "model-status err";
      status.textContent = `Could not download local transcription. Your recording setup is unchanged. Check your connection and try again. (${e})`;
    }
  } finally {
    if (unlisten) unlisten();
    speechModelDownloadInProgress = false;
    setButtonsBusy(false);
  }
};

window.__cancelSpeechModelDownload = async () => {
  const status = document.getElementById("speech-model-status");
  const cancelButton = document.getElementById("speech-model-cancel-btn") as HTMLButtonElement | null;
  if (!speechModelDownloadInProgress) return;
  try {
    if (cancelButton) cancelButton.disabled = true;
    await cancelSpeechModelDownload();
    if (status) {
      status.className = "model-status warn";
      status.textContent = "Canceling download...";
    }
  } catch (e: any) {
    if (status) {
      status.className = "model-status err";
      status.textContent = `Could not cancel download: ${e}`;
    }
    if (cancelButton) cancelButton.disabled = false;
  }
};

// ---------------------------------------------------------------------------
// Model provisioning notice handlers
// ---------------------------------------------------------------------------

function setModelNoticeState(patch: Partial<ModelNoticeData>) {
  if (!modelProvisionNotice) return;
  modelProvisionNotice = { ...modelProvisionNotice, ...patch };
  render();
}

function dismissModelNoticeAfterDelay(ms = 3000) {
  if (modelNoticeDismissTimer != null) clearTimeout(modelNoticeDismissTimer);
  modelNoticeDismissTimer = window.setTimeout(() => {
    modelProvisionNotice = null;
    modelNoticeDismissTimer = null;
    render();
  }, ms);
}

// Resolve the notice after a download the frontend didn't own (the "already
// running" reattach path), where we have no prepare result — re-probe for the
// final state.
async function finalizeModelNoticeAfterDownload() {
  if (!modelProvisionNotice) return;
  const probe = await probeSpeechModels().catch(() => null);
  if (probe?.transcription_path) {
    settings = { ...settings, parakeet_model_dir: probe.transcription_path };
    await updateSettings(settings).catch(() => undefined);
  }
  if (!modelProvisionNotice) return;
  if (probe && probe.diarization !== "ready") {
    setModelNoticeState({ state: "B", progress: null, message: "" });
  } else {
    setModelNoticeState({ state: "done", progress: null, message: "" });
    dismissModelNoticeAfterDelay(3000);
  }
}

// State B ("Adding speaker labels…") auto-clears once diarization is ready.
// Called on capture finish / after processing, when the best-effort speaker
// model may have finished landing.
async function refreshModelNoticeDiarization() {
  if (!modelProvisionNotice || modelProvisionNotice.state !== "B") return;
  const probe = await probeSpeechModels().catch(() => null);
  if (probe && probe.diarization === "ready" && modelProvisionNotice?.state === "B") {
    modelProvisionNotice = null;
    render();
  }
}

window.__modelNoticeDismiss = () => {
  modelProvisionNotice = null;
  if (modelNoticeDismissTimer != null) {
    clearTimeout(modelNoticeDismissTimer);
    modelNoticeDismissTimer = null;
  }
  render();
};

window.__modelNoticeDownload = async () => {
  if (!modelProvisionNotice) return;
  setModelNoticeState({ state: "downloading", progress: null, message: "Starting download…" });

  let unlisten: (() => void) | null = null;
  try {
    unlisten = await onSpeechModelProgress((event: SpeechModelProgressEvent) => {
      if (!modelProvisionNotice) return;
      // Once the download finishes, the backend switches to warmup/diarization
      // stages. Show a distinct "Warming up…" state instead of reusing the
      // download percent (which otherwise looks like the bar jumping backwards).
      if (event.stage === "warmup" || event.stage === "diarization") {
        if (modelProvisionNotice.state === "downloading" || modelProvisionNotice.state === "warming") {
          setModelNoticeState({ state: "warming", progress: null, message: "" });
        }
        return;
      }
      if (modelProvisionNotice.state !== "downloading") return;
      setModelNoticeState({ progress: event.progress, message: event.message });
    });
    const result = await prepareSpeechModels(settings.parakeet_model_dir ?? null);
    if (result.parakeet_model_dir) {
      settings = { ...settings, parakeet_model_dir: result.parakeet_model_dir };
      await updateSettings(settings).catch(() => undefined);
    }
    // If diarization is still missing, show state B; otherwise show brief done message
    if (!result.diarization_ready) {
      setModelNoticeState({ state: "B", progress: null, message: "" });
    } else {
      setModelNoticeState({ state: "done", progress: null, message: "" });
      dismissModelNoticeAfterDelay(3000);
    }
  } catch (e: any) {
    const msg = String(e?.message ?? e ?? "");
    if (/cancel/i.test(msg)) {
      // Cancelled — go back to state A
      setModelNoticeState({ state: "A", progress: null, message: "" });
    } else if (msg.includes("already running")) {
      // A download is already in flight (e.g. the boot-triggered check kicked
      // one off). This is benign — reattach to its progress instead of showing
      // the red error, and finalize once it completes.
      if (unlisten) { unlisten(); unlisten = null; }
      setModelNoticeState({ state: "downloading", progress: null, message: "Download already in progress…" });
      const reattach = await onSpeechModelProgress(async (event: SpeechModelProgressEvent) => {
        if (!modelProvisionNotice || (modelProvisionNotice.state !== "downloading" && modelProvisionNotice.state !== "warming")) { reattach(); return; }
        if (event.stage === "warmup" || event.stage === "diarization") {
          if (modelProvisionNotice.state === "downloading") setModelNoticeState({ state: "warming", progress: null, message: "" });
          return;
        }
        if (event.stage === "complete" || (event.progress ?? 0) >= 1) {
          reattach();
          await finalizeModelNoticeAfterDownload();
          return;
        }
        if (modelProvisionNotice.state === "downloading") setModelNoticeState({ progress: event.progress, message: event.message });
      });
      // Guard the race where the in-flight download emitted "complete" in the gap
      // before we subscribed — otherwise the pill hangs on "already in progress".
      const raceProbe = await probeSpeechModels().catch(() => null);
      if (raceProbe && raceProbe.transcription === "ready") {
        reattach();
        await finalizeModelNoticeAfterDownload();
      }
      return; // keep the reattach listener alive past the finally below
    } else {
      setModelNoticeState({ state: "error", progress: null, message: msg });
    }
  } finally {
    if (unlisten) unlisten();
  }
};

window.__modelNoticeCancel = async () => {
  try {
    await cancelSpeechModelDownload();
  } catch {
    // ignore
  }
};

window.__modelNoticeUseFound = async (path: string) => {
  try {
    settings = { ...settings, parakeet_model_dir: path };
    await updateSettings(settings);
    modelProvisionNotice = null;
    render();
  } catch (e: any) {
    console.warn("[model-notice] failed to save path:", e);
  }
};

window.__modelNoticeChooseCustom = () => {
  setModelNoticeState({ customPathInputVisible: true, message: "" });
};

window.__modelNoticeConfirmCustom = async () => {
  const input = document.getElementById("model-notice-custom-path-input") as HTMLInputElement | null;
  const customPath = input?.value?.trim() || "";
  if (!customPath) return;
  try {
    const probe = await probeSpeechModels(customPath);
    if (probe.transcription === "ready") {
      settings = { ...settings, parakeet_model_dir: customPath };
      await updateSettings(settings);
      modelProvisionNotice = null;
      render();
    } else {
      // Stay in State C with the input visible so the user can correct the path
      // — a bad path is not a download failure and must not become the red
      // connection error.
      setModelNoticeState({
        state: "C",
        foundPath: customPath,
        customPathInputVisible: true,
        message: "No speech models found in that folder. Check the path and try again.",
      });
    }
  } catch (e: any) {
    // Surface the backend's tailored message when present; fall back to generic
    // copy only when it's absent. Still return to State C, not the error state.
    const msg = String(e?.message ?? e ?? "").trim();
    setModelNoticeState({
      state: "C",
      foundPath: customPath,
      customPathInputVisible: true,
      message: msg || "Couldn't check that folder. Try again.",
    });
  }
};

window.__clearSpeechModels = async () => {
  const status = document.getElementById("speech-model-status");
  try {
    const message = await clearSpeechModels();
    settings = { ...settings, parakeet_model_dir: null };
    await updateSettings(settings).catch(() => undefined);
    const input = document.getElementById("parakeet-model-dir") as HTMLInputElement | null;
    if (input) input.value = "";
    if (status) {
      status.className = "model-status muted";
      status.textContent = message;
    }
  } catch (e: any) {
    if (status) {
      status.className = "model-status err";
      status.textContent = `Could not clear models: ${e}`;
    }
  }
};

window.__setAiMode = (mode: string) => {
  settings = {
    ...settings,
    ai_mode: mode,
  };
  if (mode === "included") void prepareIncludedAiInBackground();
  cueTierSelection = null;
  render();
  // Refresh the per-activity summary for the newly selected mode.
  refreshAiPreviewFromDom(true);
};

// Quick-assists speed tier. Balanced = no override (cues follow the mode's
// prep-class model); Fastest = the flash-lite model; Custom reveals a single
// model input. The override booleans/model are derived in collectSettingsFromDom
// from this selection, so we only need to re-render + refresh the preview here.
window.__setCueTier = (tier: string) => {
  if (tier === "fastest") {
    settings = { ...settings, backchannel_same_as_distill: false, backchannel_model: FASTEST_CUE_MODEL };
  } else if (tier === "custom") {
    // Keep any previously-typed custom model; only flip on the separate-cue flag.
    settings = { ...settings, backchannel_same_as_distill: false };
  } else {
    settings = { ...settings, backchannel_same_as_distill: true, backchannel_model: null };
  }
  // Remember the choice so "Custom" (empty model) doesn't snap back to Balanced.
  cueTierSelection = (tier === "fastest" || tier === "custom") ? tier : "balanced";
  render();
  refreshAiPreviewFromDom(true);
};

window.__signInChatgpt = async () => {
  aiStatus = { ...aiStatus, chatgpt_message: "Opening ChatGPT login in your browser..." };
  render();
  try {
    aiStatus = await signInChatgpt();
    settings = { ...settings, ai_mode: "chatgpt" };
    await updateSettings(settings).catch(() => undefined);
    await refreshAiReadiness();
    render();
  } catch (e: any) {
    aiStatus = { chatgpt_authenticated: false, chatgpt_message: `Could not complete ChatGPT login: ${e}` };
    render();
  }
};

async function prepareIncludedAiInBackground() {
  if (includedAiStatus.included_ready) return;
  if (includedAiPreparePromise) return includedAiPreparePromise;
  includedAiPreparePromise = setupIncludedAi().finally(() => {
    includedAiPreparePromise = null;
  });
  return includedAiPreparePromise;
}

async function setupIncludedAi() {
  includedAiStatus = {
    included_ready: false,
    message: "Preparing included note-making...",
  };
  render();
  try {
    includedAiStatus = await prepareIncludedAi();
    await refreshAiReadiness();
  } catch (e: any) {
    includedAiStatus = {
      included_ready: false,
      message: `Included note-making is unavailable: ${e}`,
    };
  }
  render();
}

window.__refreshCalendar = async () => {
  await getCalendarEventSuggestion()
    .then(acceptCalendarSuggestionResult)
    .catch(() => calendarSuggestion);
  render();
};

window.__useCalendarSuggestion = () => {
  const input = document.getElementById("new-session-name") as HTMLInputElement | null;
  if (input && calendarSuggestion) {
    input.value = sessionNameFromSuggestion(calendarSuggestion);
    input.focus();
  }
};

window.__peopleInputKeydown = async (event: KeyboardEvent, name: string) => {
  const input = event.target as HTMLInputElement;
  if (event.key === "Tab") {
    const topMatch = topVisiblePersonSuggestion(input);
    if (!topMatch) return;
    event.preventDefault();
    input.value = topMatch;
    filterPeopleSuggestionMenu(input);
    return;
  }
  if (event.key !== "Enter" && event.key !== ",") return;
  event.preventDefault();
  const person = resolvePeopleCandidate(name, input.value.trim().replace(/,$/, ""));
  if (!person) return;
  input.value = "";
  const session = sessions.find(s => s.name === name);
  const people = [...peopleForSessionEdit(session), person];
  await setSessionPeople(name, people);
};

window.__peopleInputInput = (event: Event) => {
  filterPeopleSuggestionMenu(event.target as HTMLInputElement);
};

function topVisiblePersonSuggestion(input: HTMLInputElement): string {
  const root = input.closest(".people-add-shell, .people-empty-add");
  const items = Array.from(root?.querySelectorAll<HTMLElement>(".people-suggestion-item") || []);
  const item = items.find(item => !item.hidden);
  return item?.dataset.person?.trim() || "";
}

function filterPeopleSuggestionMenu(input: HTMLInputElement) {
  const root = input.closest(".people-add-shell, .people-empty-add");
  const query = normalizePersonLookupKey(input.value);
  const items = Array.from(root?.querySelectorAll<HTMLElement>(".people-suggestion-item") || []);
  let visible = 0;
  for (const item of items) {
    const person = item.dataset.person || item.textContent || "";
    const key = normalizePersonLookupKey(person);
    const match = !query || key.includes(query);
    item.hidden = !match;
    if (match) visible += 1;
  }
  root?.classList.toggle("people-suggestions-empty", visible === 0);
}

window.__selectPersonSuggestion = async (name: string, person: string) => {
  const session = sessions.find(s => s.name === name);
  const people = [...peopleForSessionEdit(session), person];
  const input = document.getElementById(`people-input-${name}`) as HTMLInputElement | null;
  if (input) input.value = "";
  await setSessionPeople(name, people);
};

window.__removePerson = async (name: string, person: string) => {
  const session = sessions.find(s => s.name === name);
  const removeKey = normalizePersonLookupKey(person);
  const people = peopleForSessionEdit(session).filter(p => normalizePersonLookupKey(p) !== removeKey);
  await setSessionPeople(name, people);
};

async function setSessionPeople(name: string, people: string[]) {
  const cleaned = await updateSessionPeople(name, uniquePeople(people)).catch(() => uniquePeople(people));
  sessions = sessions.map(s => s.name === name ? { ...s, people: cleaned, frontmatter_people: cleaned } : s);
  render();
}

async function runMicTest(): Promise<boolean> {
  const selectedUid = (document.getElementById("input-device-name") as HTMLSelectElement)?.value || null;
  const selected = selectedUid ? devices.find(d => d.uid === selectedUid) : defaultCaptureDevice(devices);
  audioTestRunning = true;
  audioTestError = null;
  audioTestResult = null;
  render();
  try {
    // Follow mode probes the current system default by passing null all the way
    // through; selecting a concrete row is the only way to test/persist a pin.
    audioTestResult = await testAudioInput(selectedUid);
    if (isHostedWeb()) {
      webMicrophonePermissionState = "granted";
      webMicrophoneFailureCategory = null;
      webMicrophoneError = null;
      devices = await refreshDevices().catch(() => devices);
      return audioTestResult.ok;
    }
    await persistAudioReadinessPatch({
      audio_input_ready: audioTestResult.ok,
      input_device_mode: selectedUid ? "pinned" : "follow_default",
      input_device_uid: selectedUid,
      input_device_name: selectedUid ? selected?.name ?? settings.input_device_name ?? null : null,
    });
    return audioTestResult.ok;
  } catch (e: any) {
    audioTestError = isHostedWeb() ? webMicrophoneErrorMessage(e) : String(e);
    if (isHostedWeb()) {
      webMicrophonePermissionState = webMicrophonePermission.state;
      webMicrophoneFailureCategory = isWebMicrophoneFailure(e) ? e.category : webMicrophonePermission.failureCategory;
      webMicrophoneError = audioTestError;
      return false;
    }
    await persistAudioReadinessPatch({ audio_input_ready: false });
    return false;
  } finally {
    audioTestRunning = false;
    render();
  }
}

window.__testAudioInput = async () => {
  await runMicTest();
};

// Single primary audio check: exercises the microphone (which triggers the mic
// permission prompt) and the computer-audio tap together. System audio is
// captured automatically, so it has no separate user control — its readiness is
// established here or recovered via the blocked→restart callout.
window.__runAudioCheck = async () => {
  audioResumeState = null;
  const selectedUid = (document.getElementById("input-device-name") as HTMLSelectElement)?.value || null;
  const selected = selectedUid ? devices.find(d => d.uid === selectedUid) : defaultCaptureDevice(devices);

  // This is one user operation even though it has two native probes. Keep the
  // card mounted in a single continuous checking state and commit both results
  // together; the former nested helpers each rendered and saved independently.
  audioCheckPhase = "microphone";
  audioTestRunning = true;
  systemAudioTestRunning = true;
  audioTestError = null;
  systemAudioTestError = null;
  audioTestResult = null;
  systemAudioTestResult = null;
  renderSettings();

  try {
    try {
      audioTestResult = await testAudioInput(selectedUid);
    } catch (e: any) {
      audioTestError = String(e);
    }

    audioCheckPhase = "system";
    const micState = document.querySelector<HTMLElement>("#microphone-test-state span:last-child");
    const systemState = document.querySelector<HTMLElement>("#system-audio-test-state span:last-child");
    if (micState) micState.textContent = `Microphone: ${audioTestError ? "Check microphone access" : audioTestMessage()}`;
    if (systemState) systemState.textContent = "Computer audio: Checking…";

    try {
      systemAudioTestResult = await testSystemAudioTap();
    } catch (e: any) {
      systemAudioTestError = String(e);
    }

    const next = {
      audio_input_ready: Boolean(audioTestResult?.ok),
      system_audio_ready: systemAudioTestResult?.status === "ok",
      input_device_mode: selectedUid ? "pinned" as const : "follow_default" as const,
      input_device_uid: selectedUid,
      input_device_name: selectedUid ? selected?.name ?? settings.input_device_name ?? null : null,
    };
    settings = { ...settings, ...next };
    await updateAudioSettings(next).catch((e) => console.warn("Could not persist audio readiness", e));
    if (next.system_audio_ready) {
      systemAudioPrivacyPaneOpened = false;
      resumedAfterPermissionRestart = false;
      clearAudioSetupResumeKey();
    }
    return next.audio_input_ready;
  } finally {
    audioCheckPhase = null;
    audioTestRunning = false;
    systemAudioTestRunning = false;
    renderSettings();
  }
};

// Derive the quick-assists (backchannel) settings from the tier control. The
// tier radios only render when the section owns the cue routing (i.e. not
// ChatGPT mode, and no externally-configured separate cue key); when they are
// absent we preserve whatever is already persisted so the settings-file escape
// hatch keeps working at runtime. The cue key / base URL are no longer editable
// in the UI, so they are always carried through untouched.
function backchannelSettingsFromDom(): CueBackchannelFields {
  const tier = (document.querySelector('input[name="cue-tier"]:checked') as HTMLInputElement | null)?.value as
    | CueTier
    | undefined;
  const customModel = (document.getElementById("backchannel-model") as HTMLInputElement | null)?.value || "";
  return backchannelFieldsForTier(tier ?? null, customModel, settings);
}

function collectSettingsFromDom(): Settings {
  const mode = (document.querySelector('input[name="ai-mode"]:checked') as HTMLInputElement | null)?.value || currentAiMode();
  const inputDeviceSelect = document.getElementById("input-device-name") as HTMLSelectElement | null;
  // If the user made an unsaved pick during model download (pendingDeviceSelection),
  // use that UID authoritatively rather than reading the DOM select value which
  // may have been corrupted by device-list re-renders. This is the stable-UID
  // path; the DOM read is the fallback for the normal (no-pending) case.
  const nextInputDeviceUid = isHostedWeb()
    ? settings.input_device_uid ?? null
    : pendingDeviceSelection !== null
      ? pendingDeviceSelection.uid
      : (inputDeviceSelect ? (inputDeviceSelect.value || null) : (settings.input_device_uid || null));
  const nextInputDeviceName = isHostedWeb()
    ? settings.input_device_name ?? null
    : pendingDeviceSelection !== null
      ? pendingDeviceSelection.name
      : (nextInputDeviceUid
          ? devices.find(device => device.uid === nextInputDeviceUid)?.name ?? settings.input_device_name ?? null
          : null);
  // Commit any unsaved Notes folder change staged by __settingsChangeVaultPath.
  // Staged separately from `settings` so a window-focus disk reload can't revert
  // it before Save reads it (see pendingVaultPathChange).
  const baseSettings = pendingVaultPathChange !== null
    ? stageProjectPathChange(settings, activeProject(settings).id, pendingVaultPathChange)
    : settings;
  const projects = normalizeProjects(baseSettings);
  const active = projects.find(p => p.id === baseSettings.active_project_id) || projects[0];
  return {
    vault_path: active?.path || baseSettings.vault_path || DEFAULT_VAULT_PATH,
    projects,
    active_project_id: active?.id || baseSettings.active_project_id || null,
    ai_mode: mode,
    ai_base_url: document.getElementById("ai-base-url")
      ? ((document.getElementById("ai-base-url") as HTMLInputElement).value || null)
      : (settings.ai_base_url || null),
    ai_model: document.getElementById("ai-model")
      ? ((document.getElementById("ai-model") as HTMLInputElement).value || null)
      : (settings.ai_model || null),
    chatgpt_model: document.getElementById("ai-model-chatgpt")
      ? ((document.getElementById("ai-model-chatgpt") as HTMLInputElement).value || null)
      : (settings.chatgpt_model || null),
    ai_models_namespaced: true,
    api_key: mode === "api" ? ((document.getElementById("api-key") as HTMLInputElement | null)?.value || settings.api_key || null) : (settings.api_key || null),
    ...backchannelSettingsFromDom(),
    cleanup_policy: (document.querySelector('input[name="cleanup"]:checked') as HTMLInputElement | null)?.value || settings.cleanup_policy || "immediate",
    input_device_mode: nextInputDeviceUid ? "pinned" : "follow_default",
    input_device_uid: nextInputDeviceUid,
    input_device_name: nextInputDeviceName,
    audio_input_ready: settings.audio_input_ready ?? false,
    system_audio_ready: settings.system_audio_ready ?? false,
    // Preserve null/unset: the editor <select> renders null as "system", so a
    // Save that never touched the picker must keep the stored value (incl. null)
    // instead of committing a "system" default the user never chose.
    editor_command: resolveEditorCommand(
      (document.getElementById("editor-command") as HTMLSelectElement | null)?.value,
      selectedEditorValue(),
      settings.editor_command,
    ),
    parakeet_model_dir: (document.getElementById("parakeet-model-dir") as HTMLInputElement | null)?.value || settings.parakeet_model_dir || null,
    // Speaker labeling is now derived automatically from the recording's channel
    // reality; the UI toggle was removed. Preserve any persisted value.
    rust_diarization_enabled: settings.rust_diarization_enabled ?? false,
    auto_start_from_calendar: (document.getElementById("auto-start-from-calendar") as HTMLInputElement | null)?.checked ?? settings.auto_start_from_calendar ?? true,
    inbox_folder: active?.inbox_folder || "",
    people_folder: active?.people_folder || (document.getElementById("people-folder") as HTMLInputElement | null)?.value || settings.people_folder || "people",
    created_date_format: (document.getElementById("created-date-format") as HTMLInputElement | null)?.value || settings.created_date_format || "[[%Y-%m-%d]]",
    sidebar_date_format: (document.getElementById("sidebar-date-format") as HTMLSelectElement | null)?.value || settings.sidebar_date_format || "compact",
    note_filename_template: (document.getElementById("note-filename-template") as HTMLInputElement | null)?.value || settings.note_filename_template || "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
    person_note_template: (document.getElementById("person-note-template") as HTMLTextAreaElement | null)?.value || settings.person_note_template || "# {{name}}\n",
    distill_instructions: (document.getElementById("distill-instructions") as HTMLTextAreaElement | null)?.value || settings.distill_instructions || "",
    import_speaker_count: (() => {
      const el = document.getElementById("import-speaker-count") as HTMLSelectElement | null;
      if (!el) return settings.import_speaker_count ?? 1;
      const n = Math.round(Number(el.value));
      return Number.isFinite(n) ? Math.max(1, Math.min(4, n)) : (settings.import_speaker_count ?? 1);
    })(),
  };
}

async function persistSettingsFromDom(requireReady: boolean): Promise<boolean> {
  if (requireReady && !settingsReadyFromDom()) {
    window.__updateSettingsSaveState();
    return false;
  }
  const nextSettings = collectSettingsFromDom();
  try {
    await updateSettings(nextSettings);
    settings = nextSettings;
    // The staged Notes folder change is now baked into `settings`; drop it so a
    // later re-render doesn't re-overlay a now-committed path.
    pendingVaultPathChange = null;
    return true;
  } catch (error: unknown) {
    console.error("Failed to save settings", error);
    const hint = document.getElementById("settings-save-hint");
    if (hint) {
      hint.textContent = settingsSaveErrorMessage(error, isHostedWeb());
      hint.classList.add("error");
      hint.setAttribute("role", "alert");
    }
    return false;
  }
}

window.__saveSettings = async () => {
  const inputDeviceSelect = document.getElementById("input-device-name") as HTMLSelectElement | null;
  // Same stable-UID preference as collectSettingsFromDom: if there's an unsaved
  // pending pick, trust it over the DOM read. The live-device switch decision
  // uses the same resolved UID so they stay consistent.
  const nextInputDeviceUid = isHostedWeb()
    ? settings.input_device_uid ?? null
    : pendingDeviceSelection !== null
      ? pendingDeviceSelection.uid
      : (inputDeviceSelect ? (inputDeviceSelect.value || null) : (settings.input_device_uid || null));
  const nextInputDeviceName = pendingDeviceSelection !== null
    ? pendingDeviceSelection.name
    : (nextInputDeviceUid ? devices.find(device => device.uid === nextInputDeviceUid)?.name ?? null : null);
  const nextLiveDeviceName = nextInputDeviceName ?? defaultCaptureDevice(devices)?.name ?? null;
  const nextInputDeviceMode = nextInputDeviceUid ? "pinned" : "follow_default";
  const shouldSwitchLiveDevice = Boolean(
    !isHostedWeb()
    && recordingStatus.is_recording
    && inputDeviceSelect
    && (nextLiveDeviceName !== liveCaptureDeviceName()
      || nextInputDeviceMode !== inputDeviceMode()
      || (nextInputDeviceMode === "pinned" && nextInputDeviceUid !== settings.input_device_uid)),
  );
  // Instant tactile feedback: the persist below is a backend round-trip, so mark
  // the button busy synchronously rather than leaving it inert until the IPC
  // resolves.
  const saveBtn = document.getElementById("settings-save-btn") as HTMLButtonElement | null;
  const saveLabel = saveBtn?.textContent ?? "Save";
  if (saveBtn) {
    saveBtn.disabled = true;
    saveBtn.textContent = "Saving…";
  }

  const saved = await persistSettingsFromDom(false);
  if (!saved) {
    if (saveBtn) {
      saveBtn.disabled = false;
      saveBtn.textContent = saveLabel;
    }
    return;
  }
  pendingDeviceSelection = null;
  if (shouldSwitchLiveDevice) {
    await switchLiveCaptureDevice(nextInputDeviceUid);
  }
  settingsOverlayOpen = false;
  if (currentView === "settings") currentView = "home";
  // Close immediately once the settings are persisted; the session-list refresh
  // is cosmetic and shouldn't hold the overlay open behind a second round-trip.
  render();
  listSessions()
    .then(visibleSessions)
    .then(list => {
      sessions = mergeRefreshedSessionList(list);
      render();
    })
    .catch(() => {});
};

// ---------------------------------------------------------------------------
// Polling
// ---------------------------------------------------------------------------

function startPolling() {
  stopPolling();
  let hostedDiscoveryTick = 0;
  statusInterval = window.setInterval(async () => {
    if (recordingStartup?.phase === "starting" && !recordingStatus.is_recording) {
      recordingStatus = { ...recordingStatus, elapsed_secs: displayElapsedSecs() };
      if ((currentView === "recording" || (currentView === "home" && activeSessionTab === "backchannel")) && recordingMounted) {
        updateRecordingHeader();
      }
      return;
    }
    // A pause/resume/stop command is mid-flight: its handler has already
    // rendered the optimistic state and will apply the command's own status
    // response. A poll fetched around the command boundary can carry the
    // pre-command state and briefly revert the footer, so sit this tick out.
    if (stopInFlight) return;
    try {
      const applySnapshot = recordingStatusApplyCount;
      const selectedRecordingId = isHostedWeb()
        ? hostedRecoveryState.selectedRecordingId()
        : null;
      let polled: RecordingStatus;
      if (selectedRecordingId) {
        polled = await getHostedRecordingStatus(selectedRecordingId);
        if (!hostedRecoveryState.acceptsSelectedPoll(selectedRecordingId, polled)) return;
        // Keep local capture B's MediaRecorder/PCM posture reconciled without
        // replacing the explicitly selected recovery A in the workspace.
        if (hostedDiscoveryTick % 8 === 0) {
          const global = await getRecordingStatus().catch(() => null);
          if (global) hostedActiveRecordingStatus = hostedRecoveryState.activeStatus(global);
        }
      } else {
        polled = await getRecordingStatus();
        if (isHostedWeb()) hostedActiveRecordingStatus = hostedRecoveryState.activeStatus(polled);
      }
      hostedDiscoveryTick += 1;
      if (isHostedWeb() && hostedDiscoveryTick % 8 === 0) {
        await refreshHostedRecoveryDiscovery().catch(() => hostedRecoveryState.list());
      }
      // Stale fetch: a command response (or a faster poll) applied newer state
      // while this request was in flight.
      if (stopInFlight || recordingStatusApplyCount !== applySnapshot) return;
      const wasPaused = recordingStatus.paused ?? false;
      applyRecordingStatus(polled);
      recordCaptureHealth();
      // A paused-state change swaps the footer actions, which updateRecordingHeader
      // does not patch — fall back to a full render so Resume/Finish appear.
      if ((recordingStatus.paused ?? false) !== wasPaused) {
        render();
      } else if ((currentView === "recording" || (currentView === "home" && activeSessionTab === "backchannel")) && recordingMounted) {
        updateRecordingHeader();
      }
    } catch { /* ignore */ }
  }, 250);

  memoSyncInterval = window.setInterval(async () => {
    const sessionName = memoCaptureSessionName();
    if (!recordingStatus.is_recording || !sessionName) return;
    const recordingId = recordingStatus.web_recording_id;
    const selectedId = hostedRecoveryState.selectedRecordingId();
    if (isHostedWeb() && !hostedMemoSyncAllowed({
      recordingId,
      selectedRecoveryId: selectedId,
      selectedRecoveryHydrated: hostedRecoveryState.selectedMemoReady(recordingId),
      locallyOwned: currentTabOwnsHostedCapture(),
    })) return;
    try {
      await syncMemo(memoLines, sessionName, recordingId);
    } catch { /* ignore */ }
  }, 5000);
}

function stopPolling() {
  if (statusInterval !== null) {
    clearInterval(statusInterval);
    statusInterval = null;
  }
  if (memoSyncInterval !== null) {
    clearInterval(memoSyncInterval);
    memoSyncInterval = null;
  }
}

// ---------------------------------------------------------------------------
// Drag-and-drop hit-testing
// ---------------------------------------------------------------------------

function positionInsideSidebar(payload: FileDropPayload, sidebar: HTMLElement): boolean {
  const rect = sidebar.getBoundingClientRect();
  // Native payload coordinates are physical (device) pixels; layout rects are
  // CSS pixels. Convert before comparing.
  const dpr = window.devicePixelRatio || 1;
  const x = payload.position.x / dpr;
  const y = payload.position.y / dpr;
  return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
}

function clearDropClasses(sidebar: HTMLElement) {
  sidebar.classList.remove("is-drop-target", "is-drop-invalid", "is-drop-granola", "is-drop-audio");
}

function endDrag(sidebar: HTMLElement) {
  clearDropClasses(sidebar);
  dragInProgress = false;
  lastDropValid = true;
  lastDropKind = null;
  if (renderPendingFromDrag) {
    renderPendingFromDrag = false;
    render();
  }
}

function handleFileDrop(payload: FileDropPayload) {
  const sidebar = document.querySelector<HTMLElement>(".sidebar");
  if (!sidebar) return;

  if (payload.type === "leave") {
    endDrag(sidebar);
    return;
  }

  const inside = positionInsideSidebar(payload, sidebar);

  if (payload.type === "enter" || payload.type === "over") {
    if (!inside) {
      endDrag(sidebar);
      return;
    }
    dragInProgress = true;
    // `over` omits paths in Tauri v2; keep the validity decided on `enter`.
    if (payload.paths.length > 0) {
      const hasGranola = payload.paths.some(isAcceptedGranolaImportPath);
      const hasAudio = payload.paths.some(isAcceptedAudioPath);
      lastDropValid = hasGranola || hasAudio;
      lastDropKind = hasGranola ? "granola" : hasAudio ? "audio" : null;
    }
    sidebar.classList.toggle("is-drop-target", lastDropValid);
    sidebar.classList.toggle("is-drop-invalid", !lastDropValid);
    sidebar.classList.toggle("is-drop-granola", lastDropKind === "granola");
    sidebar.classList.toggle("is-drop-audio", lastDropKind === "audio");
    return;
  }

  if (payload.type === "drop") {
    const droppedInside = inside;
    const paths = payload.paths;
    endDrag(sidebar);
    // Granola exports import immediately; audio opens the configure step
    // (speaker count must be chosen before Transcribe).
    if (droppedInside) {
      if (paths.some(isAcceptedGranolaImportPath)) {
        void beginGranolaImport(paths);
      } else {
        beginPendingImport(paths);
      }
    }
  }
}

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

// init() performs a sequence of backend calls (settings, AI status, calendar,
// vault validation, sessions, …) before the first render(). On the Tauri path
// these resolve near-instantly, but on the HTTP/server path each is a network
// round-trip, so the page would sit blank for the whole chain. Paint a calm,
// content-agnostic shell immediately so launch shows structure (never a
// premature "you have no notes" onboarding) while the real data loads in.
function renderBootShell() {
  app.innerHTML = `
    <div class="workspace-shell app-booting" style="--sidebar-width: ${sidebarWidth}px" aria-busy="true">
      <aside class="sidebar" aria-hidden="true"></aside>
      <main class="workspace-main">
        <div class="app-boot-loading" role="status" aria-label="Loading">
          <span class="app-boot-spinner"></span>
        </div>
      </main>
    </div>`;
}

async function init() {
  renderBootShell();
  if (isHostedWeb()) {
    try {
      await negotiateHostedCaptureProtocol();
      await refreshHostedRecoveryDiscovery();
    } catch (error: any) {
      app.innerHTML = `
        <main class="app-boot-loading" role="alert">
          <h1>Reload required</h1>
          <p>${esc(error?.message || String(error))}</p>
          <button onclick="window.location.reload()">Reload Margins</button>
        </main>`;
      return;
    }
  }
  // Suppress the default WebView context menu (whose "Reload" item is
  // non-functional and would blank the app). Editable fields keep their
  // native menu so copy/paste/spellcheck still work in the memo editor.
  document.addEventListener("contextmenu", (event) => {
    const target = event.target as HTMLElement | null;
    const editable =
      target?.closest("input, textarea, [contenteditable=''], [contenteditable='true']") != null;
    if (!editable) event.preventDefault();
  });

  onFileDrop(handleFileDrop).catch(() => undefined);

  onRecallIndexing((event) => {
    const projectId = projectIdForRecallIndexingEvent(event);
    if (!projectId) return;
    if (event.status === "started") {
      recallIndexingProjectIds.add(projectId);
    } else {
      recallIndexingProjectIds.delete(projectId);
    }
    render();
  }).catch(() => undefined);

  // Backend filesystem watcher fires this whenever settings.json changes on
  // disk (e.g. after `margins projects add`). Re-fetch settings, reconcile
  // readiness for the (possibly newly active) project, and re-render so the
  // sidebar reflects the new project without a restart.
  onSettingsChanged(() => {
    void reloadSettingsAndRender();
  }).catch(() => undefined);

  onLiveTranscriptReady((_payload: { session_name: string }) => {
    transcriptWarming = false;
    renderRecordingStatusChange();
  }).catch(() => undefined);

  onLiveTranscriptDegraded((_payload: { session_name: string; reason: string }) => {
    transcriptWarming = false;
    transcriptDegraded = true;
    renderRecordingStatusChange();
  }).catch(() => undefined);

  onCaptureDeviceChanged((state: CaptureDeviceEvent) => {
    if (
      state.state === "fallback"
      || state.state === "recovered"
      || state.state === "switch_failed"
    ) {
      reconcileCaptureDeviceState(state);
      renderRecordingStatusChange();
      return;
    }
    recordingStatus = normalizeRecordingStatus({ ...recordingStatus, capture_device: state });
    reconcileCaptureDeviceState(state);
    renderRecordingStatusChange();
  }).catch(() => undefined);

  onDevicesChanged((snapshot) => {
    devices = snapshot.devices;
    if (settingsOverlayOpen || recordingStatus.is_recording) renderRecordingStatusChange();
  }).catch(() => undefined);

  onBackchannelSuggestion((event: BackchannelSuggestionEvent) => {
    if (event.session_name !== memoCaptureSessionName()) return;
    // Fresh vault healing itself in the background: honest "getting set up"
    // state, not silence. Retriable — a later mark (post-index) can still
    // produce a real cue, so scheduleBackchannelRequest clears this.
    if (event.status === "warming" || event.state === "warming") {
      delete backchannelCardsByMemo[event.memo_index];
      delete backchannelErrorByMemo[event.memo_index];
      backchannelTerminalByMemo[event.memo_index] = { status: "warming", memo_time: event.memo_time, received_at: Date.now() };
      if (activeBackchannelMemoIndex === event.memo_index) activeBackchannelMemoIndex = null;
      if (currentView === "recording") recordingMounted = false;
      render();
      return;
    }
    // Self-heal exhausted: live cues can't be set up. Honest terminal line.
    if (event.status === "unavailable" || event.state === "cue_unavailable") {
      delete backchannelCardsByMemo[event.memo_index];
      delete backchannelErrorByMemo[event.memo_index];
      const raw = event.raw_json as Record<string, unknown> | null | undefined;
      backchannelTerminalByMemo[event.memo_index] = {
        status: "unavailable",
        memo_time: event.memo_time,
        received_at: Date.now(),
        reason: typeof raw?.reason === "string" ? raw.reason : "Recall unavailable",
        hint: typeof raw?.hint === "string" ? raw.hint : "run margins setup",
      };
      if (activeBackchannelMemoIndex === event.memo_index) activeBackchannelMemoIndex = null;
      if (currentView === "recording") recordingMounted = false;
      render();
      return;
    }
    const isTerminalQuiet = event.state === "quiet" || event.status === "quiet" || (event.status === "ready" && !event.suggestion);
    if (isTerminalQuiet) {
      delete backchannelCardsByMemo[event.memo_index];
      delete backchannelErrorByMemo[event.memo_index];
      backchannelTerminalByMemo[event.memo_index] = { status: "quiet", memo_time: event.memo_time, received_at: Date.now() };
      if (activeBackchannelMemoIndex === event.memo_index) activeBackchannelMemoIndex = null;
      if (currentView === "recording") recordingMounted = false;
      render();
      return;
    }
    if ((event.status !== "ready" && event.status !== "pending") || !event.suggestion) return;
    const existing = backchannelCardsByMemo[event.memo_index];
    if (event.status === "pending" && (existing?.status === "ready" || backchannelTerminalByMemo[event.memo_index] || backchannelErrorByMemo[event.memo_index])) return;
    if (event.status === "ready") delete backchannelTerminalByMemo[event.memo_index];
    delete backchannelErrorByMemo[event.memo_index];
    if (event.status === "ready") {
      console.info("backchannel_frontend_timing", {
        memo_index: event.memo_index,
        received_unix_ms: Date.now(),
        placeholder_to_event_ms: existing?.received_at ? Date.now() - existing.received_at : null,
      });
    }
    backchannelCardsByMemo[event.memo_index] = { ...event, collapsed: false, received_at: Date.now() };
    activeBackchannelMemoIndex = event.memo_index;
    if (currentView === "recording") recordingMounted = false;
    render();
  }).catch(() => undefined);
  onBackchannelError((event: BackchannelSuggestionEvent) => {
    if (event.session_name !== memoCaptureSessionName()) return;
    if (typeof event.memo_index !== "number") { console.warn("Backchannel error", event); return; }
    const existing = backchannelCardsByMemo[event.memo_index];
    delete backchannelCardsByMemo[event.memo_index];
    backchannelTerminalByMemo[event.memo_index] = { status: "error", received_at: Date.now() };
    backchannelErrorByMemo[event.memo_index] = { memo_time: event.memo_time || existing?.memo_time || "", received_at: Date.now() };
    if (activeBackchannelMemoIndex === event.memo_index) activeBackchannelMemoIndex = null;
    if (currentView === "recording") recordingMounted = false;
    render();
  }).catch(() => undefined);

  onPrepHydration((event: PrepHydrationEvent) => {
    // Route by block_ordinal; ignore if session name doesn't match.
    if (event.session_name !== activePrepSessionName()) return;
    const ordinal = event.block_ordinal ?? 0;
    // Edge case: if block 0 hydration lands after recording started, ignore it.
    if (ordinal === 0 && recordingStatus.is_recording) return;
    // Full supersession: replace that block's marginalia wholesale
    const bm = getOrCreateBlockMargin(ordinal);
    bm.state = event.state;
    bm.marginalia = event.marginalia ?? [];
    bm.reason = event.reason ?? event.error ?? null;
    bm.hint = event.hint ?? null;
    // Do NOT reset consumed/expanded/pulledTexts — those survive re-hydrations.
    // Re-render whenever the capture surface for this block is visible and the
    // clock is stopped — covers BOTH the standalone recording view (pre-start
    // prep, currentView "recording") AND the mid-meeting pause embedded in the
    // home session workspace (currentView "home"). Gating on "recording" alone
    // dropped hydrated cards on pause even though the data had landed.
    if (isCaptureSurfaceVisible() && isClockStopped() && ordinal === currentBlockOrdinal) {
      recordingMounted = false;
      render();
    }
  }).catch(() => undefined);

  installCaptureShortcuts({
    isRecording: () => hostedCaptureBlocksStart() || recordingStartup?.phase === "starting" || captureTransitionActive(),
    startCapture: () => window.__startDefaultMeeting(),
    stopCapture: () => (recordingStatus.paused ? window.__resumeRecording() : window.__pauseRecording()),
    focusMarkInput: () => {
      const el = document.getElementById("memo-input-new") as HTMLTextAreaElement | null;
      el?.focus();
    },
    openSettings: () => window.__nav("settings"),
  });

  try {
    settings = normalizeSettings(await getSettings());
  } catch {
    settings = { ...defaultSettings(), editor_command: "system" };
  }
  if (isHostedWeb()) {
    webMicrophoneDeviceId = getPreferredWebMicrophoneDeviceId();
    webMicrophonePermission.subscribe((state) => {
      webMicrophonePermissionState = state;
      if (state !== "denied") webMicrophoneFailureCategory = null;
      if (settingsOverlayOpen && settingsActiveSection === "audio") renderSettings();
    });
    subscribeWebMicrophoneDevices(() => {
      void refreshDevices().then(nextDevices => {
        devices = nextDevices;
        if (settingsOverlayOpen && settingsActiveSection === "audio") renderSettings();
        else if (recordingStatus.is_recording) renderRecordingStatusChange();
      }).catch(() => undefined);
    });
  }
  // Audio Setup needs the live default name for Follow mode and the complete
  // device list for pinning. Enumeration failure remains non-blocking.
  devices = await listDevices().catch(() => devices);

  aiStatus = await getAiStatus().catch(() => aiStatus);
  includedAiStatus = await getIncludedAiStatus().catch(() => includedAiStatus);
  aiReadiness = await getAiReadiness().catch(() => aiReadiness);
  aiPreview = await previewAiResolution(settings).catch(() => aiPreview);
  const realHttpHarness = typeof window !== "undefined" && new URLSearchParams(window.location.search).get("backend") === "http";
  if (!realHttpHarness && currentAiMode() === "included" && !includedAiStatus.included_ready) {
    void prepareIncludedAiInBackground();
  }
  await getCalendarEventSuggestion()
    .then(acceptCalendarSuggestionResult)
    .catch(() => calendarSuggestion);
  granolaStatus = await getGranolaImportStatus().catch(() => granolaStatus);
  await onGranolaImportProgress((event) => {
    if (!granolaBusy) return;
    granolaImportStage = event.total > 0 ? `${event.stage} (${event.current}/${event.total})` : event.stage;
    announceImportStatus(granolaImportStage);
    render();
  });
  const startupProject = activeProject(settings);
  if (startupProject.path) {
    const vault = await validateVault(startupProject.path).catch(() => null);
    // Do NOT auto-create the vault folder at launch. A fresh Mac opens on a
    // clean, representative welcome state and never asks for folder permission
    // just by opening — the folder is materialized on the first capture press
    // (or when the user explicitly confirms a folder). If the folder already
    // exists (returning user), we load its sessions as before.
    obsidianVaultReady = Boolean(vault?.has_obsidian);
    notesFolderReady = Boolean(vault?.exists);
    // Recompute readiness on startup so the sidebar dot reflects the live
    // vault/.enzyme state instead of a stale persisted value.
    if (vault?.exists) {
      await setProjectReadiness(startupProject.id, vault.has_recall_index ? "ready" : "needs_setup");
    } else if (startupProject.path === DEFAULT_VAULT_PATH) {
      // The default folder simply hasn't been created yet — this is the calm,
      // expected first-run state, not an error.
      await setProjectReadiness(startupProject.id, "needs_setup");
    } else {
      // A user-typed path that does not exist is a real problem worth flagging.
      await setProjectReadiness(startupProject.id, "error");
    }
  } else {
    await setProjectReadiness(startupProject.id, "error");
  }

  sessions = mergeRefreshedSessionList(await listSessions().then(visibleSessions).catch(() => []));
  projectFilesFingerprint = (await getProjectFilesFingerprint(activeProjectId()).catch(() => null))?.fingerprint ?? null;
  if (isHostedWeb()) {
    if (sessions.length > 0) {
      activeSessionName = sessions[0].name;
      activeSessionTab = defaultTabForStatus(sessions[0].status);
    }
    const globalStatus = await getRecordingStatus().catch(() => null);
    hostedActiveRecordingStatus = globalStatus
      ? hostedRecoveryState.activeStatus(globalStatus)
      : null;
    if (hostedActiveRecordingStatus) {
      applyRecordingStatus(hostedActiveRecordingStatus);
      activeSessionName = hostedActiveRecordingStatus.session_name;
      activeSessionTab = "backchannel";
      recordCaptureHealth();
      startPolling();
      void triggerModelProvisionCheck();
    } else {
      const firstRecovery = hostedRecoveryState.list()[0] ?? null;
      if (firstRecovery) {
        hostedRecoveryState.select(firstRecovery.recording_id);
        const recoveryStatus = await getHostedRecordingStatus(firstRecovery.recording_id).catch(() => null);
        if (recoveryStatus && hostedRecoveryState.acceptsSelectedPoll(firstRecovery.recording_id, recoveryStatus)) {
          applyRecordingStatus(recoveryStatus);
          activeSessionName = recoveryStatus.session_name;
          activeSessionTab = "backchannel";
          startPolling();
        }
      }
    }
  } else if (sessions.length > 0) {
    activeSessionName = sessions[0].name;
    activeSessionTab = defaultTabForStatus(sessions[0].status);
    if (sessions[0].status === "recording") {
      applyRecordingStatus(await getRecordingStatus().catch(() => recordingStatus));
      recordCaptureHealth();
      const hostedMemo = recordingStatus.session_name
        ? await hydrateRecordingMemo(recordingStatus.session_name, recordingStatus.web_recording_id).catch(() => null)
        : null;
      if (hostedMemo) {
        memoLines = hostedMemo.map(line => ({ ...line }));
        memoLinesBySession[sessionStateKey(recordingStatus.session_name!)] = memoLines.map(line => ({ ...line }));
      } else {
        memoLines = readHarnessMemoLines() ?? memoLines;
      }
      startPolling();
      // Booting straight into an active recording (e.g. the app was reopened
      // mid-capture) still needs the model-provisioning notice if local
      // transcription isn't set up — same check finishRecordingStart runs.
      void triggerModelProvisionCheck();
      // Mock harness: treat warming/warming-then-ready scenarios as if
      // transcriptWarming was set by startCapture (no startCapture call in
      // scenario boot path).
      if (typeof window !== "undefined") {
        const _scenarioParam = new URLSearchParams(window.location.search).get("scenario") ?? "";
        if (_scenarioParam === "recording-warming" || _scenarioParam === "recording-warming-then-ready") {
          transcriptWarming = true;
        }
      }
    }
  }

  // firstNoteCelebrated starts false; it's only set true when the first note
  // completes *in this session*. Init does not need to read the storage key —
  // if the key is already set, processSession will find it and skip setting
  // firstNoteCelebrated. The flag is never persisted as "currently showing";
  // it only lives in memory for the current session rendering.

  const initialParams = typeof window === "undefined" ? new URLSearchParams() : new URLSearchParams(window.location.search);
  const initialScenario = initialParams.get("scenario") || "";
  const initialHarnessView = initialParams.get("view") || "";
  let resumeSection: string | null = null;
  try {
    resumeSection = window.localStorage.getItem(SETUP_RESUME_SECTION_KEY);
  } catch {
    resumeSection = null;
  }
  const resumeAudioSetup = resumeSection === "audio";
  if (resumeAudioSetup) {
    settingsActiveSection = "audio";
    resumedAfterPermissionRestart = true;
    // "resumed" lands the user on the Audio pane with a one-press Confirm, but
    // does NOT probe the tap — the probe touches ScreenCapture TCC and launch
    // must stay TCC-clean. The tap runs on Confirm or on the first capture press.
    audioResumeState = "resumed";
    // Consume the resume intent immediately so a dismissed setup doesn't
    // force-open Audio settings on every future launch.
    clearAudioSetupResumeKey();
  }
  settingsOverlayOpen = initialScenario === "settings-audio" || initialScenario === "settings-audio-pinned" || initialScenario === "settings-audio-blocked" || initialScenario === "settings-audio-ready" || initialScenario === "settings-ai" || initialScenario === "settings-ai-api" || initialHarnessView === "settings-audio" || initialHarnessView === "settings-ai" || resumeAudioSetup;
  currentView = "home";
  const scenario = initialScenario;
  if (scenario === "settings-audio-blocked") {
    settingsActiveSection = "audio";
    audioTestResult = { device_name: selectedDeviceName() || "System Default Microphone", peak: 0.18, drop_count: 0, ok: true };
    systemAudioTestResult = {
      peak: 0,
      drop_count: 0,
      silent_secs: 0,
      frame_count: 0,
      status: "blocked",
      message: "Enable Margins under System Audio Recording Only, then quit and reopen the app.",
      restart_recommended: true,
    };
  } else if (scenario === "settings-audio-ready") {
    settingsActiveSection = "audio";
    audioTestResult = { device_name: selectedDeviceName() || "System Default Microphone", peak: 0.18, drop_count: 0, ok: true };
    systemAudioTestResult = {
      peak: 0.04,
      drop_count: 0,
      silent_secs: 0,
      frame_count: 86_400,
      status: "ok",
      message: "Computer-audio tap is receiving signal.",
      restart_recommended: false,
    };
  } else if (scenario === "settings-audio-resume") {
    settingsOverlayOpen = true;
    settingsActiveSection = "audio";
    audioResumeState = "confirmed";
    audioTestResult = { device_name: selectedDeviceName() || "System Default Microphone", peak: 0.18, drop_count: 0, ok: true };
    systemAudioTestResult = {
      peak: 0.04,
      drop_count: 0,
      silent_secs: 0,
      frame_count: 86_400,
      status: "ok",
      message: "Computer-audio tap is receiving signal.",
      restart_recommended: false,
    };
  } else if (initialScenario === "settings-audio" || initialScenario === "settings-audio-pinned" || initialHarnessView === "settings-audio") {
    settingsActiveSection = "audio";
  } else if (initialScenario === "settings-ai" || initialScenario === "settings-ai-api" || initialHarnessView === "settings-ai") {
    settingsActiveSection = "ai";
  } else {
    settingsActiveSection = aiReady() ? settingsActiveSection : "ai";
  }

  // CDP harness: seed first-note payoff for the dedicated scenario (harness
  // clears the localStorage key, then init runs — set the flag so the callout
  // renders without requiring __processSession to be called).
  if (initialScenario === "first-note-payoff") {
    try {
      const alreadyCelebrated = window.localStorage.getItem(FIRST_NOTE_CELEBRATED_KEY);
      if (!alreadyCelebrated) {
        firstNoteCelebrated = true;
        window.localStorage.setItem(FIRST_NOTE_CELEBRATED_KEY, "1");
      }
    } catch {
      firstNoteCelebrated = true;
    }
  }

  render();
  window.__marginsAppReady = true;
  window.dispatchEvent(new CustomEvent("margins:app-ready"));
  // Do NOT auto-probe the system-audio tap here. After a permission restart we
  // land on the Audio pane in the "resumed" state; the tap probe (which touches
  // ScreenCapture TCC) runs only when the user presses Confirm
  // (__confirmSystemAudioAfterRestart) or starts a capture — keeping launch
  // TCC-clean.
  startProjectFilesRefreshLoop();
  if (activeSessionName) {
    window.__loadArtifacts(activeSessionName);
  }
  if (!recordingStatus.is_recording && recordingStartup?.phase !== "starting") {
    void installAvailableAppUpdate().catch((e) => console.warn("Margins update check failed", e));
  }
}

// Selecting and copying a passage inside a rendered note should yield plain
// Markdown, not the styled HTML the browser copies by default. The note body is
// markdown-it output, so we walk the selected DOM fragment back into Markdown
// source and put that on the clipboard as text/plain only (no text/html), so it
// pastes as raw Markdown everywhere.
function noteListToMarkdown(el: Element, ordered: boolean): string {
  const items = Array.from(el.children).filter(c => c.tagName.toLowerCase() === "li");
  return items
    .map((li, i) => {
      const marker = ordered ? `${i + 1}.` : "-";
      const content = Array.from(li.childNodes).map(noteNodeToMarkdown).join("").trim();
      // Indent any wrapped/nested lines so the bullet structure survives.
      const indented = content.replace(/\n(?!$)/g, "\n  ");
      return `${marker} ${indented}`;
    })
    .join("\n");
}

function noteNodeToMarkdown(node: Node): string {
  if (node.nodeType === Node.TEXT_NODE) {
    return (node.textContent || "").replace(/\s+/g, " ");
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return "";
  const el = node as HTMLElement;
  const tag = el.tagName.toLowerCase();
  const inner = () => Array.from(el.childNodes).map(noteNodeToMarkdown).join("");
  switch (tag) {
    case "h1": return `# ${inner().trim()}\n\n`;
    case "h2": return `## ${inner().trim()}\n\n`;
    case "h3": return `### ${inner().trim()}\n\n`;
    case "h4": return `#### ${inner().trim()}\n\n`;
    case "h5": return `##### ${inner().trim()}\n\n`;
    case "h6": return `###### ${inner().trim()}\n\n`;
    case "p": return `${inner().trim()}\n\n`;
    case "strong": case "b": return `**${inner()}**`;
    case "em": case "i": return `*${inner()}*`;
    case "del": case "s": return `~~${inner()}~~`;
    case "code":
      return el.parentElement?.tagName.toLowerCase() === "pre" ? inner() : `\`${inner()}\``;
    case "pre": return `\`\`\`\n${(el.textContent || "").replace(/\n$/, "")}\n\`\`\`\n\n`;
    case "br": return "\n";
    case "hr": return "---\n\n";
    case "a": {
      const href = el.getAttribute("href") || "";
      const text = inner();
      return href ? `[${text}](${href})` : text;
    }
    case "ul": return `${noteListToMarkdown(el, false)}\n\n`;
    case "ol": return `${noteListToMarkdown(el, true)}\n\n`;
    case "li": return inner();
    case "blockquote":
      return inner().trim().split("\n").map(l => `> ${l}`).join("\n") + "\n\n";
    default:
      return inner();
  }
}

document.addEventListener("copy", (e) => {
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
  if (!sel.toString().trim()) return;
  const anchor = sel.anchorNode;
  const host = anchor && (anchor.nodeType === Node.ELEMENT_NODE ? (anchor as Element) : anchor.parentElement);
  // Only override copies that originate inside a rendered note body.
  if (!host?.closest(".review-content")) return;
  const frag = sel.getRangeAt(0).cloneContents();
  const md = Array.from(frag.childNodes)
    .map(noteNodeToMarkdown)
    .join("")
    .replace(/\n{3,}/g, "\n\n")
    .trim();
  if (!md) return;
  e.clipboardData?.setData("text/plain", md);
  e.preventDefault();
});

// Review-view key bindings (attached once). Only act while on the review screen.
// Connected-note shortcuts. Keyed off the on-screen action elements so they
// work on both the full-screen review view and the workspace note view.
document.addEventListener("keydown", (e) => {
  const rerunBtn = document.getElementById("review-rerun") as HTMLButtonElement | null;
  const copyBtn = document.querySelector(".review-copy");
  // The settings overlay is modal: Escape dismisses it and Tab is trapped
  // inside it. Handle this before the review/copy shortcuts so the modal owns
  // the keyboard while it is open.
  if (settingsOverlayOpen) {
    const dialog = document.getElementById("settings-dialog");
    if (e.key === "Escape") {
      closeSettingsAndRestoreFocus();
      e.preventDefault();
      return;
    }
    if (e.key === "Tab" && dialog) {
      const focusables = settingsFocusables(dialog);
      if (focusables.length > 0) {
        const first = focusables[0];
        const last = focusables[focusables.length - 1];
        const activeEl = document.activeElement as HTMLElement | null;
        const inside = activeEl != null && dialog.contains(activeEl);
        if (!inside) {
          (e.shiftKey ? last : first).focus({ preventScroll: true });
          e.preventDefault();
        } else if (!e.shiftKey && activeEl === last) {
          first.focus({ preventScroll: true });
          e.preventDefault();
        } else if (e.shiftKey && activeEl === first) {
          last.focus({ preventScroll: true });
          e.preventDefault();
        }
      }
    }
    return;
  }
  if (e.key === "Escape") {
    if (isMobileNavigationViewport() && !sidebarCollapsed) {
      window.__toggleSidebar();
      e.preventDefault();
      return;
    }
    if (rerunBtn && rerunBtn.classList.contains("armed")) {
      // Disarm an armed re-run wherever it is shown.
      disarmRerun();
      e.preventDefault();
    } else if (currentView === "review") {
      // Only the full-screen review uses Esc to leave; don't navigate the workspace away.
      window.__nav("home");
      e.preventDefault();
    }
    return;
  }
  if ((e.metaKey || e.ctrlKey) && (e.key === "c" || e.key === "C")) {
    if (!copyBtn) return;
    // Respect an active text selection in the note (user copying a passage).
    const sel = window.getSelection();
    if (sel && sel.toString().length > 0) return;
    window.__copyReview();
    e.preventDefault();
  }
});

mobileNavigationQuery?.addEventListener("change", event => {
  if (event.matches) sidebarCollapsed = true;
  render();
});

init();
