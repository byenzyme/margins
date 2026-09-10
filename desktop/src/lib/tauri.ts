import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { check } from "@tauri-apps/plugin-updater";
import * as httpBackend from "./http-backend";
import { getCapabilities as _getCapabilitiesImpl, ALL_TRUE, ALL_FALSE, type Capabilities } from "./capabilities";

// Audio extensions surfaced in the drag affordance + browse dialog. The Phase 1
// backend only decodes WAV natively; non-WAV drops surface the backend's Err.
export const IMPORT_AUDIO_EXTENSIONS = ["wav", "m4a", "mp3", "flac", "aac"] as const;
export const GRANOLA_IMPORT_EXTENSIONS = ["json", "jsonl", "csv"] as const;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

export interface Settings {
  vault_path: string | null;
  projects?: ProjectSource[];
  active_project_id?: string | null;
  ai_mode?: "included" | "chatgpt" | "api" | "auto" | string | null;
  /** When true (default), opens capture surface and auto-starts at calendar event time. */
  auto_start_from_calendar?: boolean;
  ai_base_url?: string | null;
  ai_model?: string | null;
  chatgpt_model?: string | null;
  ai_models_namespaced?: boolean;
  api_key: string | null;
  backchannel_base_url?: string | null;
  backchannel_model?: string | null;
  backchannel_api_key?: string | null;
  backchannel_same_as_distill?: boolean | null;
  cleanup_policy: string;
  input_device_mode?: "follow_default" | "pinned";
  input_device_uid?: string | null;
  input_device_name?: string | null;
  audio_input_ready?: boolean;
  system_audio_ready?: boolean;
  editor_command?: string | null;
  parakeet_model_dir?: string | null;
  rust_diarization_enabled?: boolean;
  inbox_folder?: string;
  people_folder?: string;
  created_date_format?: string;
  /** Friendly display format for capture timestamps in the sidebar/header. */
  sidebar_date_format?: string;
  note_filename_template?: string;
  person_note_template?: string;
  distill_instructions?: string;
  /** Persistent default speaker cap for audio imports. 0 = Auto (no cap). */
  import_speaker_count?: number;
}

export interface ProjectSource {
  id: string;
  name: string;
  path: string;
  inbox_folder?: string;
  people_folder?: string;
  readiness?: "ready" | "updating" | "needs_setup" | "error" | string;
}

export interface AiStatus {
  chatgpt_authenticated: boolean;
  chatgpt_message: string;
}

export interface IncludedAiStatus {
  included_ready: boolean;
  message: string;
}

export interface ModeReadiness {
  ready: boolean;
  reason: string;
}

/** Single-shot readiness for all three AI modes plus the current effective
 *  mode, from `get_ai_readiness`. Replaces the old aiStatus + includedAiStatus
 *  + settings-derived stitching in the settings UI. */
export interface AiReadiness {
  mode: string;
  modes: {
    included: ModeReadiness;
    chatgpt: ModeReadiness;
    api: ModeReadiness;
  };
}

/** One resolved activity in the settings preview (`preview_ai_resolution`).
 *  `role` is the display name: "distill" | "cue" | "reprocess" | "indexing". */
export interface PreviewRole {
  role: string;
  provider_label: string;
  model_id: string;
  model_label: string;
  changed_by_override: boolean;
}

export interface ResolutionPreview {
  mode: string;
  roles: PreviewRole[];
}

export interface SessionInfo {
  name: string;
  project_id?: string | null;
  start_time: string;
  notes_path: string;
  title?: string | null;
  segment_count: number;
  duration_secs: number;
  memo_line_count: number;
  status: "recording" | "unprocessed" | "processing" | "synthesized" | "failed";
  vault_note_path: string | null;
  failure_message?: string | null;
  processing_state?: "none" | "transcribing" | "distilling" | "failed" | "done" | string | null;
  failed_stage?: "transcribe" | "distill" | "save" | string | null;
  people?: string[];
  calendar_event_title?: string | null;
  source?: "session" | "capture_note" | string;
  frontmatter_title?: string | null;
  frontmatter_created?: string | null;
  frontmatter_created_sort?: string | null;
  frontmatter_tags?: string[];
  frontmatter_people?: string[];
  frontmatter_reflection_type?: string | null;
  // ---- Client-only transient fields for in-flight audio imports ----
  // The backend never sends these; the frontend stamps them on the optimistic
  // session row created on drop so the sidebar can render import progress and
  // an error/retry affordance without a distinct backend `source`.
  import_status?: "queued" | "transcribing" | "note" | "error" | null;
  import_error?: string | null;
  import_source_path?: string | null;
}

export interface CalendarEventSuggestion {
  title: string;
  start: string | null;
  end: string | null;
  calendar_id: string | null;
  event_id: string | null;
  people: string[];
  filename: string;
}

export interface EvidenceFreshness {
  status: "fresh" | "stale" | "error" | "needs_auth" | "not_applicable";
  stale: boolean;
  reason?: string | null;
  last_successful_refresh?: string | null;
}

export interface CalendarSuggestionResult {
  schema_version: "margins.calendar-suggestion.v1";
  suggestion: CalendarEventSuggestion | null;
  freshness: EvidenceFreshness;
}

export interface DeviceInfo {
  uid: string;
  name: string;
  is_default: boolean;
  sample_rate?: number | null;
}

export interface DeviceSnapshot {
  devices: DeviceInfo[];
  generation: number;
}

export interface AudioTestResult {
  device_name: string;
  peak: number;
  drop_count: number;
  ok: boolean;
}

export interface SystemAudioTestResult {
  peak: number;
  drop_count: number;
  silent_secs: number;
  frame_count: number;
  status: "ok" | "quiet" | "blocked" | string;
  message: string;
  restart_recommended: boolean;
}

export type CaptureDeviceState =
  | { state: "active"; device_name: string }
  | { state: "switching"; from: string; to: string }
  | { state: "holding"; last_good: string; reason: "device_lost" | "open_failed" };

/** Transient facts delivered on capture-device-changed. Status never carries these. */
export type CaptureDeviceEvent =
  | CaptureDeviceState
  | { state: "fallback"; requested_uid: string | null; requested_name?: string; device_name: string }
  | { state: "recovered"; device_name: string; backend: "cpal" | "coreaudio" | string }
  | { state: "switch_failed"; device_name: string; error: string };

