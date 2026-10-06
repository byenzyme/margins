import type {
  DeviceInfo,
  AudioTestResult,
  CaptureDeviceState,
  CaptureDeviceEvent,
  SystemAudioTestResult,
  MemoLine,
  BackchannelSuggestionEvent,
  PrepHydrationEvent,
  HydratePrepRequest,
  ProcessingEvent,
  RecordingStatus,
  SessionInfo,
  Settings,
  ProjectSource,
  ProjectRegistrationResult,
  SpeechModelPrepResult,
  SpeechModelProgressEvent,
  SpeechModelProbe,
  TranscriptEntry,
  VaultValidation,
  AiStatus,
  IncludedAiStatus,
  AiReadiness,
  ResolutionPreview,
  PreviewRole,
  CalendarSuggestionResult,
  FileDropPayload,
  GranolaImportResult,
  GranolaImportSurvey,
  GranolaImportOptions,
  RecallIndexingEvent,
  GranolaImportStatus,
  GranolaImportProgressEvent,
  GranolaRemoteImportResult,
  EnsureCliToolsResult,
  UnlistenFn,
  DeviceSnapshot,
} from "../src/lib/tauri";

// Browser-only test backend for the Chrome/CDP UX loop. Keep scenario fixtures
// here so production app code stays focused on the real Tauri command surface.

export type MockScenario =
  | "first-run"
  | "settings-audio-blocked"
  | "settings-audio-ready"
  | "settings-audio-resume"
  | "settings-audio"
  | "settings-audio-pinned"
  | "settings-ai"
  | "settings-ai-api"
  | "home-empty"
  | "home-capture-notes"
  | "home-many-sessions"
  | "home-real"
  | "home-startup-lag"
  // Vault/project configured (capture-ready) but the note-making AI is NOT
  // signed in. Exercises: New button must read "New in {Project}" (not "Set
  // up"), capture starts, and note-making routes to Settings for sign-in.
  | "home-no-ai"
  | "recording-healthy"
  | "recording-device-holding"
  | "recording-device-holding-narrow"
  | "recording-device-switching"
  | "recording-device-switching-slow"
  | "recording-device-switched"
  | "recording-device-fallback"
  | "recording-device-fallback-settings"
  | "recording-device-recovered"
  | "recording-device-switch-failed"
  | "recording-muted-mic"
  | "recording-dead-tap"
  | "recording-no-system-audio-yet"
  | "recording-no-system-audio-dismissed"
  | "recording-system-audio-lost-recovering"
  | "recording-system-audio-recovered"
  | "recording-system-audio-recovery-failed"
  | "recording-with-memo"
  | "recording-dead-tap-with-memo"
  | "recording-paused"
  | "backchannel-built"
  | "backchannel-warming"
  | "backchannel-quiet"
  | "backchannel-unavailable"
  | "distill-running"
  | "distill-error-pi-login"
  | "distill-error-retry"
  | "distill-complete"
  | "ux-runtime-vault"
  | "note-streaming-grounded"
  | "grounded-note-complete"
  | "grounded-note-unused-memo"
  | "import-idle"
  | "import-drag-over"
  | "import-drag-invalid"
  | "import-configure"
  | "import-processing"
  | "import-error"
  | "import-complete"
  | "reprocess-with-people"
  | "granola-authorized"
  | "granola-needs-attention"
  | "first-note-payoff"
  | "model-notice-missing"
  | "model-notice-speakers"
  | "model-notice-found"
  | "recording-warming"
  | "recording-transcript-degraded"
  | "recording-warming-then-ready";

interface MockState {
  scenario: string;
  settings: Settings;
  devices: DeviceInfo[];
  sessions: SessionInfo[];
  recordingName: string | null;
  recordingStartedAt: number | null;
  recordingPaused: boolean;
  captureDeviceState: CaptureDeviceState | null;
  liveTranscriptionMode: "stereo_split" | "mic_diarized";
  memo: MemoLine[];
  aligned: Record<string, string>;
  notes: Record<string, string>;
  opened: string[];
  granolaAuthorized: boolean;
  granolaNeedsAttention: boolean;
  real_distill?: {
    summary_path?: string;
    note_path?: string;
    session_file?: string | null;
    elapsed_ms?: number;
    event_count?: number;
  };
}

interface MarginsMockWindow {
  getState: () => MockState | null;
  setScenario: (scenario: MockScenario) => MockState;
  patch: (patch: Partial<MockState>) => MockState;
  emitProcessing: (event: ProcessingEvent) => void;
  simulateFileDrop: (payload: Partial<FileDropPayload>) => void;
  simulateSettingsChanged: () => void;
  simulateDevicesChanged: (devices: DeviceInfo[]) => void;
}

declare global {
  interface Window {
    __marginsMock?: MarginsMockWindow;
  }
}

const fileDropListeners = new Set<(payload: FileDropPayload) => void>();
const processingListeners = new Set<(event: ProcessingEvent) => void>();
const recallIndexingListeners = new Set<(event: RecallIndexingEvent) => void>();
const speechModelListeners = new Set<(event: SpeechModelProgressEvent) => void>();
const backchannelSuggestionListeners = new Set<(event: BackchannelSuggestionEvent) => void>();
const backchannelErrorListeners = new Set<(event: BackchannelSuggestionEvent) => void>();
const settingsChangedListeners = new Set<() => void>();
const devicesChangedListeners = new Set<(snapshot: DeviceSnapshot) => void>();
const captureDeviceListeners = new Set<(state: CaptureDeviceEvent) => void>();
const granolaImportListeners = new Set<(event: GranolaImportProgressEvent) => void>();
const liveTranscriptReadyListeners = new Set<(payload: { session_name: string }) => void>();
const liveTranscriptDegradedListeners = new Set<(payload: { session_name: string; reason: string }) => void>();
let speechModelCancelRequested = false;
const distillCancelRequested = new Set<string>();
let state: MockState | null = null;

// Optional gitignored snapshot of the real local desktop app, produced by
// `npm run snapshot:state`. When present, the `home-real` scenario renders the
// actual session list/settings so the harness reflects real data and layout.
// Absent (CI / fresh clone) → scenarios fall back to synthetic fixtures.
interface LocalSnapshot {
  settings?: Partial<Settings>;
  sessions?: SessionInfo[];
  samples?: Record<string, { note?: string; aligned?: string; memo?: string; grounding?: unknown }>;
}
let localSnapshot: LocalSnapshot | null = null;

interface RuntimeFixture extends LocalSnapshot {
  run_id?: string;
  fixture?: string;
  vault_path?: string;
  cleanup_paths?: string[];
  real_distill?: MockState["real_distill"];
  samples?: Record<string, { note?: string; aligned?: string; memo?: string; capture_context?: string; grounding?: unknown }>;
  seeded_notes?: Array<{ title?: string; path?: string }>;
  seeded_note_titles?: string[];
  processing_events?: ScriptedProcessingEvent[];
}
let runtimeFixture: RuntimeFixture | null = null;

type ScriptedProcessingEvent = ProcessingEvent & { delay_ms?: number };

async function loadLocalSnapshot(): Promise<void> {
  if (typeof fetch === "undefined") return;
  try {
    const res = await fetch("/test-harness/local-snapshot.json", { cache: "no-store" });
    if (!res.ok) return;
    const data = await res.json();
    if (data && Array.isArray(data.sessions)) localSnapshot = data as LocalSnapshot;
  } catch {
    // No snapshot present; synthetic fixtures remain the fallback.
  }
}

async function loadRuntimeFixture(): Promise<void> {
  if (typeof window === "undefined" || typeof fetch === "undefined") return;
  const params = new URLSearchParams(window.location.search);
  const runId = params.get("runId");
  if (!runId) return;
  try {
    const res = await fetch(`/test-harness/ux-loop/runtime/${encodeURIComponent(runId)}.json`, { cache: "no-store" });
    if (!res.ok) return;
    const data = await res.json();
    if (data && Array.isArray(data.sessions)) runtimeFixture = data as RuntimeFixture;
  } catch {
    // No runtime fixture present; synthetic scenarios remain the fallback.
  }
}

// Top-level await: the harness imports this module dynamically, so the snapshot
// is loaded before any mock command (and thus createState) runs.
await loadLocalSnapshot();
await loadRuntimeFixture();

function sessionSortDesc(a: SessionInfo, b: SessionInfo): number {
  const ak = a.frontmatter_created_sort ?? a.start_time ?? "";
  const bk = b.frontmatter_created_sort ?? b.start_time ?? "";
  if (ak < bk) return 1;
  if (ak > bk) return -1;
  return a.name.localeCompare(b.name);
}

export const MOCK_DISTILL_CANCELLED_SENTINEL = "__margins_distill_cancelled__";

export async function getSettings(): Promise<Settings> {
  return { ...ensureState().settings };
}

export async function updateSettings(settings: Settings): Promise<void> {
  ensureState().settings = { ...settings };
}

export async function registerProject(settings: Settings, project: ProjectSource): Promise<ProjectRegistrationResult> {
  const s = ensureState();
  const nextProject: ProjectSource = {
    ...project,
    inbox_folder: project.inbox_folder?.trim() || "meetings",
    people_folder: project.people_folder?.trim() || "people",
    readiness: project.readiness || "needs_setup",
  };
  const projects = [...(settings.projects || [])].filter(p => p.id !== nextProject.id);
  const nextSettings: Settings = {
    ...settings,
    projects: [...projects, nextProject],
    active_project_id: nextProject.id,
    vault_path: nextProject.path,
    inbox_folder: nextProject.inbox_folder,
    people_folder: nextProject.people_folder,
  };
  s.settings = nextSettings;
  return {
    settings: { ...nextSettings },
    validation: {
      exists: true,
      has_obsidian: nextProject.path.includes("Obsidian") && !nextProject.path.includes("plain"),
      has_recall_index: false,
      has_margins: true,
    },
  };
}

export async function updateProjectReadiness(id: string, readiness: "ready" | "needs_setup" | "updating" | "error"): Promise<Settings> {
  const s = ensureState();
  const projects = (s.settings.projects || []).map(project => project.id === id ? { ...project, readiness } : project);
  s.settings = { ...s.settings, projects };
  return { ...s.settings };
}

export async function getAiStatus(): Promise<AiStatus> {
  const s = ensureState();
  const authed = s.settings.ai_mode === "chatgpt" && s.scenario !== "first-run" && s.scenario !== "settings-audio" && s.scenario !== "home-no-ai";
  return {
    chatgpt_authenticated: authed,
    chatgpt_message: authed
      ? "ChatGPT login is connected."
      : "Sign in with ChatGPT Plus/Pro, or use an API key below.",
  };
}

export async function getIncludedAiStatus(): Promise<IncludedAiStatus> {
  // settings-ai shows the recommended first-run state where Included is not yet
  // set up, so the selected card surfaces its "Set up needed" chip and button.
  const ready = ensureState().scenario !== "settings-ai";
  return {
    included_ready: ready,
    message: ready
      ? "Included note-making is ready on this Mac."
      : "Included note-making will create a usage-limited key before your first note.",
  };
}

export async function prepareIncludedAi(): Promise<IncludedAiStatus> {
  return { included_ready: true, message: "Included note-making is ready on this Mac." };
}

export async function signInChatgpt(): Promise<AiStatus> {
  ensureState().settings.ai_mode = "chatgpt";
  return { chatgpt_authenticated: true, chatgpt_message: "ChatGPT login is connected." };
}

// --- AI readiness / resolution preview (mirror the Rust resolver's display
// output closely enough for settings scenarios to render the role summary). ---

function mockAiMode(settings: Settings): "included" | "chatgpt" | "api" {
  const fallback = settings.api_key?.trim() ? "api" : "included";
  switch (settings.ai_mode?.trim().toLowerCase()) {
    case "included": case "margins": case "hosted": return "included";
    case "chatgpt": case "codex": return "chatgpt";
    case "api": case "api_key": case "custom": return "api";
    default: return fallback;
  }
}

const MOCK_MODEL_LABELS: Record<string, string> = {
  "anthropic/claude-sonnet-4.6": "Claude Sonnet",
  "anthropic/claude-haiku-4.5": "Claude Haiku",
  "google/gemini-3.1-flash-lite": "Gemini Flash Lite",
  "gpt-5.5": "GPT-5.5",
};
function mockModelLabel(id: string): string {
  return MOCK_MODEL_LABELS[id] || id;
}