export interface WebRecordingRecoveryStatus {
  recording_id: string;
  session_name: string;
  elapsed_secs: number;
  finalization_error: string | null;
  owner_lease_active: boolean;
  recovery_phase: "finalizing" | "failed" | "cleanup_pending";
}

export interface HostedCaptureProtocol {
  version: number;
  minimumClientVersion: number;
}

export interface RecordingStatus {
  is_recording: boolean;
  /** True when the active session is paused (segment finalized, resumable). */
  paused: boolean;
  session_name: string | null;
  /** Globally unambiguous, non-secret hosted capture identity. */
  web_recording_id?: string | null;
  /** Stable oldest-first retained hosted captures. */
  web_recoveries?: WebRecordingRecoveryStatus[];
  elapsed_secs: number;
  input_device_name: string | null;
  capture_device: CaptureDeviceState;
  /** Linear microphone peak, or null until the active capture has a real sample. */
  mic_level: number | null;
  /** Samples delivered by the current microphone route. */
  mic_audio_frame_count: number;
  spk_level: number;
  mic_drop_count: number;
  spk_drop_count: number;
  mic_gap_ms_total: number;
  mic_switch_count: number;
  timeline_reusable: boolean;
  speaker_silence_secs: number;
  system_audio_expected: boolean;
  system_audio_frame_count: number;
  system_audio_observed: boolean;
  system_audio_seen: boolean;
  tap_status: string;
  tap_warning: string | null;
  live_transcription_mode: string;
  capture_phase: string;
  /** Hosted-browser durable MediaRecorder upload telemetry (unset natively). */
  webm_chunk_count?: number | null;
  webm_bytes?: number | null;
  webm_last_received_unix_ms?: number | null;
  webm_last_received_age_ms?: number | null;
  /** Hosted-browser live-transcription PCM admission telemetry (unset natively). */
  live_pcm_batch_count?: number | null;
  live_pcm_sample_count?: number | null;
  live_pcm_last_received_unix_ms?: number | null;
  live_pcm_last_received_age_ms?: number | null;
  live_pcm_configured?: boolean | null;
  /** Server clock sampled with the transport status, for skew-safe ages. */
  web_transport_server_unix_ms?: number | null;
  /** Server-observed opaque owner lease. The owner capability itself is never returned. */
  web_owner_lease_active?: boolean | null;
  web_owner_last_heartbeat_unix_ms?: number | null;
  web_owner_heartbeat_age_ms?: number | null;
  web_owner_lease_timeout_ms?: number | null;
  web_finalization_error?: string | null;
  /** Browser-local ownership/health overlay; the server cannot observe amplitude. */
  web_capture_owner?: "local" | "recovery" | "remote" | "absent" | "unknown";
  web_durable_audio_status?: "starting" | "healthy" | "missing" | "stale" | "failed" | "interrupted" | "paused" | "unknown";
  web_live_pcm_status?: "starting" | "healthy" | "missing" | "stale" | "failed" | "unavailable" | "interrupted" | "paused" | "unknown";
  web_capture_warning?: string | null;
  web_live_pcm_warning?: string | null;
  /** Freshness of the durable live transcript, when a checkpoint exists. */
  transcript_watermark?: {
    decoded_until_ms: number;
    committed_until_ms: number;
    updated_unix_ms: number;
  } | null;
}

export interface MemoLine {
  text: string;
  created_secs: number;
  edited_secs: number | null;
  draft_started_secs?: number | null;
  audio_pending_at_mark?: boolean;
  /** null = timed (clock was running). 0 = prep block. N = pause block after segment N. */
  block_ordinal?: number | null;
}

export interface BackchannelSuggestionEvent {
  session_name: string;
  memo_index: number;
  memo_time: string;
  status: string;
  state: string;
  kind?: string | null;
  direction?: string | null;
  title?: string | null;
  why?: string | null;
  suggestion?: string | null;
  confidence?: string | null;
  safety?: string | null;
  raw_json?: unknown;
}