export async function getAiReadiness(): Promise<AiReadiness> {
  const s = ensureState();
  const includedReady = (await getIncludedAiStatus()).included_ready;
  const chatgptReady = (await getAiStatus()).chatgpt_authenticated;
  const apiReady = Boolean(s.settings.api_key?.trim());
  return {
    mode: mockAiMode(s.settings),
    modes: {
      included: {
        ready: includedReady,
        reason: includedReady
          ? "Ready to write notes — no account or key needed."
          : "A usage-limited key is created before your first note.",
      },
      chatgpt: {
        ready: chatgptReady,
        reason: chatgptReady
          ? "Connected to your ChatGPT subscription."
          : "Sign in with your ChatGPT Plus or Pro subscription.",
      },
      api: {
        ready: apiReady,
        reason: apiReady
          ? "API key saved on this Mac."
          : "Add a key from OpenAI or a compatible provider.",
      },
    },
  };
}

export async function previewAiResolution(settings: Settings): Promise<ResolutionPreview> {
  const mode = mockAiMode(settings);
  const overrideActive = settings.backchannel_same_as_distill === false
    && Boolean(settings.backchannel_model?.trim() || settings.backchannel_api_key?.trim());
  const overrideModel = settings.backchannel_model?.trim() || "";

  let providerLabel: string;
  let distillModel: string;
  let prepModel: string;
  if (mode === "included") {
    providerLabel = "Included";
    distillModel = "anthropic/claude-sonnet-4.6";
    prepModel = "anthropic/claude-haiku-4.5";
  } else if (mode === "chatgpt") {
    providerLabel = "ChatGPT subscription";
    distillModel = settings.chatgpt_model?.trim() || "gpt-5.5";
    prepModel = distillModel;
  } else {
    const base = settings.ai_base_url?.trim();
    providerLabel = base && !/api\.openai\.com/i.test(base) ? "Your provider" : "OpenAI";
    distillModel = settings.ai_model?.trim() || "gpt-5.5";
    prepModel = distillModel;
  }
  const cueModel = overrideActive && overrideModel ? overrideModel : prepModel;

  const role = (
    role: string,
    modelId: string,
    changed: boolean,
  ): PreviewRole => ({
    role,
    provider_label: providerLabel,
    model_id: modelId,
    model_label: mockModelLabel(modelId),
    changed_by_override: changed,
  });

  const roles: PreviewRole[] = [
    role("distill", distillModel, false),
    role("cue", cueModel, overrideActive),
    role("reprocess", cueModel, overrideActive),
  ];
  if (mode === "included") {
    // Indexing is pinned to the lite model and never follows the cue override.
    roles.push(role("indexing", "google/gemini-3.1-flash-lite", false));
  }
  return { mode, roles };
}

export async function validateVault(path: string): Promise<VaultValidation> {
  const exists = path.trim().length > 0 && !path.includes("missing");
  return {
    exists,
    has_obsidian: exists && path.includes("Obsidian") && !path.includes("plain"),
    has_recall_index: exists && path.includes("second-brain"),
    has_margins: exists,
  };
}

export async function ensureDefaultVault(path: string): Promise<VaultValidation> {
  // Browser harness: pretend the folder now exists.
  return {
    exists: true,
    has_obsidian: false,
    has_recall_index: path.includes("second-brain"),
    has_margins: true,
  };
}

export async function indexVault(path: string): Promise<VaultValidation> {
  await new Promise(resolve => setTimeout(resolve, 450));
  const exists = path.trim().length > 0 && !path.includes("missing");
  return {
    exists,
    has_obsidian: exists && path.includes("Obsidian") && !path.includes("plain"),
    has_recall_index: exists,
    has_margins: exists,
  };
}

export async function selectVaultFolder(defaultPath?: string | null): Promise<string | null> {
  const s = ensureState();
  return s.settings.vault_path || defaultPath || "~/Documents/margins";
}

export async function selectProjectSubfolder(_projectRoot: string): Promise<string | null> {
  // The harness has no native dialog; simulate picking a "meetings" subfolder.
  return "meetings";
}

export async function getCalendarEventSuggestion(): Promise<CalendarSuggestionResult> {
  return {
    schema_version: "margins.calendar-suggestion.v1",
    suggestion: {
      title: "Customer Call: Pilot Scope",
      start: new Date().toISOString(),
      end: new Date(Date.now() + 45 * 60_000).toISOString(),
      calendar_id: "primary",
      event_id: "mock-event",
      people: ["Marcus Webb", "Elena Ruiz"],
      filename: "2026-06-01-13-00-00-customer-call-pilot-scope",
    },
    freshness: {
      status: "fresh",
      stale: false,
    },
  };
}

export async function updateSessionPeople(name: string, people: string[]): Promise<string[]> {
  const s = ensureState();
  const cleaned = Array.from(new Set(people.map(p => p.trim()).filter(Boolean)));
  s.sessions = s.sessions.map(session => session.name === name ? { ...session, people: cleaned } : session);
  return cleaned;
}

export async function updateSessionTitle(name: string, title: string): Promise<{ title: string | null; vault_note_path: string | null }> {
  const s = ensureState();
  const cleaned = title.trim().slice(0, 160) || null;
  s.sessions = s.sessions.map(session => session.name === name ? { ...session, title: cleaned, frontmatter_title: cleaned || session.frontmatter_title } : session);
  const session = s.sessions.find(session => session.name === name);
  return { title: cleaned, vault_note_path: session?.vault_note_path || null };
}

export async function prepareSpeechModels(parakeetModelDir?: string | null): Promise<SpeechModelPrepResult> {
  speechModelCancelRequested = false;
  const parakeetReady = Boolean(parakeetModelDir?.trim());
  emitSpeechModelProgress({ stage: "prepare", message: "Preparing local transcription…", progress: 0.04 });
  await delay(120);
  for (const pct of [0.18, 0.34, 0.52, 0.7]) {
    if (speechModelCancelRequested) throw new Error("Download canceled.");
    emitSpeechModelProgress({ stage: "transcription", message: "Downloading local transcription…", progress: pct });
    await delay(120);
  }
  if (speechModelCancelRequested) throw new Error("Download canceled.");
  emitSpeechModelProgress({ stage: "diarization", message: "Downloading speaker labels…", progress: 0.86 });
  await delay(160);
  emitSpeechModelProgress({ stage: "complete", message: "Local transcription is ready.", progress: 1 });
  return {
    parakeet_ready: true,
    parakeet_message: parakeetReady
      ? "Local transcription is ready."
      : "Local transcription downloaded to this Mac.",
    parakeet_model_dir: parakeetModelDir || "~/Library/Application Support/margins/models/parakeet-tdt-0.6b-v3-int8",
    diarization_ready: true,
    diarization_message: "Speaker labeling is ready.",
  };
}

export async function cancelSpeechModelDownload(): Promise<void> {
  speechModelCancelRequested = true;
}

export async function clearSpeechModels(): Promise<string> {
  speechModelCancelRequested = true;
  const s = ensureState();
  s.settings.parakeet_model_dir = null;
  s.settings.rust_diarization_enabled = false;
  return "Downloaded speech models were cleared.";
}

export async function probeSpeechModels(customPath?: string): Promise<SpeechModelProbe> {
  const s = ensureState();
  const sc = s.scenario;
  // When a custom path is supplied (State C "Use this path"), only the known-good
  // location resolves — anything else reports missing so the notice returns to
  // State C with an inline error instead of silently succeeding.
  const KNOWN_GOOD_PATH = "/Users/me/models/parakeet-custom";
  if (customPath != null && customPath.trim() !== "") {
    const ok = customPath.trim() === KNOWN_GOOD_PATH;
    return {
      transcription: ok ? "ready" : "missing",
      transcription_path: ok ? customPath.trim() : null,
      diarization: "ready",
      download_bytes: null,
    };
  }
  // model-notice-missing: transcription not yet downloaded
  if (sc === "model-notice-missing" || sc === "recording-healthy" || sc === "recording-with-memo") {
    return {
      transcription: "missing",
      transcription_path: null,
      diarization: "missing",
      download_bytes: 464_000_000,
    };
  }
  // model-notice-speakers: transcription ready, diarization still missing
  if (sc === "model-notice-speakers") {
    return {
      transcription: "ready",
      transcription_path: s.settings.parakeet_model_dir || "~/Library/Application Support/margins/models/parakeet-tdt-0.6b-v3-int8",
      diarization: "missing",
      download_bytes: null,
    };
  }
  // model-notice-found: transcription found at a non-default/custom path
  if (sc === "model-notice-found") {
    return {
      transcription: "ready",
      transcription_path: "/Users/me/models/parakeet-custom",
      diarization: "ready",
      download_bytes: null,
    };
  }
  // default: both ready (for scenarios where models are already set up)
  return {
    transcription: s.settings.parakeet_model_dir ? "ready" : "missing",
    transcription_path: s.settings.parakeet_model_dir || null,
    diarization: s.settings.rust_diarization_enabled ? "ready" : "missing",
    download_bytes: s.settings.parakeet_model_dir ? null : 178_000_000,
  };
}

export async function listDevices(): Promise<DeviceInfo[]> {
  return [...ensureState().devices];
}

export async function testAudioInput(deviceUid?: string | null): Promise<AudioTestResult> {
  const s = ensureState();
  const device = deviceUid
    ? s.devices.find(d => d.uid === deviceUid)
    : s.devices.find(d => d.is_default) || s.devices[0];
  if (!device) throw new Error("No microphone found");
  const muted = s.scenario === "recording-muted-mic";
  const peak = muted ? 0 : 0.08 + Math.abs(Math.sin(Date.now() / 250)) * 0.22;
  return { device_name: device.name, peak, drop_count: 0, ok: peak >= 0.001 };
}

export async function testSystemAudioTap(): Promise<SystemAudioTestResult> {
  const scenario = ensureState().scenario;
  await delay(220);
  if (scenario === "recording-dead-tap" || scenario === "recording-dead-tap-with-memo" || scenario === "settings-audio-blocked") {
    return {
      peak: 0,
      drop_count: 0,
      silent_secs: 0,
      frame_count: 0,
      status: "blocked",
      message: "Enable Margins under System Audio Recording Only, then quit and reopen the app.",
      restart_recommended: true,
    };
  }
  return {
    peak: 0.04,
    drop_count: 0,
    silent_secs: 0,
    frame_count: 86_400,
    status: "ok",
    message: "Computer-audio tap is receiving signal.",
    restart_recommended: false,
  };
}

export async function openPrivacyPane(_pane: "microphone" | "system-audio"): Promise<void> {
  await delay(80);
}

export async function restartApp(): Promise<void> {
  await delay(80);
}

export async function ensureCliTools(): Promise<EnsureCliToolsResult> {
  await delay(120);
  return {
    installed: false,
    message: "Using command line tools: margins at /mock/bin/margins.",
  };
}

export async function listSessions(projectId?: string | null): Promise<SessionInfo[]> {
  const activeProjectId = ensureState().settings.active_project_id ?? null;
  return ensureState().sessions.map(s => ({ ...s, project_id: s.project_id ?? projectId ?? activeProjectId }));
}

export async function deleteSession(name: string): Promise<void> {
  const s = ensureState();
  s.sessions = s.sessions.filter(session => session.name !== name);
  delete s.aligned[name];
  delete s.notes[name];
}

// --- Audio import ----------------------------------------------------------

export async function onFileDrop(callback: (payload: FileDropPayload) => void): Promise<UnlistenFn> {
  fileDropListeners.add(callback);
  maybeAutoSimulateDrag();
  return () => fileDropListeners.delete(callback);
}