export interface TranscriptEntry {
  channel: number;
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface ProcessingEvent {
  stage: string;
  message: string;
  progress: number | null;
  session?: string | null;
  track?: "capture" | "transcript" | "note" | string | null;
  phase?: string | null;
  emitted_at_ms?: number | null;
  elapsed_ms?: number | null;
  entry?: TranscriptEntry;
}

export interface RecallIndexingEvent {
  project_id: string | null;
  path: string;
  status: "started" | "finished" | string;
}

export interface VaultValidation {
  exists: boolean;
  has_obsidian: boolean;
  has_recall_index: boolean;
  has_margins?: boolean;
}

export interface ProjectRegistrationResult {
  settings: Settings;
  validation: VaultValidation;
}

export interface SpeechModelPrepResult {
  parakeet_ready: boolean;
  parakeet_message: string;
  parakeet_model_dir: string | null;
  diarization_ready: boolean;
  diarization_message: string;
}

export interface SpeechModelProbe {
  transcription: "ready" | "missing" | "unsupported";
  transcription_path: string | null;
  diarization: "ready" | "missing";
  download_bytes: number | null;
}

export interface InstallCliResult {
  installed_path: string;
  source_path: string;
  message: string;
}

export interface EnsureCliToolsResult {
  installed: boolean;
  message: string;
}

export interface SpeechModelProgressEvent {
  stage: string;
  message: string;
  progress: number | null;
}

export interface GranolaImportSurvey {
  file_count: number;
  meeting_count: number;
  people: string[];
  organizations: string[];
  ambiguous_people: string[];
  suggested_notes_folder: string;
  suggested_people_folder: string;
  suggested_organizations_folder: string;
  folder_candidates: string[];
  sample_titles: string[];
  warnings: string[];
}

export interface GranolaImportOptions {
  notes_folder: string;
  people_folder: string;
  organizations_folder: string;
}

export interface GranolaImportResult {
  imported_count: number;
  note_paths: string[];
  people_created: number;
  organizations_created: number;
  warnings: string[];
}

export interface GranolaImportStatus {
  authorized: boolean;
  account: string | null;
  accounts: string[];
  message: string;
  code?: string;
  stage?: string;
  reason?: string;
  retryable: boolean;
}

export interface PrepHydrationMarginalia {
  anchor: string | null;
  kind: "carried" | "blind_spot" | "sharpen" | "counterevidence" | "context";
  text: string;
  sources: string[];
}

export interface PrepHydrationEvent {
  session_name: string;
  /** Which clock-stopped block this result is for. */
  block_ordinal: number;
  state: "hydrated" | "thin" | "quiet" | "warming" | "unavailable" | "error";
  posture: string | null;
  marginalia: PrepHydrationMarginalia[];
  reason: string | null;
  hint: string | null;
  error: string | null;
}

/**
 * Request payload for prep-sketch hydration. Single object so new fields stop
 * being threaded positionally through the tauri/http/mock lanes (was a 7-arg
 * signature repeated in 4 files). Keys are the camelCase the Rust command expects.
 */
export interface HydratePrepRequest {
  lines: MemoLine[];
  sessionName: string;
  people: string[];
  eventTitle: string | null;
  blockOrdinal: number;
  pulledTexts: string[];
  meetingSoFar: string | null;
}

export interface GranolaRemoteImportResult {
  imported_count: number;
  note_paths: string[];
  people_created: number;
  organizations_created: number;
  warnings: string[];
  transcripts_plan_gated: boolean;
}

export interface GranolaImportProgressEvent {
  stage: string;
  current: number;
  total: number;
}

// ---------------------------------------------------------------------------
// Backend routing
// ---------------------------------------------------------------------------

export type Backend = "tauri" | "http" | "mock";

// Keep the real Tauri API facade small. Scenario fixtures and mock behavior live
// in ../harness/mock-tauri.ts and are loaded only for the dev browser harness.
type MockTauriBackend = typeof import("../../test-harness/mock-tauri");
let mockBackendPromise: Promise<MockTauriBackend> | null = null;

function isBrowserHarness(): boolean {
  if (typeof window === "undefined") return false;
  const params = new URLSearchParams(window.location.search);
  // Real-state CDP uses Vite in dev mode but intentionally routes commands to
  // the Rust HTTP backend through the Vite proxy.
  if (params.get("backend") === "http") return false;
  // Explicit ?scenario= always forces mock harness, regardless of DEV mode.
  if (params.has("scenario")) return true;
  // In DEV, also fall back to mock if there are no Tauri internals AND no http
  // server was detected (resolveBackend handles the http probe).
  if (!import.meta.env.DEV) return false;
  return !("__TAURI_INTERNALS__" in window);
}

const mockBackendPath = "/test-harness/mock-tauri.ts";

function mockBackend(): Promise<MockTauriBackend> {
  // Vite serves this TS module in dev. Keep it out of production bundles.
  mockBackendPromise ??= import(/* @vite-ignore */ mockBackendPath) as Promise<MockTauriBackend>;
  return mockBackendPromise;
}

let resolvedBackend: Backend | null = null;
let resolveBackendPromise: Promise<Backend> | null = null;

/**
 * Returns true when the resolved backend is 'http'. Lazily probes /api/health
 * on first call; subsequent calls return the cached result synchronously via
 * the resolved promise.
 *
 * Note: isBrowserHarness() returns true for ?scenario= regardless of this.
 * The call order in each exported function is: mock first, then http, then tauri.
 */
async function isHttpBackend(): Promise<boolean> {
  const b = await resolveBackend();
  return b === "http";
}

/**
 * Determine which backend to use. Cached after the first call.
 *
 * Resolution order:
 *   1. 'tauri'  — __TAURI_INTERNALS__ present in window
 *   2. 'mock'   — ?scenario= param present (always forces mock; CDP harness)
 *   3. 'http'   — GET /api/health responds within 1.5 s (Rust server present)
 *   4. 'mock'   — DEV fallback when no server detected
 *   5. 'http'   — PROD fallback (errors will surface through the http layer)
 */
export function resolveBackend(): Promise<Backend> {
  if (resolvedBackend) return Promise.resolve(resolvedBackend);
  resolveBackendPromise ??= (async (): Promise<Backend> => {
    if (typeof window === "undefined") {
      resolvedBackend = "tauri";
      return resolvedBackend;
    }
    if ("__TAURI_INTERNALS__" in window) {
      resolvedBackend = "tauri";
      return resolvedBackend;
    }
    const params = new URLSearchParams(window.location.search);
    if (params.get("backend") === "http") {
      resolvedBackend = "http";
      return resolvedBackend;
    }
    if (params.has("scenario")) {
      resolvedBackend = "mock";
      return resolvedBackend;
    }
    // Probe the HTTP server
    try {
      const ctrl = new AbortController();
      const timer = setTimeout(() => ctrl.abort(), 1500);
      const res = await fetch("/api/health", { signal: ctrl.signal });
      clearTimeout(timer);
      if (res.ok) {
        resolvedBackend = "http";
        return resolvedBackend;
      }
    } catch {
      // health probe failed or timed out
    }
    resolvedBackend = import.meta.env.DEV ? "mock" : "http";
    return resolvedBackend;
  })();
  return resolveBackendPromise;
}

export type { UnlistenFn };

/** True when running as the hosted web app (Rust HTTP server backend), where
 *  native audio device / permission / system-audio controls do not apply.
 *  Synchronous best-effort: reflects the cached resolved backend, falling back
 *  to the explicit ?backend=http signal before resolution has completed. */
export function isHostedWeb(): boolean {
  if (resolvedBackend) return resolvedBackend === "http";
  return typeof window !== "undefined"
    && new URLSearchParams(window.location.search).get("backend") === "http";
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

export async function getSettings(): Promise<Settings> {
  if (isBrowserHarness()) return (await mockBackend()).getSettings();
  if (await isHttpBackend()) return httpBackend.getSettings();
  return invoke("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).updateSettings(settings);
  if (await isHttpBackend()) return httpBackend.updateSettings(settings);
  return invoke("update_settings", { settings });
}

export async function updateAudioSettings(settings: Pick<Settings, "audio_input_ready" | "system_audio_ready" | "input_device_mode" | "input_device_uid" | "input_device_name">): Promise<void> {
  if (isBrowserHarness()) {
    const mock = await mockBackend();
    return mock.updateSettings({ ...await mock.getSettings(), ...settings });
  }
  if (await isHttpBackend()) return httpBackend.updateAudioSettings(settings);
  return invoke("update_audio_settings", {
    audioInputReady: settings.audio_input_ready,
    systemAudioReady: settings.system_audio_ready,
    inputDeviceMode: settings.input_device_mode,
    inputDeviceUid: settings.input_device_uid,
    inputDeviceName: settings.input_device_name,
  });
}

export async function registerProject(settings: Settings, project: ProjectSource): Promise<ProjectRegistrationResult> {
  if (isBrowserHarness()) return (await mockBackend()).registerProject(settings, project);
  if (await isHttpBackend()) return httpBackend.registerProject(settings, project);
  return invoke("register_project", { settings, project });
}

export async function updateProjectReadiness(id: string, readiness: "ready" | "needs_setup" | "updating" | "error"): Promise<Settings> {
  if (isBrowserHarness()) return (await mockBackend()).updateProjectReadiness(id, readiness);
  if (await isHttpBackend()) return httpBackend.updateProjectReadiness(id, readiness);
  return invoke("update_project_readiness", { id, readiness });
}

export async function getAiStatus(): Promise<AiStatus> {
  if (isBrowserHarness()) return (await mockBackend()).getAiStatus();
  if (await isHttpBackend()) return httpBackend.getAiStatus();
  return invoke("get_ai_status");
}

export async function getIncludedAiStatus(): Promise<IncludedAiStatus> {
  if (isBrowserHarness()) return (await mockBackend()).getIncludedAiStatus();
  if (await isHttpBackend()) return httpBackend.getIncludedAiStatus();
  return invoke("get_included_ai_status");
}

export async function prepareIncludedAi(): Promise<IncludedAiStatus> {
  if (isBrowserHarness()) return (await mockBackend()).prepareIncludedAi();
  if (await isHttpBackend()) return httpBackend.prepareIncludedAi();
  return invoke("prepare_included_ai");
}

export async function signInChatgpt(): Promise<AiStatus> {
  if (isBrowserHarness()) return (await mockBackend()).signInChatgpt();
  if (await isHttpBackend()) return httpBackend.signInChatgpt();
  return invoke("sign_in_chatgpt");
}

export async function getAiReadiness(): Promise<AiReadiness> {
  if (isBrowserHarness()) return (await mockBackend()).getAiReadiness();
  if (await isHttpBackend()) return httpBackend.getAiReadiness();
  return invoke("get_ai_readiness");
}

export async function previewAiResolution(settings: Settings): Promise<ResolutionPreview> {
  if (isBrowserHarness()) return (await mockBackend()).previewAiResolution(settings);
  if (await isHttpBackend()) return httpBackend.previewAiResolution(settings);
  return invoke("preview_ai_resolution", { settings });
}

export async function validateVault(path: string): Promise<VaultValidation> {
  if (isBrowserHarness()) return (await mockBackend()).validateVault(path);
  if (await isHttpBackend()) return httpBackend.validateVault(path);
  return invoke("validate_vault", { path });
}

export async function countVaultNotes(path: string): Promise<number> {
  if (isBrowserHarness()) return 0;
  if (await isHttpBackend()) return 0;
  return invoke("count_vault_notes", { path });
}

export async function ensureDefaultVault(path: string): Promise<VaultValidation> {
  if (isBrowserHarness()) return (await mockBackend()).ensureDefaultVault(path);
  return invoke("ensure_default_vault", { path });
}

export async function indexVault(path: string): Promise<VaultValidation> {
  if (isBrowserHarness()) return (await mockBackend()).indexVault(path);
  if (await isHttpBackend()) return httpBackend.indexVault(path);
  return invoke("index_vault", { path });
}

export async function getCalendarEventSuggestion(): Promise<CalendarSuggestionResult> {
  if (isBrowserHarness()) return (await mockBackend()).getCalendarEventSuggestion();
  if (await isHttpBackend()) return httpBackend.getCalendarEventSuggestion();
  return invoke("get_calendar_event_suggestion");
}

export async function updateSessionPeople(name: string, people: string[]): Promise<string[]> {
  if (isBrowserHarness()) return (await mockBackend()).updateSessionPeople(name, people);
  if (await isHttpBackend()) return httpBackend.updateSessionPeople(name, people);
  return invoke("update_session_people", { name, people });
}

export type UpdateTitleResult = {
  title: string | null;
  vault_note_path: string | null;
};

export async function updateSessionTitle(name: string, title: string): Promise<UpdateTitleResult> {
  if (isBrowserHarness()) return (await mockBackend()).updateSessionTitle(name, title);
  if (await isHttpBackend()) return httpBackend.updateSessionTitle(name, title);
  return invoke("update_session_title", { name, title });
}

export async function selectVaultFolder(defaultPath?: string | null): Promise<string | null> {
  if (isBrowserHarness()) return (await mockBackend()).selectVaultFolder(defaultPath);
  if (await isHttpBackend()) return httpBackend.selectVaultFolder(defaultPath);
  const selected = await open({
    directory: true,
    multiple: false,
    defaultPath: defaultPath || undefined,
    title: "Choose Margins project folder",
  });
  return typeof selected === "string" ? selected : null;
}

/**
 * Open the native folder picker rooted at a project, and return the chosen
 * folder as a subpath relative to that project root (empty string = the root
 * itself). Returns null when the user cancels. Throws when the pick is outside
 * the project so the caller can show the message.
 */
export async function selectProjectSubfolder(projectRoot: string): Promise<string | null> {
  if (isBrowserHarness()) return (await mockBackend()).selectProjectSubfolder(projectRoot);
  if (await isHttpBackend()) return httpBackend.selectProjectSubfolder(projectRoot);
  const selected = await open({
    directory: true,
    multiple: false,
    defaultPath: projectRoot || undefined,
    title: "Choose where captures are saved",
  });
  if (typeof selected !== "string") return null;
  return invoke<string>("resolve_project_subfolder", { projectRoot, picked: selected });
}

export async function prepareSpeechModels(parakeetModelDir?: string | null): Promise<SpeechModelPrepResult> {
  if (isBrowserHarness()) return (await mockBackend()).prepareSpeechModels(parakeetModelDir);
  if (await isHttpBackend()) return httpBackend.prepareSpeechModels(parakeetModelDir);
  return invoke("prepare_speech_models", { parakeetModelDir: parakeetModelDir ?? null });
}

export async function probeSpeechModels(customPath?: string): Promise<SpeechModelProbe> {
  if (isBrowserHarness()) return (await mockBackend()).probeSpeechModels(customPath);
  if (await isHttpBackend()) return httpBackend.probeSpeechModels(customPath);
  return invoke("probe_speech_models", { customPath: customPath ?? null });
}

export async function cancelSpeechModelDownload(): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).cancelSpeechModelDownload();
  if (await isHttpBackend()) return httpBackend.cancelSpeechModelDownload();
  return invoke("cancel_speech_model_download");
}

export async function clearSpeechModels(): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).clearSpeechModels();
  if (await isHttpBackend()) return httpBackend.clearSpeechModels();
  return invoke("clear_speech_models");
}

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

export async function listDevices(): Promise<DeviceInfo[]> {
  if (isBrowserHarness()) return (await mockBackend()).listDevices();
  if (await isHttpBackend()) return httpBackend.listDevices();
  const snapshot = await invoke<DeviceSnapshot>("list_devices");
  return snapshot.devices;
}

export async function refreshDevices(): Promise<DeviceInfo[]> {
  if (isBrowserHarness()) return (await mockBackend()).listDevices();
  if (await isHttpBackend()) return httpBackend.listDevices();
  const snapshot = await invoke<DeviceSnapshot>("refresh_devices");
  return snapshot.devices;
}

export async function testAudioInput(deviceUid?: string | null): Promise<AudioTestResult> {
  if (isBrowserHarness()) return (await mockBackend()).testAudioInput(deviceUid);
  if (await isHttpBackend()) return httpBackend.testAudioInput(deviceUid);
  return invoke("test_audio_input", { deviceUid: deviceUid ?? null });
}

export async function testSystemAudioTap(): Promise<SystemAudioTestResult> {
  if (isBrowserHarness()) return (await mockBackend()).testSystemAudioTap();
  if (await isHttpBackend()) return httpBackend.testSystemAudioTap();
  return invoke("test_system_audio_tap");
}

export async function openPrivacyPane(pane: "microphone" | "system-audio"): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).openPrivacyPane(pane);
  if (await isHttpBackend()) return httpBackend.openPrivacyPane(pane);
  return invoke("open_privacy_pane", { pane });
}