export async function importAudioFile(path: string, maxSpeakers?: number | null): Promise<string> {
  const s = ensureState();
  const stem = path.split(/[\\/]+/).filter(Boolean).pop()?.replace(/\.[^.]+$/, "") || "imported-audio";
  const slug = stem.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "imported-audio";
  const name = uniqueMockSessionName(s, slug);
  const speakerNote = maxSpeakers && maxSpeakers >= 2 ? `${maxSpeakers}-speaker cap` : "single voice";
  emitProcessing({ stage: "prepare", message: `Reading audio file (${speakerNote})…`, progress: 0.05 });
  await delay(160);
  emitProcessing({ stage: "transcribe", message: "Transcribing audio…", progress: 0.32 });
  await delay(200);
  emitProcessing({ stage: "synthesize", message: "Searching notes for matching language…", progress: 0.7 });
  await delay(200);
  for (const chunk of mockStreamingGroundedNoteChunks()) {
    emitProcessing({ stage: "note_stream", message: chunk, progress: 0.85 });
    await delay(60);
  }
  s.aligned[name] = mockAligned(name);
  s.notes[name] = mockGroundedNote(name);
  s.sessions = [
    { ...mockSession(name, "synthesized", new Date().toISOString(), 372, 0, `/mock-vault/inbox/${name}.md`), title: importTitleFromStemMock(stem), source: "session" },
    ...s.sessions.filter(session => session.name !== name),
  ];
  emitProcessing({ stage: "complete", message: "Import complete!", progress: 1 });
  return name;
}

export async function pickAudioFiles(): Promise<string[]> {
  // The browser harness cannot open a native file dialog; return a canned WAV.
  return ["/Users/me/Recordings/team-sync.wav"];
}

export async function surveyGranolaImport(_paths: string[], _projectId?: string | null): Promise<GranolaImportSurvey> {
  await delay(180);
  return {
    file_count: 1,
    meeting_count: 3,
    people: ["Elena Ruiz", "Marcus Webb", "Maya Lin", "Priya Nair"],
    organizations: ["Acme", "Rho"],
    ambiguous_people: [],
    suggested_notes_folder: "meetings",
    suggested_people_folder: "people",
    suggested_organizations_folder: "organizations",
    folder_candidates: ["inbox", "meetings", "people", "projects", "transcripts"],
    sample_titles: ["Pilot scope review", "Rho user interview", "Pricing follow-up"],
    warnings: [],
  };
}

export async function importGranolaFiles(_paths: string[], _options: GranolaImportOptions, _projectId?: string | null): Promise<GranolaImportResult> {
  await delay(220);
  const notePath = "/mock-vault/meetings/2026-06-24 Pilot scope review.md";
  return {
    imported_count: 3,
    note_paths: [notePath],
    people_created: 4,
    organizations_created: 2,
    warnings: [],
  };
}

export async function getGranolaImportStatus(): Promise<GranolaImportStatus> {
  const s = ensureState();
  return {
    authorized: s.granolaAuthorized,
    account: s.granolaAuthorized ? "owner@example.com" : null,
    accounts: s.granolaAuthorized ? ["owner@example.com"] : [],
    message: s.granolaAuthorized
      ? "Granola import is authorized on this machine."
      : s.granolaNeedsAttention
        ? "Granola authorization needs attention. Authorize again."
        : "Authorize Granola to import meetings into this project.",
    ...(s.granolaNeedsAttention
      ? { code: "granola_oauth_refresh_failed", stage: "refresh", reason: "invalid_grant" }
      : {}),
    retryable: false,
  };
}

export async function authorizeGranolaImport(): Promise<GranolaImportStatus> {
  const s = ensureState();
  // Simulates the browser OAuth round-trip.
  await delay(600);
  s.granolaAuthorized = true;
  s.granolaNeedsAttention = false;
  return getGranolaImportStatus();
}

export async function revokeGranolaImportAuthorization(account?: string | null): Promise<GranolaImportStatus> {
  const s = ensureState();
  const status = await getGranolaImportStatus();
  if (status.accounts.length > 1 && !account) {
    throw new Error('{"code":"granola_account_ambiguous","stage":"authorization","reason":"multiple_authorized_accounts","retryable":false}');
  }
  s.granolaAuthorized = false;
  s.granolaNeedsAttention = false;
  return getGranolaImportStatus();
}

export async function importGranolaMcp(_projectId?: string | null): Promise<GranolaRemoteImportResult> {
  const s = ensureState();
  if (!s.granolaAuthorized) throw new Error("Granola import is not authorized.");
  const emit = (event: GranolaImportProgressEvent) => granolaImportListeners.forEach(cb => cb(event));
  emit({ stage: "Fetching meetings from Granola", current: 0, total: 0 });
  await delay(300);
  const titles = ["Pilot scope review", "Rho user interview"];
  for (let i = 0; i < titles.length; i++) {
    emit({ stage: `Importing “${titles[i]}”`, current: i + 1, total: titles.length });
    await delay(250);
  }
  const notePaths = titles.map(title => `/mock-vault/meetings/2026-07-02 ${title}.md`);
  emit({ stage: "Updating search index", current: 0, total: 0 });
  await delay(300);
  return {
    imported_count: titles.length,
    note_paths: notePaths,
    people_created: 2,
    organizations_created: 1,
    warnings: [],
    transcripts_plan_gated: false,
  };
}

export async function onGranolaImportProgress(
  callback: (event: GranolaImportProgressEvent) => void,
): Promise<UnlistenFn> {
  granolaImportListeners.add(callback);
  return () => granolaImportListeners.delete(callback);
}

function importTitleFromStemMock(stem: string): string {
  const words = stem.replace(/[-_]+/g, " ").trim();
  return words ? words.replace(/\b\w/g, c => c.toUpperCase()) : "Imported audio";
}

// Native onDragDrop does not exist in the browser; for the drag scenarios we
// synthesize an `over` event once the app has registered its listener + the
// sidebar DOM exists.
function maybeAutoSimulateDrag() {
  if (typeof window === "undefined") return;
  const sc = ensureState().scenario;
  if (sc !== "import-drag-over" && sc !== "import-drag-invalid" && sc !== "import-configure") return;
  const dpr = window.devicePixelRatio || 1;
  const paths = sc === "import-drag-invalid"
    ? ["/Users/me/Documents/quarterly-report.pdf"]
    : ["/Users/me/Recordings/team-sync.wav"];
  // Position is physical px; main.ts divides by dpr. 140×320 CSS lands inside
  // the default ~312px-wide sidebar.
  const position = { x: 140 * dpr, y: 320 * dpr };
  if (sc === "import-configure") {
    // Drive the full drop so main.ts opens the configure card (the pending state
    // lives in main.ts, so we trigger it through the real drop path).
    window.setTimeout(() => emitFileDrop({ type: "drop", paths, position }), 250);
    return;
  }
  window.setTimeout(() => emitFileDrop({ type: "over", paths, position }), 250);
}

function emitFileDrop(payload: FileDropPayload) {
  for (const callback of fileDropListeners) callback(payload);
}

export async function startRecording(name: string, deviceUid?: string | null): Promise<string> {
  const s = ensureState();
  if (s.scenario === "home-startup-lag") await delay(1500);
  const actualName = uniqueMockSessionName(s, name);
  s.recordingName = actualName;
  s.recordingStartedAt = Date.now();
  s.recordingPaused = false;
  s.memo = [];

  const deviceName =
    s.devices.find(d => d.uid === deviceUid)?.name ??
    s.devices.find(d => d.is_default)?.name ??
    null;
  if (deviceUid) {
    s.settings.input_device_mode = "pinned";
    s.settings.input_device_uid = deviceUid;
    s.settings.input_device_name = deviceName;
  } else {
    s.settings.input_device_mode = "follow_default";
    s.settings.input_device_uid = null;
    s.settings.input_device_name = null;
  }
  s.captureDeviceState = deviceName ? { state: "active", device_name: deviceName } : null;
  s.sessions = [
    mockSession(actualName, "recording", new Date().toISOString(), 0, 0),
    ...s.sessions.filter(session => session.name !== actualName),
  ];
  return actualName;
}

export async function cancelRecordingStartup(): Promise<boolean> {
  return true;
}

function uniqueMockSessionName(s: MockState, requested: string): string {
  const base = requested.replace(/^-+|-+$/g, "") || "meeting";
  const existing = new Set(s.sessions.map(session => session.name));
  if (!existing.has(base)) return base;
  for (let ordinal = 2; ordinal < 1000; ordinal++) {
    const suffix = `-${ordinal}`;
    const stem = base.slice(0, Math.max(1, 90 - suffix.length)).replace(/-+$/g, "") || "meeting";
    const candidate = `${stem}${suffix}`;
    if (!existing.has(candidate)) return candidate;
  }
  return `${base.slice(0, 78).replace(/-+$/g, "")}-${Date.now()}`;
}

export async function switchRecordingDevice(deviceUid?: string | null): Promise<RecordingStatus> {
  const s = ensureState();
  const deviceName = s.devices.find(d => d.uid === deviceUid)?.name ?? null;
  s.settings.input_device_mode = deviceUid ? "pinned" : "follow_default";
  s.settings.input_device_uid = deviceUid ?? null;
  s.settings.input_device_name = deviceName;
  if (s.recordingName) {
    emitCaptureDeviceChanged(deviceName
      ? { state: "active", device_name: deviceName }
      : { state: "active", device_name: s.devices.find(d => d.is_default)?.name ?? "System Default" });
    s.recordingPaused = false;
  }
  return getRecordingStatus();
}

export async function restartSystemAudioCapture(): Promise<RecordingStatus> {
  const s = ensureState();
  if (s.scenario === "recording-system-audio-recovery-failed") {
    throw new Error("Mock system-audio tap restart failed.");
  }
  s.scenario = "recording-system-audio-recovered";
  s.recordingStartedAt = Date.now() - 4_000;
  dispatchStateChanged();
  return getRecordingStatus();
}

export async function pauseRecording(): Promise<RecordingStatus> {
  const s = ensureState();
  s.recordingPaused = true;
  dispatchStateChanged();
  return getRecordingStatus();
}

export async function resumeRecording(): Promise<RecordingStatus> {
  const s = ensureState();
  s.recordingPaused = false;
  dispatchStateChanged();
  return getRecordingStatus();
}

export async function setLiveTranscriptionMode(mode: "stereo_split" | "mic_diarized"): Promise<RecordingStatus> {
  const s = ensureState();
  s.liveTranscriptionMode = mode;
  dispatchStateChanged();
  return getRecordingStatus();
}

export async function stopRecording(): Promise<string> {
  const s = ensureState();
  const name = s.recordingName ?? "mock-session";
  const duration = s.recordingStartedAt ? (Date.now() - s.recordingStartedAt) / 1000 : 95;

  s.recordingName = null;
  s.recordingStartedAt = null;
  s.recordingPaused = false;
  s.sessions = s.sessions.map(session =>
    session.name === name
      ? { ...session, status: "unprocessed", duration_secs: duration, memo_line_count: s.memo.length }
      : session,
  );
  s.aligned[name] = mockAligned(name);
  return name;
}

export async function discardRecording(): Promise<string> {
  const s = ensureState();
  const name = s.recordingName ?? "mock-session";
  s.recordingName = null;
  s.recordingStartedAt = null;
  s.recordingPaused = false;
  s.memo = [];
  s.sessions = s.sessions.filter(session => session.name !== name);
  delete s.aligned[name];
  delete s.notes[name];
  dispatchStateChanged();
  return name;
}