export async function restartApp(): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).restartApp();
  if (await isHttpBackend()) return httpBackend.restartApp();
  return invoke("restart_app");
}

export async function installCliTool(): Promise<InstallCliResult> {
  if (isBrowserHarness()) {
    return {
      installed_path: "~/.local/bin/margins",
      source_path: "/mock/margins",
      message: "Added margins terminal command at ~/.local/bin/margins.",
    };
  }
  if (await isHttpBackend()) return httpBackend.installCliTool();
  return invoke("install_cli_tool");
}

export async function ensureCliTools(): Promise<EnsureCliToolsResult> {
  if (isBrowserHarness()) return (await mockBackend()).ensureCliTools();
  if (await isHttpBackend()) return httpBackend.ensureCliTools();
  return invoke("ensure_cli_tools");
}

export async function installWorkspaceSkills(projectPath: string): Promise<void> {
  if (isBrowserHarness()) return;
  if (await isHttpBackend()) return httpBackend.installWorkspaceSkills(projectPath);
  return invoke("install_workspace_skills", { projectPath });
}

export async function installAvailableAppUpdate(): Promise<boolean> {
  if (import.meta.env.DEV) return false;
  if (isBrowserHarness()) return false;
  if (await isHttpBackend()) return httpBackend.installAvailableAppUpdate();
  const update = await check({ timeout: 15_000 });
  if (!update) return false;
  console.info(`Installing Margins ${update.version} update`);
  await update.downloadAndInstall();
  await restartApp();
  return true;
}

// ---------------------------------------------------------------------------
// Sessions
// ---------------------------------------------------------------------------

export async function listSessions(projectId?: string | null): Promise<SessionInfo[]> {
  if (isBrowserHarness()) return (await mockBackend()).listSessions(projectId);
  if (await isHttpBackend()) return httpBackend.listSessions(projectId);
  return invoke("list_sessions", { projectId: projectId ?? null });
}

export async function getProjectFilesFingerprint(projectId?: string | null): Promise<{ project_id: string | null; fingerprint: string }> {
  if (isBrowserHarness()) return { project_id: projectId ?? null, fingerprint: String(Date.now()) };
  if (await isHttpBackend()) return httpBackend.getProjectFilesFingerprint(projectId);
  return invoke("get_project_files_fingerprint", { projectId: projectId ?? null });
}

export async function deleteSession(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).deleteSession(name);
  if (await isHttpBackend()) return httpBackend.deleteSession(name, projectId);
  return invoke("delete_session", { name, projectId: projectId ?? null });
}

/** Re-scan the project's notes, purging sessions whose note file was deleted,
 *  and return the fresh (already-reconciled) session list. */
export async function reconcileProjectNotes(projectId?: string | null): Promise<SessionInfo[]> {
  if (isBrowserHarness()) return (await mockBackend()).listSessions(projectId);
  if (await isHttpBackend()) return httpBackend.reconcileProjectNotes(projectId);
  return invoke("reconcile_project_notes", { projectId: projectId ?? null });
}

// ---------------------------------------------------------------------------
// Audio import (drag-and-drop / browse)
// ---------------------------------------------------------------------------

export interface FileDropPayload {
  type: "enter" | "over" | "drop" | "leave";
  paths: string[];
  // Physical (device-pixel) coordinates from the native webview. Callers must
  // divide by devicePixelRatio before hit-testing against CSS layout rects.
  // (0,0) on `leave`, which carries neither paths nor position.
  position: { x: number; y: number };
}

/** Window-global native drag-drop. Delivers absolute fs paths + cursor position. */
export async function onFileDrop(callback: (payload: FileDropPayload) => void): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onFileDrop(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onFileDrop(callback));
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload as { type: FileDropPayload["type"]; paths?: string[]; position?: { x: number; y: number } };
    callback({
      type: p.type,
      paths: Array.isArray(p.paths) ? p.paths : [],
      position: p.position ? { x: p.position.x, y: p.position.y } : { x: 0, y: 0 },
    });
  });
}

/** Import a single audio file as a session. Returns the created session name. */
export async function importAudioFile(path: string, maxSpeakers?: number | null, projectId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).importAudioFile(path, maxSpeakers ?? null);
  if (await isHttpBackend()) return httpBackend.importAudioFile(path, maxSpeakers, projectId);
  return invoke("import_audio_file", { path, maxSpeakers: maxSpeakers ?? null, projectId: projectId ?? null });
}

export async function surveyGranolaImport(paths: string[], projectId?: string | null): Promise<GranolaImportSurvey> {
  if (isBrowserHarness()) return (await mockBackend()).surveyGranolaImport(paths, projectId ?? null);
  if (await isHttpBackend()) return httpBackend.surveyGranolaImport(paths, projectId);
  return invoke("survey_granola_import", { paths, projectId: projectId ?? null });
}

export async function importGranolaFiles(paths: string[], options: GranolaImportOptions, projectId?: string | null): Promise<GranolaImportResult> {
  if (isBrowserHarness()) return (await mockBackend()).importGranolaFiles(paths, options, projectId ?? null);
  if (await isHttpBackend()) return httpBackend.importGranolaFiles(paths, options, projectId);
  return invoke("import_granola_files", { paths, options, projectId: projectId ?? null });
}

export async function getGranolaImportStatus(): Promise<GranolaImportStatus> {
  if (isBrowserHarness()) return (await mockBackend()).getGranolaImportStatus();
  if (await isHttpBackend()) return httpBackend.getGranolaImportStatus();
  return invoke("get_granola_import_status");
}

export async function authorizeGranolaImport(): Promise<GranolaImportStatus> {
  if (isBrowserHarness()) return (await mockBackend()).authorizeGranolaImport();
  if (await isHttpBackend()) return httpBackend.authorizeGranolaImport();
  return invoke("authorize_granola_import");
}

export async function revokeGranolaImportAuthorization(account?: string | null): Promise<GranolaImportStatus> {
  if (isBrowserHarness()) return (await mockBackend()).revokeGranolaImportAuthorization(account ?? null);
  if (await isHttpBackend()) return httpBackend.revokeGranolaImportAuthorization(account);
  return invoke("revoke_granola_import_authorization", { account: account ?? null });
}

export async function importGranolaMcp(projectId?: string | null): Promise<GranolaRemoteImportResult> {
  if (isBrowserHarness()) return (await mockBackend()).importGranolaMcp(projectId ?? null);
  if (await isHttpBackend()) return httpBackend.importGranolaMcp(projectId);
  return invoke("import_granola_mcp", { projectId: projectId ?? null });
}

export function onGranolaImportProgress(
  callback: (event: GranolaImportProgressEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onGranolaImportProgress(callback));
  return isHttpBackend().then(isHttp => {
    if (isHttp) return Promise.resolve(httpBackend.onGranolaImportProgress(callback));
    return listen<GranolaImportProgressEvent>("granola-import-progress", (e) => {
      callback(e.payload);
    });
  });
}

/** Browse-fallback file picker for audio import. Returns absolute paths. */
export async function pickAudioFiles(): Promise<string[]> {
  if (isBrowserHarness()) return (await mockBackend()).pickAudioFiles();
  if (await isHttpBackend()) return httpBackend.pickAudioFiles();
  const selected = await open({
    multiple: true,
    filters: [{ name: "Audio", extensions: [...IMPORT_AUDIO_EXTENSIONS] }],
    title: "Import audio to transcribe",
  });
  if (!selected) return [];
  return Array.isArray(selected) ? selected : [selected];
}

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

export async function startRecording(
  name: string,
  deviceUid?: string | null,
  projectId?: string | null,
): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).startRecording(name, deviceUid);
  if (await isHttpBackend()) return httpBackend.startRecording(name, deviceUid, projectId);
  return invoke("start_recording", {
    name,
    deviceUid: deviceUid ?? null,
    projectId: projectId ?? null,
  });
}

export async function cancelRecordingStartup(): Promise<boolean> {
  if (isBrowserHarness()) return (await mockBackend()).cancelRecordingStartup();
  if (await isHttpBackend()) return httpBackend.cancelRecordingStartup();
  return invoke("cancel_recording_startup");
}

export async function stopRecording(recordingId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).stopRecording();
  if (await isHttpBackend()) return httpBackend.stopRecording(recordingId);
  return invoke("stop_recording");
}

export async function consumeHostedFinalizationWarning(): Promise<string | null> {
  if (isBrowserHarness()) return null;
  if (await isHttpBackend()) return httpBackend.consumeHostedFinalizationWarning();
  return null;
}

export async function discardRecording(recordingId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).discardRecording();
  if (await isHttpBackend()) return httpBackend.discardRecording(recordingId);
  return invoke("discard_recording");
}

export async function switchRecordingDevice(deviceUid?: string | null): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).switchRecordingDevice(deviceUid);
  if (await isHttpBackend()) return httpBackend.switchRecordingDevice(undefined);
  return invoke("switch_recording_device", { deviceUid: deviceUid ?? null });
}

export async function restartSystemAudioCapture(): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).restartSystemAudioCapture();
  if (await isHttpBackend()) return httpBackend.restartSystemAudioCapture();
  return invoke("restart_system_audio_capture");
}

export async function pauseRecording(): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).pauseRecording();
  if (await isHttpBackend()) return httpBackend.pauseRecording();
  return invoke("pause_recording");
}

export async function resumeRecording(): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).resumeRecording();
  if (await isHttpBackend()) return httpBackend.resumeRecording();
  return invoke("resume_recording");
}

export async function setLiveTranscriptionMode(mode: "stereo_split" | "mic_diarized"): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).setLiveTranscriptionMode(mode);
  if (await isHttpBackend()) return httpBackend.setLiveTranscriptionMode(mode);
  return invoke("set_live_transcription_mode", { mode });
}

export async function getRecordingStatus(): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).getRecordingStatus();
  if (await isHttpBackend()) return httpBackend.getRecordingStatus();
  return invoke("get_recording_status");
}

export async function negotiateHostedCaptureProtocol(): Promise<HostedCaptureProtocol | null> {
  if (isBrowserHarness()) return null;
  if (await isHttpBackend()) return httpBackend.negotiateHostedCaptureProtocol();
  return null;
}

export async function listHostedRecordingRecoveries(): Promise<WebRecordingRecoveryStatus[]> {
  if (isBrowserHarness()) return [];
  if (await isHttpBackend()) return httpBackend.listHostedRecordingRecoveries();
  return [];
}

export async function getHostedRecordingStatus(recordingId: string): Promise<RecordingStatus> {
  if (isBrowserHarness()) return (await mockBackend()).getRecordingStatus();
  if (await isHttpBackend()) return httpBackend.getHostedRecordingStatus(recordingId);
  return invoke("get_recording_status");
}

export async function syncMemo(lines: MemoLine[], sessionName: string, recordingId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).syncMemo(lines, sessionName);
  if (await isHttpBackend()) return httpBackend.syncMemo(lines, sessionName, recordingId);
  return invoke("sync_memo", { lines, sessionName });
}

/** Hosted reload hydration. Null on native/mock capture, whose memo state is
 * already owned by the in-process backend. */