export async function getRecordingStatus(): Promise<RecordingStatus> {
  const s = ensureState();
  if (!s.recordingName) {
    return {
      is_recording: false,
      paused: false,
      session_name: null,
      elapsed_secs: 0,
      input_device_name: s.settings.input_device_name ?? null,
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
      live_transcription_mode: s.liveTranscriptionMode,
      capture_phase: "idle",
    };
  }

  const elapsed = s.recordingStartedAt ? (Date.now() - s.recordingStartedAt) / 1000 : 0;
  const mic = s.scenario === "recording-muted-mic"
    ? 0.00002
    : s.captureDeviceState?.state === "holding"
      ? 0
    : 0.05 + Math.abs(Math.sin(Date.now() / 350)) * 0.22;
  const waitingForAudio = s.scenario === "recording-no-system-audio-yet";
  const noAudioDismissed = s.scenario === "recording-no-system-audio-dismissed";
  const speakerDead = s.scenario === "recording-dead-tap"
    || s.scenario === "recording-dead-tap-with-memo"
    || s.scenario === "recording-system-audio-lost-recovering"
    || s.scenario === "recording-system-audio-recovery-failed";
  const recovered = s.scenario === "recording-system-audio-recovered";
  const spk = speakerDead || waitingForAudio || noAudioDismissed ? 0 : 0.025 + Math.abs(Math.cos(Date.now() / 500)) * 0.18;
  const speakerSilence = speakerDead || waitingForAudio || noAudioDismissed ? elapsed : 0;
  const systemAudioSeen = speakerDead || recovered;
  const tapStatus: RecordingStatus["tap_status"] = waitingForAudio
    ? (elapsed > 5 ? "blocked" : "connecting")
    : speakerDead
      ? (speakerSilence > 15 ? "dead" : "silent")
      : "ok";

  return {
    is_recording: true,
    paused: s.recordingPaused,
    session_name: s.recordingName,
    elapsed_secs: elapsed,
    input_device_name: s.settings.input_device_name ?? null,
    capture_device: s.captureDeviceState ?? { state: "active", device_name: "System Default" },
    mic_level: s.recordingPaused ? 0 : mic,
    mic_audio_frame_count: s.recordingPaused || s.captureDeviceState?.state === "holding"
      ? 0
      : Math.max(1, Math.round(elapsed * 48_000)),
    spk_level: s.recordingPaused ? 0 : spk,
    mic_drop_count: 0,
    spk_drop_count: 0,
    mic_gap_ms_total: s.captureDeviceState?.state === "holding" ? Math.round(elapsed * 1000) : 0,
    mic_switch_count: s.scenario.includes("device") ? 1 : 0,
    timeline_reusable: true,
    speaker_silence_secs: speakerSilence,
    system_audio_expected: s.liveTranscriptionMode === "stereo_split",
    system_audio_frame_count: waitingForAudio ? 0 : Math.round(elapsed * 48_000),
    system_audio_observed: systemAudioSeen,
    system_audio_seen: systemAudioSeen,
    tap_status: tapStatus,
    tap_warning: waitingForAudio
      ? "Computer audio is not reaching Margins; open Audio setup to restore permission."
      : speakerDead
        ? "Computer audio dropped after it was working. Mic is still recording; try restarting computer audio capture."
        : null,
    live_transcription_mode: s.liveTranscriptionMode,
    capture_phase: s.recordingPaused ? "paused" : "recording",
  };
}

function requireMemoSession(sessionName: string) {
  const current = ensureState().recordingName;
  if (!current || current !== sessionName) {
    throw new Error(`Stale memo mutation for ${sessionName}; active capture is ${current || "none"}`);
  }
}

export async function syncMemo(lines: MemoLine[], sessionName: string): Promise<void> {
  requireMemoSession(sessionName);
  ensureState().memo = lines.map(line => ({ ...line }));
}

export async function checkpointMemoLine(lines: MemoLine[], _committedIndex: number, sessionName: string): Promise<void> {
  requireMemoSession(sessionName);
  ensureState().memo = lines.map(line => ({ ...line }));
}

export async function requestBackchannelForMemo(lines: MemoLine[], committedIndex: number, sessionName: string): Promise<void> {
  requireMemoSession(sessionName);
  const s = ensureState();
  s.memo = lines.map(line => ({ ...line }));
  const memo = s.memo[committedIndex];
  if (!memo) return;
  await delay(350);
  const memoTime = formatMockElapsed(memo.created_secs);
  // Fresh-vault self-heal: the vault has never been indexed, so the backend held
  // the cue back and kicked off a local init. The UI must show an honest
  // "getting set up" line, not silence.
  if (s.scenario === "backchannel-warming") {
    emitBackchannelSuggestion({
      session_name: s.recordingName || "customer-call",
      memo_index: committedIndex,
      memo_time: memoTime,
      status: "warming",
      state: "warming",
      kind: "other",
      direction: null,
      title: null,
      why: "Margins is still getting your notes ready, so it held this cue back rather than guess.",
      suggestion: null,
      confidence: "low",
      safety: "Your mark is saved.",
      raw_json: null,
    });
    return;
  }
  // Self-heal exhausted: a prior enzyme init failed, so live cues can't be set
  // up. Honest terminal state, no looping "getting set up".
  if (s.scenario === "backchannel-unavailable") {
    emitBackchannelSuggestion({
      session_name: s.recordingName || "customer-call",
      memo_index: committedIndex,
      memo_time: memoTime,
      status: "unavailable",
      state: "cue_unavailable",
      kind: "other",
      direction: null,
      title: null,
      why: "Margins couldn't finish getting set up, so live cues are off for now.",
      suggestion: null,
      confidence: "low",
      safety: "Your mark is saved.",
      raw_json: null,
    });
    return;
  }
  // Genuine quiet: the vault is ready but nothing earned pulling the user out of
  // the room. Still acknowledge the mark landed rather than vanish.
  if (s.scenario === "backchannel-quiet") {
    emitBackchannelSuggestion({
      session_name: s.recordingName || "customer-call",
      memo_index: committedIndex,
      memo_time: memoTime,
      status: "quiet",
      state: "quiet",
      kind: "other",
      direction: null,
      title: null,
      why: "Nothing in this moment earned pulling your eyes out of the room.",
      suggestion: null,
      confidence: "low",
      safety: "Your mark is saved.",
      raw_json: null,
    });
    return;
  }
  emitBackchannelSuggestion({
    session_name: s.recordingName || "customer-call",
    memo_index: committedIndex,
    memo_time: memoTime,
    status: "ready",
    state: "suggestion_available",
    kind: committedIndex % 2 === 0 ? "proof_debt" : "ask_specifics",
    direction: committedIndex % 2 === 0 ? "proof over positioning" : "specifics over story",
    title: committedIndex % 2 === 0 ? "proof over positioning" : "specifics over story",
    why: committedIndex % 2 === 0
      ? "The memo is drifting toward personal-tool language; vault context suggests a stronger infrastructure frame."
      : "The memo flags a broad pain; ask for the last real instance before prescribing a solution.",
    suggestion: committedIndex % 2 === 0
      ? "Should we frame this as the retrieval layer they do not want to rebuild, rather than another place to take notes?"
      : "What was the last call where the context vanished — and what decision got harder afterward?",
    confidence: "high",
    safety: "optional; do not interrupt",
    raw_json: null,
  });
}

const mockSteerHistory: Record<number, string[]> = {};
const processingHistoryBySession: Record<string, ProcessingEvent[]> = {};

export async function steerBackchannelForMemo(
  memoIndex: number,
  steering: string,
  _previousSuggestion: string | null,
  sessionName: string,
): Promise<void> {
  requireMemoSession(sessionName);
  const s = ensureState();
  const memo = s.memo[memoIndex];
  const history = (mockSteerHistory[memoIndex] = mockSteerHistory[memoIndex] || []);
  history.push(steering);
  await delay(450);
  emitBackchannelSuggestion({
    session_name: s.recordingName || "customer-call",
    memo_index: memoIndex,
    memo_time: memo ? formatMockElapsed(memo.created_secs) : "",
    status: "ready",
    state: "suggestion_available",
    kind: "other",
    direction: `steer ${history.length}: ${steering.slice(0, 28)}`,
    title: `steer ${history.length}: ${steering.slice(0, 28)}`,
    why: `Cumulative steers: ${history.join(" → ")}`,
    suggestion: `Given all that, what would make “${steering}” concrete enough to act on this week?`,
    confidence: "high",
    safety: "optional; do not interrupt",
    raw_json: null,
  });
}

export async function processSession(name: string, _overwriteExistingNote = false, maxSpeakers: number | null = null, forceTranscribe = false): Promise<void> {
  const s = ensureState();
  const error = s.scenario === "distill-error-pi-login";
  processingHistoryBySession[name] = [];
  distillCancelRequested.delete(name);
  const cancelled = () => distillCancelRequested.has(name);
  const bailIfCancelled = () => {
    if (cancelled()) {
      distillCancelRequested.delete(name);
      throw new Error(MOCK_DISTILL_CANCELLED_SENTINEL);
    }
  };

  const realReplay = runtimeFixture?.processing_events;
  if (s.scenario === "ux-runtime-vault" && realReplay?.length) {
    for (const step of realReplay) {
      bailIfCancelled();
      emitProcessingForSession(name, step);
      await delay(step.delay_ms ?? replayDelayFor(step));
    }
    finishMockNote(s, name, _overwriteExistingNote);
    if (!realReplay.some(event => event.stage === "complete")) {
      emitProcessingForSession(name, { stage: "complete", message: "Processing complete!", progress: 1 });
    }
    return;
  }

  for (const step of scriptedProcessingEvents(s, name, maxSpeakers, forceTranscribe)) {
    bailIfCancelled();
    emitProcessingForSession(name, step);
    await delay(step.delay_ms ?? 260);
  }
  bailIfCancelled();

  if (error) {
    throw new Error("AI model setup required. Sign in with ChatGPT or add an API key in Settings.");
  }

  for (const chunk of mockStreamingGroundedNoteChunks()) {
    bailIfCancelled();
    emitProcessingForSession(name, { stage: "note_stream", message: chunk, progress: 0.8 });
    await delay(110);
  }
  bailIfCancelled();

  for (const step of scriptedPostNoteEvents(s, name)) {
    bailIfCancelled();
    emitProcessingForSession(name, step);
    await delay(step.delay_ms ?? 220);
  }
  bailIfCancelled();
  finishMockNote(s, name, _overwriteExistingNote);
  emitProcessingForSession(name, { stage: "complete", message: "Processing complete!", progress: 1 });
}

export async function retrySession(name: string): Promise<void> {
  return processSession(name, false, null, false);
}

function finishMockNote(s: MockState, name: string, overwriteExistingNote = false) {
  s.aligned[name] = s.aligned[name] ?? mockAligned(name);
  s.notes[name] = runtimeFixture?.samples?.[name]?.note
    || (s.scenario === "grounded-note-unused-memo" ? mockGroundedNote(name, { includePricing: false }) : mockGroundedNote(name));
  const existingPath = s.sessions.find(session => session.name === name)?.vault_note_path || null;
  const savedPath = overwriteExistingNote && existingPath ? existingPath : mockVaultNotePath(s, name);
  s.sessions = s.sessions.map(session =>
    session.name === name
      ? {
          ...session,
          status: "synthesized",
          vault_note_path: savedPath,
          failure_message: null,
          processing_state: "done",
          failed_stage: null,
        }
      : session,
  );
}

export async function saveDraftNote(name: string, overwriteExistingNote = false): Promise<string> {
  const s = ensureState();
  const notePath = mockVaultNotePath(s, name);
  const existingPath = s.sessions.find(session => session.name === name)?.vault_note_path || null;
  const savedPath = overwriteExistingNote && existingPath ? existingPath : notePath;
  s.sessions = s.sessions.map(session =>
    session.name === name
      ? { ...session, status: "synthesized", vault_note_path: savedPath }
      : session,
  );
  return savedPath;
}

export async function discardNote(name: string): Promise<void> {
  return deleteSession(name);
}

function mockVaultNotePath(s: MockState, name: string): string {
  const root = s.settings.vault_path?.trim() || "/mock-vault";
  const inbox = s.settings.inbox_folder?.trim() || "meetings";
  return `${root.replace(/\/+$/, "")}/${inbox.replace(/^\/+|\/+$/g, "")}/${name}.md`;
}

function scriptedProcessingEvents(
  _s: MockState,
  _name: string,
  maxSpeakers: number | null,
  forceTranscribe: boolean,
): ScriptedProcessingEvent[] {
  return [
    { stage: "prepare", message: "Loaded memo, session metadata, and aligned evidence", progress: 0.05, delay_ms: 260 },
    { stage: "transcribe", message: forceTranscribe ? `Refreshing transcript before writing (${maxSpeakers ? `${maxSpeakers} speakers` : "auto speaker count"})` : "Using stored transcript from the capture", progress: 0.18, delay_ms: 260 },
    transcriptEvent(0, 0, 4200, "We should keep the pilot narrow enough to learn quickly."),
    transcriptEvent(1, 5100, 9200, "The weekly status ritual is the thing I want to replace."),
    { stage: "align", message: "Aligning marks with transcript turns...", progress: 0.52, delay_ms: 340 },
    { stage: "synthesize", message: "Pi turn 1: reading evidence and planning the next tool calls", progress: 0.62, delay_ms: 520 },
    { stage: "synthesize", message: "Tool start: scanning your notes — checking your notes for related ideas", progress: null, delay_ms: 460 },
    { stage: "synthesize", message: "Tool done: scanning your notes — mapped your notes' ideas, ready to search", progress: null, delay_ms: 320 },
    { stage: "synthesize", message: "Tool start: finding related ideas: weekly status ritual pilot scope — search query: weekly status ritual pilot scope decision memory", progress: null, delay_ms: 520 },
    { stage: "synthesize", message: "Tool done: finding related ideas: weekly status ritual pilot scope — connected to Operating cadence, Pilot design, Decision memory", progress: 0.78, delay_ms: 420 },
    { stage: "synthesize", message: "Tool start: read: Operating cadence.md — reading Operating cadence.md", progress: null, delay_ms: 340 },
    { stage: "synthesize", message: "Tool done: read: Operating cadence.md — returned 42 lines with status ritual and coordination debt anchors", progress: null, delay_ms: 300 },
  ];
}

function scriptedPostNoteEvents(s: MockState, name: string): ScriptedProcessingEvent[] {
  return [
    { stage: "synthesize", message: `Draft ready for review — ${mockVaultNotePath(s, name)}`, progress: 0.9, delay_ms: 220 },
    { stage: "cleanup", message: "Keeping audio for 7 days per settings...", progress: 0.92, delay_ms: 180 },
  ];
}

function replayDelayFor(event: ProcessingEvent): number {
  if (event.stage === "note_stream") return 40;
  if (event.stage === "transcript") return 80;
  if (/Tool start|Tool done/i.test(event.message)) return 300;
  return 240;
}

export async function cancelProcessSession(name: string): Promise<void> {
  distillCancelRequested.add(name);
}

export async function refineSession(_name: string, _message: string): Promise<void> {
  // The harness has no live Pi conversation to resume; refine is a no-op so the
  // UI flow (textbox → chat history) can still be exercised against mocks.
  await new Promise(resolve => setTimeout(resolve, 400));
}

export async function reprocessSessionWithPeople(name: string, people: string[], _projectId?: string | null): Promise<void> {
  const s = ensureState();
  distillCancelRequested.delete(name);
  const cancelled = () => distillCancelRequested.has(name);
  const bailIfCancelled = () => {
    if (cancelled()) {
      distillCancelRequested.delete(name);
      throw new Error(MOCK_DISTILL_CANCELLED_SENTINEL);
    }
  };

  // Setup events signalling the reprocess is starting.
  const firstNames = people.map(p => p.trim().replace(/^\[\[/, "").replace(/\]\]$/, "").split(/\s+/)[0] || p.trim());
  const label = firstNames.join(", ");
  emitProcessingForSession(name, { stage: "reprocess", message: `Reprocessing with ${label}…`, progress: 0.05 });
  await delay(200);
  emitProcessingForSession(name, { stage: "reprocess", message: "Aligning marks and transcript with updated attendees…", progress: 0.2 });
  await delay(180);

  // Stream the rewritten note paragraph-by-paragraph so the UI shows the
  // paragraph-swap animation. Content explicitly attributes remarks to the
  // named people so screenshots demonstrate the feature clearly.
  const rewrittenNote = mockReprocessedNote(name, people);
  const paragraphs = rewrittenNote.split(/\n\n+/);
  let progress = 0.35;
  for (const para of paragraphs) {
    bailIfCancelled();
    const chunk = para.trim() + "\n\n";
    emitProcessingForSession(name, { stage: "note_stream", message: chunk, progress });
    progress = Math.min(0.92, progress + 0.08);
    await delay(180);
  }
  bailIfCancelled();

  finishMockNote(s, name, true);
  // Overwrite with the reprocessed note so getVaultNote returns the updated copy.
  s.notes[name] = rewrittenNote;
  emitProcessingForSession(name, { stage: "complete", message: "Reprocess complete!", progress: 1 });
}

function mockReprocessedNote(name: string, people: string[]): string {
  const firstNames = people.map(p => p.trim().replace(/^\[\[/, "").replace(/\]\]$/, "").split(/\s+/)[0] || p.trim());
  const nameList = firstNames.join(", ");
  const [first, second] = firstNames;
  const speakerA = first || "the first speaker";
  const speakerB = second || "the second speaker";
  return `# ${name === "customer-call" ? "Customer call: Margins pilot scope" : name} — reprocessed with ${nameList}\n\n` +
    `<!--MARGINS:USE {"section_id":"weekly-status","memo_ids":["m002"],"mode":"absorbed","disposition":"folded_into_section","transcript_refs":[{"start_secs":64,"end_secs":79,"quote":"The weekly status ritual is the thing I want to replace."}],"vault_refs":["Operating cadence.md"]}-->\n` +
    `## Weekly status ritual\n${speakerA} is trying to replace a heavy weekly status ritual with a lighter decision-memory loop. ${speakerB} noted the key question is whether the note can preserve the decision surface, not just summarize the call.\n\n` +
    `<!--MARGINS:USE {"section_id":"pilot-scope","memo_ids":["m001"],"mode":"accounted_for","disposition":"folded_into_section","transcript_refs":[{"start_secs":13,"end_secs":21,"quote":"keep the pilot narrow enough to learn quickly"}],"vault_refs":["Pilot design.md"]}-->\n` +
    `## Pilot scope\n${speakerA} emphasized keeping the pilot narrow enough to learn quickly. ${speakerB} agreed, asking for a concrete example of the current weekly status artifact before committing to a broader scope.\n\n` +
    `<!--MARGINS:USE {"section_id":"follow-ups","memo_ids":["m003"],"mode":"accounted_for","disposition":"folded_into_action_item","transcript_refs":[],"vault_refs":[]}-->\n` +
    `## Follow-ups\n- [ ] ${speakerA} to send a one-page pilot proposal.\n- [x] ${speakerB} to share an example of their current weekly status artifact.`;
}

export async function getAlignedContent(name: string): Promise<string> {
  const s = ensureState();
  const content = s.aligned[name] ?? mockAligned(name);
  if (!content) throw new Error("No aligned content");
  return content;
}

export async function getSessionMemo(_name: string): Promise<string> {
  const s = ensureState();
  if (!s.memo.length) throw new Error("No memo marks");
  return s.memo
    .map(line => {
      const edited = line.edited_secs !== null ? ` ~${formatMockElapsed(line.edited_secs)}` : "";
      return `[${formatMockElapsed(line.created_secs)}${edited}] ${line.text}`;
    })
    .join("\n");
}

export async function getVaultNote(name: string): Promise<string> {
  const content = ensureState().notes[name];
  if (!content) throw new Error("No vault note");
  return content;
}

export async function getSessionGrounding(name: string): Promise<unknown | null> {
  const s = ensureState();
  const runtimeGrounding = runtimeFixture?.samples?.[name]?.grounding;
  if (runtimeGrounding) return runtimeGrounding;
  const localGrounding = localSnapshot?.samples?.[name]?.grounding;
  if (localGrounding) return localGrounding;
  const note = s.notes[name];
  if (!note || !/MARGINS:USE/i.test(note)) return null;
  const uses = [...note.matchAll(/<!--\s*MARGINS:USE\s+([\s\S]*?)\s*-->/g)]
    .map(match => {
      try { return JSON.parse(match[1]); } catch { return null; }
    })
    .filter(Boolean);
  return uses.length ? { uses } : null;
}

export async function getDistillTrace(name: string): Promise<ProcessingEvent[]> {
  if (processingHistoryBySession[name]?.length) return withRuntimeVaultTrace(processingHistoryBySession[name]);
  if (ensureState().scenario === "note-streaming-grounded") {
    return [
      { stage: "prepare", message: "Loaded memo, session metadata, and aligned evidence", progress: 0.05 },
      { stage: "synthesize", message: "Tool done: finding related ideas: weekly status ritual — connected to Operating cadence, Pilot design", progress: null },
      ...mockStreamingGroundedNoteChunks().slice(0, 13).map(message => ({ stage: "note_stream", message, progress: 0.8 })),
    ];
  }
  return [
    { stage: "prepare", message: "Loaded memo, session metadata, and aligned evidence", progress: 0.05 },
    { stage: "synthesize", message: "Pi turn 1: reading evidence and planning the next tool calls", progress: null },
    { stage: "synthesize", message: "Tool start: scanning your notes — checking your notes for related ideas", progress: null },
    { stage: "synthesize", message: "Tool done: scanning your notes — mapped your notes' ideas, ready to search", progress: null },
    { stage: "synthesize", message: "Tool start: finding related ideas: weekly status ritual pilot scope — search query: weekly status ritual pilot scope decision memory", progress: null },
    { stage: "synthesize", message: "Tool done: finding related ideas: weekly status ritual pilot scope — connected to Operating cadence, Pilot design, Decision memory", progress: null },
    { stage: "synthesize", message: "Tool start: read: Operating cadence.md — reading Operating cadence.md", progress: null },
    { stage: "synthesize", message: "Tool done: read: Operating cadence.md — returned 42 lines with status ritual and coordination debt anchors", progress: null },
    { stage: "synthesize", message: "Tool done: save note — saved to inbox/customer-call.md", progress: null },
    { stage: "complete", message: "Processing complete!", progress: 1 },
  ];
}

function withRuntimeVaultTrace(events: ProcessingEvent[]): ProcessingEvent[] {
  const hasVaultBreadcrumb = events.some(event => /connected to|read:\s*.+\.md/i.test(event.message || ""));
  const noteTitles = runtimeFixture?.seeded_note_titles?.length
    ? runtimeFixture.seeded_note_titles
    : (runtimeFixture?.seeded_notes || []).map(note => note.title || note.path || "").filter(Boolean);
  if (hasVaultBreadcrumb || !noteTitles.length) return [...events];
  const titles = noteTitles.slice(0, 3).join(", ");
  return [
    ...events,
    { stage: "synthesize", message: `Tool done: finding related ideas — connected to ${titles}`, progress: null },
  ];
}

export async function openNote(name: string): Promise<void> {
  ensureState().opened.push(`editor:${name}`);
}

export async function openNoteInObsidian(name: string): Promise<void> {
  const s = ensureState();
  if (!s.settings.vault_path || s.settings.vault_path.includes("plain")) {
    throw new Error("Choose a folder with .obsidian before opening in Obsidian.");
  }
  s.opened.push(`obsidian:${name}`);
}

export async function openNoteTargetInObsidian(target: string): Promise<void> {
  const s = ensureState();
  if (!s.settings.vault_path || s.settings.vault_path.includes("plain")) {
    throw new Error("Choose a folder with .obsidian before opening in Obsidian.");
  }
  s.opened.push(`obsidian-target:${target}`);
}

export async function onProcessingProgress(
  callback: (event: ProcessingEvent) => void,
): Promise<UnlistenFn> {
  processingListeners.add(callback);
  return () => processingListeners.delete(callback);
}

export async function onRecallIndexing(
  callback: (event: RecallIndexingEvent) => void,
): Promise<UnlistenFn> {
  recallIndexingListeners.add(callback);
  return () => recallIndexingListeners.delete(callback);
}

export async function onSpeechModelProgress(
  callback: (event: SpeechModelProgressEvent) => void,
): Promise<UnlistenFn> {
  speechModelListeners.add(callback);
  return () => speechModelListeners.delete(callback);
}

export async function onBackchannelSuggestion(
  callback: (event: BackchannelSuggestionEvent) => void,
): Promise<UnlistenFn> {
  backchannelSuggestionListeners.add(callback);
  return () => backchannelSuggestionListeners.delete(callback);
}

export async function onBackchannelError(
  callback: (event: BackchannelSuggestionEvent) => void,
): Promise<UnlistenFn> {
  backchannelErrorListeners.add(callback);
  return () => backchannelErrorListeners.delete(callback);
}

export async function onGlobalCaptureToggle(_callback: () => void): Promise<UnlistenFn> {
  // Global OS shortcut cannot fire in the browser harness; no-op is expected and acceptable.
  return () => {};
}