export async function hydrateRecordingMemo(sessionName: string, recordingId?: string | null): Promise<MemoLine[] | null> {
  if (isBrowserHarness()) return null;
  if (await isHttpBackend()) return httpBackend.hydrateRecordingMemo(sessionName, recordingId);
  return null;
}

export async function claimInterruptedRecording(sessionName: string, recordingId: string): Promise<MemoLine[]> {
  if (isBrowserHarness()) throw new Error("Hosted recovery is unavailable in the mock backend.");
  if (await isHttpBackend()) return httpBackend.claimInterruptedRecording(sessionName, recordingId);
  throw new Error("Hosted recovery is only available in browser capture mode.");
}

export async function checkpointMemoLine(lines: MemoLine[], committedIndex: number, sessionName: string, recordingId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).checkpointMemoLine(lines, committedIndex, sessionName);
  if (await isHttpBackend()) return httpBackend.checkpointMemoLine(lines, committedIndex, sessionName, recordingId);
  return invoke("checkpoint_memo_line", { lines, committedIndex, sessionName });
}

export async function requestBackchannelForMemo(lines: MemoLine[], committedIndex: number, sessionName: string, recordingId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).requestBackchannelForMemo(lines, committedIndex, sessionName);
  if (await isHttpBackend()) return httpBackend.requestBackchannelForMemo(lines, committedIndex, sessionName, recordingId);
  return invoke("request_backchannel_for_memo", { lines, committedIndex, sessionName });
}

export async function steerBackchannelForMemo(
  memoIndex: number,
  steering: string,
  previousSuggestion: string | null,
  sessionName: string,
  recordingId?: string | null,
): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).steerBackchannelForMemo?.(memoIndex, steering, previousSuggestion, sessionName);
  if (await isHttpBackend()) return httpBackend.steerBackchannelForMemo(memoIndex, steering, previousSuggestion, sessionName, recordingId);
  return invoke("steer_backchannel_for_memo", { memoIndex, steering, previousSuggestion, sessionName });
}

export async function hydratePrepSketch(req: HydratePrepRequest): Promise<void> {
  if (isBrowserHarness()) { (await mockBackend()).hydratePrepSketch?.(req); return; }
  if (await isHttpBackend()) return httpBackend.hydratePrepSketch(req);
  return invoke("hydrate_prep_sketch", { ...req });
}

export async function steerPrepHydration(sessionName: string, blockOrdinal: number, instruction: string): Promise<void> {
  if (isBrowserHarness()) { (await mockBackend()).steerPrepHydration?.(sessionName, blockOrdinal, instruction); return; }
  if (await isHttpBackend()) return httpBackend.steerPrepHydration(sessionName, blockOrdinal, instruction);
  return invoke("steer_prep_hydration", { sessionName, blockOrdinal, instruction });
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

export async function processSession(
  name: string,
  projectId?: string | null,
  overwriteExistingNote = false,
  maxSpeakers?: number | null,
  forceTranscribe = false,
): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).processSession(name, overwriteExistingNote, maxSpeakers ?? null, forceTranscribe);
  if (await isHttpBackend()) return httpBackend.processSession(name, projectId, overwriteExistingNote, maxSpeakers, forceTranscribe);
  return invoke("process_session", { name, projectId: projectId ?? null, overwriteExistingNote, maxSpeakers: maxSpeakers ?? null, forceTranscribe });
}

export async function retrySession(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).retrySession(name);
  if (await isHttpBackend()) return httpBackend.retrySession(name, projectId);
  return invoke("retry_session", { name, projectId: projectId ?? null });
}

export async function cancelProcessSession(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).cancelProcessSession(name);
  if (await isHttpBackend()) return httpBackend.cancelProcessSession(name, projectId);
  return invoke("cancel_process_session", { name, projectId: projectId ?? null });
}

export async function clearSessionNoteError(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return;
  if (await isHttpBackend()) return httpBackend.clearSessionNoteError(name, projectId);
  return invoke("clear_session_note_error", { name, projectId: projectId ?? null });
}

export async function saveDraftNote(name: string, projectId?: string | null, overwriteExistingNote = false): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).saveDraftNote(name, overwriteExistingNote);
  if (await isHttpBackend()) return httpBackend.saveDraftNote(name, projectId, overwriteExistingNote);
  return invoke("save_draft_note", { name, projectId: projectId ?? null, overwriteExistingNote });
}

export async function discardNote(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).discardNote(name);
  if (await isHttpBackend()) return httpBackend.discardNote(name, projectId);
  return invoke("discard_note", { name, projectId: projectId ?? null });
}

export async function refineSession(name: string, message: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).refineSession?.(name, message);
  if (await isHttpBackend()) return httpBackend.refineSession(name, message, projectId);
  return invoke("refine_session", { name, message, projectId: projectId ?? null });
}

export async function reprocessSessionWithPeople(name: string, people: string[], projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).reprocessSessionWithPeople(name, people, projectId ?? null);
  if (await isHttpBackend()) return httpBackend.reprocessSessionWithPeople(name, people, projectId);
  return invoke("reprocess_session_with_people", { name, people, projectId: projectId ?? null });
}

export const DISTILL_CANCELLED_SENTINEL = "__margins_distill_cancelled__";

export async function getAlignedContent(name: string, projectId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).getAlignedContent(name);
  if (await isHttpBackend()) return httpBackend.getAlignedContent(name, projectId);
  return invoke("get_aligned_content", { name, projectId: projectId ?? null });
}

export async function getSessionMemo(name: string, projectId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).getSessionMemo(name);
  if (await isHttpBackend()) return httpBackend.getSessionMemo(name, projectId);
  return invoke("get_session_memo", { name, projectId: projectId ?? null });
}

export async function getVaultNote(name: string, projectId?: string | null): Promise<string> {
  if (isBrowserHarness()) return (await mockBackend()).getVaultNote(name);
  if (await isHttpBackend()) return httpBackend.getVaultNote(name, projectId);
  return invoke("get_vault_note", { name, projectId: projectId ?? null });
}

export async function getSessionGrounding(name: string, projectId?: string | null): Promise<unknown | null> {
  if (isBrowserHarness()) return (await mockBackend()).getSessionGrounding(name);
  if (await isHttpBackend()) return httpBackend.getSessionGrounding(name, projectId);
  return invoke("get_session_grounding", { name, projectId: projectId ?? null });
}