export async function onCaptureDeviceChanged(callback: (state: CaptureDeviceEvent) => void): Promise<UnlistenFn> {
  captureDeviceListeners.add(callback);
  const s = ensureState();
  if (s.scenario === "recording-device-switched") {
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "switching", from: "Studio Display Microphone", to: "MacBook Pro Microphone" }), 120);
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "active", device_name: "MacBook Pro Microphone" }), 320);
  } else if (s.scenario === "recording-device-switching-slow") {
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "switching", from: "Studio Display Microphone", to: "MacBook Pro Microphone" }), 120);
  } else if (s.scenario === "recording-device-recovered") {
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "active", device_name: "System Default Microphone" }), 320);
  } else if (s.scenario === "recording-device-fallback" || s.scenario === "recording-device-fallback-settings") {
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "fallback", requested_uid: "mock-studio-display", device_name: "System Default Microphone" }), 120);
  } else if (s.scenario === "recording-device-switch-failed") {
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "switching", from: "Studio Display Microphone", to: "MacBook Pro Microphone" }), 120);
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "active", device_name: "Studio Display Microphone" }), 3_300);
    window.setTimeout(() => emitCaptureDeviceChanged({ state: "switch_failed", device_name: "Studio Display Microphone", error: "candidate confirmation timed out" }), 3_320);
  } else if (s.captureDeviceState) {
    window.setTimeout(() => callback(s.captureDeviceState as CaptureDeviceState), 120);
  }
  return () => captureDeviceListeners.delete(callback);
}

export async function onSettingsChanged(callback: () => void): Promise<UnlistenFn> {
  settingsChangedListeners.add(callback);
  return () => settingsChangedListeners.delete(callback);
}

export async function onDevicesChanged(
  callback: (snapshot: DeviceSnapshot) => void,
): Promise<UnlistenFn> {
  devicesChangedListeners.add(callback);
  return () => devicesChangedListeners.delete(callback);
}

export async function onLiveTranscriptReady(
  callback: (payload: { session_name: string }) => void,
): Promise<UnlistenFn> {
  liveTranscriptReadyListeners.add(callback);
  const s = ensureState();
  if (s.scenario === "recording-warming-then-ready") {
    // Fire ready after 3 s so the warming state is visible first.
    window.setTimeout(() => emitLiveTranscriptReady({ session_name: s.recordingName ?? "customer-call" }), 3_000);
  }
  return () => liveTranscriptReadyListeners.delete(callback);
}

export async function onLiveTranscriptDegraded(
  callback: (payload: { session_name: string; reason: string }) => void,
): Promise<UnlistenFn> {
  liveTranscriptDegradedListeners.add(callback);
  const s = ensureState();
  if (s.scenario === "recording-transcript-degraded") {
    window.setTimeout(() => emitLiveTranscriptDegraded({ session_name: s.recordingName ?? "customer-call", reason: "start_failed" }), 120);
  }
  return () => liveTranscriptDegradedListeners.delete(callback);
}

// ---------------------------------------------------------------------------
// Prep sketch hydration (no-op mock — Rust backend implements the real logic)
// ---------------------------------------------------------------------------

const prepHydrationListeners = new Set<(event: PrepHydrationEvent) => void>();

export async function hydratePrepSketch(req: HydratePrepRequest): Promise<void> {
  const { lines, sessionName, blockOrdinal, pulledTexts } = req;
  if (lines.length === 0) return;
  // Emit a mock hydrated event after a short delay
  await new Promise(r => window.setTimeout(r, 1_200));
  const allMarginalia: PrepHydrationEvent["marginalia"] = [
    {
      anchor: lines[0]?.text?.slice(0, 40) ?? null,
      kind: "carried",
      text: "Pricing thread with Sam left open — you parked per-seat pushback last time.",
      sources: ["meetings/2026-07-12-sam-check-in.md"],
    },
    {
      anchor: null,
      kind: "blind_spot",
      text: "No mention of the onboarding gap — two customers flagged it on 6/30.",
      sources: ["meetings/2026-06-30-customer-review.md"],
    },
  ];
  // Drop marginalia whose text appears in pulledTexts (near-match: exact or substring)
  const normalizeText = (t: string) => t.toLowerCase().replace(/\s+/g, " ").replace(/[.!?,;]+$/, "").trim();
  const normalizedPulled = pulledTexts.map(normalizeText);
  const filtered = allMarginalia.filter(m => {
    const norm = normalizeText(m.text);
    return !normalizedPulled.some(p =>
      p === norm
      || (p.length > 0 && norm.includes(p) && p.length / norm.length >= 0.6)
      || (norm.length > 0 && p.includes(norm) && norm.length / p.length >= 0.6),
    );
  });
  const event: PrepHydrationEvent = {
    session_name: sessionName,
    block_ordinal: blockOrdinal,
    state: filtered.length > 0 ? "hydrated" : "thin",
    posture: "Discovery call — understanding fit, not selling",
    marginalia: filtered,
    reason: null,
    hint: null,
    error: null,
  };
  for (const cb of prepHydrationListeners) cb(event);
}

export async function steerPrepHydration(_sessionName: string, _blockOrdinal: number, _instruction: string): Promise<void> {
  // no-op in mock
}

export async function onPrepHydration(
  callback: (event: PrepHydrationEvent) => void,
): Promise<UnlistenFn> {
  prepHydrationListeners.add(callback);
  return () => prepHydrationListeners.delete(callback);
}

function ensureState(): MockState {
  if (!state) state = createState(scenarioName());
  installWindowHook();
  return state;
}

function scenarioName(): MockScenario {
  if (typeof window === "undefined") return "home-empty";
  return (new URLSearchParams(window.location.search).get("scenario") || "home-empty") as MockScenario;
}

function installWindowHook() {
  if (typeof window === "undefined" || window.__marginsMock) return;
  window.__marginsMock = {
    getState: () => state,
    setScenario: (scenario: MockScenario) => {
      state = createState(scenario);
      dispatchStateChanged();
      return state;
    },
    patch: (patch: Partial<MockState>) => {
      state = { ...ensureState(), ...patch };
      dispatchStateChanged();
      return state;
    },
    emitProcessing,
    simulateFileDrop: (payload: Partial<FileDropPayload>) => {
      const dpr = window.devicePixelRatio || 1;
      emitFileDrop({
        type: payload.type ?? "over",
        paths: payload.paths ?? ["/Users/me/Recordings/team-sync.wav"],
        position: payload.position ?? { x: 140 * dpr, y: 320 * dpr },
      });
    },
    simulateSettingsChanged: () => {
      settingsChangedListeners.forEach(cb => cb());
    },
    simulateDevicesChanged: (devices: DeviceInfo[]) => {
      const s = ensureState();
      s.devices = devices;
      const snapshot: DeviceSnapshot = { devices, generation: Date.now() };
      devicesChangedListeners.forEach(cb => cb(snapshot));
    },
  };
}

function dispatchStateChanged() {
  window.dispatchEvent(new CustomEvent("margins:mock-state", { detail: state }));
}

function createState(scenario: MockScenario): MockState {
  const settings: Settings = scenario === "first-run" || scenario === "settings-audio" || scenario === "settings-audio-pinned" || scenario === "settings-audio-blocked" || scenario === "settings-audio-ready" || scenario === "settings-ai"
    ? {
        vault_path: "~/Documents/margins",
        ai_mode: "included",
        ai_base_url: null,
        ai_model: null,
        api_key: null,
        cleanup_policy: "immediate",
        input_device_mode: "follow_default",
        input_device_uid: null,
        input_device_name: null,
        audio_input_ready: false,
        system_audio_ready: false,
        editor_command: null,
        parakeet_model_dir: null,
        rust_diarization_enabled: false,
        inbox_folder: "meetings",
        people_folder: "people",
        created_date_format: "[[%Y-%m-%d]]",
        sidebar_date_format: "compact",
        note_filename_template: "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
        person_note_template: "# {{name}}\n",
        distill_instructions: "Create the final Markdown note with created date and attendee people links.",
        import_speaker_count: 1,
      }
    : {
        vault_path: "~/Obsidian/second-brain",
        projects: [
          { id: "second-brain", name: "Second Brain", path: "~/Obsidian/second-brain", inbox_folder: "meetings", people_folder: "people", readiness: "ready" },
          { id: "acme", name: "Acme Client", path: "~/Obsidian/acme", inbox_folder: "meetings", people_folder: "people", readiness: "updating" },
          { id: "personal", name: "Personal", path: "~/Obsidian/personal", inbox_folder: "journal", people_folder: "people", readiness: "needs_setup" },
        ],
        active_project_id: "second-brain",
        ai_mode: "api",
        ai_base_url: null,
        ai_model: null,
        api_key: "••••••••",
        cleanup_policy: "7days",
        input_device_mode: "pinned",
        input_device_uid: "mock-studio-display",
        input_device_name: "Studio Display Microphone",
        audio_input_ready: true,
        system_audio_ready: true,
        editor_command: "vscode",
        parakeet_model_dir: "~/Library/Application Support/margins/models/parakeet-tdt-0.6b-v3-int8",
        rust_diarization_enabled: true,
        auto_start_from_calendar: true,
        inbox_folder: "meetings",
        people_folder: "people",
        created_date_format: "[[%Y-%m-%d]]",
        sidebar_date_format: "compact",
        note_filename_template: "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}",
        person_note_template: "# {{name}}\n",
        distill_instructions: "Create the final Markdown note with created date and attendee people links.",
        import_speaker_count: 1,
      };
  // Capture-ready vault but note-making AI signed out: switch to ChatGPT mode
  // with no auth so aiReady() is false while the project path stays configured.
  if (scenario === "home-no-ai") {
    settings.ai_mode = "chatgpt";
    settings.api_key = null;
  }
  // settings-ai: recommended-default first-run state — Included mode, no key,
  // included NOT ready (getIncludedAiStatus reports not-ready for this scenario).
  if (scenario === "settings-ai") {
    settings.ai_mode = "included";
    settings.api_key = null;
  }
  // settings-ai-api: user API-key mode with a model-only live-cue override, so
  // the Live cues <details> renders open with the cue model filled in.
  if (scenario === "settings-ai-api") {
    settings.ai_mode = "api";
    settings.backchannel_same_as_distill = false;
    settings.backchannel_model = "google/gemini-3-flash-preview";
    settings.backchannel_api_key = null;
    settings.backchannel_base_url = null;
  }
  if (scenario === "settings-audio-pinned") {
    settings.input_device_mode = "pinned";
    settings.input_device_uid = "mock-yeti";
    settings.input_device_name = "Yeti Stereo Microphone";
  }
  if (scenario === "recording-device-switched") {
    settings.input_device_mode = "follow_default";
    settings.input_device_uid = null;
    settings.input_device_name = null;
  }
  const devices: DeviceInfo[] = [
    { uid: "mock-default", name: "System Default Microphone", is_default: true },
    { uid: "mock-studio-display", name: "Studio Display Microphone", is_default: false },
    { uid: "mock-yeti", name: "Yeti Stereo Microphone", is_default: false },
    { uid: "mock-blackhole", name: "BlackHole 2ch", is_default: false },
    { uid: "mock-macbook", name: "MacBook Pro Microphone", is_default: false },
  ];
  const now = new Date(Date.now() - 12 * 60 * 1000).toISOString();
  const sessions: SessionInfo[] = [];
  const aligned: Record<string, string> = {};
  const notes: Record<string, string> = {};
  let recordingName: string | null = null;
  let recordingStartedAt: number | null = null;
  let memo: MemoLine[] = [];
  let captureDeviceState: CaptureDeviceState | null = null;

  if (scenario.startsWith("recording") || scenario === "backchannel-warming" || scenario === "backchannel-quiet" || scenario === "backchannel-unavailable") {
    recordingName = scenario === "recording-dead-tap" || scenario === "recording-dead-tap-with-memo" ? "dead-tap-check" : "customer-call";
    recordingStartedAt = Date.now() - 88_000;
    memo = scenario === "recording-with-memo" || scenario === "recording-dead-tap-with-memo" || scenario === "recording-paused"
      ? mockMemoLines().slice(0, 2)
      : [];
    if (scenario === "recording-device-holding" || scenario === "recording-device-holding-narrow") {
      captureDeviceState = { state: "holding", last_good: "Studio Display Microphone", reason: "device_lost" };
    } else if (scenario === "recording-device-switching" || scenario === "recording-device-switching-slow") {
      captureDeviceState = { state: "switching", from: "Studio Display Microphone", to: "MacBook Pro Microphone" };
    } else if (scenario === "recording-device-switched") {
      captureDeviceState = { state: "active", device_name: "Studio Display Microphone" };
    } else if (scenario === "recording-device-fallback" || scenario === "recording-device-fallback-settings") {
      captureDeviceState = { state: "active", device_name: "System Default Microphone" };
    } else if (scenario === "recording-device-recovered") {
      captureDeviceState = { state: "holding", last_good: "Studio Display Microphone", reason: "device_lost" };
    } else if (scenario === "recording-device-switch-failed") {
      captureDeviceState = { state: "active", device_name: "Studio Display Microphone" };
    } else {
      captureDeviceState = { state: "active", device_name: settings.input_device_name || "Studio Display Microphone" };
    }
    sessions.push(mockSession(recordingName, "recording", now, 88, memo.length));
  } else if (scenario === "home-many-sessions") {
    const fixtures = mockManySessions();
    for (const f of fixtures) sessions.push(f);
  } else if (scenario === "home-real") {
    if (localSnapshot?.sessions?.length) {
      for (const s of localSnapshot.sessions) sessions.push({ ...s });
      sessions.sort(sessionSortDesc);
      for (const [name, sample] of Object.entries(localSnapshot.samples ?? {})) {
        if (sample.aligned) aligned[name] = sample.aligned;
        if (sample.note) notes[name] = sample.note;
      }
      if (localSnapshot.settings) Object.assign(settings, localSnapshot.settings);
    } else {
      // No real snapshot captured yet — fall back to the synthetic dense list so
      // the scenario still renders in CI.
      for (const f of mockManySessions()) sessions.push(f);
    }
  } else if (scenario === "ux-runtime-vault") {
    if (runtimeFixture?.sessions?.length) {
      for (const s of runtimeFixture.sessions) sessions.push({ ...s });
      sessions.sort(sessionSortDesc);
      for (const [name, sample] of Object.entries(runtimeFixture.samples ?? {})) {
        if (sample.aligned) aligned[name] = sample.aligned;
        if (sample.memo) memo = parseRuntimeMemo(sample.memo);
        if (sample.note && sessions.find(session => session.name === name)?.status === "synthesized") {
          notes[name] = sample.note;
        }
      }
      if (runtimeFixture.settings) Object.assign(settings, runtimeFixture.settings);
      if (runtimeFixture.vault_path) {
        settings.vault_path = runtimeFixture.vault_path;
      }
    } else {
      memo = mockMemoLines();
      sessions.push(mockSession("customer-call", "unprocessed", now, 713, memo.length));
      aligned["customer-call"] = mockAligned("customer-call");
    }
  } else if (scenario === "home-capture-notes") {
    const note = mockSession(
      "note-customer-call-pilot-scope-a1b2c3d4",
      "synthesized",
      now,
      0,
      0,
      "/Users/me/Obsidian/second-brain/inbox/customer-call-pilot-scope.md",
    );
    note.source = "capture_note";
    note.title = "Customer Call: Pilot Scope";
    note.people = [];
    note.calendar_event_title = null;
    sessions.push(note);
    notes[note.name] = mockNote("Customer Call: Pilot Scope");
  } else if (scenario === "backchannel-built") {
    memo = mockMemoLines();
    sessions.push(mockSession("customer-call", "unprocessed", now, 713, memo.length));
    aligned["customer-call"] = mockAligned("customer-call");
  } else if (scenario === "reprocess-with-people") {
    memo = mockMemoLines();
    sessions.push({
      ...mockSession(
        "customer-call",
        "synthesized",
        now,
        713,
        memo.length,
        "/Users/me/Obsidian/second-brain/inbox/customer-call.md",
      ),
      frontmatter_title: "Customer call: Pilot scope review",
      frontmatter_people: ["Marcus Webb", "Elena Ruiz"],
      people: ["Marcus Webb", "Elena Ruiz"],
    });
    aligned["customer-call"] = mockAligned("customer-call");
    notes["customer-call"] = mockGroundedNote("customer-call");
  } else if (scenario === "distill-complete" || scenario === "first-note-payoff" || scenario === "grounded-note-complete" || scenario === "grounded-note-unused-memo") {
    memo = mockMemoLines();
    sessions.push({
      ...mockSession(
        "customer-call",
        "synthesized",
        now,
        713,
        memo.length,
        "/Users/me/Obsidian/second-brain/inbox/customer-call.md",
      ),
      // Distillation never writes meta `title`; the header title now derives
      // from the saved note's frontmatter `title:` / `# H1` instead.
      frontmatter_title: "Customer call: Pilot scope review",
    });
    aligned["customer-call"] = mockAligned("customer-call");
    notes["customer-call"] = (scenario === "distill-complete" || scenario === "first-note-payoff") ? mockNote("customer-call") : mockGroundedNote("customer-call", { includePricing: scenario !== "grounded-note-unused-memo" });
  } else if (scenario === "distill-running" || scenario === "note-streaming-grounded") {
    memo = mockMemoLines();
    sessions.push(mockSession("customer-call", "processing", now, 713, memo.length));
    aligned["customer-call"] = mockAligned("customer-call");
  } else if (scenario === "distill-error-pi-login") {
    memo = mockMemoLines();
    sessions.push(mockSession("customer-call", "unprocessed", now, 713, memo.length));
  } else if (scenario === "distill-error-retry") {
    memo = mockMemoLines();
    sessions.push({
      ...mockSession("customer-call", "failed", now, 713, memo.length),
      failure_message: "The AI note writer timed out while generating the note.",
      processing_state: "failed",
      failed_stage: "distill",
    });
    aligned["customer-call"] = mockAligned("customer-call");
  } else if (scenario === "import-processing") {
    const row = mockSession("team-sync", "processing", now, 0, 0);
    row.title = "Team Sync";
    row.source = "session";
    row.people = [];
    row.calendar_event_title = null;
    row.import_status = "transcribing";
    row.import_source_path = "/Users/me/Recordings/team-sync.wav";
    sessions.push(row);
    // A second, earlier saved note so the list shows the import alongside history.
    sessions.push(mockSession("customer-call", "synthesized", new Date(Date.now() - 3 * 3600_000).toISOString(), 713, 4, "/mock-vault/inbox/customer-call.md"));
  } else if (scenario === "import-error") {
    const row = mockSession("team-sync", "unprocessed", now, 0, 0);
    row.title = "Team Sync";
    row.source = "session";
    row.people = [];
    row.calendar_event_title = null;
    row.import_status = "error";
    row.import_error = "Import needs a .wav file — m4a/mp3 support is coming. Convert it or drop a .wav.";
    row.import_source_path = "/Users/me/Recordings/team-sync.m4a";
    sessions.push(row);
    sessions.push(mockSession("customer-call", "synthesized", new Date(Date.now() - 3 * 3600_000).toISOString(), 713, 4, "/mock-vault/inbox/customer-call.md"));
  } else if (scenario === "import-complete") {
    const row = mockSession("team-sync", "synthesized", now, 372, 0, "/mock-vault/inbox/team-sync.md");
    row.title = "Team Sync";
    row.source = "session";
    row.people = [];
    row.calendar_event_title = null;
    sessions.push(row);
    notes["team-sync"] = mockGroundedNote("team-sync");
    aligned["team-sync"] = mockAligned("team-sync");
  } else if (scenario === "model-notice-missing" || scenario === "model-notice-speakers" || scenario === "model-notice-found") {
    recordingName = "customer-call";
    recordingStartedAt = Date.now() - 88_000;
    memo = [];
    sessions.push(mockSession(recordingName, "recording", now, 88, memo.length));
    // For model-notice-missing/found: no models configured yet
    if (scenario === "model-notice-missing") {
      settings.parakeet_model_dir = null;
      settings.rust_diarization_enabled = false;
    } else if (scenario === "model-notice-found") {
      // Found at a custom path but not yet saved in settings
      settings.parakeet_model_dir = null;
      settings.rust_diarization_enabled = false;
    }
  } else if (scenario !== "first-run" && scenario !== "settings-audio-blocked" && scenario !== "settings-audio-ready" && scenario !== "settings-audio" && scenario !== "settings-audio-pinned" && scenario !== "settings-ai" && scenario !== "settings-ai-api" && scenario !== "home-empty" && scenario !== "home-startup-lag") {
    memo = mockMemoLines();
    sessions.push(mockSession("customer-call", "unprocessed", now, 713, memo.length));
  }

  return {
    scenario,
    settings,
    devices,
    sessions,
    recordingName,
    recordingStartedAt,
    recordingPaused: scenario === "recording-paused",
    captureDeviceState,
    liveTranscriptionMode: "stereo_split",
    memo,
    aligned,
    notes,
    opened: [],
    granolaAuthorized: scenario === "granola-authorized",
    granolaNeedsAttention: scenario === "granola-needs-attention",
    real_distill: runtimeFixture?.real_distill,
  };
}

function mockManySessions(): SessionInfo[] {
  const items: SessionInfo[] = [];
  const now = Date.now();

  type Spec = {
    name: string;
    title: string;
    offsetMin: number;
    duration: number;
    memo: number;
    status: SessionInfo["status"];
    people?: string[];
    tags?: string[];
    note?: boolean;
  };
  const specs: Spec[] = [
    { name: "customer-call-pilot-scope", title: "Customer call: Pilot scope review with Acme leadership team", offsetMin: 8, duration: 0, memo: 0, status: "recording", people: ["Marcus Webb", "Elena Ruiz"] },
    { name: "design-review-export-pipeline", title: "Design review: export pipeline", offsetMin: 95, duration: 1842, memo: 7, status: "processing", people: ["Priya Nair"] },
    { name: "standup", title: "Standup", offsetMin: 240, duration: 720, memo: 2, status: "synthesized", people: ["team"] },
    { name: "1-1-elena", title: "1:1 with Elena", offsetMin: 360, duration: 1820, memo: 5, status: "synthesized", people: ["Elena Ruiz"] },
    { name: "user-interview-rho", title: "User interview — Rho ops lead", offsetMin: 540, duration: 2640, memo: 9, status: "synthesized", people: ["Maya Lin"] },
    { name: "yesterday-standup", title: "Standup", offsetMin: 60 * 24 + 15, duration: 695, memo: 1, status: "synthesized" },
    { name: "yesterday-pricing-strategy", title: "Pricing strategy sync — finance + GTM joint", offsetMin: 60 * 24 + 180, duration: 3120, memo: 11, status: "synthesized", people: ["Marek Novak", "Sara Holm"], tags: ["pricing", "gtm"], note: true },
    { name: "yesterday-eng-roadmap", title: "Eng roadmap Q3 planning", offsetMin: 60 * 24 + 300, duration: 2900, memo: 6, status: "synthesized", people: ["Priya Nair", "Tom Becker"] },
    { name: "two-day-coffee-mike", title: "Coffee with Mike", offsetMin: 60 * 48 + 240, duration: 1450, memo: 3, status: "unprocessed", people: ["Mike Chen"] },
    { name: "two-day-board-prep", title: "Board prep dry run", offsetMin: 60 * 48 + 480, duration: 2750, memo: 8, status: "synthesized", tags: ["board"] },
    { name: "three-day-customer-feedback", title: "Customer feedback round-up — week of design partners", offsetMin: 60 * 72 + 180, duration: 1980, memo: 6, status: "synthesized", note: true, tags: ["research"] },
    { name: "three-day-hiring-debrief", title: "Hiring debrief: senior infra eng", offsetMin: 60 * 72 + 420, duration: 1640, memo: 4, status: "synthesized", people: ["Priya Nair"] },
    { name: "four-day-product-review", title: "Product review", offsetMin: 60 * 96 + 240, duration: 2200, memo: 5, status: "synthesized" },
    { name: "earlier-strategy-offsite", title: "Strategy offsite — day 1 morning session on positioning", offsetMin: 60 * 24 * 10, duration: 5400, memo: 18, status: "synthesized", note: true, tags: ["strategy", "offsite"] },
    { name: "earlier-strategy-offsite-pm", title: "Strategy offsite — day 1 afternoon", offsetMin: 60 * 24 * 10 + 300, duration: 4800, memo: 14, status: "synthesized", note: true },
    { name: "earlier-strategy-offsite-d2", title: "Strategy offsite — day 2", offsetMin: 60 * 24 * 11, duration: 4500, memo: 12, status: "synthesized", note: true },
    { name: "earlier-investor-update", title: "Investor update call", offsetMin: 60 * 24 * 14, duration: 1850, memo: 5, status: "synthesized", note: true },
    { name: "earlier-design-partner-onboard", title: "Design partner setup — Helio", offsetMin: 60 * 24 * 16, duration: 2100, memo: 7, status: "synthesized", note: true },
    { name: "earlier-arch-deep-dive", title: "Architecture deep dive: ingestion", offsetMin: 60 * 24 * 18, duration: 3200, memo: 9, status: "synthesized" },
    { name: "earlier-1-1-tom", title: "1:1 with Tom", offsetMin: 60 * 24 * 19, duration: 1500, memo: 3, status: "synthesized" },
    { name: "earlier-postmortem-incident-42", title: "Postmortem: incident 42 root-cause walkthrough", offsetMin: 60 * 24 * 21, duration: 2700, memo: 11, status: "synthesized", note: true, tags: ["postmortem"] },
    { name: "earlier-roadmap-async", title: "Async roadmap review notes", offsetMin: 60 * 24 * 24, duration: 0, memo: 0, status: "synthesized", note: true },
    { name: "earlier-q2-retro", title: "Q2 retro", offsetMin: 60 * 24 * 28, duration: 3600, memo: 13, status: "synthesized", note: true },
    { name: "earlier-customer-call-helio", title: "Customer call — Helio renewal terms", offsetMin: 60 * 24 * 30, duration: 1980, memo: 6, status: "synthesized" },
    { name: "earlier-team-allhands", title: "Team all-hands", offsetMin: 60 * 24 * 33, duration: 2400, memo: 4, status: "synthesized" },
    // Abandoned captures — undistilled recordings that never became a note.
    // Short unfinished captures stay in the same recency list.
    { name: "union-square-park-1", title: "", offsetMin: 60 * 24 * 2 + 30, duration: 8, memo: 0, status: "unprocessed" },
    { name: "union-square-park-2", title: "", offsetMin: 60 * 24 * 2 + 28, duration: 3, memo: 0, status: "unprocessed" },
    { name: "false-start-meeting", title: "", offsetMin: 60 * 24 * 5 + 90, duration: 14, memo: 0, status: "unprocessed" },
    { name: "earlier-quick-thought", title: "", offsetMin: 60 * 24 * 12, duration: 22, memo: 1, status: "unprocessed" },
    { name: "earlier-aborted-call", title: "", offsetMin: 60 * 24 * 20, duration: 5, memo: 0, status: "unprocessed" },
  ];

  for (const spec of specs) {
    const start = new Date(now - spec.offsetMin * 60_000).toISOString();
    const notePath = spec.note ? `/Users/me/Obsidian/second-brain/inbox/${spec.name}.md` : null;
    const session: SessionInfo = {
      name: spec.name,
      start_time: start,
      notes_path: `${spec.name}.md`,
      segment_count: 1,
      duration_secs: spec.duration,
      memo_line_count: spec.memo,
      status: spec.status,
      vault_note_path: notePath,
      people: spec.people ?? [],
      calendar_event_title: null,
      source: spec.note && spec.duration === 0 ? "capture_note" : "session",
      title: spec.title,
      frontmatter_created: notePath ? start.slice(0, 10) : null,
      frontmatter_created_sort: start,
      frontmatter_tags: spec.tags ?? [],
      frontmatter_people: spec.people ?? [],
      frontmatter_reflection_type: spec.tags?.[0] ?? null,
    };
    items.push(session);
  }
  return items;
}