export async function getDistillTrace(name: string, projectId?: string | null): Promise<ProcessingEvent[]> {
  if (isBrowserHarness()) return (await mockBackend()).getDistillTrace(name);
  if (await isHttpBackend()) return httpBackend.getDistillTrace(name, projectId);
  return invoke("get_distill_trace", { name, projectId: projectId ?? null });
}

export async function openNote(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).openNote(name);
  if (await isHttpBackend()) return httpBackend.openNote(name, projectId);
  return invoke("open_note", { name, projectId: projectId ?? null });
}

export async function openNoteInObsidian(name: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).openNoteInObsidian(name);
  if (await isHttpBackend()) return httpBackend.openNoteInObsidian(name, projectId);
  return invoke("open_note_in_obsidian", { name, projectId: projectId ?? null });
}

export async function openNoteTargetInObsidian(target: string, projectId?: string | null): Promise<void> {
  if (isBrowserHarness()) return (await mockBackend()).openNoteTargetInObsidian(target);
  if (await isHttpBackend()) return httpBackend.openNoteTargetInObsidian(target, projectId);
  return invoke("open_note_target_in_obsidian", { target, projectId: projectId ?? null });
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

export async function onProcessingProgress(
  callback: (event: ProcessingEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onProcessingProgress(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onProcessingProgress(callback));
  return listen<ProcessingEvent>("processing-progress", (e) => {
    callback(e.payload);
  });
}

export async function onRecallIndexing(
  callback: (event: RecallIndexingEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onRecallIndexing(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onRecallIndexing(callback));
  return listen<RecallIndexingEvent>("recall-indexing", (e) => {
    callback(e.payload);
  });
}

export async function onSpeechModelProgress(
  callback: (event: SpeechModelProgressEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onSpeechModelProgress(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onSpeechModelProgress(callback));
  return listen<SpeechModelProgressEvent>("speech-model-progress", (e) => {
    callback(e.payload);
  });
}

export async function onBackchannelSuggestion(
  callback: (event: BackchannelSuggestionEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onBackchannelSuggestion(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onBackchannelSuggestion(callback));
  return listen<BackchannelSuggestionEvent>("backchannel-suggestion", (e) => {
    callback(e.payload);
  });
}

export async function onBackchannelError(
  callback: (event: BackchannelSuggestionEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onBackchannelError(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onBackchannelError(callback));
  return listen<BackchannelSuggestionEvent>("backchannel-error", (e) => {
    callback(e.payload);
  });
}

export async function onGlobalCaptureToggle(callback: () => void): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onGlobalCaptureToggle(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onGlobalCaptureToggle(callback));
  return listen<unknown>("global-capture-toggle", () => callback());
}

export async function onCaptureDeviceChanged(
  callback: (state: CaptureDeviceEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onCaptureDeviceChanged(callback));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onCaptureDeviceChanged(callback));
  return listen<CaptureDeviceEvent>("capture-device-changed", (e) => {
    callback(e.payload);
  });
}

export async function onDevicesChanged(
  callback: (snapshot: DeviceSnapshot) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return (await mockBackend()).onDevicesChanged(callback);
  if (await isHttpBackend()) return () => {};
  return listen<DeviceSnapshot>("devices-changed", (event) => callback(event.payload));
}

/**
 * Listen for settings-changed events emitted by the backend filesystem watcher
 * when settings.json is modified externally (e.g. by the `margins projects add`
 * CLI command). The frontend should re-fetch getSettings() and re-render.
 */
export async function onSettingsChanged(callback: () => void): Promise<UnlistenFn> {
  if (isBrowserHarness()) return (await mockBackend()).onSettingsChanged(callback);
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onSettingsChanged(callback));
  return listen<unknown>("settings-changed", () => callback());
}

export async function onLiveTranscriptReady(
  callback: (payload: { session_name: string }) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onLiveTranscriptReady(callback));
  if (await isHttpBackend()) return () => {};
  return listen<{ session_name: string }>("live-transcript-ready", (e) => callback(e.payload));
}

export async function onLiveTranscriptDegraded(
  callback: (payload: { session_name: string; reason: string }) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onLiveTranscriptDegraded(callback));
  if (await isHttpBackend()) return () => {};
  return listen<{ session_name: string; reason: string }>("live-transcript-degraded", (e) => callback(e.payload));
}

export async function onPrepHydration(
  callback: (event: PrepHydrationEvent) => void,
): Promise<UnlistenFn> {
  if (isBrowserHarness()) return mockBackend().then(m => m.onPrepHydration?.(callback) ?? (() => {}));
  if (await isHttpBackend()) return Promise.resolve(httpBackend.onPrepHydration(callback));
  return listen<PrepHydrationEvent>("prep-hydration", (e) => {
    callback(e.payload);
  });
}

// ---------------------------------------------------------------------------
// Capabilities
// ---------------------------------------------------------------------------

export type { Capabilities };

/**
 * Fetch capabilities from the backend. On Tauri, returns all-true if the
 * command is not reachable (native macOS assumed capable). On http, returns
 * all-false defaults on failure.
 */
export async function getCapabilities(): Promise<Capabilities> {
  if (isBrowserHarness()) return ALL_TRUE;
  if (await isHttpBackend()) {
    return _getCapabilitiesImpl(
      (cmd) => invokeHttp<Capabilities>(cmd),
      ALL_FALSE,
    );
  }
  // Tauri path: invoke with all-true fallback
  return _getCapabilitiesImpl(
    (cmd) => invoke<Capabilities>(cmd),
    ALL_TRUE,
  );
}

// Re-export the cached accessor so other modules can read a snapshot
// without triggering a new fetch. Import capabilities.ts directly for
// the full interface; this re-export is a convenience.
export { capabilities } from "./capabilities";

// Internal helper used only within this module for getCapabilities.
// Reimplemented inline to avoid importing an unexported symbol from http-backend.
async function invokeHttp<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  const token = typeof window !== "undefined" ? window.__MARGINS_TOKEN__ : undefined;
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (token) headers["Authorization"] = `Bearer ${token}`;
  const res = await fetch(`/api/invoke/${command}`, {
    method: "POST",
    headers,
    body: JSON.stringify(args ?? {}),
  });
  if (!res.ok) {
    const text = await res.text().catch(() => res.statusText);
    throw new Error(`HTTP ${res.status} from ${command}: ${text}`);
  }
  const envelope = await res.json() as { ok: boolean; result?: T; error?: string };
  if (!envelope.ok) throw new Error(envelope.error ?? `Command ${command} returned ok:false`);
  return envelope.result as T;
}