function mockSession(
  name: string,
  status: SessionInfo["status"],
  startTime: string,
  duration: number,
  memoCount: number,
  vaultNotePath: string | null = null,
): SessionInfo {
  return {
    name,
    start_time: startTime,
    notes_path: `${name}.md`,
    segment_count: 1,
    duration_secs: duration,
    memo_line_count: memoCount,
    status,
    vault_note_path: vaultNotePath,
    people: ["Marcus Webb", "Elena Ruiz"],
    calendar_event_title: "Customer Call: Pilot Scope",
    source: "session",
    frontmatter_created: vaultNotePath ? startTime.slice(0, 10) : null,
    frontmatter_created_sort: vaultNotePath ? startTime.slice(0, 10) : null,
    frontmatter_tags: vaultNotePath ? ["enzyme/gtm", "ai-ux", "capture"] : [],
    frontmatter_people: vaultNotePath ? ["Marcus Webb", "Elena Ruiz"] : [],
    frontmatter_reflection_type: vaultNotePath ? "strategy" : null,
  };
}

function transcriptEvent(channel: number, startMs: number, endMs: number, text: string): ProcessingEvent {
  const entry: TranscriptEntry = { channel, start_ms: startMs, end_ms: endMs, text };
  return { stage: "transcript", message: "", progress: null, entry };
}

function emitProcessing(event: ProcessingEvent) {
  for (const callback of processingListeners) callback(event);
}

function emitProcessingForSession(sessionName: string, event: ProcessingEvent) {
  processingHistoryBySession[sessionName] = [...(processingHistoryBySession[sessionName] || []), event];
  emitProcessing(event);
}

function emitSpeechModelProgress(event: SpeechModelProgressEvent) {
  for (const callback of speechModelListeners) callback(event);
}

function emitBackchannelSuggestion(event: BackchannelSuggestionEvent) {
  for (const callback of backchannelSuggestionListeners) callback(event);
}

function emitLiveTranscriptReady(payload: { session_name: string }) {
  for (const callback of liveTranscriptReadyListeners) callback(payload);
}

function emitLiveTranscriptDegraded(payload: { session_name: string; reason: string }) {
  for (const callback of liveTranscriptDegradedListeners) callback(payload);
}

function emitCaptureDeviceChanged(state: CaptureDeviceEvent) {
  const s = ensureState();
  if (
    state.state !== "fallback"
    && state.state !== "recovered"
    && state.state !== "switch_failed"
  ) {
    s.captureDeviceState = state;
  }
  if (state.state === "active") {
    s.recordingPaused = false;
  }
  for (const callback of captureDeviceListeners) callback(state);
  dispatchStateChanged();
}

function formatMockElapsed(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const m = Math.floor(total / 60);
  const s = total % 60;
  return `${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
}

function mockAligned(name: string): string {
  return `# ${name} — Aligned Timeline\n\n` +
    `**Session start**: 2026-06-06T17:14:00-04:00\n` +
    `**Duration**: 11m 53s\n` +
    `**Segments**: 1 stereo audio segment\n\n` +
    `---\n\n` +
    `## Timeline\n\n` +
    `### 00:12–00:58\n\n` +
    `**[00:20 memo]** frame as infrastructure, not personal notes app\n\n` +
    `> [transcript ch0] I'm trying to explain why the capture layer matters without making it sound like another AI notepad.\n\n` +
    `> [transcript ch1] The part I care about is whether it works inside the workflow we already have, not whether it's another place to write.\n\n` +
    `### 02:14–03:05\n\n` +
    `**[02:21 memo]** ask for concrete failed workflow before pitching\n\n` +
    `> [transcript ch1] The painful thing is after a call, the context disappears into Slack and random docs. Nobody knows what changed.\n\n` +
    `> [transcript ch0] So the test is whether the note can preserve the decision surface, not just summarize the call.\n\n` +
    `### 05:36–06:18\n\n` +
    `**[05:48 memo]** tension: too much PKM ceremony vs useful memory\n\n` +
    `> [transcript ch0] Obsidian people will tolerate structure, but not if the app makes them babysit a taxonomy while they're trying to listen.\n\n` +
    `### 08:42–09:34\n\n` +
    `**[08:57 memo]** follow up with one-page pilot scoped to one real corpus\n\n` +
    `> [transcript ch1] I would try this if the ask was narrow: one real corpus, one workflow, and show me what context it saves.\n\n` +
    `---\n\n` +
    `## Session metadata\n\n` +
    `- Memo lines: 4\n` +
    `- Transcript entries: 6\n`;
}

function mockNote(name: string): string {
  return `# ${name}\n\n` +
    `## Core read\nThe customer is not asking for a bigger dashboard. They are trying to replace the weekly status ritual with a lighter decision-memory loop.\n\n` +
    `## Related notes\n- [[Operating cadence]] — status rituals as coordination debt.\n- [[Pilot design|Pilot design principles]] — keep scope narrow enough to learn.\n- [Launch memo](https://example.com/launch-memo) — external reference from the call.\n\n` +
    `## Follow-ups\n- [ ] Send a one-page pilot proposal.\n- [x] Ask for an example of their current weekly status artifact.`;
}

function mockMemoLines(): MemoLine[] {
  return [
    { created_secs: 9, edited_secs: null, text: "Pricing concern; asked about pilot scope." },
    { created_secs: 42, edited_secs: 71, text: "Strong signal around replacing the weekly status ritual." },
    { created_secs: 74, edited_secs: null, text: "Follow up: ask for current weekly status artifact." },
  ];
}

function parseRuntimeMemo(text: string): MemoLine[] {
  return text
    .split(/\r?\n/)
    .map((line): MemoLine | null => {
      const match = line.match(/^\s*\[(\d{2}):(\d{2})(?::(\d{2}))?\]\s+(.+?)\s*$/);
      if (!match) return null;
      const first = Number(match[1]);
      const second = Number(match[2]);
      const third = match[3] ? Number(match[3]) : null;
      const createdSecs = third === null
        ? first * 60 + second
        : first * 3600 + second * 60 + third;
      const memoLine: MemoLine = {
        created_secs: createdSecs,
        edited_secs: null,
        text: match[4],
      };
      return memoLine;
    })
    .filter((line): line is MemoLine => Boolean(line));
}

function mockGroundedNote(name: string, options: { includePricing?: boolean } = {}): string {
  const includePricing = options.includePricing !== false;
  return `# ${mockGroundedNoteTitle(name)}\n\n` +
    `<!--MARGINS:USE {"section_id":"weekly-status","memo_ids":["m002"],"mode":"absorbed","disposition":"folded_into_section","transcript_refs":[{"start_secs":64,"end_secs":79,"quote":"The weekly status ritual is the thing I want to replace."}],"vault_refs":["Operating cadence.md"]}-->\n` +
    `## Weekly status ritual\nThe customer is trying to replace a heavy weekly status ritual with a lighter decision-memory loop. The useful product shape is a system that preserves decisions as they happen instead of asking teams to reconstruct them every Friday.\n\n` +
    (includePricing
      ? `<!--MARGINS:USE {"section_id":"pilot-scope","memo_ids":["m001"],"mode":"accounted_for","disposition":"folded_into_section","transcript_refs":[{"start_secs":13,"end_secs":21,"quote":"keep the pilot narrow enough to learn quickly"}],"vault_refs":["Pilot design.md"]}-->\n` +
        `## Pilot scope\nKeep the first pilot narrow enough to learn quickly. Pricing concerns should be handled as scope control: fewer surfaces, clearer success criteria, and one decision log the customer can compare against their current process.\n\n`
      : "") +
    `<!--MARGINS:USE {"section_id":"follow-ups","memo_ids":["m003"],"mode":"accounted_for","disposition":"folded_into_action_item","transcript_refs":[],"vault_refs":[]}-->\n` +
    `## Follow-ups\n- [ ] Send a one-page pilot proposal.\n- [x] Ask for an example of their current weekly status artifact.`;
}

function mockGroundedNoteTitle(name: string): string {
  if (name === "customer-call") return "Customer call: Margins pilot scope";
  return importTitleFromStemMock(name);
}

function mockStreamingGroundedNoteChunks(): string[] {
  return mockGroundedNote("customer-call")
    .split("\n")
    .map(line => `${line}\n`);
}

function delay(ms: number): Promise<void> {
  return new Promise(resolve => setTimeout(resolve, ms));
}

installWindowHook();

// The reprocess suggestion compares current people against the people the note
// was written with, persisted in localStorage (read lazily by the app). For
// the reprocess scenario, pretend the note was distilled before Marcus/Elena
// were added so the suggestion appears; keep every other scenario hermetic.
if (typeof window !== "undefined") {
  if (scenarioName() === "reprocess-with-people") {
    window.localStorage.setItem("margins.reprocessPeopleBaseline", JSON.stringify({ "customer-call": [] }));
    window.localStorage.removeItem("margins.reprocessPeopleDismissed");
  } else {
    window.localStorage.removeItem("margins.reprocessPeopleBaseline");
    window.localStorage.removeItem("margins.reprocessPeopleDismissed");
  }
  // Beat 7: clear the first-note celebrated flag so the payoff callout shows.
  if (scenarioName() === "first-note-payoff") {
    window.localStorage.removeItem("margins.firstNoteCelebrated");
  }
}
