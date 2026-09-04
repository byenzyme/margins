mod agent_hooks;
mod ai_auth;
mod ai_config;
mod aux_layout;
mod aux_windows;
pub(crate) mod async_runtime {
    #[cfg(feature = "tauri-app")]
    pub(crate) use tauri::async_runtime::{spawn, spawn_blocking};
    #[cfg(not(feature = "tauri-app"))]
    pub(crate) use tokio::task::{spawn, spawn_blocking};
}
mod audio_devices;
mod backchannel_ai;
mod calendar;
pub mod ctx;
mod desktop_live;
mod device_registry;
mod dispatch;
mod editor;
mod ffmpeg;
mod granola_import;
mod granola_mcp;
mod hosted_distill_skill;
mod live_asr_worker;
mod live_backchannel;
mod mic_control;
mod note_artifacts;
pub mod pi_distill;
mod pi_events;
mod prep_ai;
mod processing_events;
mod recording;
mod reprocess;
#[cfg(feature = "server")]
pub mod server;
mod session_index;
mod settings;
mod speech_models;
mod transcript_store;
#[cfg(feature = "tauri-app")]
mod transcription;
mod web_live_asr;
mod web_session;
mod webm_opus;

use ai_auth::AiStatus;
use ai_config::{
    configure_ai_for_backchannel, configure_ai_for_note_job, configure_ai_for_prep,
    configure_ai_for_reprocess,
};
use calendar::{calendar_event_suggestion, normalize_people, CalendarSuggestionResult};
use chrono::{Duration as ChronoDuration, Local};
use editor::{editor_preference, open_path_with_editor, uri_encode, EditorPreference};
use live_backchannel::LiveTranscriptionMode;
#[cfg(all(test, feature = "coreml-asr", target_os = "macos"))]
use margins::recorder;
use margins::{audio_info, project::UpsertProject, session};
#[cfg(feature = "tauri-app")]
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use processing_events::{
    write_processing_steps_markdown, write_processing_trace, ProcessingEvent, ProcessingTrack,
};
use recording::{
    capture_state_from_snapshot, export_memo, idle_recording_status, recording_status_from_state,
    RecordingPhase, RecordingState, RecordingStatus, SegmentController, SegmentControllerStart,
    SegmentDiagnostics, CAPTURE_DEVICE_CHANGED_EVENT,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use session_index::{note_path_for_session_or_capture, SessionInfoDto};
use settings::{
    active_project as settings_active_project, configure_pi_agent_dir, default_vault_path_string,
    default_work_dir, load_settings, normalize_settings, pi_agent_dir, redacted_settings,
    save_settings, settings_path, try_load_settings, InputDeviceMode, LoadSettingsResult,
    ProjectSource, Settings,
};
use speech_models::{clear_speech_models_blocking, probe_speech_models_impl, SpeechModelProbe};
use speech_models::{prepare_speech_models_blocking, SpeechModelPrepResult};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};
#[cfg(feature = "tauri-app")]
use tauri::{path::BaseDirectory, AppHandle, Emitter, Manager};
#[cfg(feature = "tauri-app")]
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

// ---------------------------------------------------------------------------
// App state — everything must be Send + Sync
// ---------------------------------------------------------------------------

pub(crate) struct AppState {
    pub(crate) work_dir: Mutex<PathBuf>,
    pub(crate) settings: Mutex<Settings>,
    pub(crate) device_registry: Arc<device_registry::DeviceRegistry>,
    /// Recording state — only contains Send+Sync types.
    /// The actual RecorderHandle lives on a dedicated thread.
    pub(crate) recording: Mutex<Option<RecordingState>>,
    /// Present only while the native segment controller is opening its first
    /// audio routes. A separate Tauri command can flip the token without
    /// waiting for the blocking startup command to return.
    pub(crate) recording_startup_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub(crate) capture_finalizing: Mutex<HashSet<String>>,
    pub(crate) speech_model_cancel: Mutex<Option<Arc<AtomicBool>>>,
    pub(crate) distill_cancel: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// Vault key -> whether another note landed while its refresh was running.
    pub(crate) enzyme_refreshing: Mutex<HashMap<String, bool>>,
    /// Frozen per-mark context for backchannel steering, keyed by memo index.
    /// Lets the user steer a cue without re-running ASR/enzyme. Overwritten each
    /// time a mark fires a fresh request.
    pub(crate) backchannel_snapshots: Mutex<HashMap<usize, BackchannelSnapshot>>,
    /// Small per-process cache for expensive live-cue vault context lookups.
    /// Keyed by vault + memo/transcript query so retries/duplicate marks do not
    /// pay the Enzyme startup/search cost before the model request.
    pub(crate) backchannel_context_cache:
        Mutex<HashMap<String, backchannel_ai::BackchannelContext>>,
    /// Per-web-session recording state keyed by session-id.  WP3 will flesh
    /// out `WebRecordingState`; for now this just needs to exist.
    pub(crate) web_sessions: Mutex<HashMap<String, web_session::WebRecordingState>>,
    /// Hosted captures removed from the singleton active slot while Finish is
    /// running, then retained here if a durable finalization step fails. These
    /// remain session/owner scoped for retry or discard without displacing a
    /// newer active capture.
    pub(crate) failed_web_sessions:
        Mutex<std::collections::BTreeMap<String, web_session::WebRecordingState>>,
    /// Frozen prep context keyed by session_name, reused for steering re-rolls.
    pub(crate) prep_snapshots: Mutex<HashMap<String, prep_ai::PrepSnapshot>>,
    /// Materialized hosted Margins distillation host preamble. Server startup
    /// installs this from the embedded bundle under MARGINS_DATA_DIR.
    pub(crate) hosted_distill_skill_path: Mutex<Option<PathBuf>>,
}

/// Construct the shared application state.  Called once at startup (Tauri path)
/// and can also be called from the headless server entry point (WP2).
pub(crate) fn build_app_state(work_dir: PathBuf, mut settings: Settings) -> Arc<AppState> {
    margins::initialize_sqlite_runtime().expect("failed to initialize shared SQLite runtime");
    let device_registry = Arc::new(device_registry::DeviceRegistry::default());
    let snapshot = device_registry.refresh();
    if settings::migrate_input_device_uid(&mut settings, &snapshot.devices) {
        if let Err(error) = save_settings(&settings) {
            eprintln!("margins: could not persist microphone UID migration: {error}");
        }
    }
    let failed_web_sessions = web_session::reconstruct_failed_web_sessions(&work_dir, &settings);
    Arc::new(AppState {
        work_dir: Mutex::new(work_dir),
        settings: Mutex::new(settings),
        device_registry,
        recording: Mutex::new(None),
        recording_startup_cancel: Mutex::new(None),
        capture_finalizing: Mutex::new(HashSet::new()),
        speech_model_cancel: Mutex::new(None),
        distill_cancel: Mutex::new(HashMap::new()),
        enzyme_refreshing: Mutex::new(HashMap::new()),
        backchannel_snapshots: Mutex::new(HashMap::new()),
        backchannel_context_cache: Mutex::new(HashMap::new()),
        web_sessions: Mutex::new(HashMap::new()),
        failed_web_sessions: Mutex::new(failed_web_sessions),
        prep_snapshots: Mutex::new(HashMap::new()),
        hosted_distill_skill_path: Mutex::new(None),
    })
}

#[cfg(test)]
pub(crate) fn initialize_test_sqlite_runtime() {
    margins::initialize_sqlite_runtime().expect("failed to initialize shared SQLite runtime");
}

const BACKCHANNEL_CONTEXT_CACHE_LIMIT: usize = 32;
const DEFAULT_DISTILL_TIMEOUT_SECS: u64 = 600;
const DISTILL_TIMEOUT_ENV: &str = "MARGINS_DISTILL_TIMEOUT_SECS";

fn distill_timeout() -> Duration {
    timeout_from_env_secs(DISTILL_TIMEOUT_ENV, DEFAULT_DISTILL_TIMEOUT_SECS)
}

fn timeout_from_env_secs(name: &str, default_secs: u64) -> Duration {
    let secs = std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}

#[derive(Clone, Serialize)]
struct RecallIndexingEvent {
    project_id: Option<String>,
    path: String,
    status: String,
}

struct CachedBackchannelContext {
    context: backchannel_ai::BackchannelContext,
    cache_key: Option<String>,
    cache_hit: bool,
}

struct BackchannelTiming {
    started: Instant,
    last_stage: Instant,
    settings_ms: u128,
    ai_config_ms: u128,
    transcript_snapshot_ms: u128,
    prompt_context_trace_ms: u128,
    context_prepare_ms: u128,
    context_cache_hit: bool,
    snapshot_store_ms: u128,
    model_ms: u128,
    /// Whether the text model was actually invoked. Local no-model outcomes
    /// (warming/quiet/unavailable) keep this false and `model_ms` at 0 so the
    /// trace proves grounded-safety held.
    model_called: bool,
    emit_ms: Option<u128>,
}

impl BackchannelTiming {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_stage: now,
            settings_ms: 0,
            ai_config_ms: 0,
            transcript_snapshot_ms: 0,
            prompt_context_trace_ms: 0,
            context_prepare_ms: 0,
            context_cache_hit: false,
            snapshot_store_ms: 0,
            model_ms: 0,
            model_called: false,
            emit_ms: None,
        }
    }

    fn mark_stage(&mut self) -> u128 {
        let elapsed = self.last_stage.elapsed().as_millis();
        self.last_stage = Instant::now();
        elapsed
    }

    fn total_ms(&self) -> u128 {
        self.started.elapsed().as_millis()
    }
}

/// Snapshot captured when a backchannel cue is generated so a later steer can
/// reuse the exact same grounding instead of re-fetching live context.
#[derive(Clone)]
struct BackchannelSnapshot {
    session_name: String,
    memo_time: String,
    request: backchannel_ai::BackchannelAiRequest,
    context: backchannel_ai::BackchannelContext,
    /// The first cue produced for this mark; the anchor of the steer lineage.
    original_cue: Option<String>,
    /// Each steer the user has applied, with the cue it produced. Grows per
    /// steer so every regeneration sees the cumulative history.
    steer_history: Vec<backchannel_ai::SteerExchange>,
}

#[derive(Serialize)]
struct UpdateTitleResult {
    title: Option<String>,
    vault_note_path: Option<String>,
}

#[derive(Serialize)]
struct InstallCliResult {
    installed_path: String,
    source_path: String,
    message: String,
}

#[derive(Serialize)]
struct EnsureCliToolsResult {
    installed: bool,
    message: String,
}

#[derive(Serialize)]
struct ProjectFilesFingerprint {
    project_id: Option<String>,
    fingerprint: String,
}

/// Pull the user-facing suggestion text out of a raw backchannel JSON response.
fn backchannel_suggestion_text(raw: &str) -> Option<String> {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| {
            v.get("suggestion")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub const DISTILL_CANCELLED_SENTINEL: &str = "__margins_distill_cancelled__";

/// RAII guard that ensures the distill-cancel slot for a session is released
/// when this scope exits, no matter which `?` returns early.
struct DistillCancelGuard {
    state: Arc<AppState>,
    name: String,
    owner: Arc<AtomicBool>,
    released: bool,
}

impl DistillCancelGuard {
    fn release(&mut self) {
        if self.released {
            return;
        }
        let mut slot = self.state.distill_cancel.lock().unwrap();
        if let Some(existing) = slot.get(&self.name) {
            if Arc::ptr_eq(existing, &self.owner) {
                slot.remove(&self.name);
            }
        }
        self.released = true;
    }

    fn is_cancelled(&self) -> bool {
        self.owner.load(Ordering::SeqCst)
    }
}

impl Drop for DistillCancelGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct MemoLine {
    pub(crate) text: String,
    pub(crate) created_secs: f64,
    pub(crate) edited_secs: Option<f64>,
    #[serde(default)]
    pub(crate) draft_started_secs: Option<f64>,
    #[serde(default)]
    pub(crate) audio_pending_at_mark: bool,
    /// null = timed (clock was running); 0 = prep block; N = pause block after segment N.
    /// When non-null, created_secs is NOT a timeline mark.
    #[serde(default)]
    pub(crate) block_ordinal: Option<u32>,
}

#[derive(Clone, Serialize, Deserialize)]
struct BackchannelSuggestionEvent {
    session_name: String,
    memo_index: usize,
    memo_time: String,
    status: String,
    state: String,
    kind: Option<String>,
    direction: Option<String>,
    title: Option<String>,
    why: Option<String>,
    suggestion: Option<String>,
    confidence: Option<String>,
    safety: Option<String>,
    raw_json: Option<Value>,
}

// ---------------------------------------------------------------------------
// DTOs
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportTranscriptArgs {
    pub(crate) name: Option<String>,
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) people: Vec<String>,
    pub(crate) transcript: String,
    pub(crate) memo: Option<String>,
    #[serde(alias = "project_id")]
    pub(crate) project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ImportTranscriptResult {
    pub(crate) name: String,
    pub(crate) title: String,
    pub(crate) people: Vec<String>,
    pub(crate) memo_path: String,
    pub(crate) transcript_path: String,
    pub(crate) capture_context_path: String,
    pub(crate) transcript_count: usize,
    pub(crate) duration_secs: f64,
}

pub(crate) fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Session name cannot be empty".into());
    }
    let valid = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if !valid || name.contains("..") || name.starts_with('-') || name.ends_with('-') {
        return Err("Use lowercase letters, numbers, and single dashes for session names".into());
    }
    Ok(())
}

pub(crate) fn unique_session_name(work_dir: &Path, margins_dir: &Path, requested: &str) -> String {
    let base = requested.trim_matches('-').trim();
    let base = if base.is_empty() { "meeting" } else { base };
    for ordinal in 1..1000 {
        let candidate = ordinal_session_name(base, ordinal);
        if !session::session_exists(margins_dir, &candidate).unwrap_or(false)
            && !session_artifact_exists(work_dir, margins_dir, &candidate)
        {
            return candidate;
        }
    }
    ordinal_session_name(base, Local::now().timestamp() as usize)
}

fn ordinal_session_name(base: &str, ordinal: usize) -> String {
    let suffix = if ordinal <= 1 {
        String::new()
    } else {
        format!("-{ordinal}")
    };
    let max_base_len = 90usize.saturating_sub(suffix.len());
    let mut stem = base.chars().take(max_base_len).collect::<String>();
    stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        stem = "meeting".to_string();
    }
    format!("{stem}{suffix}")
}

fn session_artifact_exists(work_dir: &Path, margins_dir: &Path, name: &str) -> bool {
    let candidates = [
        margins_dir.join(format!("{name}.md")),
        work_dir.join(format!("{name}.md")),
        session_transcript_artifact_path(margins_dir, name),
        session_capture_context_scratch_path(margins_dir, name),
        margins_dir.join(format!("{name}_transcript.json")),
        margins_dir.join(format!("{name}_aligned.md")),
        margins_dir.join(format!("{name}_capture_context.md")),
        margins_dir
            .join("recordings")
            .join(format!("{name}_seg0.wav")),
        margins_dir.join(format!("{name}_seg0.wav")),
    ];
    candidates.iter().any(|path| path.exists())
}

/// On-disk path to a session's timestamped memo markdown. New sessions keep the
/// memo inside `.margins/` so the notes folder only ever receives finished notes;
/// older sessions stored it loose in the work dir. Prefer the new location, but
/// fall back to the legacy path when only it exists so pre-existing captures
/// keep loading.
pub(crate) fn session_memo_path(work_dir: &Path, name: &str) -> PathBuf {
    let nested = work_dir.join(".margins").join(format!("{name}.md"));
    let legacy = work_dir.join(format!("{name}.md"));
    if !nested.exists() && legacy.exists() {
        legacy
    } else {
        nested
    }
}

fn session_artifact_dir(margins_dir: &Path, session_name: &str) -> PathBuf {
    margins_dir.join("artifacts").join(session_name)
}

fn session_transcript_artifact_path(margins_dir: &Path, session_name: &str) -> PathBuf {
    session_artifact_dir(margins_dir, session_name).join("transcript.md")
}

fn session_capture_context_scratch_path(margins_dir: &Path, session_name: &str) -> PathBuf {
    session_artifact_dir(margins_dir, session_name)
        .join("scratch")
        .join("capture-context.md")
}

fn recording_segment_rel_path(session_name: &str, segment_index: i64) -> String {
    format!(".margins/recordings/{session_name}_seg{segment_index}.wav")
}

fn recording_combined_rel_path(session_name: &str) -> String {
    format!(".margins/recordings/{session_name}_combined.wav")
}

fn session_transcript_artifact_registry_path(session_name: &str) -> String {
    format!(".margins/artifacts/{session_name}/transcript.md")
}

fn session_capture_context_scratch_registry_path(session_name: &str) -> String {
    format!(".margins/artifacts/{session_name}/scratch/capture-context.md")
}

fn legacy_aligned_sidecar_path(margins_dir: &Path, session_name: &str) -> PathBuf {
    margins_dir.join(format!("{session_name}_aligned.md"))
}

fn legacy_capture_context_sidecar_path(margins_dir: &Path, session_name: &str) -> PathBuf {
    margins_dir.join(format!("{session_name}_capture_context.md"))
}

fn temporary_artifact_expires_at() -> String {
    (Local::now() + ChronoDuration::days(7)).to_rfc3339()
}

fn register_transcript_artifact(margins_dir: &Path, session_name: &str) -> Result<(), String> {
    session::upsert_session_artifact(
        margins_dir,
        session_name,
        session::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &session_transcript_artifact_registry_path(session_name),
        "durable",
        None,
    )
    .map_err(|e| format!("failed to register transcript artifact: {e}"))
}

fn register_capture_context_scratch_artifact(
    margins_dir: &Path,
    session_name: &str,
) -> Result<(), String> {
    let expires_at = temporary_artifact_expires_at();
    session::upsert_session_artifact(
        margins_dir,
        session_name,
        "capture_context",
        0,
        &session_capture_context_scratch_registry_path(session_name),
        "temporary",
        Some(&expires_at),
    )
    .map_err(|e| format!("failed to register capture context scratch artifact: {e}"))
}

fn read_session_transcript_artifact(margins_dir: &Path, session_name: &str) -> Option<String> {
    let registered_paths = session::list_session_artifacts(margins_dir, session_name)
        .unwrap_or_default()
        .into_iter()
        .filter(|artifact| artifact.kind == session::SESSION_ARTIFACT_KIND_TRANSCRIPT)
        .filter_map(|artifact| confined_artifact_registry_disk_path(margins_dir, &artifact.path));
    let fallback_paths = [
        session_transcript_artifact_path(margins_dir, session_name),
        legacy_aligned_sidecar_path(margins_dir, session_name),
    ];
    for path in registered_paths.chain(fallback_paths) {
        if let Ok(content) = std::fs::read_to_string(path) {
            if !content.trim().is_empty() {
                return Some(content);
            }
        }
    }
    None
}

fn session_transcript_checkpoint_session_name(content: &str) -> Option<&str> {
    content.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("Session: `")?;
        let end = rest.find('`')?;
        Some(&rest[..end])
    })
}

fn is_valid_session_transcript_checkpoint(content: &str, session_name: &str) -> bool {
    !content.trim().is_empty()
        && session_transcript_checkpoint_session_name(content) == Some(session_name)
        && transcript_count_in_markdown(content) > 0
}

/// Validation for a *terminal* offline/headless transcript — the last-resort
/// record for a capture. Unlike a live checkpoint (only worth *reusing* when it
/// holds real speech, gated by `is_valid_session_transcript_checkpoint`), a
/// terminal offline pass is the genuine record for the session even when it
/// found no speech and carries no memo: a user who only jots marks, a memo-only
/// capture, *or a near-silent room* must still get a note rather than a hard
/// "did not validate" failure.
///
/// Genuine transcription failures (missing models, unreadable audio) never reach
/// this check — `build_session_aligned{,_headless}` returns `Err` first — so a
/// well-formed artifact for this session is, by construction, a real terminal
/// record. We therefore validate only genuineness: non-empty content whose
/// `Session:` header names this session. This is the fix for the
/// silent / memo-only / capture-without-model finish flow.
fn is_valid_terminal_transcript_checkpoint(content: &str, session_name: &str) -> bool {
    !content.trim().is_empty()
        && session_transcript_checkpoint_session_name(content) == Some(session_name)
}

fn read_valid_session_transcript_checkpoint(
    margins_dir: &Path,
    session_name: &str,
) -> Option<String> {
    let path = session_transcript_artifact_path(margins_dir, session_name);
    let content = std::fs::read_to_string(path).ok()?;
    is_valid_session_transcript_checkpoint(&content, session_name).then_some(content)
}

fn write_atomic_utf8(path: &Path, content: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid path {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| {
        format!(
            "failed to create artifact directory {}: {e}",
            parent.display()
        )
    })?;
    let temp = parent.join(format!(
        ".{}.tmp-{}-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("artifact"),
        std::process::id(),
        Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::write(&temp, content)
        .map_err(|e| format!("failed to write temporary artifact {}: {e}", temp.display()))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!(
            "failed to publish artifact {} from {}: {e}",
            path.display(),
            temp.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_live_checkpoint_fires_at_three_minutes_since_last_success() {
        assert!(!live_transcript_checkpoint_due(179_999, 0));
        assert!(live_transcript_checkpoint_due(180_000, 0));
        assert!(!live_transcript_checkpoint_due(359_999, 180_000));
        assert!(live_transcript_checkpoint_due(360_000, 180_000));
    }

    #[test]
    fn legacy_capture_device_fallback_event_includes_null_requested_uid() {
        assert_eq!(
            legacy_capture_device_fallback_event("Missing Mic".into(), "Built-in Mic".into()),
            json!({
                "state": "fallback",
                "requested_uid": null,
                "requested_name": "Missing Mic",
                "device_name": "Built-in Mic",
            })
        );
    }

    #[test]
    fn granola_import_cannot_report_success_after_index_refresh_failure() {
        let committed = granola_mcp::GranolaRemoteImportResult {
            imported_count: 1,
            note_paths: Vec::new(),
            people_created: 0,
            organizations_created: 0,
            warnings: Vec::new(),
            transcripts_plan_gated: false,
        };
        let error = finish_granola_import_after_index_refresh(
            committed,
            Err("https://sentinel.invalid/provider detail".to_string()),
        )
        .unwrap_err();
        assert!(!error.contains("sentinel.invalid"));
        let error: serde_json::Value = serde_json::from_str(&error).unwrap();
        assert_eq!(error["code"], "granola_index_refresh_failed");
        assert_eq!(error["stage"], "index_refresh");
        assert_eq!(error["retryable"], false);
    }

    #[test]
    fn memo_mutations_reject_a_prior_capture_identity() {
        assert!(ensure_memo_session("capture-a", "capture-a").is_ok());
        assert_eq!(
            ensure_memo_session("capture-a", "capture-b").unwrap_err(),
            "Stale memo mutation for capture-a; active capture is capture-b"
        );
    }

    fn healthy_qualification() -> LiveTranscriptQualification<'static> {
        LiveTranscriptQualification {
            transcript: "[00:01] user: stable words",
            mic_audio_end_ms: 188_922,
            system_audio_end_ms: 188_880,
            mic_decoded_until_ms: 188_922,
            system_decoded_until_ms: 188_880,
            system_audio_expected: true,
            system_audio_seen: true,
            terminal_journal_durable: true,
            mic_frame_coverage: Some(0.999),
            system_frame_coverage: Some(0.998),
        }
    }

    #[test]
    fn healthy_terminal_rolling_capture_qualifies_after_pause_at_finish() {
        // The E2E wall clock ended around 248.7s because Finish was invoked
        // while paused. Qualification uses the 188.9s channel endpoints and
        // therefore proves the terminal drain rather than rejecting pause time.
        assert!(live_transcript_reuse_rejections(&healthy_qualification()).is_empty());
    }

    #[test]
    fn live_transcript_reuse_tolerates_marginal_coverage_but_rejects_collapse() {
        assert_eq!(delivered_sample_coverage(995, 5), Some(0.995));
        assert_eq!(delivered_sample_coverage(0, 0), None);

        // An unrecorded coverage figure is uncertainty, not known loss: reuse.
        let mut input = healthy_qualification();
        input.system_frame_coverage = None;
        assert!(live_transcript_reuse_rejections(&input).is_empty());

        // Coverage a hair under the old 99.5% bar is now cosmetic: reuse.
        input.system_frame_coverage = Some(0.994);
        assert!(live_transcript_reuse_rejections(&input).is_empty());

        // Only a pathological collapse below the catastrophic floor rejects.
        input.system_frame_coverage = Some(0.80);
        let collapsed = live_transcript_reuse_rejections(&input);
        assert!(collapsed
            .iter()
            .any(|reason| reason.contains("delivery collapsed")));
    }

    #[test]
    fn qualification_tolerates_marginal_lag_and_rejects_catastrophic_tail() {
        // Sub-second decode lag and scattered drops recover nothing a human
        // would notice from the same WAV, so the transcript is reused.
        let mut input = healthy_qualification();
        input.mic_decoded_until_ms = input.mic_audio_end_ms - 81;
        assert!(live_transcript_reuse_rejections(&input).is_empty());

        // A large contiguous undecoded tail means the end was never
        // transcribed live; re-decoding the WAV recovers it.
        input.mic_decoded_until_ms = input.mic_audio_end_ms - (CATASTROPHIC_TAIL_GAP_MS + 1);
        let reasons = live_transcript_reuse_rejections(&input);
        assert!(reasons
            .iter()
            .any(|reason| reason.contains("never caught up to captured audio")));
    }

    #[test]
    fn mic_holding_keeps_periodic_checkpoints_eligible() {
        assert!(periodic_live_checkpoint_allowed(
            false,
            RecordingPhase::Recording,
            recording::SegmentLifecycle::Running,
        ));
        assert!(!periodic_live_checkpoint_allowed(
            true,
            RecordingPhase::Paused,
            recording::SegmentLifecycle::Paused,
        ));
        assert!(!periodic_live_checkpoint_allowed(
            false,
            RecordingPhase::Recording,
            recording::SegmentLifecycle::Failed,
        ));
    }

    #[test]
    fn workspace_has_no_coreaudio_default_input_write() {
        fn visit(path: &Path, findings: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(path) else {
                return;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if !matches!(
                        path.file_name().and_then(|name| name.to_str()),
                        Some(".git" | "target" | "node_modules")
                    ) {
                        visit(&path, findings);
                    }
                    continue;
                }
                if !matches!(
                    path.extension().and_then(|extension| extension.to_str()),
                    Some("rs" | "c" | "h" | "m" | "mm" | "swift")
                ) {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let lines: Vec<_> = source.lines().collect();
                for (index, window) in lines.windows(12).enumerate() {
                    let text = window.join("\n");
                    let default_input_selector = ["HW_DEFAULT_", "INPUT_DEVICE"].concat();
                    let c_selector = ["kAudioHardwarePropertyDefault", "InputDevice"].concat();
                    let writes_property = [
                        ["AudioObject", "SetPropertyData"].concat(),
                        ["set_", "prop"].concat(),
                        ["set", "_property"].concat(),
                    ]
                    .iter()
                    .any(|setter| text.contains(setter));
                    if writes_property
                        && (text.contains(&default_input_selector) || text.contains(&c_selector))
                    {
                        findings.push(format!("{}:{}", path.display(), index + 1));
                    }
                }
            }
        }

        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut findings = Vec::new();
        visit(&workspace.join("src"), &mut findings);
        visit(&workspace.join("desktop/src-tauri/src"), &mut findings);
        assert!(
            findings.is_empty(),
            "CoreAudio default-input property write found at {findings:?}"
        );
    }

    #[test]
    fn qualification_tolerates_scattered_live_queue_drops() {
        // Scattered live-queue loss that keeps delivery above the catastrophic
        // floor is cosmetic — the distiller treats the sidecar as partial — so
        // the live transcript is reused rather than re-transcribed.
        let mut input = healthy_qualification();
        input.system_frame_coverage = Some(0.999_999);

        assert!(live_transcript_reuse_rejections(&input).is_empty());
    }

    #[test]
    fn mic_only_capture_does_not_require_unexpected_system_audio() {
        let mut input = healthy_qualification();
        input.system_audio_expected = false;
        input.system_audio_seen = false;
        input.system_audio_end_ms = 0;
        input.system_decoded_until_ms = 0;
        input.system_frame_coverage = None;

        assert!(live_transcript_reuse_rejections(&input).is_empty());
    }

    #[test]
    fn expected_but_unobserved_system_audio_still_rejects_reuse() {
        let mut input = healthy_qualification();
        input.system_audio_seen = false;
        let reasons = live_transcript_reuse_rejections(&input);
        assert!(reasons
            .iter()
            .any(|reason| reason == "required system-audio channel was not observed"));
    }

    #[test]
    fn process_session_resolver_reuses_auto_and_refreshes_explicit_speaker_counts() {
        initialize_test_sqlite_runtime();
        let root = std::env::temp_dir().join(format!(
            "margins-process-session-speakers-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let margins_dir = root.join(".margins");
        let name = "speaker-branch";
        std::fs::create_dir_all(&margins_dir).unwrap();
        session::create_session(
            &margins_dir,
            name,
            &Local::now(),
            &format!(".margins/{name}.md"),
        )
        .unwrap();
        write_qualified_live_transcript_artifact(
            &margins_dir,
            name,
            "",
            "[00:01] you (mic): reusable live words",
        )
        .unwrap();

        let mut refresh_calls = 0;
        let auto =
            resolve_process_session_transcript(&margins_dir, name, Some(false), None, || {
                refresh_calls += 1;
                Ok("offline refresh should not run".to_string())
            })
            .unwrap();
        assert_eq!(
            auto.source,
            ProcessSessionTranscriptSource::ReusedCheckpoint
        );
        assert!(auto.content.contains("Transcript source: `live_qualified`"));
        assert_eq!(refresh_calls, 0);

        for explicit_count in [1, 2, 3, 8] {
            let refreshed = resolve_process_session_transcript(
                &margins_dir,
                name,
                Some(false),
                Some(explicit_count),
                || {
                    refresh_calls += 1;
                    Ok(format!("offline refresh for {explicit_count} speakers"))
                },
            )
            .unwrap();
            assert_eq!(
                refreshed.source,
                ProcessSessionTranscriptSource::RefreshedOffline
            );
            assert_eq!(
                refreshed.content,
                format!("offline refresh for {explicit_count} speakers")
            );
        }
        assert_eq!(refresh_calls, 4);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn qualified_live_transcript_retains_audio_fallback() {
        initialize_test_sqlite_runtime();
        let root = std::env::temp_dir().join(format!(
            "margins-live-audio-retention-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let margins_dir = root.join(".margins");
        let recordings = margins_dir.join("recordings");
        std::fs::create_dir_all(&recordings).unwrap();
        session::create_session(
            &margins_dir,
            "live-retention",
            &Local::now(),
            ".margins/live-retention.md",
        )
        .unwrap();
        session::add_segment(
            &margins_dir,
            "live-retention",
            0,
            ".margins/recordings/live-retention_seg0.wav",
            0,
            Some(1.0),
        )
        .unwrap();
        let audio = recordings.join("live-retention_seg0.wav");
        std::fs::write(&audio, b"fallback").unwrap();
        let transcript = session_transcript_artifact_path(&margins_dir, "live-retention");
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(
            &transcript,
            "# Transcript\n\nSession: `live-retention`\nTranscript source: `live_qualified`\n\n[00:01] user: words\n",
        )
        .unwrap();
        let meta = session::get_session_meta(&margins_dir, "live-retention").unwrap();

        delete_audio_after_aligned_transcript(&root, &margins_dir, "live-retention", &meta);
        assert!(audio.exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn qualified_live_transcript_is_registered_as_reusable_checkpoint() {
        initialize_test_sqlite_runtime();
        let root = std::env::temp_dir().join(format!(
            "margins-live-checkpoint-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        session::create_session(
            &margins_dir,
            "live-checkpoint",
            &Local::now(),
            ".margins/live-checkpoint.md",
        )
        .unwrap();

        write_qualified_live_transcript_artifact(
            &margins_dir,
            "live-checkpoint",
            "[00:02] memo: remember this",
            "[00:01] you (mic): hello\n[00:03] them (system): hi",
        )
        .unwrap();
        let content = read_valid_session_transcript_checkpoint(&margins_dir, "live-checkpoint")
            .expect("qualified live transcript should be reusable");
        assert!(content.contains("Transcript source: `live_qualified`"));
        assert!(content.contains("you (mic): hello"));
        assert!(content.contains("them (system): hi"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn native_coreml_rolling_fixture() -> Option<(std::path::PathBuf, Vec<f32>)> {
        let wav = std::env::var_os("MARGINS_COREML_ROLLING_WAV")
            .or_else(|| std::env::var_os("MARGINS_COREML_SMOKE_WAV"))
            .map(std::path::PathBuf::from);
        let Some(wav) = wav else {
            eprintln!(
                "skipping: set MARGINS_COREML_ROLLING_WAV (or MARGINS_COREML_SMOKE_WAV) to a spoken mono WAV"
            );
            return None;
        };
        if !wav.exists() {
            eprintln!(
                "skipping: CoreML rolling WAV does not exist: {}",
                wav.display()
            );
            return None;
        }
        let audio = margins::audio_pipeline::mono_16k_from_wav(&wav)
            .unwrap_or_else(|error| panic!("could not load {}: {error}", wav.display()));
        Some((wav, audio))
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn native_coreml_harness_root(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "margins-native-coreml-rolling-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    /// Wait until the live backchannel handle signals ready (A2 compile helper).
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn wait_until_ready(
        handle: &live_backchannel::LiveBackchannelHandle,
        timeout: std::time::Duration,
    ) {
        let started = std::time::Instant::now();
        while !handle.is_ready() {
            if started.elapsed() >= timeout {
                panic!("Timed out waiting for live backchannel worker to become ready");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn transcript_line_count(transcript: &str) -> usize {
        transcript
            .lines()
            .filter(|line| parse_context_line_ms(line.trim()).is_some())
            .count()
    }

    /// Drives the actual macOS CoreML live worker through its bounded capture
    /// queue. This is deliberately separate from the HTTP/ONNX PCM injector:
    /// it exercises rolling updates, final drain, terminal journaling, and the
    /// exact qualification/reuse decision used by native Finish.
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    #[test]
    #[ignore = "requires local FluidAudio CoreML assets and MARGINS_COREML_ROLLING_WAV"]
    fn native_coreml_rolling_wav_injector_exercises_finish_reuse_and_drop_rejection() {
        initialize_test_sqlite_runtime();
        let Some((wav, audio)) = native_coreml_rolling_fixture() else {
            return;
        };
        assert!(
            audio.len() >= 6 * margins::coreml_asr::SAMPLE_RATE as usize,
            "fixture must contain at least six seconds of speech for an incremental checkpoint"
        );
        // This affects only the cfg(test) hook above. Production capture remains
        // at the three-minute checkpoint cadence.
        TEST_LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS.store(3_000, Ordering::Relaxed);

        let sample_rate = margins::coreml_asr::SAMPLE_RATE as usize;
        // Two order-faithful, accelerated live chunks: the first crosses the
        // shortened checkpoint boundary; the second carries the terminal tail.
        let checkpoint_samples = (4_640 * sample_rate / 1_000).min(audio.len() - sample_rate);
        let checkpoint_ms = checkpoint_samples as u64 * 1_000 / sample_rate as u64;
        let total_ms = audio.len() as u64 * 1_000 / sample_rate as u64;
        assert!(live_transcript_checkpoint_due(checkpoint_ms, 0));

        let root = native_coreml_harness_root("healthy");
        let margins_dir = root.join(".margins");
        let session_name = "native-coreml-rolling-healthy";
        std::fs::create_dir_all(&margins_dir).unwrap();
        session::create_session(
            &margins_dir,
            session_name,
            &Local::now(),
            &format!(".margins/{session_name}.md"),
        )
        .unwrap();

        let settings = Settings::default();
        let handle = live_backchannel::start_live_backchannel(
            &settings,
            margins_dir.clone(),
            session_name.to_string(),
            LiveTranscriptionMode::StereoSplit,
        )
        .expect("native CoreML worker should start")
        .expect("local CoreML assets should be available");
        assert!(
            handle.system_audio_expected(),
            "StereoSplit topology must require system audio even when UI readiness is false"
        );
        // A2: wait for worker to signal ready before injecting audio.
        wait_until_ready(&handle, crate::live_asr_worker::STARTUP_TIMEOUT);
        let client = handle.client();
        let started = Instant::now();
        for (channel, samples) in [
            (
                recorder::LiveAudioChannel::Mic,
                audio[..checkpoint_samples].to_vec(),
            ),
            (
                recorder::LiveAudioChannel::System,
                audio[..checkpoint_samples].to_vec(),
            ),
        ] {
            handle.inject_test_audio(channel, samples).unwrap();
        }
        let checkpoint = client
            .checkpoint_memo(checkpoint_ms)
            .expect("checkpoint snapshot should complete");
        let checkpoint_words = transcript_line_count(&checkpoint.transcript);
        assert!(
            checkpoint_words > 0,
            "rolling update must emit spoken words before terminal Finish"
        );
        let checkpoint_journal_started = Instant::now();
        transcript_store::append_live_transcript_segment(
            &margins_dir,
            session_name,
            "checkpoint",
            Some(0),
            Some("00:04"),
            &checkpoint,
        )
        .expect("checkpoint journal must be durable");
        let checkpoint_journal_ms = checkpoint_journal_started.elapsed().as_millis();

        for (channel, samples) in [
            (
                recorder::LiveAudioChannel::Mic,
                audio[checkpoint_samples..].to_vec(),
            ),
            (
                recorder::LiveAudioChannel::System,
                audio[checkpoint_samples..].to_vec(),
            ),
        ] {
            handle.inject_test_audio(channel, samples).unwrap();
        }
        let terminal = handle
            .finish_and_join(total_ms)
            .expect("terminal native CoreML drain should complete")
            .expect("native CoreML worker should return a terminal snapshot");
        let terminal_journal_started = Instant::now();
        let terminal_journal_durable = transcript_store::append_live_transcript_segment(
            &margins_dir,
            session_name,
            "final",
            None,
            None,
            &terminal.context,
        )
        .is_ok();
        let terminal_journal_ms = terminal_journal_started.elapsed().as_millis();
        assert!(terminal_journal_durable);
        let healthy_rejections = live_transcript_reuse_rejections(&LiveTranscriptQualification {
            transcript: &terminal.context.transcript,
            mic_audio_end_ms: terminal.mic_audio_end_ms,
            system_audio_end_ms: terminal.system_audio_end_ms,
            mic_decoded_until_ms: terminal.mic_decoded_until_ms,
            system_decoded_until_ms: terminal.system_decoded_until_ms,
            system_audio_expected: true,
            system_audio_seen: true,
            terminal_journal_durable,
            mic_frame_coverage: delivered_sample_coverage(
                terminal.context.mic_accepted_samples,
                terminal.context.mic_dropped_samples,
            ),
            system_frame_coverage: delivered_sample_coverage(
                terminal.context.system_accepted_samples,
                terminal.context.system_dropped_samples,
            ),
        });
        assert!(
            healthy_rejections.is_empty(),
            "healthy native terminal snapshot unexpectedly rejected: {healthy_rejections:?}"
        );
        assert!(
            terminal
                .mic_audio_end_ms
                .saturating_sub(terminal.mic_decoded_until_ms)
                <= 80
        );
        assert!(
            terminal
                .system_audio_end_ms
                .saturating_sub(terminal.system_decoded_until_ms)
                <= 80
        );
        assert!(terminal.context.hypothesis_transcript.trim().is_empty());

        write_qualified_live_transcript_artifact(
            &margins_dir,
            session_name,
            "",
            &terminal.context.transcript,
        )
        .unwrap();
        let reusable = read_valid_session_transcript_checkpoint(&margins_dir, session_name)
            .expect("qualified native terminal transcript should be reusable");
        assert!(reusable.contains("Transcript source: `live_qualified`"));
        let auto_resolution = resolve_process_session_transcript(
            &margins_dir,
            session_name,
            Some(false),
            None,
            || panic!("Auto must reuse the qualified native transcript"),
        )
        .unwrap();
        assert_eq!(
            auto_resolution.source,
            ProcessSessionTranscriptSource::ReusedCheckpoint
        );
        let explicit_resolution = resolve_process_session_transcript(
            &margins_dir,
            session_name,
            Some(false),
            Some(2),
            || Ok("offline refresh selected".to_string()),
        )
        .unwrap();
        assert_eq!(
            explicit_resolution.source,
            ProcessSessionTranscriptSource::RefreshedOffline
        );

        let journal = std::fs::read_to_string(
            margins_dir.join(format!("{session_name}_live_transcript_segments.jsonl")),
        )
        .unwrap();
        assert!(journal
            .lines()
            .any(|line| line.contains("\"kind\":\"checkpoint\"")));
        assert!(journal
            .lines()
            .any(|line| line.contains("\"kind\":\"final\"")));
        let trace = std::fs::read_to_string(
            margins_dir.join(format!("{session_name}_backchannel_trace.jsonl")),
        )
        .unwrap();
        let background_updates = trace
            .lines()
            .filter(|line| line.contains("\"kind\":\"live_transcript_background_decode\""))
            .count();
        assert!(
            background_updates >= 2,
            "expected two rolling background decodes"
        );
        eprintln!(
            "native CoreML rolling healthy: wav={} checkpoint_words={} terminal_words={} checkpoint_audio_ms={} terminal_audio_ms={} checkpoint_journal_ms={} terminal_journal_ms={} mic={}/{}ms system={}/{}ms background_updates={} elapsed_ms={} qualified=true rejections=[] auto=live_qualified explicit_speakers=offline_refresh",
            wav.display(),
            checkpoint_words,
            transcript_line_count(&terminal.context.transcript),
            checkpoint_ms,
            total_ms,
            checkpoint_journal_ms,
            terminal_journal_ms,
            terminal.mic_decoded_until_ms,
            terminal.mic_audio_end_ms,
            terminal.system_decoded_until_ms,
            terminal.system_audio_end_ms,
            background_updates,
            started.elapsed().as_millis(),
        );

        // A second, shorter real worker run injects a single observed
        // capture-side loss before Finish. It stays reusable: the loss is
        // scattered and far above the catastrophic floor, and CoreML drains all
        // audio that did arrive.
        let degraded_root = native_coreml_harness_root("drop");
        let degraded_margins = degraded_root.join(".margins");
        let degraded_name = "native-coreml-rolling-drop";
        std::fs::create_dir_all(&degraded_margins).unwrap();
        let degraded = live_backchannel::start_live_backchannel(
            &settings,
            degraded_margins.clone(),
            degraded_name.to_string(),
            LiveTranscriptionMode::StereoSplit,
        )
        .unwrap()
        .expect("native CoreML assets should still be available");
        wait_until_ready(&degraded, crate::live_asr_worker::STARTUP_TIMEOUT);
        for channel in [
            recorder::LiveAudioChannel::Mic,
            recorder::LiveAudioChannel::System,
        ] {
            degraded
                .inject_test_audio(channel, audio[..checkpoint_samples].to_vec())
                .unwrap();
        }
        // A single scattered queue-dropped sample keeps aggregate delivery far
        // above the catastrophic floor. Under the catastrophic-only policy the
        // offline pass could not recover anything a human would notice, so this
        // run stays reusable rather than paying for a re-transcription.
        degraded.inject_test_drop(recorder::LiveAudioChannel::System, 1);
        let degraded_terminal = degraded
            .finish_and_join(checkpoint_ms)
            .unwrap()
            .expect("degraded native worker should still return a terminal snapshot");
        let degraded_journal_durable = transcript_store::append_live_transcript_segment(
            &degraded_margins,
            degraded_name,
            "final",
            None,
            None,
            &degraded_terminal.context,
        )
        .is_ok();
        let degraded_rejections = live_transcript_reuse_rejections(&LiveTranscriptQualification {
            transcript: &degraded_terminal.context.transcript,
            mic_audio_end_ms: degraded_terminal.mic_audio_end_ms,
            system_audio_end_ms: degraded_terminal.system_audio_end_ms,
            mic_decoded_until_ms: degraded_terminal.mic_decoded_until_ms,
            system_decoded_until_ms: degraded_terminal.system_decoded_until_ms,
            system_audio_expected: true,
            system_audio_seen: true,
            terminal_journal_durable: degraded_journal_durable,
            mic_frame_coverage: delivered_sample_coverage(
                degraded_terminal.context.mic_accepted_samples,
                degraded_terminal.context.mic_dropped_samples,
            ),
            system_frame_coverage: delivered_sample_coverage(
                degraded_terminal.context.system_accepted_samples,
                degraded_terminal.context.system_dropped_samples,
            ),
        });
        assert!(
            degraded_rejections.is_empty(),
            "single scattered drop must stay reusable under the catastrophic-only gate: {degraded_rejections:?}"
        );
        eprintln!(
            "native CoreML rolling degraded: terminal_words={} system_coverage={:?} rejections={degraded_rejections:?}",
            transcript_line_count(&degraded_terminal.context.transcript),
            delivered_sample_coverage(
                degraded_terminal.context.system_accepted_samples,
                degraded_terminal.context.system_dropped_samples,
            ),
        );

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(degraded_root).unwrap();
        TEST_LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS.store(0, Ordering::Relaxed);
    }

    #[test]
    fn ordinal_session_names_keep_the_base_then_add_numbers() {
        assert_eq!(ordinal_session_name("customer-call", 1), "customer-call");
        assert_eq!(ordinal_session_name("customer-call", 2), "customer-call-2");
        assert!(ordinal_session_name(&"a".repeat(90), 12).len() <= 90);
    }

    #[test]
    fn update_note_title_metadata_strips_yaml_quote_wrappers() {
        let path = std::env::temp_dir().join(format!(
            "margins-title-metadata-{}-{}.md",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::write(&path, "---\ntags:\n---\n# Body\n").unwrap();

        update_note_title_metadata(&path, r#""Customer Sync Recap""#).unwrap();

        let updated = std::fs::read_to_string(&path).unwrap();
        assert!(updated.contains("title: 'Customer Sync Recap'"));
        assert!(!updated.contains(r#"title: '\"Customer Sync Recap\"'"#));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn count_vault_notes_counts_md_files_and_respects_exclusions() {
        let tmp = std::env::temp_dir().join(format!(
            "margins-count-vault-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let write = |rel: &str| {
            let p = tmp.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "# note").unwrap();
        };
        // included
        write("a.md");
        write("sub/b.md");
        write("sub/nested/c.md");
        // excluded — enzyme/margins/obsidian/cache dirs
        write(".enzyme/index.md");
        write(".margins/session.md");
        write(".obsidian/config.md");
        write("caches/x.md");
        write("cache/y.md");
        // non-md file should not count
        write("readme.txt");

        let count = count_vault_notes_impl(&tmp);
        assert_eq!(count, 3, "expected only the three non-excluded .md files");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn count_vault_notes_caps_at_1000() {
        let tmp = std::env::temp_dir().join(format!(
            "margins-count-vault-cap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        for i in 0..1001u32 {
            std::fs::write(tmp.join(format!("{i}.md")), "").unwrap();
        }
        let count = count_vault_notes_impl(&tmp);
        assert_eq!(count, 1000, "count should cap at 1000");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn compatibility_recall_never_provisions_a_missing_index() {
        let tmp = std::env::temp_dir().join(format!(
            "margins-recall-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let notes = tmp.join("notes");
        std::fs::create_dir_all(&notes).unwrap();
        let workspace = margins_workflows::workspace::create_workspace(
            &tmp.join("margins-home"),
            "test",
            None,
            &notes,
        )
        .unwrap();
        let args = vec![
            "catalyze".to_string(),
            "--vault".to_string(),
            workspace.state_dir.to_string_lossy().to_string(),
            "--query".to_string(),
            "anything".to_string(),
        ];

        let result = recall_search_compat(&args);

        assert!(result.is_err(), "missing recall index must fail closed");
        assert!(!workspace.recall_path().exists());
        std::fs::remove_dir_all(tmp).unwrap();
    }

    fn legacy_pi_migration_fixture(
        tag: &str,
        entries: &[(i64, &str)],
    ) -> (PathBuf, PathBuf, String) {
        let root = std::env::temp_dir().join(format!(
            "margins-pi-session-migration-{tag}-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let legacy = root.join("legacy.sqlite");
        let header = json!({
            "type": "session",
            "version": 3,
            "id": format!("margins-legacy-{tag}"),
            "timestamp": "2026-08-22T00:00:00.000Z",
            "cwd": root.to_string_lossy(),
        })
        .to_string();
        let connection = rusqlite::Connection::open(&legacy).unwrap();
        connection
            .execute_batch(
                "
                CREATE TABLE pi_session_header (id TEXT PRIMARY KEY, json TEXT NOT NULL);
                CREATE TABLE pi_session_entries (seq INTEGER PRIMARY KEY, json TEXT NOT NULL);
                ",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO pi_session_header (id, json) VALUES (?1, ?2)",
                (format!("margins-legacy-{tag}"), &header),
            )
            .unwrap();
        for (seq, entry) in entries {
            connection
                .execute(
                    "INSERT INTO pi_session_entries (seq, json) VALUES (?1, ?2)",
                    (seq, entry),
                )
                .unwrap();
        }
        drop(connection);
        std::fs::write(
            pi_session_pointer_path(&root, "capture"),
            legacy.to_string_lossy().as_bytes(),
        )
        .unwrap();
        (root, legacy, header)
    }

    fn expected_legacy_jsonl(header: &str, entries: &[&str]) -> String {
        let mut expected = format!("{header}\n");
        for entry in entries {
            expected.push_str(entry);
            expected.push('\n');
        }
        expected
    }

    #[test]
    fn legacy_pi_sqlite_migrates_non_empty_entries_in_sequence_order_and_is_idempotent() {
        let first = r#"{"type":"message","id":"first"}"#;
        let second = r#"{"type":"message","id":"second"}"#;
        let (root, legacy, header) =
            legacy_pi_migration_fixture("ordered", &[(20, second), (10, first)]);
        let expected = expected_legacy_jsonl(&header, &[first, second]);

        let migrated = read_pi_session_file(&root, "capture")
            .unwrap()
            .expect("legacy pointer should resolve");

        assert_eq!(migrated, legacy.with_extension("jsonl"));
        assert!(
            legacy.is_file(),
            "migration must preserve the legacy database"
        );
        assert_eq!(std::fs::read_to_string(&migrated).unwrap(), expected);
        assert_eq!(
            std::fs::read_to_string(pi_session_pointer_path(&root, "capture"))
                .unwrap()
                .trim(),
            migrated.to_string_lossy()
        );
        assert_eq!(
            migrate_legacy_pi_session(&legacy).unwrap(),
            migrated,
            "a complete existing migration must be accepted idempotently"
        );
        assert_eq!(std::fs::read_to_string(&migrated).unwrap(), expected);

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_pi_migration_rejects_truncated_target_and_preserves_source_and_pointer() {
        let first = r#"{"type":"message","id":"first"}"#;
        let second = r#"{"type":"message","id":"second"}"#;
        let (root, legacy, header) =
            legacy_pi_migration_fixture("truncated", &[(10, first), (20, second)]);
        let target = legacy.with_extension("jsonl");
        let truncated = expected_legacy_jsonl(&header, &[first]);
        std::fs::write(&target, &truncated).unwrap();

        let error = read_pi_session_file(&root, "capture").unwrap_err();

        assert!(error.contains("differs from the complete legacy migration"));
        assert!(legacy.is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), truncated);
        assert_eq!(
            std::fs::read_to_string(pi_session_pointer_path(&root, "capture"))
                .unwrap()
                .trim(),
            legacy.to_string_lossy()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_pi_migration_rejects_divergent_target_with_matching_header() {
        let source = r#"{"type":"message","id":"source"}"#;
        let divergent = r#"{"type":"message","id":"other"}"#;
        let (root, legacy, header) = legacy_pi_migration_fixture("divergent", &[(10, source)]);
        let target = legacy.with_extension("jsonl");
        let conflicting = expected_legacy_jsonl(&header, &[divergent]);
        std::fs::write(&target, &conflicting).unwrap();

        let error = read_pi_session_file(&root, "capture").unwrap_err();

        assert!(error.contains("differs from the complete legacy migration"));
        assert!(legacy.is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), conflicting);
        assert_eq!(
            std::fs::read_to_string(pi_session_pointer_path(&root, "capture"))
                .unwrap()
                .trim(),
            legacy.to_string_lossy()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_pi_migration_rejects_malformed_target_and_preserves_pointer() {
        let source = r#"{"type":"message","id":"source"}"#;
        let (root, legacy, header) = legacy_pi_migration_fixture("malformed", &[(10, source)]);
        let target = legacy.with_extension("jsonl");
        let malformed = format!("{header}\n{{not-json\n");
        std::fs::write(&target, &malformed).unwrap();

        let error = read_pi_session_file(&root, "capture").unwrap_err();

        assert!(error.contains("invalid JSONL record 2"));
        assert!(legacy.is_file());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), malformed);
        assert_eq!(
            std::fs::read_to_string(pi_session_pointer_path(&root, "capture"))
                .unwrap()
                .trim(),
            legacy.to_string_lossy()
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_replace_publication_rejects_a_divergent_concurrent_winner() {
        let root = std::env::temp_dir().join(format!(
            "margins-pi-session-race-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("race.jsonl");
        let first = expected_legacy_jsonl(
            &json!({"type":"session","id":"first"}).to_string(),
            &[r#"{"type":"message","id":"first"}"#],
        );
        let second = expected_legacy_jsonl(
            &json!({"type":"session","id":"second"}).to_string(),
            &[r#"{"type":"message","id":"second"}"#],
        );
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let run = |expected: String| {
            let target = target.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                publish_migrated_pi_session(&target, &expected)
            })
        };
        let first_thread = run(first.clone());
        let second_thread = run(second.clone());
        barrier.wait();
        let results = [first_thread.join().unwrap(), second_thread.join().unwrap()];

        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(results.iter().filter(|result| result.is_err()).count(), 1);
        let published = std::fs::read_to_string(&target).unwrap();
        assert!(published == first || published == second);
        assert!(results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.contains("differs")));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn no_replace_publication_accepts_exact_concurrent_content_idempotently() {
        let root = std::env::temp_dir().join(format!(
            "margins-pi-session-idempotent-race-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let target = root.join("race.jsonl");
        let expected = expected_legacy_jsonl(
            &json!({"type":"session","id":"same"}).to_string(),
            &[r#"{"type":"message","id":"same"}"#],
        );
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let run = || {
            let target = target.clone();
            let expected = expected.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                publish_migrated_pi_session(&target, &expected)
            })
        };
        let first_thread = run();
        let second_thread = run();
        barrier.wait();

        first_thread.join().unwrap().unwrap();
        second_thread.join().unwrap().unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), expected);
        std::fs::remove_dir_all(root).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Commands: Settings
// ---------------------------------------------------------------------------

pub(crate) fn get_settings_impl(ctx: &ctx::Ctx) -> Settings {
    redacted_settings(&ctx.state.settings.lock().unwrap())
}

async fn save_settings_async(settings: &Settings) -> Result<(), String> {
    let settings = settings.clone();
    crate::async_runtime::spawn_blocking(move || save_settings(&settings))
        .await
        .map_err(|error| format!("settings save task failed: {error}"))?
}

async fn load_settings_async() -> Result<Settings, String> {
    crate::async_runtime::spawn_blocking(load_settings)
        .await
        .map_err(|error| format!("settings load task failed: {error}"))
}

pub(crate) async fn update_settings_impl(
    ctx: &ctx::Ctx,
    mut settings: Settings,
) -> Result<(), String> {
    normalize_settings(&mut settings);
    let settings_to_save = settings.clone();
    crate::async_runtime::spawn_blocking(move || {
        // The notes folder only has to exist at capture time, not at settings-save
        // time. Creating it here is best-effort so a not-yet-existing (or not-yet-
        // permitted) vault folder never fails the save with a phantom "os error 2".
        if let Err(e) = ensure_notes_destination(&settings_to_save) {
            eprintln!("margins: could not pre-create notes folder (will retry at capture): {e}");
        }
        save_settings(&settings_to_save)
    })
    .await
    .map_err(|error| format!("settings save task failed: {error}"))??;
    let settings = load_settings_async().await?;
    *ctx.state.settings.lock().unwrap() = settings.clone();
    refresh_backchannel_snapshot_settings(&ctx.state, &settings).await;
    Ok(())
}

pub(crate) async fn update_hosted_settings_impl(
    ctx: &ctx::Ctx,
    mut settings: Settings,
) -> Result<(), String> {
    settings::reject_hosted_settings_secrets(&settings)?;
    normalize_settings(&mut settings);
    let settings_to_save = settings.clone();
    crate::async_runtime::spawn_blocking(move || {
        if let Err(error) = ensure_notes_destination(&settings_to_save) {
            eprintln!(
                "margins: could not pre-create notes folder (will retry at capture): {error}"
            );
        }
        settings::save_settings_document(&settings_to_save)
    })
    .await
    .map_err(|error| format!("settings save task failed: {error}"))??;
    let settings = load_settings_async().await?;
    *ctx.state.settings.lock().unwrap() = settings.clone();
    refresh_backchannel_snapshot_settings(&ctx.state, &settings).await;
    Ok(())
}

pub(crate) async fn update_audio_settings_impl(
    ctx: &ctx::Ctx,
    audio_input_ready: bool,
    system_audio_ready: bool,
    input_device_mode: settings::InputDeviceMode,
    input_device_uid: Option<String>,
    input_device_name: Option<String>,
) -> Result<(), String> {
    let mut next = ctx.state.settings.lock().unwrap().clone();
    next.audio_input_ready = audio_input_ready;
    next.system_audio_ready = system_audio_ready;
    next.input_device_mode = input_device_mode;
    next.input_device_uid = input_device_uid;
    next.input_device_name = input_device_name;
    let to_save = next.clone();
    crate::async_runtime::spawn_blocking(move || settings::save_settings_document(&to_save))
        .await
        .map_err(|error| format!("audio settings save task failed: {error}"))??;
    *ctx.state.settings.lock().unwrap() = next;
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_settings(state: tauri::State<'_, Arc<AppState>>) -> Result<Settings, String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    Ok(get_settings_impl(&ctx))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn update_settings(
    state: tauri::State<'_, Arc<AppState>>,
    mut settings: Settings,
) -> Result<(), String> {
    normalize_settings(&mut settings);
    let settings_to_save = settings.clone();
    crate::async_runtime::spawn_blocking(move || {
        // Best-effort: the notes folder is required at capture time, not at
        // settings-save time. Never fail the save on a not-yet-created folder.
        if let Err(e) = ensure_notes_destination(&settings_to_save) {
            eprintln!("margins: could not pre-create notes folder (will retry at capture): {e}");
        }
        save_settings(&settings_to_save)
    })
    .await
    .map_err(|error| format!("settings save task failed: {error}"))??;
    let settings = load_settings_async().await?;
    *state.settings.lock().unwrap() = settings.clone();
    refresh_backchannel_snapshot_settings(&*state, &settings).await;
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn update_audio_settings(
    state: tauri::State<'_, Arc<AppState>>,
    audio_input_ready: bool,
    system_audio_ready: bool,
    input_device_mode: settings::InputDeviceMode,
    input_device_uid: Option<String>,
    input_device_name: Option<String>,
) -> Result<(), String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    update_audio_settings_impl(
        &ctx,
        audio_input_ready,
        system_audio_ready,
        input_device_mode,
        input_device_uid,
        input_device_name,
    )
    .await
}

#[derive(Serialize)]
struct RegisterProjectResult {
    settings: Settings,
    validation: Value,
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn register_project(
    _app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    _settings: Settings,
    project: ProjectSource,
) -> Result<RegisterProjectResult, String> {
    let root = clean_project_root(&project.path)?;
    if !root.exists() {
        return Err(format!("Project folder not found: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("Project path is not a folder: {}", root.display()));
    }

    let margins_dir = root.join(".margins");
    std::fs::create_dir_all(&margins_dir)
        .map_err(|e| format!("Could not create {}: {e}", margins_dir.display()))?;

    let upsert_input = UpsertProject {
        path: &project.path,
        name: Some(&project.name)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        inbox_folder: Some(&project.inbox_folder)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        people_folder: Some(&project.people_folder)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        readiness: Some(&project.readiness)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        id_hint: Some(&project.id)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
    };
    margins::project::upsert_project(&upsert_input)
        .map_err(|e| format!("Could not register project: {e}"))?;

    let settings = load_settings_async().await?;
    refresh_backchannel_snapshot_settings(&*state, &settings).await;
    *state.settings.lock().unwrap() = settings.clone();

    Ok(RegisterProjectResult {
        settings: redacted_settings(&settings),
        validation: project_validation(&root),
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn update_project_readiness(
    state: tauri::State<'_, Arc<AppState>>,
    id: String,
    readiness: String,
) -> Result<Settings, String> {
    let cleaned = readiness.trim();
    if !matches!(cleaned, "ready" | "needs_setup" | "updating" | "error") {
        return Err(format!("Unknown project readiness: {readiness}"));
    }

    let mut settings = state.settings.lock().unwrap().clone();
    let project = settings
        .projects
        .iter_mut()
        .find(|project| project.id == id)
        .ok_or_else(|| format!("Unknown project: {id}"))?;
    project.readiness = cleaned.to_string();
    normalize_settings(&mut settings);
    save_settings_async(&settings).await?;
    let settings = load_settings_async().await?;
    refresh_backchannel_snapshot_settings(&*state, &settings).await;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(redacted_settings(&settings))
}

fn clean_project_root(path: &str) -> Result<PathBuf, String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("Choose a project folder first.".to_string());
    }
    Ok(PathBuf::from(expand_tilde(path)))
}

fn project_validation(path: &Path) -> Value {
    json!({
        "exists": path.exists(),
        "has_obsidian": path.join(".obsidian").exists(),
        "has_recall_index": path.join(".margins").join("recall").join("index.db").exists(),
        "has_margins": path.join(".margins").exists(),
    })
}

async fn refresh_backchannel_snapshot_settings(state: &Arc<AppState>, settings: &Settings) {
    let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
        match configure_ai_for_backchannel(settings).await {
            Ok(config) => config,
            Err(error) => {
                ai_config::log_backchannel_snapshot_config_error(error).await;
                return;
            }
        };
    let vault_path = vault_root(settings);
    let mut snapshots = state.backchannel_snapshots.lock().unwrap();
    for snapshot in snapshots.values_mut() {
        snapshot.request.ai_provider = ai_provider.clone();
        snapshot.request.ai_model = ai_model.clone();
        snapshot.request.ai_api_key = ai_api_key.clone();
        snapshot.request.ai_credential_generation = ai_credential_generation;
        snapshot.request.instructions = settings.distill_instructions.clone();
        snapshot.request.vault_path = vault_path.clone();
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn get_ai_status() -> Result<AiStatus, String> {
    ai_auth::get_ai_status().await
}

#[derive(Serialize)]
struct IncludedAiStatus {
    included_ready: bool,
    message: String,
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_included_ai_status() -> Result<IncludedAiStatus, String> {
    let included_ready = ai_config::included_ai_key_ready();
    Ok(IncludedAiStatus {
        included_ready,
        message: if included_ready {
            "Included note-making is ready on this Mac.".to_string()
        } else {
            "Included note-making will create a usage-limited key before your first note."
                .to_string()
        },
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn prepare_included_ai() -> Result<IncludedAiStatus, String> {
    ai_config::prepare_included_ai_key().await?;
    get_included_ai_status()
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn sign_in_chatgpt() -> Result<AiStatus, String> {
    ai_auth::sign_in_chatgpt().await
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn get_ai_readiness() -> Result<ai_config::AiReadiness, String> {
    get_ai_readiness_impl().await
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn preview_ai_resolution(settings: Settings) -> Result<ai_config::ResolutionPreview, String> {
    preview_ai_resolution_impl(settings)
}

/// Walk a vault directory and count `.md` files, skipping enzyme/margins/obsidian dirs.
/// Returns at most 1000 (caller displays "999+" when the result equals 1000).
pub(crate) fn count_vault_notes_impl(root: &std::path::Path) -> u32 {
    const CAP: u32 = 1000;
    const SKIP: &[&str] = &[".margins", ".enzyme", ".obsidian", "caches", "cache"];
    let mut count = 0u32;
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if ft.is_dir() {
                if !SKIP.contains(&name_str.as_ref()) {
                    dirs.push(entry.path());
                }
            } else if ft.is_file() && name_str.ends_with(".md") {
                count += 1;
                if count >= CAP {
                    return CAP;
                }
            }
        }
    }
    count
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn count_vault_notes(path: String) -> u32 {
    let p = PathBuf::from(expand_tilde(&path));
    count_vault_notes_impl(&p)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn validate_vault(path: String) -> Result<serde_json::Value, String> {
    let p = PathBuf::from(expand_tilde(&path));
    let default_p = PathBuf::from(expand_tilde(&default_vault_path_string()));
    let can_create_default =
        p == default_p && p.parent().map(|parent| parent.exists()).unwrap_or(false);
    let usable = p.exists() || can_create_default;
    Ok(json!({
        "exists": usable,
        "has_obsidian": p.join(".obsidian").exists(),
        "has_recall_index": p.join(".margins").join("recall").join("index.db").exists(),
        "has_margins": p.join(".margins").exists(),
    }))
}

/// Convert a folder the user picked in the native dialog into the subfolder path
/// stored as `inbox_folder`, relative to the project root. Returns an empty
/// string when the picked folder *is* the root (captures land at the root), and
/// an error when the pick is outside the project so the caller can surface it.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn resolve_project_subfolder(project_root: String, picked: String) -> Result<String, String> {
    let root = PathBuf::from(expand_tilde(&project_root));
    let picked = PathBuf::from(expand_tilde(&picked));
    // The picked dir exists (the user just chose it); the root may not exist yet,
    // so fall back to the raw path when canonicalize fails.
    let root_c = std::fs::canonicalize(&root).unwrap_or(root);
    let picked_c = std::fs::canonicalize(&picked).unwrap_or(picked);
    if picked_c == root_c {
        return Ok(String::new());
    }
    match picked_c.strip_prefix(&root_c) {
        Ok(rel) => Ok(rel.to_string_lossy().replace('\\', "/")),
        Err(_) => Err("Choose a folder inside the project.".to_string()),
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn ensure_default_vault(path: String) -> Result<serde_json::Value, String> {
    let expanded = expand_tilde(&path);
    let p = PathBuf::from(&expanded);
    std::fs::create_dir_all(&p).map_err(|e| format!("Could not create notes folder: {e}"))?;
    Ok(json!({
        "exists": true,
        "has_obsidian": p.join(".obsidian").exists(),
        "has_recall_index": p.join(".margins").join("recall").join("index.db").exists(),
        "has_margins": p.join(".margins").exists(),
    }))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn index_vault(path: String) -> Result<serde_json::Value, String> {
    crate::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(expand_tilde(&path));
        run_enzyme_index_command(&p)?;
        Ok(json!({
            "exists": true,
            "has_obsidian": p.join(".obsidian").exists(),
            "has_recall_index": p.join(".margins").join("recall").join("index.db").exists(),
            "has_margins": p.join(".margins").exists(),
        }))
    })
    .await
    .map_err(|e| format!("index task failed: {e}"))?
}

/// Guards recall index writes. Provisioning (explicit init / post-distill refresh)
/// takes the write lock; read-only searches take the read lock, so many searches
/// run concurrently but never overlap a write on the same SQLite index.
#[cfg(feature = "recall")]
static RECALL_INDEX_LOCK: std::sync::RwLock<()> = std::sync::RwLock::new(());

/// Provision (build/refresh) the in-process recall index for `vault`. Enzyme's
/// resolver consumes only the setup-selected local model or hosted bundle.
/// This is the WRITE path used by explicit init and post-distill refresh.
#[cfg(feature = "recall")]
fn provision_recall(vault: &Path) -> Result<margins::recall::SearchHandle, String> {
    let _guard = RECALL_INDEX_LOCK.write().unwrap_or_else(|p| p.into_inner());
    let workspace = resolve_recall_workspace(vault)?;
    margins::recall::provision_workspace(&workspace)
        .map_err(|e| format!("recall provisioning failed: {e:#}"))
}

#[cfg(feature = "recall")]
fn resolve_recall_workspace(
    path: &Path,
) -> Result<margins_workflows::workspace::ResolvedWorkspace, String> {
    if let Ok(workspace) = margins_workflows::workspace::resolve_state_dir(path) {
        return Ok(workspace);
    }
    let home = margins_workflows::workspace::margins_home().map_err(|error| error.to_string())?;
    margins_workflows::workspace::resolve_workspace(&home, None, path)
        .map_err(|error| error.to_string())
}

/// In-process replacement for shelling `enzyme petri|catalyze`. Parses the legacy
/// arg vector, searches the recall index, and returns the SAME JSON envelope the
/// callers' existing parsers consume.
///
/// This compatibility search is read-only: missing indexes are reported to the
/// caller and are built only by explicit init or post-distill refresh.
#[cfg(feature = "recall")]
pub(crate) fn recall_search_compat(args: &[String]) -> Result<String, String> {
    let sub = args.first().map(String::as_str).unwrap_or("");
    let (mut vault, mut query) = (None::<String>, None::<String>);
    let (mut top, mut budget, mut limit) = (8usize, 2usize, 5usize);
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--vault" => {
                vault = args.get(i + 1).cloned();
                i += 2;
            }
            "--query" => {
                query = args.get(i + 1).cloned();
                i += 2;
            }
            "--top" => {
                top = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(top);
                i += 2;
            }
            "--catalyst-budget" => {
                budget = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(budget);
                i += 2;
            }
            "--limit" => {
                limit = args
                    .get(i + 1)
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(limit);
                i += 2;
            }
            other => {
                if !other.starts_with("--") && query.is_none() {
                    query = Some(other.to_string());
                }
                i += 1;
            }
        }
    }
    let vault = vault.ok_or_else(|| "recall: missing --vault".to_string())?;
    let query = query.unwrap_or_default();
    let vault_path = Path::new(&vault);
    let workspace = resolve_recall_workspace(vault_path)?;

    let handle = {
        let _read = RECALL_INDEX_LOCK.read().unwrap_or_else(|p| p.into_inner());
        margins::recall::open_workspace(&workspace)
            .map_err(|e| format!("recall open failed: {e:#}"))?
    }
    .ok_or_else(|| "recall index not established; run `margins init`".to_string())?;
    match sub {
        "petri" => handle
            .petri_json(&query, top, budget)
            .map_err(|e| e.to_string()),
        "catalyze" => handle
            .catalyze_json(&query, limit)
            .map_err(|e| e.to_string()),
        other => Err(format!("recall: unsupported subcommand '{other}'")),
    }
}

#[cfg(not(feature = "recall"))]
pub(crate) fn recall_search_compat(_args: &[String]) -> Result<String, String> {
    Err("recall is unavailable in this server build".to_string())
}

// Build/refresh the recall index in-process (formerly `enzyme init|refresh`).
// This is the WARM-PATH KEEPER: the background refresh runs it so a distillation
// search later opens the index read-only and never provisions on the hot path.
// Setup writes the selected local-model configuration or hosted bundle before
// this path runs; lookup never discovers or injects credentials.
#[cfg(feature = "recall")]
fn run_enzyme_index_command(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err("Project folder not found.".to_string());
    }
    provision_recall(path).map(|_| ())
}

#[cfg(not(feature = "recall"))]
fn run_enzyme_index_command(_path: &Path) -> Result<(), String> {
    Err("recall is unavailable in this server build".to_string())
}

fn project_id_for_refresh(
    settings: &Settings,
    project_id: Option<&str>,
    vault: &Path,
) -> Option<String> {
    project_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .or_else(|| project_id_for_work_dir(settings, vault))
        .or_else(|| settings_active_project(settings).map(|project| project.id.clone()))
}

#[cfg(feature = "tauri-app")]
fn schedule_background_enzyme_refresh(
    app: &AppHandle,
    settings: &Settings,
    project_id: Option<&str>,
) {
    let Some(vault) = vault_root(settings) else {
        return;
    };
    let resolved_project_id = project_id_for_refresh(settings, project_id, &vault);
    let key = resolved_project_id
        .clone()
        .unwrap_or_else(|| vault.to_string_lossy().to_string());
    let state = app.state::<Arc<AppState>>();
    {
        let mut refreshing = state.enzyme_refreshing.lock().unwrap();
        if let Some(refresh_again) = refreshing.get_mut(&key) {
            *refresh_again = true;
            return;
        }
        refreshing.insert(key.clone(), false);
    }

    let app = app.clone();
    crate::async_runtime::spawn(async move {
        let path_text = vault.to_string_lossy().to_string();
        let start_event = RecallIndexingEvent {
            project_id: resolved_project_id.clone(),
            path: path_text.clone(),
            status: "started".to_string(),
        };
        let _ = app.emit("recall-indexing", start_event);

        loop {
            let vault_for_task = vault.clone();
            let result = crate::async_runtime::spawn_blocking(move || {
                run_enzyme_index_command(&vault_for_task)
            })
            .await
            .map_err(|e| format!("recall refresh task failed: {e}"))
            .and_then(|result| result);

            if let Err(error) = result {
                eprintln!("margins: background recall refresh failed for {path_text}: {error}");
            }

            let state = app.state::<Arc<AppState>>();
            let mut refreshing = state.enzyme_refreshing.lock().unwrap();
            if refreshing.get(&key).copied().unwrap_or(false) {
                refreshing.insert(key.clone(), false);
                continue;
            }
            refreshing.remove(&key);
            break;
        }

        let finish_event = RecallIndexingEvent {
            project_id: resolved_project_id,
            path: path_text,
            status: "finished".to_string(),
        };
        let _ = app.emit("recall-indexing", finish_event);
    });
}

fn local_bin_dir() -> Result<PathBuf, String> {
    Ok(dirs::home_dir()
        .ok_or_else(|| "Could not resolve your home directory.".to_string())?
        .join(".local")
        .join("bin"))
}

fn margins_cli_has_official_recall(path: &Path) -> bool {
    let Ok(output) = Command::new(path).arg("capabilities").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return false;
    };
    value.get("schema").and_then(Value::as_i64) == Some(1)
        && value.get("product").and_then(Value::as_str) == Some("margins")
        && value.get("official").and_then(Value::as_bool) == Some(true)
        && value.pointer("/recall/indexing").and_then(Value::as_bool) == Some(true)
        && value.pointer("/recall/lookup").and_then(Value::as_bool) == Some(true)
}

fn margins_cli_is_public_portable(path: &Path) -> bool {
    let Ok(output) = Command::new(path).arg("capabilities").output() else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    let Ok(value) = serde_json::from_slice::<Value>(&output.stdout) else {
        return false;
    };
    value.get("schema").and_then(Value::as_i64) == Some(1)
        && value.get("product").and_then(Value::as_str) == Some("margins")
        && value.get("official").and_then(Value::as_bool) == Some(false)
        && value.pointer("/recall/indexing").and_then(Value::as_bool) == Some(false)
        && value.pointer("/recall/lookup").and_then(Value::as_bool) == Some(false)
}

#[derive(Debug)]
struct BinaryInstallResult {
    path: PathBuf,
    installed: bool,
    used_existing: bool,
}

fn managed_marker_path(destination: &Path) -> PathBuf {
    let file_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("tool");
    destination.with_file_name(format!(".{file_name}.margins-managed"))
}

fn write_managed_marker(destination: &Path, source: &Path, tool_name: &str) {
    let marker = managed_marker_path(destination);
    let temp = marker.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let body = format!(
        "managed-by=margins-desktop\ntool={tool_name}\nsource={}\n",
        source.display()
    );
    if std::fs::write(&temp, body).is_ok() {
        let _ = std::fs::rename(&temp, marker);
    } else {
        let _ = std::fs::remove_file(temp);
    }
}

fn is_margins_managed_destination(destination: &Path) -> bool {
    if managed_marker_path(destination).is_file() {
        return true;
    }
    let Ok(metadata) = destination.symlink_metadata() else {
        return false;
    };
    if !metadata.file_type().is_symlink() {
        return false;
    }
    let Ok(target) = std::fs::read_link(destination) else {
        return false;
    };
    target
        .components()
        .any(|component| component.as_os_str() == OsStr::new("Margins.app"))
}

#[cfg(target_os = "macos")]
fn strip_distribution_xattrs(path: &Path) {
    for attr in ["com.apple.quarantine", "com.apple.provenance"] {
        let _ = Command::new("xattr").arg("-d").arg(attr).arg(path).output();
    }
}

#[cfg(not(target_os = "macos"))]
fn strip_distribution_xattrs(_path: &Path) {}

#[cfg(unix)]
fn mark_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("Could not mark {} as executable: {e}", path.display()))
}

#[cfg(not(unix))]
fn mark_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn replace_with_copied_binary(
    source: &Path,
    destination: &Path,
    tool_name: &str,
) -> Result<(), String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            destination.display()
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
    let file_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(tool_name);
    let temp = parent.join(format!(
        ".{file_name}.margins-install-{}-{}",
        std::process::id(),
        Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    let cleanup = |result: Result<(), String>| {
        let _ = std::fs::remove_file(&temp);
        result
    };

    if let Err(e) = std::fs::copy(source, &temp) {
        return cleanup(Err(format!(
            "Could not copy {} to {}: {e}",
            source.display(),
            temp.display()
        )));
    }
    if let Err(e) = mark_executable(&temp) {
        return cleanup(Err(e));
    }
    strip_distribution_xattrs(&temp);

    #[cfg(unix)]
    {
        std::fs::rename(&temp, destination).map_err(|e| {
            format!(
                "Could not install {tool_name} at {}: {e}",
                destination.display()
            )
        })?;
    }
    #[cfg(not(unix))]
    {
        if destination.exists() {
            std::fs::remove_file(destination).map_err(|e| {
                format!(
                    "Could not replace existing {} at {}: {e}",
                    tool_name,
                    destination.display()
                )
            })?;
        }
        std::fs::rename(&temp, destination).map_err(|e| {
            format!(
                "Could not install {tool_name} at {}: {e}",
                destination.display()
            )
        })?;
    }

    strip_distribution_xattrs(destination);
    write_managed_marker(destination, source, tool_name);
    Ok(())
}

fn preserve_replaced_binary(destination: &Path, tool_name: &str) -> Result<PathBuf, String> {
    let parent = destination.parent().ok_or_else(|| {
        format!(
            "Could not resolve parent directory for {}",
            destination.display()
        )
    })?;
    let file_name = destination
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or(tool_name);
    let preserved = parent.join(format!(
        ".{file_name}.public-portable.{}",
        Local::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::rename(destination, &preserved).map_err(|e| {
        format!(
            "Could not preserve existing public Margins CLI {} before installing official CLI: {e}",
            destination.display()
        )
    })?;
    Ok(preserved)
}

fn install_or_use_binary<F>(
    source: &Path,
    destination: &Path,
    tool_name: &str,
    existing_is_usable: F,
) -> Result<BinaryInstallResult, String>
where
    F: Fn(&Path) -> bool,
{
    if !source.is_file() {
        return Err(format!(
            "Could not find bundled {tool_name} at {}",
            source.display()
        ));
    }

    match destination.symlink_metadata() {
        Ok(metadata) => {
            if is_margins_managed_destination(destination) {
                replace_with_copied_binary(source, destination, tool_name)?;
                return Ok(BinaryInstallResult {
                    path: destination.to_path_buf(),
                    installed: true,
                    used_existing: false,
                });
            }
            if metadata.file_type().is_symlink() {
                if existing_is_usable(destination) {
                    return Ok(BinaryInstallResult {
                        path: destination.to_path_buf(),
                        installed: false,
                        used_existing: true,
                    });
                }
                return Err(format!(
                    "{} already exists as a symlink and is not a Margins-managed install. Remove it or choose another PATH entry before setup.",
                    destination.display()
                ));
            }
            if metadata.is_file() {
                if existing_is_usable(destination) {
                    return Ok(BinaryInstallResult {
                        path: destination.to_path_buf(),
                        installed: false,
                        used_existing: true,
                    });
                }
                if tool_name == "margins" && margins_cli_is_public_portable(destination) {
                    let _preserved = preserve_replaced_binary(destination, tool_name)?;
                    replace_with_copied_binary(source, destination, tool_name)?;
                    return Ok(BinaryInstallResult {
                        path: destination.to_path_buf(),
                        installed: true,
                        used_existing: false,
                    });
                }
                return Err(format!(
                    "{} already exists and is not a Margins-managed {tool_name}. Margins preserved it instead of replacing it.",
                    destination.display()
                ));
            }
            Err(format!(
                "{} already exists and is not a regular file.",
                destination.display()
            ))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            replace_with_copied_binary(source, destination, tool_name)?;
            Ok(BinaryInstallResult {
                path: destination.to_path_buf(),
                installed: true,
                used_existing: false,
            })
        }
        Err(e) => Err(format!("Could not inspect {}: {e}", destination.display())),
    }
}

/// Ensure the CLI tools the agent-setup prompt depends on are present:
/// copy bundled sidecars to `~/.local/bin` where possible, verify they run, and
/// use the network enzyme release only when no local engine is usable.
pub(crate) fn ensure_cli_tools_impl() -> Result<EnsureCliToolsResult, String> {
    // The search engine is in-process now, so the agent-setup handoff only needs
    // the `margins` CLI (the terminal agent runs `margins init`/`margins recall`).
    // No separate engine binary is installed or resolved.
    let margins_install = install_cli_tool_impl()?;
    let installed = margins_install.message.starts_with("Installed");
    let verb = if installed { "Installed" } else { "Using" };
    Ok(EnsureCliToolsResult {
        installed,
        message: format!(
            "{verb} command line tools: margins at {}.",
            margins_install.installed_path
        ),
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn ensure_cli_tools() -> Result<EnsureCliToolsResult, String> {
    crate::async_runtime::spawn_blocking(ensure_cli_tools_impl)
        .await
        .map_err(|e| format!("Engine install task failed: {e}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_calendar_event_suggestion(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<CalendarSuggestionResult, String> {
    let settings = state.settings.lock().unwrap().clone();
    let work_dir = active_work_dir(&state);
    let workspace = resolve_recall_workspace(&work_dir)?;
    calendar_event_suggestion(&workspace, &settings, Local::now())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn update_session_people(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    people: Vec<String>,
) -> Result<Vec<String>, String> {
    validate_session_name(&name)?;
    let cleaned = normalize_people(people);
    let work_dir = active_work_dir(&state);
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    if let Ok(path) = note_path_for_session_or_capture(&margins_dir, &name, &settings) {
        update_note_people_metadata(&path, &cleaned)?;
    }
    session::set_people(&margins_dir, &name, cleaned.clone()).map_err(|e| e.to_string())?;
    ensure_people_files(&settings, &cleaned)?;
    Ok(cleaned)
}

fn update_note_people_metadata(path: &Path, people: &[String]) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read note before updating people: {e}"))?;
    let updated = note_artifacts::update_people_frontmatter(&text, people);
    std::fs::write(path, updated).map_err(|e| format!("Failed to update note people metadata: {e}"))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn update_session_title(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    title: String,
) -> Result<UpdateTitleResult, String> {
    validate_session_name(&name)?;
    let cleaned = note_artifacts::clean_frontmatter_scalar(&title);
    if cleaned.chars().count() > 160 {
        return Err("Title must be 160 characters or less".into());
    }
    let work_dir = active_work_dir(&state);
    let margins_dir = work_dir.join(".margins");
    let saved = session::set_title(
        &margins_dir,
        &name,
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned.to_string())
        },
    )
    .map_err(|e| e.to_string())?;

    let settings = state.settings.lock().unwrap().clone();
    let mut vault_note_path = None;
    if let Some(saved_title) = saved.as_deref() {
        if let Ok(current_path) = note_path_for_session_or_capture(&margins_dir, &name, &settings) {
            update_note_title_metadata(&current_path, saved_title)?;
            let renamed_path = rename_note_file_for_title(&current_path, saved_title)?;
            let renamed_path_string = renamed_path.to_string_lossy().to_string();
            let current_path_string = current_path.to_string_lossy().to_string();
            if session::get_session_meta(&margins_dir, &name).is_ok() {
                session::move_session_vault_note_path(
                    &margins_dir,
                    &name,
                    &current_path_string,
                    &renamed_path_string,
                )
                .map_err(|e| e.to_string())?;
            } else {
                session::move_vault_note_path_by_id(
                    &margins_dir,
                    &name,
                    &current_path_string,
                    &renamed_path_string,
                )
                .map_err(|e| e.to_string())?;
            }
            vault_note_path = Some(renamed_path_string);
        }
    }

    Ok(UpdateTitleResult {
        title: saved,
        vault_note_path,
    })
}

fn update_note_title_metadata(path: &Path, title: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read note before renaming: {e}"))?;
    let title = note_artifacts::clean_frontmatter_scalar(title);
    let escaped = title.replace('\'', "''");
    let mut lines: Vec<String> = text.lines().map(ToString::to_string).collect();

    if lines.first().map(|line| line.trim()) == Some("---") {
        if let Some(end) = lines.iter().enumerate().skip(1).find_map(|(idx, line)| {
            if line.trim() == "---" {
                Some(idx)
            } else {
                None
            }
        }) {
            if let Some(title_idx) = lines[1..end]
                .iter()
                .position(|line| line.trim_start().starts_with("title:"))
                .map(|idx| idx + 1)
            {
                lines[title_idx] = format!("title: '{escaped}'");
            } else {
                lines.insert(end, format!("title: '{escaped}'"));
            }
            let updated = format!("{}\n", lines.join("\n"));
            std::fs::write(path, updated)
                .map_err(|e| format!("Failed to update note title metadata: {e}"))?;
            return Ok(());
        }
    }

    let updated = format!("---\ntitle: '{escaped}'\n---\n\n{text}");
    std::fs::write(path, updated).map_err(|e| format!("Failed to add note title metadata: {e}"))
}

fn rename_note_file_for_title(path: &Path, title: &str) -> Result<PathBuf, String> {
    let Some(parent) = path.parent() else {
        return Ok(path.to_path_buf());
    };
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("note");
    let prefix = timestamp_prefix(stem).unwrap_or("").trim();
    let suffix = safe_descriptive_filename_suffix(title);
    if suffix.is_empty() {
        return Ok(path.to_path_buf());
    }
    let new_stem = if prefix.is_empty() {
        suffix
    } else {
        format!("{prefix} {suffix}")
    };
    let requested = parent.join(format!("{new_stem}.md"));
    let target = unique_note_rename_path(path, &requested);
    if target == path {
        return Ok(path.to_path_buf());
    }
    std::fs::rename(path, &target).map_err(|e| format!("Failed to rename note file: {e}"))?;
    Ok(target)
}

fn timestamp_prefix(stem: &str) -> Option<&str> {
    for len in [19usize, 16usize] {
        if stem.len() >= len && is_timestamp_prefix(&stem[..len]) {
            return Some(&stem[..len]);
        }
    }
    None
}

fn is_timestamp_prefix(value: &str) -> bool {
    let bytes = value.as_bytes();
    let dash = |idx| bytes.get(idx).copied() == Some(b'-');
    let digit = |idx| bytes.get(idx).is_some_and(u8::is_ascii_digit);
    match value.len() {
        16 => {
            (0..16).all(|idx| matches!(idx, 4 | 7 | 10 | 13) || digit(idx))
                && dash(4)
                && dash(7)
                && dash(10)
                && dash(13)
        }
        19 => {
            (0..19).all(|idx| matches!(idx, 4 | 7 | 10 | 13 | 16) || digit(idx))
                && dash(4)
                && dash(7)
                && dash(10)
                && dash(13)
                && dash(16)
        }
        _ => false,
    }
}

fn safe_descriptive_filename_suffix(title: &str) -> String {
    let mut out = String::new();
    let mut last_was_space = false;
    for ch in title.trim().to_lowercase().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_was_space = false;
        } else if ch.is_whitespace() || ch == '-' || ch == '_' || ch == '/' || ch == '\\' {
            if !last_was_space && !out.is_empty() {
                out.push(' ');
                last_was_space = true;
            }
        }
    }
    out.trim().to_string()
}

fn unique_note_rename_path(current: &Path, requested: &Path) -> PathBuf {
    if requested == current || !requested.exists() {
        return requested.to_path_buf();
    }
    let parent = requested.parent().unwrap_or_else(|| Path::new(""));
    let stem = requested
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("note");
    for ordinal in 2..1000 {
        let candidate = parent.join(format!("{stem}-{ordinal}.md"));
        if candidate == current || !candidate.exists() {
            return candidate;
        }
    }
    parent.join(format!("{stem}-{}.md", Local::now().timestamp()))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn survey_granola_import(
    state: tauri::State<'_, Arc<AppState>>,
    paths: Vec<String>,
    project_id: Option<String>,
) -> Result<granola_import::GranolaImportSurvey, String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    survey_granola_import_impl(&ctx, paths, project_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn import_granola_files(
    state: tauri::State<'_, Arc<AppState>>,
    paths: Vec<String>,
    options: granola_import::GranolaImportOptions,
    project_id: Option<String>,
) -> Result<granola_import::GranolaImportResult, String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    import_granola_files_impl(&ctx, paths, options, project_id)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_granola_import_status(
    state: tauri::State<'_, Arc<AppState>>,
) -> granola_mcp::GranolaImportStatus {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    get_granola_import_status_impl(&ctx)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn authorize_granola_import(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<granola_mcp::GranolaImportStatus, String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    authorize_granola_import_impl(&ctx).await
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn revoke_granola_import_authorization(
    state: tauri::State<'_, Arc<AppState>>,
    account: Option<String>,
) -> Result<granola_mcp::GranolaImportStatus, String> {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    revoke_granola_import_authorization_impl(&ctx, account.as_deref())
}

#[derive(Clone, Serialize)]
struct GranolaImportProgressEvent {
    stage: String,
    current: usize,
    total: usize,
}

fn finish_granola_import_after_index_refresh(
    result: granola_mcp::GranolaRemoteImportResult,
    index_outcome: Result<(), String>,
) -> Result<granola_mcp::GranolaRemoteImportResult, String> {
    index_outcome.map_err(|_| {
        granola_mcp::typed_error(
            "granola_index_refresh_failed",
            "index_refresh",
            "refresh_failed",
            false,
        )
    })?;
    Ok(result)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn import_granola_mcp(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<granola_mcp::GranolaRemoteImportResult, String> {
    let settings = state.settings.lock().unwrap().clone();
    let vault_root = vault_root_for_project(&settings, project_id.as_deref()).map_err(|_| {
        granola_mcp::typed_error(
            "granola_workspace_unavailable",
            "binding_validation",
            "workspace_unavailable",
            false,
        )
    })?;
    std::fs::create_dir_all(&vault_root).map_err(|_| {
        granola_mcp::typed_error(
            "granola_workspace_unavailable",
            "binding_validation",
            "workspace_storage_unavailable",
            false,
        )
    })?;
    let status = granola_mcp::authorization_status();
    let account = granola_mcp::resolve_authorized_account(&status, None)?;
    let (inbox_folder, people_folder) = folders_for_project(&settings, project_id.as_deref());
    let options = granola_import::default_options(&vault_root, &inbox_folder, &people_folder);

    let app_for_progress = app.clone();
    let vault_for_import = vault_root.clone();
    let result = crate::async_runtime::spawn_blocking(move || {
        granola_mcp::import_blocking(
            &account,
            &vault_for_import,
            &options,
            &move |stage, current, total| {
                let _ = app_for_progress.emit(
                    "granola-import-progress",
                    GranolaImportProgressEvent {
                        stage: stage.to_string(),
                        current,
                        total,
                    },
                );
            },
        )
    })
    .await
    .map_err(|_| {
        granola_mcp::typed_error(
            "granola_import_task_failed",
            "import_task",
            "task_failed",
            false,
        )
    })??;

    // Keep the vault's search index current without a separate manual step.
    let _ = app.emit(
        "granola-import-progress",
        GranolaImportProgressEvent {
            stage: "Updating search index".to_string(),
            current: 0,
            total: 0,
        },
    );
    let vault_for_index = vault_root.clone();
    let index_outcome =
        crate::async_runtime::spawn_blocking(move || run_enzyme_index_command(&vault_for_index))
            .await
            .map_err(|_| {
                granola_mcp::typed_error(
                    "granola_index_refresh_failed",
                    "index_refresh",
                    "task_failed",
                    false,
                )
            })?;
    finish_granola_import_after_index_refresh(result, index_outcome)
}

fn vault_root(settings: &Settings) -> Option<PathBuf> {
    settings_active_project(settings)
        .map(|project| project.path.as_str())
        .or(settings.vault_path.as_deref())
        .filter(|p| !p.trim().is_empty())
        .map(expand_tilde)
        .map(PathBuf::from)
}

fn vault_root_for_project(
    settings: &Settings,
    project_id: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) {
        if let Some(project) = settings
            .projects
            .iter()
            .find(|project| project.id == project_id)
        {
            return Ok(PathBuf::from(expand_tilde(&project.path)));
        }
    }
    vault_root(settings)
        .ok_or_else(|| "Choose an Margins project folder before importing Granola.".to_string())
}

fn folders_for_project(settings: &Settings, project_id: Option<&str>) -> (String, String) {
    if let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) {
        if let Some(project) = settings
            .projects
            .iter()
            .find(|project| project.id == project_id)
        {
            return (project.inbox_folder.clone(), project.people_folder.clone());
        }
    }
    (
        settings.inbox_folder.clone(),
        settings.people_folder.clone(),
    )
}

fn settings_work_dir(settings: &Settings, fallback_work_dir: &Path) -> PathBuf {
    vault_root(settings)
        .map(|vault| vault.to_path_buf())
        .unwrap_or_else(|| fallback_work_dir.to_path_buf())
}

fn ensure_notes_destination(settings: &Settings) -> std::io::Result<()> {
    let Some(root) = vault_root(settings) else {
        return Ok(());
    };
    let inbox = settings.inbox_folder.trim();
    let destination = if inbox.is_empty() {
        root
    } else {
        root.join(inbox)
    };
    std::fs::create_dir_all(destination)
}

fn settings_work_dir_for_project(
    settings: &Settings,
    fallback_work_dir: &Path,
    project_id: Option<&str>,
) -> PathBuf {
    let Some(project_id) = project_id.filter(|id| !id.trim().is_empty()) else {
        return settings_work_dir(settings, fallback_work_dir);
    };
    settings
        .projects
        .iter()
        .find(|project| project.id == project_id)
        .map(|project| PathBuf::from(expand_tilde(&project.path)))
        .unwrap_or_else(|| settings_work_dir(settings, fallback_work_dir))
}

pub(crate) fn work_dir_for_project_id(state: &Arc<AppState>, project_id: Option<&str>) -> PathBuf {
    let fallback = state.work_dir.lock().unwrap().clone();
    let settings = state.settings.lock().unwrap().clone();
    settings_work_dir_for_project(&settings, &fallback, project_id)
}

fn project_id_for_work_dir(settings: &Settings, work_dir: &Path) -> Option<String> {
    let target = work_dir
        .canonicalize()
        .unwrap_or_else(|_| work_dir.to_path_buf());
    settings.projects.iter().find_map(|project| {
        let candidate = PathBuf::from(expand_tilde(&project.path));
        let candidate = candidate.canonicalize().unwrap_or(candidate);
        if candidate == target {
            Some(project.id.clone())
        } else {
            None
        }
    })
}

fn active_work_dir(state: &Arc<AppState>) -> PathBuf {
    let fallback = state.work_dir.lock().unwrap().clone();
    let settings = state.settings.lock().unwrap().clone();
    settings_work_dir(&settings, &fallback)
}

fn ensure_people_files(settings: &Settings, people: &[String]) -> Result<(), String> {
    let Some(vault) = vault_root(settings) else {
        return Ok(());
    };
    note_artifacts::ensure_people_files(
        &vault,
        settings_active_project(settings)
            .map(|project| project.people_folder.as_str())
            .unwrap_or(settings.people_folder.as_str()),
        &settings.person_note_template,
        people,
    )
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn prepare_speech_models(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    _parakeet_model_dir: Option<String>,
) -> Result<SpeechModelPrepResult, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.speech_model_cancel.lock().unwrap();
        if slot.is_some() {
            return Err("A speech model download is already running.".to_string());
        }
        *slot = Some(cancel.clone());
    }

    let sink_for_worker: Arc<dyn ctx::EventSink> = Arc::new(ctx::TauriSink(app.clone()));
    let settings = state.settings.lock().unwrap().clone();
    let join_result = crate::async_runtime::spawn_blocking(move || {
        prepare_speech_models_blocking(sink_for_worker, settings, cancel)
    })
    .await;
    // Always release the cancel slot — even if the blocking task panicked — so a
    // failed run never leaves the slot stuck at Some() and blocks every future
    // download with "already running".
    *state.speech_model_cancel.lock().unwrap() = None;
    join_result.map_err(|e| format!("Speech model preparation task failed: {e}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn cancel_speech_model_download(state: tauri::State<'_, Arc<AppState>>) -> Result<(), String> {
    if let Some(cancel) = state.speech_model_cancel.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn clear_speech_models(state: tauri::State<'_, Arc<AppState>>) -> Result<String, String> {
    cancel_speech_model_download(state)?;
    crate::async_runtime::spawn_blocking(clear_speech_models_blocking)
        .await
        .map_err(|e| format!("Clear speech models task failed: {e}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn probe_speech_models(
    state: tauri::State<'_, Arc<AppState>>,
    custom_path: Option<String>,
) -> Result<SpeechModelProbe, String> {
    let settings = state.settings.lock().unwrap().clone();
    Ok(crate::async_runtime::spawn_blocking(move || {
        probe_speech_models_impl(&settings, custom_path)
    })
    .await
    .map_err(|e| format!("probe_speech_models task failed: {e}"))?)
}

// ---------------------------------------------------------------------------
// Commands: Devices
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn list_devices(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<device_registry::DeviceSnapshot, String> {
    Ok((*state.device_registry.snapshot()).clone())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn refresh_devices(
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<device_registry::DeviceSnapshot, String> {
    let registry = Arc::clone(&state.device_registry);
    let snapshot = crate::async_runtime::spawn_blocking(move || registry.refresh())
        .await
        .map_err(|error| format!("refresh_devices task failed: {error}"))?;
    Ok((*snapshot).clone())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn test_audio_input(
    state: tauri::State<'_, Arc<AppState>>,
    device_uid: Option<String>,
) -> Result<audio_devices::AudioTestResult, String> {
    let registry = Arc::clone(&state.device_registry);
    crate::async_runtime::spawn_blocking(move || {
        audio_devices::test_audio_input(registry, device_uid)
    })
    .await
    .map_err(|error| format!("test_audio_input task failed: {error}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn test_system_audio_tap() -> Result<audio_devices::SystemAudioTestResult, String> {
    crate::async_runtime::spawn_blocking(audio_devices::test_system_audio_tap)
        .await
        .map_err(|error| format!("test_system_audio_tap task failed: {error}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn open_privacy_pane(pane: String) -> Result<(), String> {
    audio_devices::open_privacy_pane(pane)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn restart_app(app: AppHandle) {
    app.restart();
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn install_cli_tool() -> Result<InstallCliResult, String> {
    install_cli_tool_impl()
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn install_agent_hooks(
    dir: String,
    agents: Option<Vec<String>>,
) -> Result<agent_hooks::AgentHooksInstallResult, String> {
    agent_hooks::install_agent_hooks_impl(dir, agents)
}

fn find_or_build_cli_binary() -> Result<PathBuf, String> {
    let installed_cli_name = format!("margins{}", std::env::consts::EXE_SUFFIX);
    let private_cli_name = format!("margins-private{}", std::env::consts::EXE_SUFFIX);
    let current_exe =
        std::env::current_exe().map_err(|e| format!("Could not locate Margins app: {e}"))?;
    if let Some(parent) = current_exe.parent() {
        // Packaged apps ship the CLI as the `margins-cli` sidecar (it can't be
        // named `margins`: Contents/MacOS/ holds the `Margins` executable and
        // APFS is case-insensitive).
        let sidecar = parent.join(format!("margins-cli{}", std::env::consts::EXE_SUFFIX));
        if sidecar.is_file() {
            return Ok(sidecar);
        }
        let sibling = parent.join(&installed_cli_name);
        if sibling.is_file() {
            return Ok(sibling);
        }
    }

    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "Could not resolve Margins source checkout.".to_string())?
        .to_path_buf();

    let mut candidates = Vec::new();
    if let Ok(target_dir) = std::env::var("CARGO_TARGET_DIR") {
        let target_dir = PathBuf::from(target_dir);
        candidates.push(target_dir.join("release").join(&private_cli_name));
        candidates.push(target_dir.join("debug").join(&private_cli_name));
    }
    // The target dir may also be set via .cargo/config.toml; cargo metadata
    // resolves it either way.
    if let Some(target_dir) = cargo_metadata_target_dir(&repo_root) {
        candidates.push(target_dir.join("release").join(&private_cli_name));
        candidates.push(target_dir.join("debug").join(&private_cli_name));
    }
    candidates.push(
        repo_root
            .join("target")
            .join("release")
            .join(&private_cli_name),
    );
    candidates.push(
        repo_root
            .join("target")
            .join("debug")
            .join(&private_cli_name),
    );

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }

    if !repo_root.join("Cargo.toml").exists() {
        return Err(
            "This Margins build does not include a CLI binary or source checkout to install from."
                .to_string(),
        );
    }

    let mut command = Command::new("cargo");
    command.arg("build").arg("--locked");
    #[cfg(not(feature = "tauri-app"))]
    command.arg("--no-default-features");
    command
        .arg("--bin")
        .arg("margins-private")
        .current_dir(&repo_root);
    configure_cargo_command_env(&mut command);

    let status = command
        .status()
        .map_err(|e| format!("Could not run cargo to build the CLI: {e}"))?;
    if !status.success() {
        return Err("Could not build the Margins CLI with cargo.".to_string());
    }

    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    Err("Built the CLI, but could not find the resulting private CLI binary.".to_string())
}

fn configure_cargo_command_env(command: &mut Command) {
    if std::env::var_os("CARGO_HOME").is_some() && std::env::var_os("RUSTUP_HOME").is_some() {
        return;
    }
    let Some(cargo_path) = find_path_executable("cargo") else {
        return;
    };
    let Some(cargo_home) = cargo_path.parent().and_then(Path::parent) else {
        return;
    };
    if cargo_home.file_name() != Some(OsStr::new(".cargo")) {
        return;
    }
    if std::env::var_os("CARGO_HOME").is_none() && cargo_home.is_dir() {
        command.env("CARGO_HOME", cargo_home);
    }
    if std::env::var_os("RUSTUP_HOME").is_none() {
        if let Some(home) = cargo_home.parent() {
            let rustup_home = home.join(".rustup");
            if rustup_home.is_dir() {
                command.env("RUSTUP_HOME", rustup_home);
            }
        }
    }
}

fn find_path_executable(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().count() > 1 {
        return path.is_file().then_some(path.to_path_buf());
    }
    std::env::var_os("PATH")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

fn cargo_metadata_target_dir(repo_root: &Path) -> Option<PathBuf> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(repo_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let metadata: Value = serde_json::from_slice(&output.stdout).ok()?;
    metadata
        .get("target_directory")
        .and_then(Value::as_str)
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Commands: Sessions
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn list_sessions(
    state: tauri::State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<Vec<SessionInfoDto>, String> {
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let settings = state.settings.lock().unwrap().clone();
    let resolved_project_id = project_id.or_else(|| project_id_for_work_dir(&settings, &work_dir));
    let recording_name = state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.session_name.clone());
    session_index::list_sessions_with_notes(
        &work_dir,
        &settings,
        resolved_project_id.as_deref(),
        recording_name.as_deref(),
    )
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_project_files_fingerprint(
    state: tauri::State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> ProjectFilesFingerprint {
    let settings = state.settings.lock().unwrap().clone();
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let resolved_project_id = project_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| project_id_for_work_dir(&settings, &work_dir));
    let (inbox_folder, people_folder) =
        folders_for_project(&settings, resolved_project_id.as_deref());
    let mut parts = Vec::new();
    push_path_fingerprint(&mut parts, "root", &work_dir);
    push_path_fingerprint(&mut parts, "margins", &work_dir.join(".margins"));
    push_path_fingerprint(
        &mut parts,
        "sessions",
        &work_dir.join(".margins/sessions.sqlite"),
    );
    let inbox_path = work_dir.join(inbox_folder.trim());
    push_path_fingerprint(&mut parts, "inbox", &inbox_path);
    push_markdown_tree_fingerprints(&mut parts, "inbox-md", &inbox_path);
    push_path_fingerprint(&mut parts, "people", &work_dir.join(people_folder.trim()));
    push_known_note_fingerprints(&mut parts, &work_dir.join(".margins"));
    ProjectFilesFingerprint {
        project_id: resolved_project_id,
        fingerprint: parts.join("|"),
    }
}

fn push_known_note_fingerprints(parts: &mut Vec<String>, margins_dir: &Path) {
    if let Ok(sessions) = session::list_sessions(margins_dir) {
        for session in sessions {
            if let Ok(meta) = session::get_session_meta(margins_dir, &session.name) {
                if let Some(path) = meta.vault_note_path.as_deref() {
                    push_path_fingerprint(
                        parts,
                        &format!("session-note:{}", session.name),
                        Path::new(path),
                    );
                }
            }
        }
    }
    if let Ok(notes) = session::list_vault_notes(margins_dir) {
        for note in notes {
            push_path_fingerprint(
                parts,
                &format!("capture-note:{}", note.id),
                Path::new(&note.absolute_path),
            );
        }
    }
}

const PROJECT_FINGERPRINT_MAX_MARKDOWN_DEPTH: usize = 8;
const PROJECT_FINGERPRINT_MAX_MARKDOWN_ENTRIES: usize = 1500;

fn push_markdown_tree_fingerprints(parts: &mut Vec<String>, label: &str, root: &Path) {
    let mut count = 0usize;
    push_markdown_tree_fingerprints_inner(parts, label, root, root, 0, &mut count);
    if count >= PROJECT_FINGERPRINT_MAX_MARKDOWN_ENTRIES {
        parts.push(format!("{label}:truncated:{count}"));
    }
}

fn push_markdown_tree_fingerprints_inner(
    parts: &mut Vec<String>,
    label: &str,
    root: &Path,
    dir: &Path,
    depth: usize,
    count: &mut usize,
) {
    if depth > PROJECT_FINGERPRINT_MAX_MARKDOWN_DEPTH
        || *count >= PROJECT_FINGERPRINT_MAX_MARKDOWN_ENTRIES
        || should_skip_markdown_fingerprint_dir(dir, depth)
    {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if *count >= PROJECT_FINGERPRINT_MAX_MARKDOWN_ENTRIES {
            return;
        }
        let path = entry.path();
        if path.is_dir() {
            if should_skip_markdown_fingerprint_dir(&path, depth + 1) {
                continue;
            }
            *count += 1;
            push_path_fingerprint(
                parts,
                &format!("{}:dir:{}", label, fingerprint_relative_path(root, &path)),
                &path,
            );
            push_markdown_tree_fingerprints_inner(parts, label, root, &path, depth + 1, count);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        *count += 1;
        push_path_fingerprint(
            parts,
            &format!("{}:file:{}", label, fingerprint_relative_path(root, &path)),
            &path,
        );
    }
}

fn should_skip_markdown_fingerprint_dir(path: &Path, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    matches!(
        path.file_name().and_then(|name| name.to_str()),
        Some(".git" | ".margins" | ".obsidian" | ".trash" | "node_modules" | "target")
    )
}

fn fingerprint_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn push_path_fingerprint(parts: &mut Vec<String>, label: &str, path: &Path) {
    let Ok(meta) = std::fs::metadata(path) else {
        parts.push(format!("{label}:missing"));
        return;
    };
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| format!("{}.{:09}", duration.as_secs(), duration.subsec_nanos()))
        .unwrap_or_else(|| "unknown".to_string());
    let kind = if meta.is_dir() { "dir" } else { "file" };
    parts.push(format!("{label}:{kind}:{}:{modified}", meta.len()));
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn delete_session(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    cancel_distill_for_session(&state, &name);
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");

    delete_session_fully(&work_dir, &margins_dir, &name)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn reconcile_project_notes(
    state: tauri::State<'_, Arc<AppState>>,
    project_id: Option<String>,
) -> Result<Vec<SessionInfoDto>, String> {
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    let resolved_project_id = project_id
        .clone()
        .or_else(|| project_id_for_work_dir(&settings, &work_dir));
    let recording_name = state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.session_name.clone());
    for name in session_index::sessions_with_deleted_notes(
        &work_dir,
        &settings,
        resolved_project_id.as_deref(),
    ) {
        if recording_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        cancel_distill_for_session(&state, &name);
        let _ = delete_session_fully(&work_dir, &margins_dir, &name);
    }
    session_index::list_sessions_with_notes(
        &work_dir,
        &settings,
        resolved_project_id.as_deref(),
        recording_name.as_deref(),
    )
}

fn cancel_distill_for_session(state: &Arc<AppState>, name: &str) {
    if let Some(flag) = state.distill_cancel.lock().unwrap().get(name) {
        flag.store(true, Ordering::SeqCst);
    }
}

fn ensure_session_not_tombstoned(margins_dir: &Path, name: &str) -> Result<(), String> {
    if session::is_session_tombstoned(margins_dir, name).map_err(|e| e.to_string())? {
        Err("This capture was deleted.".to_string())
    } else {
        Ok(())
    }
}

fn delete_session_fully(work_dir: &Path, margins_dir: &Path, name: &str) -> Result<(), String> {
    session::begin_delete_session(margins_dir, name).map_err(|e| e.to_string())?;
    let delete_result = delete_session_fully_inner(work_dir, margins_dir, name);
    match delete_result {
        Ok(()) => {
            session::finalize_delete_session(margins_dir, name).map_err(|e| e.to_string())?;
            Ok(())
        }
        Err(err) => Err(err),
    }
}

fn delete_session_fully_inner(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
) -> Result<(), String> {
    let meta = session::get_session_meta(margins_dir, name).ok();
    if let Some(meta) = &meta {
        delete_session_artifacts(work_dir, margins_dir, name, meta)?;
    } else if session::vault_note_path_by_id(margins_dir, name)
        .map_err(|e| e.to_string())?
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(delete_file_if_present)
        .transpose()?
        .is_some()
    {
        let _ = session::remove_vault_note_by_id(margins_dir, name);
        return Ok(());
    }
    delete_registered_session_artifacts(margins_dir, name)?;
    let _ = std::fs::remove_file(margins_dir.join(format!("{name}_transcript.json")));
    let _ = std::fs::remove_file(margins_dir.join(format!("{name}_live_transcript_snapshot.json")));
    let _ = std::fs::remove_dir_all(session_artifact_dir(margins_dir, name));
    let _ = std::fs::remove_file(legacy_aligned_sidecar_path(margins_dir, name));
    let _ = std::fs::remove_file(legacy_capture_context_sidecar_path(margins_dir, name));
    let _ = std::fs::remove_file(margins_dir.join(format!("{name}_grounding.json")));
    let _ = std::fs::remove_file(session_memo_path(work_dir, name));
    Ok(())
}

fn delete_registered_session_artifacts(margins_dir: &Path, name: &str) -> Result<(), String> {
    let artifacts =
        session::list_session_artifacts(margins_dir, name).map_err(|e| e.to_string())?;
    for artifact in artifacts {
        let Some(path) = confined_artifact_registry_disk_path(margins_dir, &artifact.path) else {
            continue;
        };
        delete_registered_artifact_path_if_present(&path)?;
    }
    let _ = session::delete_session_artifacts_registry_rows(margins_dir, name, None);
    Ok(())
}

fn confined_artifact_registry_disk_path(
    margins_dir: &Path,
    registry_path: &str,
) -> Option<PathBuf> {
    let path = Path::new(registry_path);
    if path.is_absolute() {
        return None;
    }

    let mut components = path.components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(first)), Some(Component::Normal(second)))
            if first == OsStr::new(".margins") && second == OsStr::new("artifacts") => {}
        _ => return None,
    }

    let mut out = margins_dir.join("artifacts");
    let mut child_count = 0usize;
    for component in components {
        match component {
            Component::Normal(part) => {
                out.push(part);
                child_count += 1;
            }
            _ => return None,
        }
    }
    if child_count >= 2 {
        Some(out)
    } else {
        None
    }
}

fn delete_registered_artifact_path_if_present(path: &Path) -> Result<(), String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => std::fs::remove_dir_all(path).map_err(|e| {
            format!(
                "failed to delete artifact directory {}: {e}",
                path.display()
            )
        }),
        Ok(_) => std::fs::remove_file(path)
            .map_err(|e| format!("failed to delete artifact file {}: {e}", path.display())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!(
            "failed to inspect artifact path {}: {err}",
            path.display()
        )),
    }
}

fn delete_session_artifacts(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: &session::SessionMeta,
) -> Result<(), String> {
    if let Some(note_path) = meta
        .vault_note_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
    {
        delete_file_if_present(note_path)?;
        let _ = session::remove_vault_note_by_path(margins_dir, note_path);
    }

    for seg in &meta.segments {
        let _ = std::fs::remove_file(work_dir.join(&seg.wav_path));
    }
    let _ = std::fs::remove_file(margins_dir.join(format!("{name}_grounding.json")));
    Ok(())
}

fn delete_audio_after_aligned_transcript(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    meta: &session::SessionMeta,
) {
    let transcript_path = session_transcript_artifact_path(margins_dir, name);
    let Ok(transcript) = std::fs::read_to_string(&transcript_path) else {
        return;
    };
    // A qualified live transcript is safe for immediate note generation, but
    // retain WAV audio as the durable repair/retranscription fallback. Only an
    // independently rebuilt offline/import transcript permits audio cleanup.
    if transcript.contains("Transcript source: `live_qualified`") {
        return;
    }

    for seg in &meta.segments {
        let _ = std::fs::remove_file(work_dir.join(&seg.wav_path));
    }
    let _ = std::fs::remove_file(work_dir.join(recording_combined_rel_path(name)));
    let _ = std::fs::remove_file(margins_dir.join(format!("{name}_combined.wav")));
    let _ = std::fs::remove_dir(margins_dir.join("recordings"));
}

pub(crate) fn verify_capture_ready_for_processing(
    work_dir: &Path,
    margins_dir: &Path,
    meta: &session::SessionMeta,
) -> Result<(), String> {
    if meta.segments.is_empty() {
        return Err("Capture is not ready yet: no audio segments were registered.".to_string());
    }

    let transcript_ready =
        read_valid_session_transcript_checkpoint(margins_dir, &meta.name).is_some();
    for seg in &meta.segments {
        if seg.duration_secs.is_none() {
            return Err(format!(
                "Capture is still saving audio for segment {}. Try again in a moment.",
                seg.segment_index
            ));
        }
        let wav_path = work_dir.join(&seg.wav_path);
        if !transcript_ready && !wav_path.is_file() {
            return Err(format!(
                "Capture audio for segment {} is not available yet.",
                seg.segment_index
            ));
        }
        if !transcript_ready {
            audio_info::probe(&wav_path).map_err(|e| {
                format!(
                    "Capture audio for segment {} is not finalized yet: {e}",
                    seg.segment_index
                )
            })?;
        }
    }
    Ok(())
}

fn delete_file_if_present(path: &str) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("failed to delete file: {err}")),
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commands: Recording
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn start_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    device_uid: Option<String>,
    project_id: Option<String>,
) -> Result<String, String> {
    let reconcile_app = app.clone();
    let reconcile_state = Arc::clone(&*state);
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    aux_windows::mark_starting(&reconcile_app);
    let result = crate::async_runtime::spawn_blocking(move || {
        start_recording_impl(&ctx, name, device_uid, project_id)
    })
    .await
    .map_err(|error| format!("start_recording task failed: {error}"))?;
    aux_windows::reconcile_from_state(&reconcile_app, &reconcile_state);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn cancel_recording_startup(app: AppHandle, state: tauri::State<'_, Arc<AppState>>) -> bool {
    let result = cancel_recording_startup_impl(&state);
    aux_windows::reconcile_from_state(&app, &state);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn stop_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let reconcile_app = app.clone();
    let reconcile_state = Arc::clone(&*state);
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    aux_windows::mark_finalizing(&reconcile_app);
    let result = stop_recording_impl(&ctx).await;
    aux_windows::reconcile_from_state(&reconcile_app, &reconcile_state);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn discard_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<String, String> {
    let reconcile_app = app.clone();
    let reconcile_state = Arc::clone(&*state);
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    let result = discard_recording_impl(&ctx).await;
    aux_windows::reconcile_from_state(&reconcile_app, &reconcile_state);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn switch_recording_device(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    device_uid: Option<String>,
) -> Result<RecordingStatus, String> {
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    crate::async_runtime::spawn_blocking(move || switch_recording_device_impl(&ctx, device_uid))
        .await
        .map_err(|error| format!("switch_recording_device task failed: {error}"))?
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn restart_system_audio_capture(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<RecordingStatus, String> {
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    restart_system_audio_capture_impl(&ctx)
}

/// Pause the active capture: finalize the current audio segment and stop the
/// recorder thread, but keep the session live in memory so it can be resumed.
/// The live backchannel worker stays warm; marks are persisted to the memo file
/// so a crash mid-pause does not lose them.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn pause_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<RecordingStatus, String> {
    let reconcile_app = app.clone();
    let reconcile_state = Arc::clone(&*state);
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    let result = pause_recording_impl(&ctx);
    aux_windows::reconcile_from_state(&reconcile_app, &reconcile_state);
    result
}

/// Resume a paused capture by opening a fresh audio segment. The segment's
/// `offset_ms` is the wall-clock distance from session start, so the pause gap
/// is preserved on the reconstructed timeline and mark timestamps stay aligned.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn resume_recording(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<RecordingStatus, String> {
    let reconcile_app = app.clone();
    let reconcile_state = Arc::clone(&*state);
    let ctx = ctx::Ctx::tauri(Arc::clone(&*state), app);
    let result = resume_recording_impl(&ctx);
    aux_windows::reconcile_from_state(&reconcile_app, &reconcile_state);
    result
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn set_live_transcription_mode(
    state: tauri::State<'_, Arc<AppState>>,
    mode: String,
) -> Result<RecordingStatus, String> {
    let mode = LiveTranscriptionMode::parse(&mode)?;
    let live = {
        let guard = state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        rec.live_backchannel
            .as_ref()
            .ok_or_else(|| "Live transcription is not running for this capture.".to_string())?
            .client()
    };

    crate::async_runtime::spawn_blocking(move || live.set_transcription_mode(mode))
        .await
        .map_err(|e| format!("Live transcription mode task failed: {e}"))??;

    let mut guard = state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    rec.live_transcription_mode = mode;
    Ok(recording_status_from_state(rec))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_recording_status(state: tauri::State<'_, Arc<AppState>>) -> RecordingStatus {
    let mut guard = state.recording.lock().unwrap();
    match guard.as_mut() {
        Some(rec) => recording_status_from_state(rec),
        None => idle_recording_status(),
    }
}

/// Toggle exactly one Pad for the active capture. Window visibility is
/// presentation-only and never mutates recording ownership or lifecycle.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn toggle_active_pad(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
) -> Result<Option<String>, String> {
    let active = state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map(|recording| (recording.session_name.clone(), recording.work_dir.clone()));
    let Some((session_name, work_dir)) = active else {
        return Ok(None);
    };
    let project_id = {
        let settings = state.settings.lock().unwrap();
        project_id_for_work_dir(&settings, &work_dir)
    };
    aux_windows::toggle_pad(
        &app,
        &work_dir,
        &session_name,
        project_id.as_deref(),
        &default_work_dir(),
    )
}

/// Append a mark from the Pad into the same canonical memo vector/file used by
/// the main capture surface. The Pad intentionally has no parallel document or
/// revision store in this bounded integration.
#[cfg(feature = "tauri-app")]
#[tauri::command]
fn append_active_pad_line(
    state: tauri::State<'_, Arc<AppState>>,
    session_name: String,
    text: String,
) -> Result<(), String> {
    validate_session_name(&session_name)?;
    desktop_live::append_active_memo_text(&state, &session_name, text).map_err(|error| {
        if error == "Memo text cannot be empty." {
            "A Pad mark cannot be empty.".to_string()
        } else {
            error
        }
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn show_main_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or("The main Margins window is unavailable.")?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn sync_memo(
    state: tauri::State<'_, Arc<AppState>>,
    lines: Vec<MemoLine>,
    session_name: String,
) -> Result<(), String> {
    let mut guard = state.recording.lock().unwrap();
    if let Some(rec) = guard.as_mut() {
        // Clock is running — normal path.
        ensure_memo_session(&session_name, &rec.session_name)?;
        // The renderer's periodic sync is also the crash-durability boundary.
        // Keeping these edits only in RecordingState could lose up to the
        // entire capture's memo if the app exits before pause/finish.
        persist_live_memo(&rec.work_dir, &rec.session_name, &lines)?;
        rec.memo_lines = lines;
    } else {
        // Clock stopped (prep or pause) — persist to disk without recording state.
        drop(guard);
        let work_dir = ensure_session_writable(&state, &session_name)?;
        persist_live_memo(&work_dir, &session_name, &lines)?;
    }
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn checkpoint_memo_line(
    _app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    lines: Vec<MemoLine>,
    committed_index: usize,
    session_name: String,
) -> Result<(), String> {
    // If not recording (clock stopped — prep/pause), just persist and return.
    {
        let guard = state.recording.lock().unwrap();
        if guard.is_none() {
            drop(guard);
            let work_dir = ensure_session_writable(&state, &session_name)?;
            persist_live_memo(&work_dir, &session_name, &lines)?;
            return Ok(());
        }
    }

    // Block lines (block_ordinal != null) are untimed — never transcript-anchored.
    if let Some(committed_line) = lines.get(committed_index) {
        if committed_line.block_ordinal.is_some() {
            let guard = state.recording.lock().unwrap();
            if let Some(rec) = guard.as_ref() {
                ensure_memo_session(&session_name, &rec.session_name)?;
                let work_dir = rec.work_dir.clone();
                drop(guard);
                persist_live_memo(&work_dir, &session_name, &lines)?;
            }
            return Ok(());
        }
    }

    let (
        margins_dir,
        session_name,
        memo_secs,
        memo_text,
        capture_ms,
        live_client,
        last_ms,
        next_attempt_ms,
    ) = {
        let mut guard = state.recording.lock().unwrap();
        let rec = guard.as_mut().ok_or("Not recording")?;
        ensure_memo_session(&session_name, &rec.session_name)?;
        let work_dir = rec.work_dir.clone();
        rec.memo_lines = lines.clone();
        let memo = lines
            .get(committed_index)
            .ok_or_else(|| "Committed memo index is out of range".to_string())?;
        let session_name = rec.session_name.clone();
        persist_live_memo(&work_dir, &session_name, &lines)?;
        (
            work_dir.join(".margins"),
            session_name,
            memo.created_secs,
            memo.text.clone(),
            (Local::now() - rec.start_time).num_milliseconds().max(0) as u64,
            rec.live_backchannel.as_ref().map(|live| live.client()),
            Arc::clone(&rec.last_live_checkpoint_ms),
            Arc::clone(&rec.next_live_checkpoint_attempt_ms),
        )
    };

    if memo_text.trim().is_empty() {
        return Ok(());
    }

    crate::async_runtime::spawn_blocking(move || {
        run_memo_checkpoint(
            margins_dir,
            session_name,
            committed_index,
            memo_secs,
            memo_text,
            capture_ms,
            live_client,
            last_ms,
            next_attempt_ms,
        );
    });
    Ok(())
}

#[cfg(feature = "tauri-app")]
fn run_memo_checkpoint(
    margins_dir: PathBuf,
    session_name: String,
    memo_index: usize,
    memo_secs: f64,
    memo_text: String,
    capture_ms: u64,
    live_client: Option<live_backchannel::LiveBackchannelClient>,
    last_checkpoint_ms: Arc<AtomicU64>,
    next_checkpoint_attempt_ms: Arc<AtomicU64>,
) {
    let memo_time = format_elapsed_for_backchannel(memo_secs);
    let Some(client) = live_client else {
        let _ = append_backchannel_trace(
            &margins_dir,
            &session_name,
            json!({"kind":"memo_transcript_checkpoint_unavailable","memo_index":memo_index,"memo_time":memo_time,"text":memo_text,"reason":"live_coreml_worker_not_started"}),
        );
        return;
    };

    match client.checkpoint_memo((memo_secs * 1000.0).max(0.0) as u64) {
        Ok(snapshot) => {
            let durable = transcript_store::append_live_transcript_segment(
                &margins_dir,
                &session_name,
                "checkpoint",
                Some(memo_index),
                Some(&memo_time),
                &snapshot,
            );
            if durable.is_ok() {
                last_checkpoint_ms.store(capture_ms, Ordering::Release);
                next_checkpoint_attempt_ms.store(0, Ordering::Release);
            }
            let _ = append_backchannel_trace(
                &margins_dir,
                &session_name,
                json!({
                    "kind":"memo_transcript_checkpoint",
                    "memo_index":memo_index,
                    "memo_time":memo_time,
                    "text":memo_text,
                    "decoded_until_ms": snapshot.decoded_until_ms,
                    "committed_until_ms": snapshot.committed_until_ms,
                    "previous_memo_checkpoint_ms": snapshot.previous_memo_checkpoint_ms,
                    "transcript_source": snapshot.transcript_source,
                    "has_transcript": !snapshot.transcript.trim().is_empty(),
                    "has_new_transcript_since_previous_snapshot": !snapshot.new_transcript.trim().is_empty(),
                    "transcript": snapshot.transcript,
                    "previous_transcript": snapshot.previous_transcript,
                    "new_transcript": snapshot.new_transcript,
                    "committed_transcript": snapshot.committed_transcript,
                    "hypothesis_transcript": snapshot.hypothesis_transcript,
                }),
            );
        }
        Err(e) => {
            let _ = append_backchannel_trace(
                &margins_dir,
                &session_name,
                json!({"kind":"memo_transcript_checkpoint_error","memo_index":memo_index,"memo_time":memo_time,"text":memo_text,"reason":e}),
            );
        }
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn request_backchannel_for_memo(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    lines: Vec<MemoLine>,
    committed_index: usize,
    session_name: String,
) -> Result<(), String> {
    // Block lines (block_ordinal != null) are clock-stopped — use prep lane, not live backchannel.
    if let Some(committed_line) = lines.get(committed_index) {
        if committed_line.block_ordinal.is_some() {
            return Ok(());
        }
    }

    let (session_name, work_dir, memo_secs, memo_text, memo_lines_for_context, live_client) = {
        let mut guard = state.recording.lock().unwrap();
        let rec = guard.as_mut().ok_or("Not recording")?;
        ensure_memo_session(&session_name, &rec.session_name)?;
        let work_dir = rec.work_dir.clone();
        rec.memo_lines = lines.clone();
        let memo = lines
            .get(committed_index)
            .ok_or_else(|| "Committed memo index is out of range".to_string())?;
        let session_name = rec.session_name.clone();
        persist_live_memo(&work_dir, &session_name, &lines)?;
        (
            session_name,
            work_dir,
            memo.created_secs,
            memo.text.clone(),
            lines.clone(),
            rec.live_backchannel.as_ref().map(|live| live.client()),
        )
    };

    // Memo commits are attention triggers, not ASR boundaries. The historical
    // prototype rolled/finalized recording segments here and ran Python ASR on
    // the segment. Do not reintroduce that path: live backchanneling should read
    // from a long-lived CoreML streaming session once the live worker is wired.
    if committed_index + 1 != lines.len() || memo_text.trim().is_empty() {
        return Ok(());
    }

    crate::async_runtime::spawn(async move {
        run_backchannel_request(
            app,
            session_name,
            work_dir,
            committed_index,
            memo_secs,
            memo_text,
            memo_lines_for_context,
            live_client,
        )
        .await;
    });
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn steer_backchannel_for_memo(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    memo_index: usize,
    steering: String,
    previous_suggestion: Option<String>,
    session_name: String,
) -> Result<(), String> {
    if steering.trim().is_empty() {
        return Err("Steering message is empty".to_string());
    }
    let active_session_name = state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .ok_or("Not recording")?
        .session_name
        .clone();
    ensure_memo_session(&session_name, &active_session_name)?;
    let snapshot = {
        let snapshots = state.backchannel_snapshots.lock().unwrap();
        snapshots.get(&memo_index).cloned()
    };
    let Some(snapshot) = snapshot else {
        return Err("No cached cue context to steer; press the mark again first".to_string());
    };
    ensure_memo_session(&session_name, &snapshot.session_name)?;

    crate::async_runtime::spawn(async move {
        run_backchannel_steer_request(app, memo_index, snapshot, steering, previous_suggestion)
            .await;
    });
    Ok(())
}

// ---------------------------------------------------------------------------
// Prep hydration commands
// ---------------------------------------------------------------------------

/// Emit payload from a prep hydration result (or an error) on the
/// "prep-hydration" event. Called from the blocking prep task.
#[cfg(feature = "tauri-app")]
fn emit_prep_hydration(
    app: &AppHandle,
    session_name: &str,
    block_ordinal: u32,
    result: &prep_ai::HydrationResult,
    error: Option<&str>,
) {
    use tauri::Emitter;
    let _ = app.emit(
        "prep-hydration",
        serde_json::json!({
            "session_name": session_name,
            "block_ordinal": block_ordinal,
            "state": result.state,
            "posture": result.posture,
            "marginalia": result.marginalia,
            "reason": result.reason,
            "hint": result.hint,
            "error": error,
        }),
    );
}

/// Emit an error-only prep-hydration event without a result.
#[cfg(feature = "tauri-app")]
fn emit_prep_hydration_error(
    app: &AppHandle,
    session_name: &str,
    block_ordinal: u32,
    state: &str,
    error: &str,
) {
    use tauri::Emitter;
    let _ = app.emit(
        "prep-hydration",
        serde_json::json!({
            "session_name": session_name,
            "block_ordinal": block_ordinal,
            "state": state,
            "posture": null,
            "marginalia": [],
            "reason": error,
            "hint": "run margins setup",
            "error": error,
        }),
    );
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn hydrate_prep_sketch(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    lines: Vec<MemoLine>,
    session_name: String,
    people: Vec<String>,
    event_title: Option<String>,
    block_ordinal: u32,
    pulled_texts: Vec<String>,
    meeting_so_far: Option<String>,
) -> Result<(), String> {
    if lines.is_empty() {
        return Ok(());
    }
    let sketch_text = lines
        .iter()
        .filter(|l| l.block_ordinal == Some(block_ordinal))
        .map(|l| l.text.as_str())
        .filter(|t| !t.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if sketch_text.trim().is_empty() {
        return Ok(());
    }

    let settings = state.settings.lock().unwrap().clone();
    let work_dir = state.work_dir.lock().unwrap().clone();
    let vault_path = vault_root(&settings);

    // Fire the AI warm-up hook. Recall indexing happens only after a note is saved.
    ai_config::schedule_capture_ai_preflight(settings.clone(), "prep");

    let session_name_clone = session_name.clone();
    let app_clone = app.clone();
    let state_clone = Arc::clone(&*state);

    crate::async_runtime::spawn(async move {
        let margins_dir = work_dir.join(".margins");
        let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
            match configure_ai_for_prep(&settings).await {
                Ok(config) => config,
                Err(e) => {
                    emit_prep_hydration_error(
                        &app_clone,
                        &session_name_clone,
                        block_ordinal,
                        "unavailable",
                        &e,
                    );
                    return;
                }
            };

        let request = prep_ai::PrepAiRequest {
            request_id: format!("{session_name_clone}:prep:{block_ordinal}"),
            work_dir: work_dir.clone(),
            margins_dir: margins_dir.clone(),
            session_name: session_name_clone.clone(),
            sketch_text,
            people,
            event_title,
            vault_path,
            people_folder: settings.people_folder.clone(),
            ai_provider,
            ai_model,
            ai_api_key,
            ai_credential_generation,
            instructions: settings.distill_instructions.clone(),
            block_ordinal,
            pulled_texts,
            meeting_so_far,
        };

        let app_for_blocking = app_clone.clone();
        let request_for_blocking = request.clone();
        let session_for_blocking = session_name_clone.clone();

        let result = crate::async_runtime::spawn_blocking(move || {
            // Build context once, then run hydration against it
            let context = prep_ai::build_prep_context(&request_for_blocking);
            let hydration =
                prep_ai::run_prep_hydration_with_context_pub(&request_for_blocking, &context);
            (context, hydration, request_for_blocking)
        })
        .await;

        let snapshot_key = prep_ai::prep_snapshot_key(&session_for_blocking, block_ordinal);

        match result {
            Ok((context, Ok(hydration), req)) => {
                // Store snapshot for steering (composite key)
                if let Ok(mut snapshots) = state_clone.prep_snapshots.lock() {
                    snapshots.insert(
                        snapshot_key,
                        prep_ai::PrepSnapshot {
                            session_name: session_for_blocking.clone(),
                            request: req,
                            context,
                            original_marginalia: Some(
                                serde_json::to_string(&hydration.marginalia).unwrap_or_default(),
                            ),
                            steer_history: vec![],
                        },
                    );
                }
                emit_prep_hydration(
                    &app_for_blocking,
                    &session_for_blocking,
                    block_ordinal,
                    &hydration,
                    None,
                );
            }
            Ok((_, Err(e), _)) => {
                emit_prep_hydration_error(
                    &app_for_blocking,
                    &session_for_blocking,
                    block_ordinal,
                    "error",
                    &e,
                );
            }
            Err(e) => {
                emit_prep_hydration_error(
                    &app_clone,
                    &session_name_clone,
                    block_ordinal,
                    "error",
                    &format!("prep task failed: {e}"),
                );
            }
        }
    });

    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn steer_prep_hydration(
    app: AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    session_name: String,
    block_ordinal: u32,
    instruction: String,
) -> Result<(), String> {
    if instruction.trim().is_empty() {
        return Err("Steering instruction is empty".to_string());
    }
    let snapshot_key = prep_ai::prep_snapshot_key(&session_name, block_ordinal);
    let snapshot = {
        let snapshots = state.prep_snapshots.lock().unwrap();
        snapshots.get(&snapshot_key).cloned()
    };
    let Some(snapshot) = snapshot else {
        return Err("No prep context cached; hydrate first".to_string());
    };

    let state_clone = Arc::clone(&*state);
    let app_clone = app.clone();
    let session_name_clone = session_name.clone();

    crate::async_runtime::spawn(async move {
        let instruction_for_history = instruction.clone();
        let result = crate::async_runtime::spawn_blocking(move || {
            prep_ai::run_prep_steer(
                snapshot.request.clone(),
                &snapshot.context,
                snapshot.original_marginalia.as_deref(),
                &snapshot.steer_history,
                instruction,
            )
            .map(|hydration| (hydration, snapshot))
        })
        .await;

        let key = prep_ai::prep_snapshot_key(&session_name_clone, block_ordinal);

        match result {
            Ok(Ok((hydration, mut snap))) => {
                // Append exchange
                snap.steer_history.push(prep_ai::SteerExchange {
                    instruction: instruction_for_history,
                    result: serde_json::to_string(&hydration.marginalia).unwrap_or_default(),
                });
                if let Ok(mut snapshots) = state_clone.prep_snapshots.lock() {
                    snapshots.insert(key, snap);
                }
                emit_prep_hydration(
                    &app_clone,
                    &session_name_clone,
                    block_ordinal,
                    &hydration,
                    None,
                );
            }
            Ok(Err(e)) => {
                emit_prep_hydration_error(
                    &app_clone,
                    &session_name_clone,
                    block_ordinal,
                    "error",
                    &e,
                );
            }
            Err(e) => {
                emit_prep_hydration_error(
                    &app_clone,
                    &session_name_clone,
                    block_ordinal,
                    "error",
                    &format!("steer prep task failed: {e}"),
                );
            }
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// (end prep hydration commands)
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
async fn run_backchannel_steer_request(
    app: AppHandle,
    memo_index: usize,
    snapshot: BackchannelSnapshot,
    steering: String,
    previous_suggestion: Option<String>,
) {
    let BackchannelSnapshot {
        session_name,
        memo_time,
        request,
        context,
        original_cue,
        steer_history,
    } = snapshot;
    // Anchor the lineage on the captured original cue, falling back to whatever
    // cue the UI is currently showing if it hasn't been recorded yet.
    let original_cue = original_cue.or_else(|| previous_suggestion.clone());
    let steering_for_history = steering.clone();
    let app_for_blocking = app.clone();
    let original_for_call = original_cue.clone();
    let result = crate::async_runtime::spawn_blocking(move || {
        backchannel_ai::run_backchannel_steer(
            request,
            &context,
            original_for_call,
            &steer_history,
            steering,
        )
    })
    .await;

    match result {
        Ok(Ok(raw)) => {
            let state = app.state::<Arc<AppState>>();
            let margins_dir = active_work_dir(&state).join(".margins");
            // Append this round to the lineage so the next steer sees it.
            if let Some(cue) = backchannel_suggestion_text(&raw) {
                if let Ok(mut snapshots) = state.backchannel_snapshots.lock() {
                    if let Some(snapshot) = snapshots.get_mut(&memo_index) {
                        if snapshot.original_cue.is_none() {
                            snapshot.original_cue = original_cue.clone();
                        }
                        snapshot.steer_history.push(backchannel_ai::SteerExchange {
                            steering: steering_for_history,
                            cue,
                        });
                    }
                }
            }
            emit_backchannel_suggestion(
                &app,
                &margins_dir,
                &session_name,
                memo_index,
                &memo_time,
                raw,
            );
        }
        Ok(Err(e)) => emit_backchannel_error(&app, &session_name, memo_index, "steer", &e),
        Err(e) => emit_backchannel_error(
            &app_for_blocking,
            &session_name,
            memo_index,
            "task",
            &format!("Steer task failed: {e}"),
        ),
    }
}

pub(crate) fn persist_live_memo(
    work_dir: &std::path::Path,
    session_name: &str,
    lines: &[MemoLine],
) -> Result<(), String> {
    let memo_content = export_memo(lines);
    let notes_path = session_memo_path(work_dir, session_name);
    std::fs::write(&notes_path, &memo_content).map_err(|e| e.to_string())
}

fn ensure_memo_session(expected: &str, active: &str) -> Result<(), String> {
    if expected == active {
        Ok(())
    } else {
        Err(format!(
            "Stale memo mutation for {expected}; active capture is {active}"
        ))
    }
}

/// Returns `work_dir` when the requested session is either the active recording
/// or the session exists on disk. Used by memo commands that must work while
/// the clock is stopped (prep / pause phases).
fn ensure_session_writable(state: &AppState, session_name: &str) -> Result<PathBuf, String> {
    // Fast path: active recording matches.
    {
        let guard = state.recording.lock().unwrap();
        if let Some(rec) = guard.as_ref() {
            if rec.session_name == session_name {
                return Ok(rec.work_dir.clone());
            }
        }
    }
    // Fall-through: clock stopped — verify the session dir exists on disk.
    let work_dir = state.work_dir.lock().unwrap().clone();
    let margins_dir = work_dir.join(".margins");
    if session::session_exists(&margins_dir, session_name).unwrap_or(false) {
        return Ok(work_dir);
    }
    Err(format!(
        "Session '{session_name}' not found; it may not have been created yet"
    ))
}

#[cfg(feature = "tauri-app")]
async fn run_backchannel_request(
    app: AppHandle,
    session_name: String,
    work_dir: PathBuf,
    memo_index: usize,
    memo_secs: f64,
    memo_text: String,
    memo_lines: Vec<MemoLine>,
    live_client: Option<live_backchannel::LiveBackchannelClient>,
) {
    let state = Arc::clone(&*app.state::<Arc<AppState>>());
    let mut timing = BackchannelTiming::new();
    let settings = state.settings.lock().unwrap().clone();
    timing.settings_ms = timing.mark_stage();
    let vault_path = vault_root(&settings);
    let instructions = settings.distill_instructions.clone();
    let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
        match configure_ai_for_backchannel(&settings).await {
            Ok(config) => config,
            Err(e) => {
                emit_backchannel_error(&app, &session_name, memo_index, "ai_config", &e);
                return;
            }
        };
    timing.ai_config_ms = timing.mark_stage();

    let app_for_blocking = app.clone();
    let result = crate::async_runtime::spawn_blocking(move || {
        run_backchannel_request_blocking(
            app_for_blocking,
            state,
            work_dir,
            &session_name,
            memo_index,
            memo_secs,
            &memo_text,
            &memo_lines,
            live_client,
            timing,
            vault_path,
            ai_provider,
            ai_model,
            ai_api_key,
            ai_credential_generation,
            instructions,
        )
    })
    .await;

    if let Err(e) = result {
        emit_backchannel_error(
            &app,
            "unknown",
            memo_index,
            "task",
            &format!("Backchannel task failed: {e}"),
        );
    }
}

#[cfg(feature = "tauri-app")]
fn run_backchannel_request_blocking(
    app: AppHandle,
    state: Arc<AppState>,
    work_dir: PathBuf,
    session_name: &str,
    memo_index: usize,
    memo_secs: f64,
    memo_text: &str,
    memo_lines: &[MemoLine],
    live_client: Option<live_backchannel::LiveBackchannelClient>,
    mut timing: BackchannelTiming,
    vault_path: Option<PathBuf>,
    ai_provider: Option<String>,
    ai_model: Option<String>,
    ai_api_key: Option<String>,
    ai_credential_generation: Option<u64>,
    instructions: String,
) {
    let margins_dir = work_dir.join(".margins");
    let memo_time = format_elapsed_for_backchannel(memo_secs);
    let _ = append_backchannel_trace(
        &margins_dir,
        session_name,
        json!({"kind":"memo_committed","memo_index":memo_index,"memo_time":memo_time,"text":memo_text}),
    );

    let (transcript, catalyze_transcript) = if let Some(client) = live_client {
        match client.checkpoint_memo((memo_secs * 1000.0).max(0.0) as u64) {
            Ok(snapshot) => {
                let _ = transcript_store::append_live_transcript_segment(
                    &margins_dir,
                    session_name,
                    "checkpoint",
                    Some(memo_index),
                    Some(&memo_time),
                    &snapshot,
                );
                let raw_transcript_for_catalyze = snapshot.transcript.clone();
                let _ = append_backchannel_trace(
                    &margins_dir,
                    session_name,
                    json!({
                        "kind":"live_transcript_snapshot",
                        "memo_index":memo_index,
                        "decoded_until_ms": snapshot.decoded_until_ms,
                        "committed_until_ms": snapshot.committed_until_ms,
                        "previous_memo_checkpoint_ms": snapshot.previous_memo_checkpoint_ms,
                        "transcript_source": snapshot.transcript_source,
                        "timing": snapshot.timing.as_ref().map(|timing| json!({
                            "command_wait_ms": timing.command_wait_ms,
                            "mic_update_ms": timing.mic_update_ms,
                            "system_update_ms": timing.system_update_ms,
                            "format_ms": timing.format_ms,
                        })),
                        "has_transcript": !snapshot.transcript.trim().is_empty(),
                        "has_new_transcript_since_previous_snapshot": !snapshot.new_transcript.trim().is_empty(),
                        "transcript": snapshot.transcript,
                        "previous_transcript": snapshot.previous_transcript,
                        "new_transcript": snapshot.new_transcript,
                        "committed_transcript": snapshot.committed_transcript,
                        "hypothesis_transcript": snapshot.hypothesis_transcript,
                    }),
                );
                (
                    format_live_transcript_prompt_context(&snapshot, memo_lines, memo_index),
                    raw_transcript_for_catalyze,
                )
            }
            Err(e) => {
                let _ = append_backchannel_trace(
                    &margins_dir,
                    session_name,
                    json!({"kind":"live_transcript_unavailable","memo_index":memo_index,"reason":e,"transcript":""}),
                );
                (String::new(), String::new())
            }
        }
    } else {
        let _ = append_backchannel_trace(
            &margins_dir,
            session_name,
            json!({"kind":"live_transcript_unavailable","memo_index":memo_index,"reason":"live_coreml_worker_not_started","transcript":""}),
        );
        (String::new(), String::new())
    };
    timing.transcript_snapshot_ms = timing.mark_stage();

    let _ = append_backchannel_trace(
        &margins_dir,
        session_name,
        json!({
            "kind":"backchannel_prompt_context",
            "memo_index":memo_index,
            "memo_time":memo_time.clone(),
            "active_memo":memo_text,
            "memo_context":format_memo_context_for_trace(memo_lines, memo_index),
            "transcript":transcript.clone(),
            "has_prompt_transcript": !transcript.trim().is_empty(),
            "ai_provider":ai_provider.clone(),
            "ai_model":ai_model.clone(),
            "vault_path":vault_path.as_ref().map(|p| p.display().to_string()),
        }),
    );
    timing.prompt_context_trace_ms = timing.mark_stage();

    let request = backchannel_ai::BackchannelAiRequest {
        request_id: format!("{session_name}:{memo_index}"),
        memo_index,
        work_dir: work_dir.clone(),
        margins_dir: margins_dir.clone(),
        session_name: session_name.to_string(),
        memo_text: memo_text.to_string(),
        memo_time: memo_time.clone(),
        transcript,
        catalyze_transcript,
        vault_path,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
        instructions,
    };

    // Build the cache-stable context once and snapshot it so a later steer can
    // reuse the exact grounding without re-running ASR/enzyme.
    let CachedBackchannelContext {
        context,
        cache_key,
        cache_hit,
    } = prepare_cached_backchannel_context(&state, &request);
    timing.context_prepare_ms = timing.mark_stage();
    timing.context_cache_hit = cache_hit;
    append_backchannel_context_cache_trace(
        &margins_dir,
        session_name,
        memo_index,
        &cache_key,
        cache_hit,
        timing.context_prepare_ms,
    );
    {
        let mut snapshots = state.backchannel_snapshots.lock().unwrap();
        snapshots.insert(
            memo_index,
            BackchannelSnapshot {
                session_name: session_name.to_string(),
                memo_time: memo_time.clone(),
                request: request.clone(),
                context: context.clone(),
                original_cue: None,
                steer_history: Vec::new(),
            },
        );
    }
    timing.snapshot_store_ms = timing.mark_stage();

    match backchannel_ai::run_backchannel_ai(request, &context) {
        Ok(raw) => {
            // Local no-model outcomes tag themselves; keep model_ms at 0 for them
            // so "no model call" stays provable in the timing trace.
            let stage_ms = timing.mark_stage();
            timing.model_called = !backchannel_ai::raw_is_local_only(&raw);
            timing.model_ms = if timing.model_called { stage_ms } else { 0 };
            // Anchor the steer lineage on the first real cue so later steers can
            // show the model where the chain started.
            if let Some(cue) = backchannel_suggestion_text(&raw) {
                if let Ok(mut snapshots) = state.backchannel_snapshots.lock() {
                    if let Some(snapshot) = snapshots.get_mut(&memo_index) {
                        snapshot.original_cue = Some(cue);
                    }
                }
            }
            emit_backchannel_suggestion(
                &app,
                &margins_dir,
                session_name,
                memo_index,
                &memo_time,
                raw,
            );
            timing.emit_ms = Some(timing.mark_stage());
            append_backchannel_timing_trace(&margins_dir, session_name, memo_index, &timing, None);
        }
        Err(e) => {
            // An Err only comes from the model path (local outcomes return Ok),
            // so a model call was attempted.
            timing.model_called = true;
            timing.model_ms = timing.mark_stage();
            append_backchannel_timing_trace(
                &margins_dir,
                session_name,
                memo_index,
                &timing,
                Some(e.as_str()),
            );
            emit_backchannel_error(&app, session_name, memo_index, "suggest", &e)
        }
    }
}

fn prepare_cached_backchannel_context(
    state: &AppState,
    request: &backchannel_ai::BackchannelAiRequest,
) -> CachedBackchannelContext {
    let cache_key = backchannel_ai::backchannel_context_cache_key(request);
    let cached_context = cache_key.as_ref().and_then(|key| {
        state
            .backchannel_context_cache
            .lock()
            .unwrap()
            .get(key)
            .cloned()
    });
    let cache_hit = cached_context.is_some();
    let context = cached_context.unwrap_or_else(|| {
        let context = backchannel_ai::prepare_backchannel_context(request);
        if let Some(key) = cache_key.as_ref() {
            let mut cache = state.backchannel_context_cache.lock().unwrap();
            if cache.len() >= BACKCHANNEL_CONTEXT_CACHE_LIMIT {
                if let Some(first_key) = cache.keys().next().cloned() {
                    cache.remove(&first_key);
                }
            }
            cache.insert(key.clone(), context.clone());
        }
        context
    });

    CachedBackchannelContext {
        context,
        cache_key,
        cache_hit,
    }
}

fn append_backchannel_context_cache_trace(
    margins_dir: &Path,
    session_name: &str,
    memo_index: usize,
    cache_key: &Option<String>,
    cache_hit: bool,
    duration_ms: u128,
) {
    let _ = append_backchannel_trace(
        margins_dir,
        session_name,
        json!({
            "kind":"backchannel_context_cache",
            "memo_index":memo_index,
            "hit":cache_hit,
            "cache_key":cache_key,
            "duration_ms":duration_ms,
        }),
    );
}

fn append_backchannel_timing_trace(
    margins_dir: &Path,
    session_name: &str,
    memo_index: usize,
    timing: &BackchannelTiming,
    error: Option<&str>,
) {
    let mut event = json!({
        "kind":"backchannel_timing",
        "memo_index":memo_index,
        "settings_ms":timing.settings_ms,
        "ai_config_ms":timing.ai_config_ms,
        "transcript_snapshot_ms":timing.transcript_snapshot_ms,
        "prompt_context_trace_ms":timing.prompt_context_trace_ms,
        "context_prepare_ms":timing.context_prepare_ms,
        "context_cache_hit":timing.context_cache_hit,
        "snapshot_store_ms":timing.snapshot_store_ms,
        "model_ms":timing.model_ms,
        "model_called":timing.model_called,
        "total_ms":timing.total_ms(),
    });
    if let Value::Object(fields) = &mut event {
        if let Some(emit_ms) = timing.emit_ms {
            fields.insert("emit_ms".into(), json!(emit_ms));
        }
        if let Some(error) = error {
            fields.insert("error".into(), json!(error));
        }
    }
    let _ = append_backchannel_trace(margins_dir, session_name, event);
}

fn format_live_transcript_prompt_context(
    snapshot: &live_backchannel::LiveTranscriptContext,
    memo_lines: &[MemoLine],
    active_index: usize,
) -> String {
    let has_previous = !snapshot.previous_transcript.trim().is_empty();
    let has_new = !snapshot.new_transcript.trim().is_empty();
    let has_any_memo = memo_lines
        .iter()
        .take(active_index + 1)
        .any(|line| !line.text.trim().is_empty());
    if !has_previous && !has_new && !has_any_memo {
        return String::new();
    }

    let active_memo_ms = memo_lines
        .get(active_index)
        .map(|line| seconds_to_ms(line.created_secs))
        .unwrap_or(snapshot.decoded_until_ms);
    let previous = interleave_transcript_and_memos(
        &snapshot.previous_transcript,
        memo_lines,
        active_index,
        0,
        snapshot.previous_memo_checkpoint_ms,
    );
    let current = interleave_transcript_and_memos(
        &snapshot.new_transcript,
        memo_lines,
        active_index,
        snapshot.previous_memo_checkpoint_ms,
        active_memo_ms.max(snapshot.previous_memo_checkpoint_ms),
    );

    let mut out = format!(
        "<meeting_context transcript_source=\"{}\" decoded_until_ms=\"{}\" committed_until_ms=\"{}\" previous_memo_checkpoint_ms=\"{}\" active_memo_index=\"{}\">\n",
        xml_escape(&snapshot.transcript_source),
        snapshot.decoded_until_ms,
        snapshot.committed_until_ms,
        snapshot.previous_memo_checkpoint_ms,
        active_index,
    );
    out.push_str("  <previous_context>\n");
    if previous.trim().is_empty() {
        out.push_str("    <none />\n");
    } else {
        for line in previous.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("  </previous_context>\n");
    out.push_str("  <current_segment purpose=\"use-this-to-generate-the-backchannel\">\n");
    if current.trim().is_empty() {
        out.push_str("    <none />\n");
    } else {
        for line in current.lines() {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("  </current_segment>\n");
    out.push_str("</meeting_context>");
    out
}

fn format_memo_context_for_trace(memo_lines: &[MemoLine], active_index: usize) -> String {
    memo_lines
        .iter()
        .enumerate()
        .take(active_index + 1)
        .filter(|(_, line)| !line.text.trim().is_empty())
        .map(|(index, line)| format_memo_marker(index, line, index == active_index))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug)]
struct ContextLine {
    ms: u64,
    order: u8,
    text: String,
}

fn interleave_transcript_and_memos(
    transcript: &str,
    memo_lines: &[MemoLine],
    active_index: usize,
    start_ms: u64,
    end_ms: u64,
) -> String {
    let mut lines = parse_transcript_context_lines(transcript);
    for (index, memo) in memo_lines.iter().enumerate().take(active_index + 1) {
        // Block lines (clock-stopped) are not transcript-anchored — skip.
        if memo.block_ordinal.is_some() {
            continue;
        }
        let memo_ms = seconds_to_ms(memo.created_secs);
        let include = if end_ms == 0 {
            memo_ms == 0
        } else {
            memo_ms > start_ms && memo_ms <= end_ms
        };
        if include && !memo.text.trim().is_empty() {
            lines.push(ContextLine {
                ms: memo_ms,
                order: 1,
                text: format_memo_marker(index, memo, index == active_index),
            });
        }
    }
    lines.sort_by_key(|line| (line.ms, line.order));
    lines
        .into_iter()
        .map(|line| line.text)
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_transcript_context_lines(transcript: &str) -> Vec<ContextLine> {
    transcript
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let ms = parse_context_line_ms(trimmed)?;
            Some(ContextLine {
                ms,
                order: 0,
                text: xml_escape(trimmed),
            })
        })
        .collect()
}

fn parse_context_line_ms(line: &str) -> Option<u64> {
    let end = line.find(']')?;
    if !line.starts_with('[') || end <= 1 {
        return None;
    }
    parse_elapsed_to_ms(&line[1..end])
}

fn parse_elapsed_to_ms(value: &str) -> Option<u64> {
    let parts = value.split(':').collect::<Vec<_>>();
    let seconds = match parts.as_slice() {
        [m, s] => m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?,
        [h, m, s] => {
            h.parse::<u64>().ok()? * 3600 + m.parse::<u64>().ok()? * 60 + s.parse::<u64>().ok()?
        }
        _ => return None,
    };
    Some(seconds * 1000)
}

fn format_memo_marker(index: usize, memo: &MemoLine, active: bool) -> String {
    let mut attrs = String::new();
    if active {
        attrs.push_str(" active=\"true\" trigger=\"backchannel\"");
    }
    if memo.audio_pending_at_mark {
        attrs.push_str(" audio_grounding=\"pending_at_mark\"");
    }
    format!(
        "[{}] <memo index=\"{}\"{}>{}</memo>",
        format_elapsed_for_backchannel(memo.created_secs),
        index,
        attrs,
        xml_escape(memo.text.trim())
    )
}

fn seconds_to_ms(seconds: f64) -> u64 {
    (seconds.max(0.0) * 1000.0).round() as u64
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod backchannel_context_tests {
    use super::*;

    fn memo(text: &str, secs: f64) -> MemoLine {
        MemoLine {
            text: text.to_string(),
            created_secs: secs,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: None,
        }
    }

    fn snapshot() -> live_backchannel::LiveTranscriptContext {
        live_backchannel::LiveTranscriptContext {
            transcript: "[00:00] user: old words\n[01:05] user: new words".to_string(),
            previous_transcript: "[00:00] user: old words".to_string(),
            new_transcript: "[01:05] user: new words".to_string(),
            committed_transcript: String::new(),
            hypothesis_transcript: String::new(),
            transcript_source: "committed".to_string(),
            previous_memo_checkpoint_ms: 62_000,
            decoded_until_ms: 71_000,
            committed_until_ms: 0,
            mic_accepted_samples: 0,
            system_accepted_samples: 0,
            mic_dropped_samples: 0,
            system_dropped_samples: 0,
            timing: None,
        }
    }

    fn create_test_session(margins_dir: &Path, name: &str) {
        initialize_test_sqlite_runtime();
        session::create_session(
            margins_dir,
            name,
            &Local::now(),
            &format!(".margins/{name}.md"),
        )
        .unwrap();
    }

    #[test]
    fn interleaves_memos_into_previous_and_current_segments() {
        let memos = vec![
            memo("first", 10.0),
            memo("second", 40.0),
            memo("checkpoint", 62.0),
            memo("active", 71.0),
        ];
        let context = format_live_transcript_prompt_context(&snapshot(), &memos, 3);

        assert!(context.contains("<previous_context>"));
        assert!(context.contains("[00:10] <memo index=\"0\">first</memo>"));
        assert!(context.contains("[00:40] <memo index=\"1\">second</memo>"));
        assert!(context.contains("[01:02] <memo index=\"2\">checkpoint</memo>"));
        assert!(
            context.contains("<current_segment purpose=\"use-this-to-generate-the-backchannel\">")
        );
        assert!(context.contains("[01:05] user: new words"));
        assert!(context.contains(
            "[01:11] <memo index=\"3\" active=\"true\" trigger=\"backchannel\">active</memo>"
        ));
        assert!(!context.contains("Full transcript up to this memo"));
    }

    #[test]
    fn empty_transcript_still_includes_active_memo_context() {
        let mut snapshot = snapshot();
        snapshot.transcript.clear();
        snapshot.previous_transcript.clear();
        snapshot.new_transcript.clear();
        snapshot.previous_memo_checkpoint_ms = 0;
        let memos = vec![memo("active only", 5.0)];

        let context = format_live_transcript_prompt_context(&snapshot, &memos, 0);
        assert!(context.contains("<current_segment"));
        assert!(context.contains("active only"));
        assert!(!context.contains("Live CoreML transcript context"));
    }

    #[test]
    fn artifact_paths_are_session_owned_and_legacy_transcript_is_readable() {
        let root = std::env::temp_dir().join(format!(
            "margins-artifact-path-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");

        assert_eq!(
            session_transcript_artifact_path(&margins_dir, "demo"),
            margins_dir.join("artifacts/demo/transcript.md")
        );
        assert_eq!(
            session_capture_context_scratch_path(&margins_dir, "demo"),
            margins_dir.join("artifacts/demo/scratch/capture-context.md")
        );

        std::fs::write(legacy_aligned_sidecar_path(&margins_dir, "demo"), "legacy").unwrap();
        assert_eq!(
            read_session_transcript_artifact(&margins_dir, "demo").as_deref(),
            Some("legacy")
        );

        let transcript_path = session_transcript_artifact_path(&margins_dir, "demo");
        std::fs::create_dir_all(transcript_path.parent().unwrap()).unwrap();
        std::fs::write(&transcript_path, "new transcript").unwrap();
        register_transcript_artifact(&margins_dir, "demo").unwrap();
        assert_eq!(
            read_session_transcript_artifact(&margins_dir, "demo").as_deref(),
            Some("new transcript")
        );
        let artifacts = session::list_session_artifacts(&margins_dir, "demo").unwrap();
        let transcript = artifacts
            .iter()
            .find(|artifact| artifact.kind == session::SESSION_ARTIFACT_KIND_TRANSCRIPT)
            .unwrap();
        assert_eq!(transcript.path, ".margins/artifacts/demo/transcript.md");
        assert_eq!(transcript.retention_class, "durable");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn transcript_checkpoint_validation_requires_session_and_entries() {
        let valid = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            "",
            "[00:01] you (mic): hello",
        );
        assert!(is_valid_session_transcript_checkpoint(&valid, "demo"));
        assert_eq!(
            session_transcript_checkpoint_session_name(&valid),
            Some("demo")
        );

        let wrong_session = valid.replace("Session: `demo`", "Session: `other`");
        assert!(!is_valid_session_transcript_checkpoint(
            &wrong_session,
            "demo"
        ));

        let empty_timeline = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            "",
            "",
        );
        assert!(!is_valid_session_transcript_checkpoint(
            &empty_timeline,
            "demo"
        ));
    }

    #[test]
    fn terminal_checkpoint_accepts_memo_only_and_silent_room() {
        // Capture that started with no model / found no speech but has a memo:
        // the offline artifact has zero transcript entries yet one timed memo.
        // The strict reuse validator rejects it; the terminal validator accepts
        // it so the note still ships (capture-without-model / silent-room flow).
        let memo_only = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            "[00:03] Need to verify round two notes path and live cue",
            "",
        );
        assert!(memo_only.contains("] memo:"));
        assert!(!is_valid_session_transcript_checkpoint(&memo_only, "demo"));
        assert!(is_valid_terminal_transcript_checkpoint(&memo_only, "demo"));

        // Untimed memo/reflection lines are also a valid terminal record.
        let untimed_memo_only = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            "a free-form reflection with no timestamp",
            "",
        );
        assert!(is_valid_terminal_transcript_checkpoint(
            &untimed_memo_only,
            "demo"
        ));

        // No speech and no memo (near-silent room): the terminal artifact is the
        // genuine — if empty — record for the session, so the note must still ship.
        // Round-3 native verification hit exactly this placeholder-only artifact.
        let truly_empty = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            "",
            "",
        );
        assert!(truly_empty.contains("_No timestamped transcript or memo entries were available._"));
        assert!(is_valid_terminal_transcript_checkpoint(
            &truly_empty,
            "demo"
        ));
        // ...but it is not a reusable live checkpoint (nothing worth reusing).
        assert!(!is_valid_session_transcript_checkpoint(
            &truly_empty,
            "demo"
        ));

        // Genuinely empty content (a botched write) still fails: no `Session:`
        // header means we can't prove this belongs to the session.
        assert!(!is_valid_terminal_transcript_checkpoint("", "demo"));
        assert!(!is_valid_terminal_transcript_checkpoint("   \n\n", "demo"));

        // Session-name mismatch still fails.
        let wrong_session = memo_only.replace("Session: `demo`", "Session: `other`");
        assert!(!is_valid_terminal_transcript_checkpoint(
            &wrong_session,
            "demo"
        ));
    }

    #[test]
    fn silent_memo_only_finish_produces_a_valid_note_source() {
        // Round-3 native journey: capture with zero recognized speech entries plus
        // one memo mark. Finish must succeed and save a note. This exercises the
        // two gates the finish path crosses: (1) the terminal-transcript checkpoint
        // validator that `write_aligned_sidecar` applies, and (2) the
        // `process_session` "must have a full transcript" count gate.
        let session = "2026-07-30-00-46";
        let source = "Margins Desktop offline both-channel transcript and memo context.";

        // Zero speech (empty offline timeline) + one memo mark on the timeline.
        let memo_only = render_aligned_markdown(
            session,
            source,
            "[00:03] verify the memo-only finish path saves a note",
            "",
        );
        // Gate 1: the checkpoint the offline write path validates.
        assert!(is_valid_terminal_transcript_checkpoint(&memo_only, session));
        // Gate 2: zero spoken entries, but a valid terminal artifact -> do not
        // hard-fail; distillation proceeds from the memo/marks.
        assert_eq!(transcript_count_in_markdown(&memo_only), 0);
        let bails_on_gate_two = transcript_count_in_markdown(&memo_only) == 0
            && !is_valid_terminal_transcript_checkpoint(&memo_only, session);
        assert!(
            !bails_on_gate_two,
            "memo-only finish must not bail at the transcript-count gate"
        );

        // A fully silent capture (no speech, no memo) crosses both gates too.
        let silent = render_aligned_markdown(session, source, "", "");
        assert!(is_valid_terminal_transcript_checkpoint(&silent, session));
        let silent_bails = transcript_count_in_markdown(&silent) == 0
            && !is_valid_terminal_transcript_checkpoint(&silent, session);
        assert!(
            !silent_bails,
            "near-silent finish must not bail at the transcript-count gate"
        );
    }

    #[test]
    fn retry_checkpoint_reader_uses_only_valid_artifact_path() {
        let root = std::env::temp_dir().join(format!(
            "margins-checkpoint-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");

        std::fs::write(
            legacy_aligned_sidecar_path(&margins_dir, "demo"),
            render_aligned_markdown("demo", "legacy", "", "[00:01] speaker: legacy"),
        )
        .unwrap();
        assert!(read_valid_session_transcript_checkpoint(&margins_dir, "demo").is_none());

        let path = session_transcript_artifact_path(&margins_dir, "demo");
        let valid = render_aligned_markdown("demo", "artifact", "", "[00:01] speaker: artifact");
        write_atomic_utf8(&path, &valid).unwrap();
        assert_eq!(
            read_valid_session_transcript_checkpoint(&margins_dir, "demo").as_deref(),
            Some(valid.as_str())
        );

        write_atomic_utf8(
            &path,
            &render_aligned_markdown("other", "artifact", "", "[00:01] speaker: wrong"),
        )
        .unwrap();
        assert!(read_valid_session_transcript_checkpoint(&margins_dir, "demo").is_none());

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn capture_ready_can_resume_from_valid_checkpoint_without_audio() {
        let root = std::env::temp_dir().join(format!(
            "margins-checkpoint-ready-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");
        session::add_segment(
            &margins_dir,
            "demo",
            0,
            ".margins/recordings/demo_seg0.wav",
            0,
            Some(10.0),
        )
        .unwrap();
        let transcript_path = session_transcript_artifact_path(&margins_dir, "demo");
        let valid = render_aligned_markdown("demo", "artifact", "", "[00:01] speaker: artifact");
        write_atomic_utf8(&transcript_path, &valid).unwrap();

        let meta = session::get_session_meta(&margins_dir, "demo").unwrap();
        assert!(verify_capture_ready_for_processing(&root, &margins_dir, &meta).is_ok());

        write_atomic_utf8(
            &transcript_path,
            &render_aligned_markdown("other", "artifact", "", "[00:01] speaker: wrong"),
        )
        .unwrap();
        let err = verify_capture_ready_for_processing(&root, &margins_dir, &meta)
            .expect_err("invalid checkpoint should not bypass missing audio");
        assert!(err.contains("Capture audio for segment 0 is not available yet."));

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn renders_capture_context_from_preassembled_live_context() {
        let finalized = json!({
            "transcript_source": "committed",
            "decoded_until_ms": 8_000,
            "committed_until_ms": 7_000,
        });
        let memo = "[00:03] follow up on pricing\nuntimed reflection after the call";
        let transcript = "[00:01] user: intro\n[00:05] assistant: pricing depends on usage";

        let context = render_capture_context_markdown("demo", memo, transcript, &finalized);

        assert!(context.contains("# Capture Context"));
        assert!(context.contains("Session: `demo`"));
        assert!(context.contains("Source: Margins Desktop memo and live transcript state."));
        assert!(context.contains("Transcript source: `committed`"));
        assert!(context.contains("[00:01] user: intro"));
        assert!(context.contains("[00:03] memo: follow up on pricing"));
        assert!(context.contains("[00:05] assistant: pricing depends on usage"));
        // Old-style untimed lines (no [block N] prefix) are no longer included in the context.
        assert!(!context.contains("## Untimed memo / reflection lines"));
        assert!(context.find("[00:01] user").unwrap() < context.find("[00:03] memo").unwrap());
        assert!(context.find("[00:03] memo").unwrap() < context.find("[00:05] assistant").unwrap());
    }

    #[test]
    fn writes_recording_capture_context_from_finalized_live_trace_without_python() {
        let root = std::env::temp_dir().join(format!(
            "margins-live-context-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");
        let memo_path = root.join("demo.md");
        let capture_context_path = session_capture_context_scratch_path(&margins_dir, "demo");
        let trace_path = margins_dir.join("demo_backchannel_trace.jsonl");
        std::fs::write(&memo_path, "[00:02] key memo").unwrap();
        std::fs::write(
            &trace_path,
            concat!(
                "{\"kind\":\"memo_committed\",\"text\":\"ignored\"}\n",
                "{\"kind\":\"live_transcript_finalized\",\"transcript\":\"[00:01] user: hello\\n[00:03] assistant: world\",\"transcript_source\":\"committed\",\"decoded_until_ms\":3000,\"committed_until_ms\":3000}\n"
            ),
        )
        .unwrap();

        let summary = write_capture_context_sidecar(&margins_dir, "demo", &memo_path).unwrap();
        let context = std::fs::read_to_string(&capture_context_path).unwrap();

        assert_eq!(summary, "1 memo lines, 2 transcript entries");
        assert!(context.contains("[00:01] user: hello"));
        assert!(context.contains("[00:02] memo: key memo"));
        assert!(context.contains("[00:03] assistant: world"));
        assert!(!context.contains("margins.py"));
        let artifacts = session::list_session_artifacts(&margins_dir, "demo").unwrap();
        let scratch = artifacts
            .iter()
            .find(|artifact| artifact.kind == "capture_context")
            .unwrap();
        assert_eq!(
            scratch.path,
            ".margins/artifacts/demo/scratch/capture-context.md"
        );
        assert_eq!(scratch.retention_class, "temporary");
        assert!(scratch.expires_at.is_some());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn assembles_capture_context_from_incremental_live_segments() {
        let root = std::env::temp_dir().join(format!(
            "margins-live-segments-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");
        let memo_path = root.join("demo.md");
        let capture_context_path = session_capture_context_scratch_path(&margins_dir, "demo");
        let segments_path = margins_dir.join("demo_live_transcript_segments.jsonl");
        std::fs::write(&memo_path, "[00:02] key memo\n[00:09] closing memo").unwrap();
        std::fs::write(
            &segments_path,
            concat!(
                "{\"kind\":\"checkpoint\",\"segment_transcript\":\"[00:01] user: early\\n[00:03] other: response\",\"transcript_source\":\"committed\",\"decoded_until_ms\":3000,\"committed_until_ms\":3000}\n",
                "{\"kind\":\"final\",\"segment_transcript\":\"[00:08] user: final words\",\"transcript_source\":\"committed\",\"decoded_until_ms\":9000,\"committed_until_ms\":9000}\n"
            ),
        )
        .unwrap();

        let summary = write_capture_context_sidecar(&margins_dir, "demo", &memo_path).unwrap();
        let context = std::fs::read_to_string(&capture_context_path).unwrap();

        assert_eq!(summary, "2 memo lines, 3 transcript entries");
        assert!(context.contains("[00:01] user: early"));
        assert!(context.contains("[00:02] memo: key memo"));
        assert!(context.contains("[00:03] other: response"));
        assert!(context.contains("[00:08] user: final words"));
        assert!(context.contains("[00:09] memo: closing memo"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn writes_recording_capture_context_from_memo_when_live_trace_is_unavailable() {
        let root = std::env::temp_dir().join(format!(
            "margins-memo-context-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let margins_dir = root.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        create_test_session(&margins_dir, "demo");
        let memo_path = root.join("demo.md");
        let capture_context_path = session_capture_context_scratch_path(&margins_dir, "demo");
        std::fs::write(&memo_path, "[00:02] key memo\nuntimed reflection").unwrap();

        let summary = write_capture_context_sidecar(&margins_dir, "demo", &memo_path).unwrap();
        let context = std::fs::read_to_string(&capture_context_path).unwrap();

        assert_eq!(summary, "1 memo lines, 0 transcript entries");
        assert!(context.contains("Transcript source: `none`"));
        assert!(context.contains("[00:02] memo: key memo"));
        // Old-style untimed lines (no [block N] prefix) are no longer included.
        assert!(!context.contains("- memo: untimed reflection"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn aligned_markdown_labels_both_channels_and_interleaves_memo() {
        let memo = "[00:02] follow up on pricing\nuntimed reflection after the call";
        let timeline = "[00:01] you (mic): intro\n[00:05] them (system): pricing depends on usage";

        let aligned = render_aligned_markdown(
            "demo",
            "Margins Desktop offline both-channel transcript and memo context.",
            memo,
            timeline,
        );

        assert!(aligned.contains("# Transcript"));
        assert!(aligned.contains("Transcript source: `offline`"));
        assert!(aligned.contains("[00:01] you (mic): intro"));
        assert!(aligned.contains("[00:02] memo: follow up on pricing"));
        assert!(aligned.contains("[00:05] them (system): pricing depends on usage"));
        assert!(aligned.contains("- memo: untimed reflection after the call"));
        // Memo line interleaves between the two transcript channels by timestamp.
        assert!(
            aligned.find("you (mic): intro").unwrap() < aligned.find("memo: follow up").unwrap()
        );
        assert!(
            aligned.find("memo: follow up").unwrap()
                < aligned.find("them (system): pricing").unwrap()
        );
    }

    #[test]
    fn timeline_labeller_distinguishes_mic_and_system_channels() {
        assert_eq!(recording_channel_label(0), "you (mic)");
        assert_eq!(recording_channel_label(1), "them (system)");
        assert_eq!(recording_channel_label(2), "speaker 3");
        assert_eq!(
            recording_transcript_label(0, Some("recording_channel")),
            "you (mic)"
        );
        assert_eq!(
            recording_transcript_label(1, Some("diarized_speaker")),
            "speaker 2"
        );
        assert_eq!(import_speaker_label(0), "speaker 1");
        assert_eq!(import_speaker_label_for(None)(0), "speaker 1");
        assert_eq!(import_speaker_label_for(Some(2))(1), "speaker 2");
        assert_eq!(import_speaker_label_for(Some(1))(0), "speaker");
    }

    #[test]
    fn diarized_recording_timeline_uses_neutral_speaker_labels() {
        let path = std::env::temp_dir().join(format!(
            "margins-diarized-transcript-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{
              "channel_semantics": "diarized_speaker",
              "transcripts": [{
                "words": [
                  {"channel": 0, "start_ms": 1000, "end_ms": 1500, "text": "interviewer question"},
                  {"channel": 1, "start_ms": 2000, "end_ms": 2500, "text": "faith response"}
                ]
              }]
            }"#,
        )
        .unwrap();
        let timeline = transcript_json_paths_to_timeline_by(
            &[path.to_string_lossy().to_string()],
            recording_transcript_label,
        );
        let _ = std::fs::remove_file(path);

        assert!(timeline.contains("[00:01] speaker 1: interviewer question"));
        assert!(timeline.contains("[00:02] speaker 2: faith response"));
        assert!(!timeline.contains("you (mic)"));
        assert!(!timeline.contains("them (system)"));
    }

    #[test]
    fn tauri_bundle_uses_shared_distillation_skill_not_python_script() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tauri_conf = std::fs::read_to_string(manifest_dir.join("tauri.conf.json")).unwrap();
        let shared_skill = manifest_dir.join("../../skills/margins/SKILL.md");
        let distillation_core = manifest_dir.join("../../skills/margins/distillation-core.md");
        let desktop_host = manifest_dir.join("../../skills/margins/hosts/desktop.md");
        let shared_skill_text = std::fs::read_to_string(&shared_skill).unwrap();
        let distillation_core_text = std::fs::read_to_string(&distillation_core).unwrap();
        let desktop_host_text = std::fs::read_to_string(&desktop_host).unwrap();

        assert!(tauri_conf.contains("../../skills/margins/SKILL.md"));
        assert!(tauri_conf.contains("../../skills/margins/distillation-core.md"));
        assert!(tauri_conf.contains("../../skills/margins/hosts/desktop.md"));
        assert!(!tauri_conf.contains("margins-workspace-setup/SKILL.md"));
        // The setup CLI ships as a sidecar so first-run setup never needs a
        // download and the terminal-command install works in packaged apps.
        assert!(tauri_conf.contains("\"externalBin\""));
        assert!(tauri_conf.contains("binaries/margins-cli"));
        assert!(!tauri_conf.contains("binaries/enzyme"));
        assert!(tauri_conf.contains("skills/margins/templates/"));
        assert!(!tauri_conf.contains("skills/margins-desktop"));
        assert!(!tauri_conf.contains("skills/margins/scripts"));
        assert!(!tauri_conf.contains("margins.py"));
        assert!(shared_skill_text.contains("skills/margins/distillation-core.md"));
        assert!(distillation_core_text.contains("People-page enrichment is state-dependent"));
        assert!(desktop_host_text.contains("Do not run transcription or alignment commands"));
        assert!(!shared_skill_text.contains("margins.py"));
        assert!(!distillation_core_text.contains("margins.py"));
        assert!(!desktop_host_text.contains("margins.py"));
    }

    #[test]
    fn tauri_bundle_declares_supported_macos_floor() {
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let tauri_conf = std::fs::read_to_string(manifest_dir.join("tauri.conf.json")).unwrap();
        let tauri_conf_json: serde_json::Value = serde_json::from_str(&tauri_conf).unwrap();

        assert_eq!(
            tauri_conf_json
                .pointer("/bundle/macOS/minimumSystemVersion")
                .and_then(|value| value.as_str()),
            Some("11.0")
        );
    }

    #[test]
    fn project_validation_does_not_report_stale_local_setup_skill() {
        let dir = std::env::temp_dir().join(format!(
            "margins-stale-skill-validation-test-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        let stale_skill = dir
            .as_path()
            .join(".margins/skills/margins-workspace-setup/SKILL.md");
        std::fs::create_dir_all(stale_skill.parent().unwrap()).unwrap();
        std::fs::write(&stale_skill, "stale").unwrap();

        let validation = project_validation(&dir);

        assert_eq!(validation["has_margins"], true);
        assert!(validation.get("has_local_setup_skill").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn register_project_result_no_longer_serializes_local_skill_path() {
        let value = serde_json::to_value(RegisterProjectResult {
            settings: Settings::default(),
            validation: serde_json::json!({"exists": true}),
        })
        .unwrap();

        assert!(value.get("local_skill_path").is_none());
    }

    #[cfg(not(feature = "tauri-app"))]
    #[test]
    fn workspace_skill_install_compat_command_is_read_only() {
        let dir = std::env::temp_dir().join(format!(
            "margins-skill-compat-noop-test-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        install_workspace_skills_from_checkout(&dir.to_string_lossy()).unwrap();

        assert!(!dir.join(".margins").exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn managed_sidecar_symlink_is_replaced_with_copied_binary() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "margins-managed-copy-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let bin_dir = root.join("bin");
        let old_bundle_dir = root.join("Applications/Margins.app/Contents/MacOS");
        std::fs::create_dir_all(&bin_dir).unwrap();
        std::fs::create_dir_all(&old_bundle_dir).unwrap();

        let source = root.join("source-tool");
        std::fs::write(&source, "#!/bin/sh\nexit 0\n").unwrap();
        let old_target = old_bundle_dir.join("margins-cli");
        std::fs::write(&old_target, "#!/bin/sh\nexit 0\n").unwrap();
        let destination = bin_dir.join("margins");
        symlink(&old_target, &destination).unwrap();

        let result = install_or_use_binary(&source, &destination, "margins", |_| false).unwrap();

        assert!(result.installed);
        assert!(!result.used_existing);
        assert_eq!(result.path, destination);
        assert!(!std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "#!/bin/sh\nexit 0\n"
        );
        assert!(managed_marker_path(&destination).is_file());

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    fn write_capability_script(path: &Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn cli_capability_probe_requires_official_recall() {
        let root = std::env::temp_dir().join(format!(
            "margins-cli-capability-test-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let official = root.join("official");
        let public = root.join("public");
        write_capability_script(
            &official,
            r#"#!/bin/sh
if [ "$1" = "capabilities" ]; then
  printf '%s\n' '{"schema":1,"product":"margins","official":true,"recall":{"indexing":true,"lookup":true}}'
  exit 0
fi
exit 1
"#,
        );
        write_capability_script(
            &public,
            r#"#!/bin/sh
if [ "$1" = "capabilities" ]; then
  printf '%s\n' '{"schema":1,"product":"margins","official":false,"recall":{"indexing":false,"lookup":false}}'
  exit 0
fi
exit 1
"#,
        );

        assert!(margins_cli_has_official_recall(&official));
        assert!(!margins_cli_has_official_recall(&public));
        assert!(margins_cli_is_public_portable(&public));
        assert!(!margins_cli_is_public_portable(&official));

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unmanaged_public_portable_cli_is_preserved_then_migrated() {
        let root = std::env::temp_dir().join(format!(
            "margins-public-migration-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let source = root.join("official-source");
        std::fs::write(&source, "#!/bin/sh\nexit 0\n").unwrap();
        let destination = bin_dir.join("margins");
        write_capability_script(
            &destination,
            r#"#!/bin/sh
if [ "$1" = "capabilities" ]; then
  printf '%s\n' '{"schema":1,"product":"margins","official":false,"recall":{"indexing":false,"lookup":false}}'
  exit 0
fi
exit 1
"#,
        );

        let result = install_or_use_binary(
            &source,
            &destination,
            "margins",
            margins_cli_has_official_recall,
        )
        .unwrap();

        assert!(result.installed);
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "#!/bin/sh\nexit 0\n"
        );
        let preserved = std::fs::read_dir(&bin_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(OsStr::to_str)
                    .is_some_and(|name| name.starts_with(".margins.public-portable."))
            })
            .expect("public CLI was not preserved");
        assert!(std::fs::read_to_string(preserved)
            .unwrap()
            .contains("official\":false"));

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn unrelated_unmanaged_cli_is_not_overwritten() {
        let root = std::env::temp_dir().join(format!(
            "margins-unmanaged-preserve-test-{}-{}",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let bin_dir = root.join("bin");
        std::fs::create_dir_all(&bin_dir).unwrap();
        let source = root.join("official-source");
        std::fs::write(&source, "#!/bin/sh\nexit 0\n").unwrap();
        let destination = bin_dir.join("margins");
        write_capability_script(&destination, "#!/bin/sh\necho unrelated\nexit 0\n");

        let error = install_or_use_binary(
            &source,
            &destination,
            "margins",
            margins_cli_has_official_recall,
        )
        .unwrap_err();

        assert!(error.contains("preserved it instead of replacing it"));
        assert_eq!(
            std::fs::read_to_string(&destination).unwrap(),
            "#!/bin/sh\necho unrelated\nexit 0\n"
        );

        let _ = std::fs::remove_dir_all(root);
    }

    // R6 — block marker export and parse tests
    #[test]
    fn block_ordinal_lines_export_with_block_prefix() {
        let lines = vec![
            MemoLine {
                text: "prep note".to_string(),
                created_secs: 0.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: Some(0),
            },
            MemoLine {
                text: "timed note".to_string(),
                created_secs: 30.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
            MemoLine {
                text: "pause note".to_string(),
                created_secs: 0.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: Some(1),
            },
        ];
        let exported = export_memo(&lines);
        assert!(
            exported.contains("[block 0] prep note"),
            "block 0 should have [block 0] prefix"
        );
        assert!(
            exported.contains("[block 1] pause note"),
            "block 1 should have [block 1] prefix"
        );
        assert!(
            exported.contains("[00:30] timed note"),
            "timed lines should have timestamp"
        );
        assert!(
            !exported.contains("[00:00] prep note"),
            "block lines must not have timestamp"
        );
    }

    #[test]
    fn parse_memo_markdown_with_blocks_splits_correctly() {
        let memo = "[block 0] prep thought\n[00:30] timed entry\n[block 1] pause reflection\n";
        let (timed, blocks) = parse_memo_markdown_with_blocks(memo);
        assert_eq!(timed.len(), 1);
        assert!(timed[0].text.contains("timed entry"));
        assert!(blocks.contains_key(&0));
        assert!(blocks[&0][0].contains("prep thought"));
        assert!(blocks.contains_key(&1));
        assert!(blocks[&1][0].contains("pause reflection"));
    }

    #[test]
    fn render_capture_context_has_prep_block_before_timeline() {
        let memo = "[block 0] my prep note\n[00:30] timed memo\n";
        let transcript = "[00:30] user: hello";
        let finalized = serde_json::json!({
            "transcript_source": "test",
            "decoded_until_ms": 30000u64,
            "committed_until_ms": 30000u64,
        });
        let result = render_capture_context_markdown("test-session", memo, transcript, &finalized);
        let prep_pos = result
            .find("## Prep (block 0)")
            .expect("Prep block 0 section missing");
        let timeline_pos = result
            .find("## Timeline")
            .expect("Timeline section missing");
        assert!(
            prep_pos < timeline_pos,
            "Prep block should appear before Timeline"
        );
        assert!(result.contains("my prep note"));
        assert!(result.contains("timed memo"));
    }

    #[test]
    fn block_lines_excluded_from_alignment_interleave() {
        let lines = vec![
            MemoLine {
                text: "block line".to_string(),
                created_secs: 5.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: Some(0),
            },
            MemoLine {
                text: "timed line".to_string(),
                created_secs: 10.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
        ];
        let result = interleave_transcript_and_memos("", &lines, 1, 0, 20_000);
        assert!(
            !result.contains("block line"),
            "block lines must not appear in alignment"
        );
        assert!(
            result.contains("timed line"),
            "timed lines must appear in alignment"
        );
    }

    #[test]
    #[ignore = "live: builds or copies the margins CLI into ~/.local/bin"]
    fn ensure_cli_tools_live_installs_margins_cli() {
        let result = crate::ensure_cli_tools_impl().expect("ensure_cli_tools failed");
        let margins_bin = crate::local_bin_dir()
            .unwrap()
            .join(format!("margins{}", std::env::consts::EXE_SUFFIX));
        assert!(margins_bin.is_file(), "missing {}", margins_bin.display());
        let output = std::process::Command::new(&margins_bin)
            .arg("--version")
            .output()
            .expect("installed margins CLI did not run");
        assert!(output.status.success());
        println!(
            "margins: {} ({}), installed={}, message: {}",
            margins_bin.display(),
            String::from_utf8_lossy(&output.stdout).trim(),
            result.installed,
            result.message
        );
    }
}

#[cfg(feature = "tauri-app")]
fn emit_backchannel_suggestion(
    app: &AppHandle,
    margins_dir: &std::path::Path,
    session_name: &str,
    memo_index: usize,
    memo_time: &str,
    raw: String,
) {
    let parsed: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|_| json!({"state":"suggestion_available","suggestion":raw}));
    let state = parsed
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("suggestion_available")
        .to_string();
    if state == "cue_unavailable" {
        // Recall is not initialized; be honest that live cues are unavailable
        // rather than silently swallowing the mark as a normal quiet.
        let _ = append_backchannel_trace(
            margins_dir,
            session_name,
            json!({"kind":"suggestion_unavailable","memo_index":memo_index,"raw":parsed}),
        );
        let event = BackchannelSuggestionEvent {
            session_name: session_name.to_string(),
            memo_index,
            memo_time: memo_time.to_string(),
            status: "unavailable".to_string(),
            state,
            kind: None,
            direction: None,
            title: None,
            why: parsed
                .get("why")
                .and_then(Value::as_str)
                .map(str::to_string),
            suggestion: None,
            confidence: None,
            safety: parsed
                .get("safety")
                .and_then(Value::as_str)
                .map(str::to_string),
            raw_json: Some(parsed.clone()),
        };
        let _ = app.emit("backchannel-suggestion", event);
        return;
    }
    if state == "quiet" {
        let _ = append_backchannel_trace(
            margins_dir,
            session_name,
            json!({"kind":"suggestion_quiet","memo_index":memo_index,"raw":parsed}),
        );
        let event = BackchannelSuggestionEvent {
            session_name: session_name.to_string(),
            memo_index,
            memo_time: memo_time.to_string(),
            status: "quiet".to_string(),
            state,
            kind: parsed
                .get("type")
                .and_then(Value::as_str)
                .map(str::to_string),
            direction: parsed
                .get("direction")
                .and_then(Value::as_str)
                .map(str::to_string),
            title: parsed
                .get("title")
                .and_then(Value::as_str)
                .map(str::to_string),
            why: parsed
                .get("why")
                .and_then(Value::as_str)
                .map(str::to_string),
            suggestion: None,
            confidence: parsed
                .get("confidence")
                .and_then(Value::as_str)
                .map(str::to_string),
            safety: parsed
                .get("safety")
                .and_then(Value::as_str)
                .map(str::to_string),
            raw_json: Some(parsed.clone()),
        };
        let _ = app.emit("backchannel-suggestion", event);
        return;
    }
    let event = BackchannelSuggestionEvent {
        session_name: session_name.to_string(),
        memo_index,
        memo_time: memo_time.to_string(),
        status: "ready".to_string(),
        state,
        kind: parsed
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string),
        direction: parsed
            .get("direction")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: parsed
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        why: parsed
            .get("why")
            .and_then(Value::as_str)
            .map(str::to_string),
        suggestion: parsed
            .get("suggestion")
            .and_then(Value::as_str)
            .map(str::to_string),
        confidence: parsed
            .get("confidence")
            .and_then(Value::as_str)
            .map(str::to_string),
        safety: parsed
            .get("safety")
            .and_then(Value::as_str)
            .map(str::to_string),
        raw_json: Some(parsed.clone()),
    };
    let _ = append_backchannel_trace(
        margins_dir,
        session_name,
        json!({"kind":"suggestion_ready","event":event}),
    );
    let _ = app.emit("backchannel-suggestion", event);
}

#[cfg(feature = "tauri-app")]
fn emit_backchannel_error(
    app: &AppHandle,
    session_name: &str,
    memo_index: usize,
    stage: &str,
    message: &str,
) {
    if let Some(state) = app.try_state::<Arc<AppState>>() {
        let margins_dir = active_work_dir(&state).join(".margins");
        let _ = append_backchannel_trace(
            &margins_dir,
            session_name,
            json!({"kind":"backchannel_error","memo_index":memo_index,"stage":stage,"message":message,"recoverable":true,"recording_continues":true}),
        );
    }
    let _ = app.emit(
        "backchannel-error",
        BackchannelSuggestionEvent {
            session_name: session_name.to_string(),
            memo_index,
            memo_time: "".to_string(),
            status: "error".to_string(),
            state: "error".to_string(),
            kind: Some(stage.to_string()),
            direction: None,
            title: Some("Backchannel unavailable".to_string()),
            why: Some(message.to_string()),
            suggestion: None,
            confidence: None,
            safety: Some("Recording continues.".to_string()),
            raw_json: None,
        },
    );
}

fn emit_web_backchannel_suggestion(
    sink: &dyn ctx::EventSink,
    margins_dir: &std::path::Path,
    session_name: &str,
    memo_index: usize,
    memo_time: &str,
    raw: String,
    request_id: &str,
) {
    let parsed: Value = serde_json::from_str(&raw)
        .unwrap_or_else(|_| json!({"state":"suggestion_available","suggestion":raw}));
    let state = parsed
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("suggestion_available")
        .to_string();
    let quiet = state == "quiet";
    let event = BackchannelSuggestionEvent {
        session_name: session_name.to_string(),
        memo_index,
        memo_time: memo_time.to_string(),
        status: if quiet { "quiet" } else { "ready" }.to_string(),
        state,
        kind: parsed
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string),
        direction: parsed
            .get("direction")
            .and_then(Value::as_str)
            .map(str::to_string),
        title: parsed
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_string),
        why: parsed
            .get("why")
            .and_then(Value::as_str)
            .map(str::to_string),
        suggestion: if quiet {
            None
        } else {
            parsed
                .get("suggestion")
                .and_then(Value::as_str)
                .map(str::to_string)
        },
        confidence: parsed
            .get("confidence")
            .and_then(Value::as_str)
            .map(str::to_string),
        safety: parsed
            .get("safety")
            .and_then(Value::as_str)
            .map(str::to_string),
        raw_json: Some(parsed),
    };
    let _ = append_backchannel_trace(
        margins_dir,
        session_name,
        json!({
            "kind":if quiet { "suggestion_quiet" } else { "suggestion_ready" },
            "unix_ms":unix_ms_now(),
            "request_id":request_id,
            "memo_index":memo_index,
            "event":event,
        }),
    );
    sink.emit(
        "backchannel-suggestion",
        serde_json::to_value(event).unwrap_or_default(),
    );
}

fn emit_web_backchannel_error(
    ctx: &ctx::Ctx,
    session_name: &str,
    memo_index: usize,
    stage: &str,
    message: &str,
) {
    let margins_dir = active_work_dir(&ctx.state).join(".margins");
    emit_web_backchannel_error_with_sink(
        ctx.sink.as_ref(),
        &margins_dir,
        session_name,
        memo_index,
        stage,
        message,
    );
}

fn emit_web_backchannel_error_with_sink(
    sink: &dyn ctx::EventSink,
    margins_dir: &std::path::Path,
    session_name: &str,
    memo_index: usize,
    stage: &str,
    message: &str,
) {
    let _ = append_backchannel_trace(
        margins_dir,
        session_name,
        json!({
            "kind":"backchannel_error",
            "unix_ms":unix_ms_now(),
            "memo_index":memo_index,
            "stage":stage,
            "message":message,
        }),
    );
    let event = BackchannelSuggestionEvent {
        session_name: session_name.to_string(),
        memo_index,
        memo_time: String::new(),
        status: "error".to_string(),
        state: "error".to_string(),
        kind: Some(stage.to_string()),
        direction: None,
        title: Some("Backchannel unavailable".to_string()),
        why: Some(message.to_string()),
        suggestion: None,
        confidence: None,
        safety: Some("Recording continues.".to_string()),
        raw_json: None,
    };
    sink.emit(
        "backchannel-error",
        serde_json::to_value(event).unwrap_or_default(),
    );
}

fn unix_ms_now() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn append_backchannel_trace(
    margins_dir: &std::path::Path,
    session_name: &str,
    event: Value,
) -> Result<(), String> {
    std::fs::create_dir_all(margins_dir).map_err(|e| e.to_string())?;
    let path = margins_dir.join(format!("{}_backchannel_trace.jsonl", session_name));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())
    )
    .map_err(|e| e.to_string())
}

fn format_elapsed_for_backchannel(secs: f64) -> String {
    let total = secs.floor() as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{:02}:{:02}:{:02}", h, m, s)
    } else {
        format!("{:02}:{:02}", m, s)
    }
}

struct CaptureContext {
    content: String,
    summary: String,
    transcript_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CaptureContextSource {
    LiveTranscript,
    Import,
    Other,
}

fn transcript_count_in_markdown(content: &str) -> usize {
    content
        .lines()
        .filter(|line| parse_context_line_ms(line.trim()).is_some())
        .filter(|line| !line.trim_start().contains("] memo:"))
        .count()
}

fn capture_context_source(content: &str) -> CaptureContextSource {
    if content.contains("Source: Margins Desktop memo and live transcript state.") {
        CaptureContextSource::LiveTranscript
    } else if content.contains("Transcript source: `import`")
        || content.contains("Source: Imported audio file")
    {
        CaptureContextSource::Import
    } else {
        CaptureContextSource::Other
    }
}

fn build_session_capture_context(
    margins_dir: &Path,
    session_name: &str,
    memo_path: &Path,
) -> Result<CaptureContext, String> {
    for sidecar_path in [
        session_capture_context_scratch_path(margins_dir, session_name),
        legacy_capture_context_sidecar_path(margins_dir, session_name),
    ] {
        if let Ok(content) = std::fs::read_to_string(&sidecar_path) {
            if !content.trim().is_empty() {
                let bytes = content.len();
                let existing_count = transcript_count_in_markdown(&content);
                let source = capture_context_source(&content);
                if source == CaptureContextSource::LiveTranscript {
                    if let Ok(live) =
                        build_live_capture_context(margins_dir, session_name, memo_path)
                    {
                        if live.transcript_count >= existing_count {
                            return Ok(live);
                        }
                    }
                }
                return Ok(CaptureContext {
                    content,
                    summary: format!("stored capture context, {bytes} bytes"),
                    transcript_count: existing_count,
                });
            }
        }
    }

    build_live_capture_context(margins_dir, session_name, memo_path)
}

fn build_live_capture_context(
    margins_dir: &Path,
    session_name: &str,
    memo_path: &Path,
) -> Result<CaptureContext, String> {
    let assembled = transcript_store::assembled_live_transcript(margins_dir, session_name)?;
    let finalized = assembled
        .as_ref()
        .map(|live| {
            json!({
                "transcript_source": live.transcript_source,
                "decoded_until_ms": live.decoded_until_ms,
                "committed_until_ms": live.committed_until_ms,
            })
        })
        .unwrap_or_else(|| {
            json!({
                "transcript_source": "none",
                "decoded_until_ms": 0,
                "committed_until_ms": 0,
            })
        });
    let transcript = assembled
        .as_ref()
        .map(|live| live.transcript.as_str())
        .unwrap_or_default();
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    let content = render_capture_context_markdown(session_name, &memo, transcript, &finalized);
    let transcript_count = transcript
        .lines()
        .filter(|line| parse_context_line_ms(line.trim()).is_some())
        .count();
    let memo_count = parse_memo_markdown_for_context(&memo).0.len();
    Ok(CaptureContext {
        content,
        summary: format!(
            "{} memo lines, {} transcript entries",
            memo_count, transcript_count
        ),
        transcript_count,
    })
}

pub(crate) fn write_capture_context_sidecar(
    margins_dir: &Path,
    session_name: &str,
    memo_path: &Path,
) -> Result<String, String> {
    let context = build_live_capture_context(margins_dir, session_name, memo_path)?;
    let path = session_capture_context_scratch_path(margins_dir, session_name);
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid capture context path {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| {
        format!(
            "failed to create capture context directory {}: {e}",
            parent.display()
        )
    })?;
    std::fs::write(&path, &context.content)
        .map_err(|e| format!("failed to write capture context {}: {e}", path.display()))?;
    register_capture_context_scratch_artifact(margins_dir, session_name)?;
    Ok(context.summary)
}

fn render_capture_context_markdown(
    session_name: &str,
    memo: &str,
    transcript: &str,
    finalized: &Value,
) -> String {
    let (memo_lines, block_lines_by_ordinal) = parse_memo_markdown_with_blocks(memo);
    let mut timeline = parse_transcript_context_lines(transcript);
    timeline.extend(memo_lines);
    timeline.sort_by_key(|line| (line.ms, line.order));

    let transcript_source = finalized
        .get("transcript_source")
        .and_then(Value::as_str)
        .unwrap_or("live");
    let decoded_until_ms = finalized
        .get("decoded_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let committed_until_ms = finalized
        .get("committed_until_ms")
        .and_then(Value::as_u64)
        .unwrap_or(0);

    let mut out = format!(
        "# Capture Context\n\nSession: `{}`\nSource: Margins Desktop memo and live transcript state.\nTranscript source: `{}`\nDecoded until: {}\nCommitted until: {}\n\n",
        session_name,
        transcript_source,
        format_elapsed_for_backchannel(decoded_until_ms as f64 / 1000.0),
        format_elapsed_for_backchannel(committed_until_ms as f64 / 1000.0),
    );

    // Block 0 (prep) goes BEFORE the timeline.
    if let Some(prep_lines) = block_lines_by_ordinal.get(&0) {
        out.push_str("## Prep (block 0)\n\n");
        for line in prep_lines {
            out.push_str("- memo: ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }

    out.push_str("## Timeline\n\n");
    if timeline.is_empty() {
        out.push_str("_No timestamped transcript or memo entries were available._\n");
    } else {
        // Determine mid-meeting pause blocks (ordinal > 0)
        let mut pause_blocks: Vec<u32> = block_lines_by_ordinal
            .keys()
            .copied()
            .filter(|&k| k > 0)
            .collect();
        pause_blocks.sort();

        // Emit all timeline lines, then append pause blocks after the timeline.
        for line in &timeline {
            out.push_str(&line.text);
            out.push('\n');
        }

        // Append pause blocks (N > 0) after the timeline in order.
        for ordinal in pause_blocks {
            if let Some(lines) = block_lines_by_ordinal.get(&ordinal) {
                out.push_str(&format!("\n### Pause block {ordinal} (untimed)\n\n"));
                for line in lines {
                    out.push_str("- memo: ");
                    out.push_str(line);
                    out.push('\n');
                }
            }
        }
    }
    out
}

/// Parse memo markdown into (timed ContextLines, block_lines_by_ordinal).
/// Block lines use `[block N]` prefix; timed lines use `[MM:SS]`.
fn parse_memo_markdown_with_blocks(
    memo: &str,
) -> (
    Vec<ContextLine>,
    std::collections::HashMap<u32, Vec<String>>,
) {
    let mut timed = Vec::new();
    let mut blocks: std::collections::HashMap<u32, Vec<String>> = std::collections::HashMap::new();
    for raw in memo.lines() {
        let line = raw.trim();
        if line.is_empty() || line == "---" || line.starts_with('#') {
            continue;
        }
        if let Some(ordinal) = parse_block_memo_line_ordinal(line) {
            let text = line[line.find(']').map(|i| i + 1).unwrap_or(0)..]
                .trim()
                .to_string();
            blocks.entry(ordinal).or_default().push(text);
        } else if let Some((ms, text)) = parse_memo_line_for_context(line) {
            timed.push(ContextLine {
                ms,
                order: 1,
                text: format!(
                    "[{}] memo: {}",
                    format_elapsed_for_backchannel(ms as f64 / 1000.0),
                    text
                ),
            });
        }
        // Lines that match neither are ignored (e.g. old-style untimed lines without block marker).
    }
    (timed, blocks)
}

/// Parse `[block N]` prefix, returning N.
fn parse_block_memo_line_ordinal(line: &str) -> Option<u32> {
    if !line.starts_with("[block ") {
        return None;
    }
    let end = line.find(']')?;
    let inner = line[7..end].trim(); // strip "[block "
    inner.parse::<u32>().ok()
}

// ---------------------------------------------------------------------------
// Import transcript helpers
// ---------------------------------------------------------------------------

fn import_transcript_speaker_label(speaker: &str, people: &[String]) -> String {
    let speaker = speaker.trim();
    if speaker == "memo" {
        return "memo".to_string();
    }
    if let Some(index) = speaker
        .strip_prefix("ch")
        .and_then(|suffix| suffix.parse::<usize>().ok())
    {
        if let Some(person) = people.get(index).filter(|person| !person.trim().is_empty()) {
            return person.clone();
        }
    }
    speaker.to_string()
}

fn normalize_imported_transcript_timeline(transcript: &str, people: &[String]) -> (String, f64) {
    let mut max_ms = 0u64;
    let lines = transcript
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let ms = parse_context_line_ms(trimmed)?;
            let end = trimmed.find(']')?;
            let rest = trimmed[end + 1..].trim();
            if rest.is_empty() {
                return None;
            }
            max_ms = max_ms.max(ms);
            let time = format_elapsed_for_backchannel(ms as f64 / 1000.0);
            if let Some((speaker, text)) = rest.split_once(':') {
                let label = import_transcript_speaker_label(speaker, people);
                Some(format!("[{time}] {label}: {}", text.trim()))
            } else {
                Some(format!("[{time}] {rest}"))
            }
        })
        .collect::<Vec<_>>();
    let duration_secs = ((max_ms as f64) / 1000.0 + 1.0).max(1.0);
    (lines.join("\n"), duration_secs)
}

fn import_transcript_for_state(
    state: &Arc<AppState>,
    args: ImportTranscriptArgs,
) -> Result<ImportTranscriptResult, String> {
    let title = args.title.trim().to_string();
    if title.is_empty() {
        return Err("Transcript title cannot be empty".to_string());
    }
    if args.transcript.trim().is_empty() {
        return Err("Transcript cannot be empty".to_string());
    }

    let requested_name = match args.name {
        Some(name) if !name.trim().is_empty() => name.trim().to_string(),
        _ => slugify(&title),
    };
    validate_session_name(&requested_name)?;

    let people = normalize_people(args.people);
    let (timeline, duration_secs) =
        normalize_imported_transcript_timeline(&args.transcript, &people);
    if transcript_count_in_markdown(&timeline) == 0 {
        return Err("Transcript must include at least one timestamped non-memo line.".to_string());
    }

    let work_dir = work_dir_for_project_id(state, args.project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir).map_err(|e| e.to_string())?;
    let name = unique_session_name(&work_dir, &margins_dir, &requested_name);
    let notes_path = format!(".margins/{name}.md");
    let created = Local::now();
    session::create_session(&margins_dir, &name, &created, &notes_path)
        .map_err(|e| e.to_string())?;

    let memo_content = args
        .memo
        .unwrap_or_else(|| format!("# {title}\n\nImported transcript captured through Margins.\n"));
    let memo_path = session_memo_path(&work_dir, &name);
    if let Some(parent) = memo_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create memo directory {}: {e}", parent.display()))?;
    }
    std::fs::write(&memo_path, &memo_content)
        .map_err(|e| format!("failed to write memo {}: {e}", memo_path.display()))?;

    session::add_segment(
        &margins_dir,
        &name,
        0,
        &format!(".margins/recordings/{name}_import_transcript.wav"),
        0,
        Some(duration_secs),
    )
    .map_err(|e| e.to_string())?;
    session::set_title(&margins_dir, &name, Some(title.clone())).map_err(|e| e.to_string())?;
    session::set_people(&margins_dir, &name, people.clone()).map_err(|e| e.to_string())?;

    let transcript = render_imported_transcript_markdown_with_source(
        &name,
        "Imported transcript supplied through import_transcript.",
        "",
        &timeline,
    );
    let transcript_path = session_transcript_artifact_path(&margins_dir, &name);
    write_atomic_utf8(&transcript_path, &transcript)?;
    if !is_valid_session_transcript_checkpoint(&transcript, &name) {
        return Err(format!(
            "transcript checkpoint {} was written but did not validate for session {name}",
            transcript_path.display()
        ));
    }
    register_transcript_artifact(&margins_dir, &name)?;

    let capture_context = render_capture_context_markdown(
        &name,
        "",
        &timeline,
        &json!({
            "transcript_source": "import_transcript",
            "decoded_until_ms": (duration_secs * 1000.0).round() as u64,
            "committed_until_ms": (duration_secs * 1000.0).round() as u64,
        }),
    );
    let capture_context_path = session_capture_context_scratch_path(&margins_dir, &name);
    write_atomic_utf8(&capture_context_path, &capture_context)?;
    register_capture_context_scratch_artifact(&margins_dir, &name)?;

    let meta = session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string())?;
    verify_capture_ready_for_processing(&work_dir, &margins_dir, &meta)?;

    Ok(ImportTranscriptResult {
        name,
        title,
        people,
        memo_path: memo_path.display().to_string(),
        transcript_path: transcript_path.display().to_string(),
        capture_context_path: capture_context_path.display().to_string(),
        transcript_count: transcript_count_in_markdown(&transcript),
        duration_secs,
    })
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn import_transcript(
    state: tauri::State<'_, Arc<AppState>>,
    name: Option<String>,
    title: String,
    people: Vec<String>,
    transcript: String,
    memo: Option<String>,
    project_id: Option<String>,
) -> Result<ImportTranscriptResult, String> {
    import_transcript_for_state(
        &state,
        ImportTranscriptArgs {
            name,
            title,
            people,
            transcript,
            memo,
            project_id,
        },
    )
}

pub(crate) fn import_transcript_impl(
    ctx: &ctx::Ctx,
    args: ImportTranscriptArgs,
) -> Result<ImportTranscriptResult, String> {
    import_transcript_for_state(&ctx.state, args)
}

// ---------------------------------------------------------------------------
// Import audio file helpers
// ---------------------------------------------------------------------------

/// Map a diarized channel index to a human-readable label.
fn import_speaker_label(channel: u32) -> String {
    format!("speaker {}", channel + 1)
}

/// Build the labeler for imported audio after the user has chosen a speaker cap.
///
/// A one-speaker import is an explicit user assertion, not a diarization
/// request. Keep the transcript substrate clean instead of adding a meaningless
/// numeric label that can leak into the generated note.
fn import_speaker_label_for(max_speakers: Option<usize>) -> impl Fn(u32) -> String {
    move |channel| {
        if max_speakers == Some(1) {
            "speaker".to_string()
        } else {
            import_speaker_label(channel)
        }
    }
}

/// Map a recording channel index to a human-readable label.
///
/// Margins records a stereo WAV where ch0 is the local mic and ch1 is the system
/// audio (the remote participant). Higher channel indices, when present from a
/// diarization split, fall back to the generic speaker labelling.
fn recording_channel_label(channel: u32) -> String {
    match channel {
        0 => "you (mic)".to_string(),
        1 => "them (system)".to_string(),
        other => format!("speaker {}", other + 1),
    }
}

fn recording_transcript_label(channel: u32, channel_semantics: Option<&str>) -> String {
    if channel_semantics == Some("diarized_speaker") {
        import_speaker_label(channel)
    } else {
        recording_channel_label(channel)
    }
}

/// Render the full, durable transcript artifact from the offline both-channel
/// spoken transcript interleaved with the user's memo on a shared timeline.
///
/// This is the rich, complete record of a session. Unlike capture context
/// scratch, which is assembled from the partial *live* transcript (one channel
/// and/or the tail of the conversation), this artifact is produced from the
/// offline transcription of the recorded WAV, so it carries both the local mic
/// and the remote/system channel in full.
fn render_aligned_markdown(
    session_name: &str,
    source_label: &str,
    memo: &str,
    transcript_timeline: &str,
) -> String {
    let (memo_lines, untimed_memos) = parse_memo_markdown_for_context(memo);
    let mut timeline = parse_transcript_context_lines(transcript_timeline);
    timeline.extend(memo_lines);
    timeline.sort_by_key(|line| (line.ms, line.order));

    let mut out = format!(
        "# Transcript\n\nSession: `{}`\nSource: {}\nTranscript source: `offline` (full both-channel transcription of the recorded audio)\n\n## Timeline\n\n",
        session_name, source_label,
    );
    if timeline.is_empty() {
        out.push_str("_No timestamped transcript or memo entries were available._\n");
    } else {
        for line in timeline {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    if !untimed_memos.is_empty() {
        out.push_str("\n## Untimed memo / reflection lines\n\n");
        for line in untimed_memos {
            out.push_str("- memo: ");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn write_qualified_live_transcript_artifact(
    margins_dir: &Path,
    session_name: &str,
    memo: &str,
    transcript_timeline: &str,
) -> Result<(), String> {
    write_reusable_live_transcript_artifact(
        margins_dir,
        session_name,
        memo,
        transcript_timeline,
        "Margins Desktop qualified terminal live transcript and memo context.",
        "terminal both-channel transcript; >=99.5% live-queue delivery per channel, no observed queue loss or unstable hypothesis",
    )
}

pub(crate) fn write_qualified_headless_live_transcript_artifact(
    margins_dir: &Path,
    session_name: &str,
    memo: &str,
    transcript_timeline: &str,
) -> Result<(), String> {
    write_reusable_live_transcript_artifact(
        margins_dir,
        session_name,
        memo,
        transcript_timeline,
        "Margins hosted capture qualified terminal ONNX transcript and memo context.",
        "terminal headless ONNX transcript; complete injected PCM delivery, no observed queue loss or unstable hypothesis",
    )
}

fn write_reusable_live_transcript_artifact(
    margins_dir: &Path,
    session_name: &str,
    memo: &str,
    transcript_timeline: &str,
    source: &str,
    qualification: &str,
) -> Result<(), String> {
    let (memo_lines, untimed_memos) = parse_memo_markdown_for_context(memo);
    let mut timeline = parse_transcript_context_lines(transcript_timeline);
    timeline.extend(memo_lines);
    timeline.sort_by_key(|line| (line.ms, line.order));

    let mut content = format!(
        "# Transcript\n\nSession: `{}`\nSource: {}\nTranscript source: `live_qualified` ({})\n\n## Timeline\n\n",
        session_name, source, qualification,
    );
    for line in timeline {
        content.push_str(&line.text);
        content.push('\n');
    }
    if !untimed_memos.is_empty() {
        content.push_str("\n## Untimed memo / reflection lines\n\n");
        for line in untimed_memos {
            content.push_str("- memo: ");
            content.push_str(&line);
            content.push('\n');
        }
    }
    if !is_valid_session_transcript_checkpoint(&content, session_name) {
        return Err("Qualified live transcript did not produce a valid durable artifact.".into());
    }
    write_atomic_utf8(
        &session_transcript_artifact_path(margins_dir, session_name),
        &content,
    )?;
    register_transcript_artifact(margins_dir, session_name)
}

/// Build the full both-channel transcript for a session by running offline
/// transcription on the recorded WAV segments, then interleaving the resulting
/// transcript with the memo. Returns the rendered markdown plus a short summary.
///
/// This is a durable artifact written separately from capture context scratch.
/// It exists because the live capture context can be lopsided — the live worker
/// sometimes commits only the local channel or just the tail of the conversation
/// — and the distillation needs the complete record.
#[cfg(feature = "tauri-app")]
fn build_session_aligned(
    app: &AppHandle,
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    memo_path: &Path,
    settings: &Settings,
    max_speakers: Option<usize>,
) -> Result<(String, String), String> {
    let meta = session::get_session_meta(margins_dir, name).map_err(|e| e.to_string())?;
    // Offline transcription of the recorded audio. Diarization is forced so a
    // mono import still splits speakers; stereo recordings already carry the
    // mic/system channels.
    let transcript_paths = transcription::transcribe_segments(
        app,
        name,
        work_dir,
        &meta,
        "",
        settings,
        max_speakers,
        true,
    )?;
    let timeline =
        transcript_json_paths_to_timeline_by(&transcript_paths, recording_transcript_label);
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    let content = render_aligned_markdown(
        name,
        "Margins Desktop offline both-channel transcript and memo context.",
        &memo,
        &timeline,
    );
    let transcript_count = timeline
        .lines()
        .filter(|line| parse_context_line_ms(line.trim()).is_some())
        .count();
    let memo_count = parse_memo_markdown_for_context(&memo).0.len();
    let summary = format!(
        "{} memo lines, {} transcript entries (offline both-channel)",
        memo_count, transcript_count
    );
    Ok((content, summary))
}

/// Write the durable transcript artifact for a session.
#[cfg(feature = "tauri-app")]
fn write_aligned_sidecar(
    app: &AppHandle,
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    memo_path: &Path,
    settings: &Settings,
    max_speakers: Option<usize>,
) -> Result<String, String> {
    let (content, summary) = build_session_aligned(
        app,
        work_dir,
        margins_dir,
        name,
        memo_path,
        settings,
        max_speakers,
    )?;
    let path = session_transcript_artifact_path(margins_dir, name);
    write_atomic_utf8(&path, &content)?;
    if !is_valid_terminal_transcript_checkpoint(&content, name) {
        return Err(format!(
            "transcript checkpoint {} was written but did not validate for session {name}",
            path.display()
        ));
    }
    register_transcript_artifact(margins_dir, name)?;
    Ok(summary)
}

#[cfg(feature = "parakeet-asr")]
fn build_session_aligned_headless(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    memo_path: &Path,
) -> Result<(String, String), String> {
    let meta = session::get_session_meta(margins_dir, name).map_err(|e| e.to_string())?;
    let mut entries: Vec<(u64, String)> = Vec::new();
    let mut backend = None;
    for seg in &meta.segments {
        let wav = work_dir.join(&seg.wav_path);
        let mono_16k = margins::audio_pipeline::mono_16k_from_wav(&wav)
            .map_err(|e| format!("Failed to prepare audio for transcription: {e}"))?;
        let transcript = margins::offline_asr::transcribe_mono_16k(&mono_16k)
            .map_err(|e| format!("Headless transcription failed: {e}"))?;
        backend = Some(transcript.backend);
        let offset_ms = seg.offset_ms.max(0) as u64;
        for entry in transcript.entries {
            let text = entry.text.trim();
            if !text.is_empty() {
                entries.push((offset_ms + entry.start_ms, text.to_string()));
            }
        }
    }
    entries.sort_by_key(|(start_ms, _)| *start_ms);
    let timeline = entries
        .iter()
        .map(|(start_ms, text)| {
            format!(
                "[{}] speaker: {}",
                format_elapsed_for_backchannel(*start_ms as f64 / 1000.0),
                text
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    // Only hard-fail when there is neither speech nor memo context; a memo-only
    // capture still yields a valid note (see is_valid_terminal_transcript_checkpoint).
    let memo_context = parse_memo_markdown_for_context(&memo);
    if timeline.trim().is_empty() && memo_context.0.is_empty() && memo_context.1.is_empty() {
        return Err("No speech detected in the captured audio.".to_string());
    }
    let source = format!(
        "Headless {} transcript and memo context.",
        backend.unwrap_or("ASR")
    );
    let content = render_aligned_markdown(name, &source, &memo, &timeline);
    let transcript_count = entries.len();
    let memo_count = parse_memo_markdown_for_context(&memo).0.len();
    let summary = format!(
        "{} memo lines, {} transcript entries (headless {})",
        memo_count,
        transcript_count,
        backend.unwrap_or("ASR")
    );
    Ok((content, summary))
}

#[cfg(feature = "parakeet-asr")]
fn write_aligned_sidecar_headless(
    work_dir: &Path,
    margins_dir: &Path,
    name: &str,
    memo_path: &Path,
) -> Result<String, String> {
    let (content, summary) =
        build_session_aligned_headless(work_dir, margins_dir, name, memo_path)?;
    let path = session_transcript_artifact_path(margins_dir, name);
    write_atomic_utf8(&path, &content)?;
    if !is_valid_terminal_transcript_checkpoint(&content, name) {
        return Err(format!(
            "transcript checkpoint {} was written but did not validate for session {name}",
            path.display()
        ));
    }
    register_transcript_artifact(margins_dir, name)?;
    Ok(summary)
}

/// Parse the /tmp transcript JSON paths returned by `transcribe_segments` into
/// a timeline string with a caller-supplied channel labeller.
fn transcript_json_paths_to_timeline_with(
    json_paths: &[String],
    label: impl Fn(u32) -> String,
) -> String {
    transcript_json_paths_to_timeline_by(json_paths, |channel, _channel_semantics| label(channel))
}

fn transcript_json_paths_to_timeline_by(
    json_paths: &[String],
    label: impl Fn(u32, Option<&str>) -> String,
) -> String {
    #[derive(serde::Deserialize)]
    struct Word {
        channel: u32,
        start_ms: u64,
        text: String,
    }
    #[derive(serde::Deserialize)]
    struct Transcript {
        words: Vec<Word>,
    }
    #[derive(serde::Deserialize)]
    struct Root {
        #[serde(default)]
        channel_semantics: Option<String>,
        transcripts: Vec<Transcript>,
    }

    let mut entries: Vec<(u64, u32, Option<String>, String)> = Vec::new();
    for path in json_paths {
        let Ok(raw) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(root) = serde_json::from_str::<Root>(&raw) else {
            continue;
        };
        for transcript in root.transcripts {
            for word in transcript.words {
                let text = word.text.trim().to_string();
                if !text.is_empty() {
                    entries.push((
                        word.start_ms,
                        word.channel,
                        root.channel_semantics.clone(),
                        text,
                    ));
                }
            }
        }
    }
    entries.sort_by_key(|(ms, channel, _, _)| (*ms, *channel));
    entries
        .into_iter()
        .map(|(start_ms, channel, channel_semantics, text)| {
            format!(
                "[{}] {}: {}",
                format_elapsed_for_backchannel(start_ms as f64 / 1000.0),
                label(channel, channel_semantics.as_deref()),
                text
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Produce the durable transcript artifact for an imported audio file.
fn render_imported_transcript_markdown(
    session_name: &str,
    source_path: &str,
    memo: &str,
    transcript_timeline: &str,
) -> String {
    render_imported_transcript_markdown_with_source(
        session_name,
        &format!("Imported audio file {source_path}"),
        memo,
        transcript_timeline,
    )
}

fn render_imported_transcript_markdown_with_source(
    session_name: &str,
    source_label: &str,
    memo: &str,
    transcript_timeline: &str,
) -> String {
    let (memo_lines, untimed_memos) = parse_memo_markdown_for_context(memo);
    let mut timeline = parse_transcript_context_lines(transcript_timeline);
    timeline.extend(memo_lines);
    timeline.sort_by_key(|line| (line.ms, line.order));

    let mut out = format!(
        "# Transcript\n\nSession: `{}`\nSource: {}\nTranscript source: `import`\n\n## Timeline\n\n",
        session_name, source_label,
    );
    if timeline.is_empty() {
        out.push_str("_No timestamped transcript or memo entries were available._\n");
    } else {
        for line in timeline {
            out.push_str(&line.text);
            out.push('\n');
        }
    }
    if !untimed_memos.is_empty() {
        out.push_str("\n## Untimed memo / reflection lines\n\n");
        for line in untimed_memos {
            out.push_str("- memo: ");
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

/// Slugify a string to `[a-z0-9-]+`, collapsing and stripping leading/trailing dashes.
fn slugify(s: &str) -> String {
    let slug: String = s
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    // Collapse consecutive dashes and strip leading/trailing dashes.
    let mut out = String::new();
    let mut prev_dash = true; // treat start as if preceded by dash to strip leading ones
    for c in slug.chars() {
        if c == '-' {
            if !prev_dash {
                out.push('-');
                prev_dash = true;
            }
        } else {
            out.push(c);
            prev_dash = false;
        }
    }
    // Strip trailing dash
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "import".to_string()
    } else {
        out
    }
}

// ---------------------------------------------------------------------------
// Command: import_audio_file
// ---------------------------------------------------------------------------

/// Import an existing audio file (WAV, M4A, MP3, FLAC, AAC) as an Margins session.
///
/// Flow:
/// 1. Validate path and extension (allowlist: wav, m4a, mp3, flac, aac).
/// 2. Derive session name from file stem (slugified + uniquified).
/// 3. Read file creation date for `start_time`.
/// 4. Create DB session row (no calendar suggestion).
/// 5. Non-WAV inputs are transcoded via ffmpeg to 16 kHz mono s16 PCM WAV before
///    the load/downmix step; WAV files use the fast path unchanged.
/// 6. Downmix WAV to mono and write `.margins/recordings/{name}_seg0.wav`, add segment.
/// 7. Transcribe (+ optional diarization) via `transcription::transcribe_segments`.
/// 8. Build `[H:MM:SS] speaker N: text` timeline from transcript JSON output.
/// 9. Reject silently-empty audio before burning an AI distill run.
/// 10. Write stub memo + durable transcript artifact.
/// 11. Call `process_session` to distill.
///
/// Returns the session name string so the frontend can show optimistic progress
/// (same contract as `start_recording`).  A proper DTO is Phase 2.
#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn import_audio_file(
    app: tauri::AppHandle,
    path: String,
    max_speakers: Option<usize>,
    project_id: Option<String>,
) -> Result<String, String> {
    // 1. Validate path and extension.
    let src_path = std::path::Path::new(&path);
    if !src_path.exists() {
        return Err(format!("File not found: {path}"));
    }
    let ext = src_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    const ACCEPTED_EXTS: &[&str] = &["wav", "m4a", "mp3", "flac", "aac"];
    if !ACCEPTED_EXTS.contains(&ext.as_str()) {
        return Err(format!(
            "Unsupported audio format .{ext}; supported: wav, m4a, mp3, flac, aac"
        ));
    }

    let state = app.state::<Arc<AppState>>();
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    std::fs::create_dir_all(&margins_dir).map_err(|e| e.to_string())?;

    // 2. Derive session name from file stem.
    let stem = src_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("import");
    let slug = slugify(stem);
    validate_session_name(&slug)?;
    let name = unique_session_name(&work_dir, &margins_dir, &slug);

    // 3. Get file creation date; fall back to modified, then now.
    let created: chrono::DateTime<Local> = std::fs::metadata(&path)
        .and_then(|m| m.created())
        .map(chrono::DateTime::<Local>::from)
        .or_else(|_| {
            std::fs::metadata(&path)
                .and_then(|m| m.modified())
                .map(chrono::DateTime::<Local>::from)
        })
        .unwrap_or_else(|_| Local::now());

    // 4. Create DB session row — NO calendar suggestion for imports.
    let notes_path = format!(".margins/{name}.md");
    session::create_session(&margins_dir, &name, &created, &notes_path)
        .map_err(|e| e.to_string())?;

    // 5-8 are CPU-heavy; run inside spawn_blocking.
    let app_clone = app.clone();
    let work_dir_clone = work_dir.clone();
    let margins_dir_clone = margins_dir.clone();
    let name_clone = name.clone();
    let path_clone = path.clone();
    let settings_clone = settings.clone();
    let ext_clone = ext.clone();
    let effective_max_speakers = max_speakers.or_else(|| settings.import_speaker_count_opt());

    let result = crate::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let app = &app_clone;
        let work_dir = &work_dir_clone;
        let margins_dir = &margins_dir_clone;
        let name = &name_clone;
        let path = &path_clone;
        let settings = &settings_clone;
        let ext = &ext_clone;

        // 5. Transcode non-WAV inputs to 16 kHz mono s16 PCM WAV via ffmpeg,
        //    then load and downmix (downmix is a no-op for already-mono output).
        let transcoded_path: Option<std::path::PathBuf>;
        let decode_src: std::path::PathBuf = if ext == "wav" {
            transcoded_path = None;
            std::path::Path::new(path).to_path_buf()
        } else {
            let dst = margins_dir
                .join("recordings")
                .join(format!("{name}_import_src.wav"));
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create recordings directory: {e}"))?;
            }
            transcode_to_wav_16k_mono(path, &dst)?;
            transcoded_path = Some(dst.clone());
            dst
        };

        let audio = margins::audio_pipeline::load_wav(decode_src.to_str().unwrap_or(path))
            .map_err(|e| format!("Failed to load audio file: {e}"))?;
        let mono = margins::audio_pipeline::downmix_to_mono(&audio)
            .map_err(|e| format!("Failed to downmix audio: {e}"))?;
        let seg_rel = recording_segment_rel_path(name, 0);
        if let Some(parent) = work_dir.join(&seg_rel).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create recordings directory: {e}"))?;
        }
        let dur = margins::audio_pipeline::write_interleaved_wav(
            work_dir.join(&seg_rel),
            &mono,
            audio.sample_rate,
            1,
        )
        .map_err(|e| format!("Failed to write segment WAV: {e}"))?;
        session::add_segment(margins_dir, name, 0, &seg_rel, 0, Some(dur))
            .map_err(|e| e.to_string())?;

        // Clean up transcoded temp file now that segment has been written.
        if let Some(ref tmp) = transcoded_path {
            let _ = std::fs::remove_file(tmp);
        }

        // 6. Get meta and transcribe.
        let meta = session::get_session_meta(margins_dir, name).map_err(|e| e.to_string())?;
        // script_path is only used by the Python fallback; coreml/rust branches ignore it.
        let script_path = "";
        // Imported audio is transcribed as a single voice by default; diarization is
        // strictly opt-in via an explicit speaker count >= 2. A count of 1 (or the
        // default) yields a single-speaker transcript with no diarization.
        let diarize = effective_max_speakers.map_or(false, |n| n >= 2);
        let transcript_paths = transcription::transcribe_segments(
            app,
            name,
            work_dir,
            &meta,
            script_path,
            settings,
            effective_max_speakers,
            diarize,
        )?;

        // 7. Build [H:MM:SS] speaker N: text timeline from transcript JSON paths.
        let timeline = transcript_json_paths_to_timeline_with(
            &transcript_paths,
            import_speaker_label_for(effective_max_speakers),
        );

        // 8. Reject silently-empty audio before burning an AI run.
        if timeline.trim().is_empty() {
            return Err("No speech detected in the imported audio.".to_string());
        }

        // 9. Write stub memo + durable transcript artifact.
        let stub_memo = format!(
            "# {name}\n\nImported from {} on {}.\n",
            path,
            created.format("%Y-%m-%d %H:%M:%S")
        );
        std::fs::write(session_memo_path(work_dir, name), &stub_memo)
            .map_err(|e| format!("Failed to write stub memo: {e}"))?;

        let transcript = render_imported_transcript_markdown(name, path, &stub_memo, &timeline);
        let transcript_path = session_transcript_artifact_path(margins_dir, name);
        if let Some(parent) = transcript_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create transcript artifact directory: {e}"))?;
        }
        std::fs::write(&transcript_path, transcript)
            .map_err(|e| format!("Failed to write transcript artifact: {e}"))?;
        register_transcript_artifact(margins_dir, name)?;
        let meta = session::get_session_meta(margins_dir, name).map_err(|e| e.to_string())?;
        delete_audio_after_aligned_transcript(work_dir, margins_dir, name, &meta);

        Ok(())
    })
    .await
    .map_err(|e| format!("Import task panicked: {e}"))?;

    // Surface errors from the blocking task before launching distillation.
    if let Err(e) = &result {
        eprintln!("[margins-import] '{name}': transcribe/diarize stage failed: {e}");
    }
    result?;

    // 10. Kick off distillation (async, same as the recording flow).
    if let Err(e) = process_session(
        app.clone(),
        name.clone(),
        project_id.clone(),
        None,
        None,
        None,
    )
    .await
    {
        eprintln!("[margins-import] '{name}': distill stage failed: {e}");
        return Err(e);
    }

    Ok(name)
}

/// Transcode `src` to 16 kHz mono signed-16 PCM WAV at `dst` using ffmpeg.
///
/// Flags chosen for determinism and to avoid common pitfalls:
/// - `-vn` drops cover-art/video streams present in mp3/m4a files.
/// - `-ac 1` downmixes to mono at the decoder (averaging channels, not splitting).
/// - `-ar 16000` resamples to the ASR/diarization 16 kHz expectation.
/// - `-sample_fmt s16 -f wav` forces 16-bit PCM that `hound::WavReader` handles.
/// - `-map_metadata -1` strips tags; `-nostdin -y` avoid interactive hangs.
fn transcode_to_wav_16k_mono(src: &str, dst: &std::path::Path) -> Result<(), String> {
    let ffmpeg = ffmpeg::resolve_binary().map_err(|error| {
        let ext = std::path::Path::new(src)
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("?");
        format!("ffmpeg is required to import .{ext} files: {error}")
    })?;

    let out = std::process::Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-y",
            "-i",
            src,
            "-vn",
            "-map_metadata",
            "-1",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-sample_fmt",
            "s16",
            "-f",
            "wav",
        ])
        .arg(dst)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| {
            let ext = std::path::Path::new(src)
                .extension()
                .and_then(|ex| ex.to_str())
                .unwrap_or("?");
            if e.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "ffmpeg is required to import .{ext} files. \
                     Install it (brew install ffmpeg) or drop a .wav."
                )
            } else {
                format!("Failed to spawn ffmpeg: {e}")
            }
        })?;

    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "ffmpeg failed to transcode {src}: {}",
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

fn parse_memo_markdown_for_context(memo: &str) -> (Vec<ContextLine>, Vec<String>) {
    let mut timed = Vec::new();
    let mut untimed = Vec::new();
    for raw in memo.lines() {
        let line = raw.trim();
        if line.is_empty() || line == "---" || line.starts_with('#') {
            continue;
        }
        if let Some((ms, text)) = parse_memo_line_for_context(line) {
            timed.push(ContextLine {
                ms,
                order: 1,
                text: format!(
                    "[{}] memo: {}",
                    format_elapsed_for_backchannel(ms as f64 / 1000.0),
                    text
                ),
            });
        } else {
            untimed.push(line.to_string());
        }
    }
    (timed, untimed)
}

fn parse_memo_line_for_context(line: &str) -> Option<(u64, String)> {
    let end = line.find(']')?;
    if !line.starts_with('[') || end <= 1 {
        return None;
    }
    let timestamp = line[1..end].split('~').next()?.trim();
    let ms = parse_elapsed_to_ms(timestamp)?;
    let text = line[end + 1..].trim().to_string();
    Some((ms, text))
}

// ---------------------------------------------------------------------------
// Commands: Processing pipeline
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn cancel_process_session(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    _project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    if let Some(flag) = state.distill_cancel.lock().unwrap().get(&name) {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn clear_session_note_error(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    session::clear_note_error(&margins_dir, &name).map_err(|e| e.to_string())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn process_session(
    app: AppHandle,
    name: String,
    project_id: Option<String>,
    overwrite_existing_note: Option<bool>,
    max_speakers: Option<usize>,
    force_transcribe: Option<bool>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let state = app.state::<Arc<AppState>>();
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    if state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|rec| rec.session_name == name)
    {
        return Err("Capture is still active. Finish capture before writing the note.".to_string());
    }
    if state.capture_finalizing.lock().unwrap().contains(&name) {
        return Err(
            "Capture is still saving audio. Try writing the note again in a moment.".to_string(),
        );
    }
    let settings = state.settings.lock().unwrap().clone();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.distill_cancel.lock().unwrap();
        // Mark any prior in-flight distill for this session as cancelled before
        // taking over — its emit closures hold the old Arc and will silently drop.
        if let Some(prev) = slot.remove(&name) {
            prev.store(true, Ordering::SeqCst);
        }
        slot.insert(name.clone(), cancel.clone());
    }
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let _ = std::fs::create_dir_all(&margins_dir);
    let _ = std::fs::create_dir_all(&trace_dir);
    let _ = session::clear_note_error(&margins_dir, &name);
    // Advisory progress only: terminal UI status must come from note_error and saved-note state.
    let _ = session::set_processing_state(&margins_dir, &name, "none", None);
    let trace_path = trace_dir.join(format!("{}_distill_trace.jsonl", name));
    let trace_file = std::fs::File::create(&trace_path)
        .ok()
        .map(|f| Arc::new(Mutex::new(f)));
    let steps_path = margins_dir.join(format!("{}_margins_trace.md", name));
    let steps_file = std::fs::File::create(&steps_path).ok().map(|mut f| {
        let _ = writeln!(f, "# Margins trace: {name}\n");
        Arc::new(Mutex::new(f))
    });
    let run_started_at = Instant::now();

    let trace_for_emit = trace_file.clone();
    let steps_for_emit = steps_file.clone();
    let cancel_for_emit = cancel.clone();
    let emit = |stage: &str, msg: &str, progress: Option<f32>| {
        if cancel_for_emit.load(Ordering::SeqCst) {
            return;
        }
        let event = ProcessingEvent::timed(&run_started_at, stage, msg, progress);
        write_processing_trace(&trace_for_emit, &event);
        write_processing_steps_markdown(&steps_for_emit, &event);
        let _ = app.emit("processing-progress", event);
    };
    let trace_for_lifecycle_emit = trace_file.clone();
    let steps_for_lifecycle_emit = steps_file.clone();
    let cancel_for_lifecycle_emit = cancel.clone();
    let emit_lifecycle =
        |track: ProcessingTrack, phase: &str, stage: &str, msg: &str, progress: Option<f32>| {
            if cancel_for_lifecycle_emit.load(Ordering::SeqCst) {
                return;
            }
            let event = ProcessingEvent::timed_lifecycle(
                &run_started_at,
                &name,
                track,
                phase,
                stage,
                msg,
                progress,
            );
            write_processing_trace(&trace_for_lifecycle_emit, &event);
            write_processing_steps_markdown(&steps_for_lifecycle_emit, &event);
            let _ = app.emit("processing-progress", event);
        };

    let mut cancel_guard = DistillCancelGuard {
        state: Arc::clone(&*state),
        name: name.clone(),
        owner: cancel.clone(),
        released: false,
    };
    let mut failed_stage: Option<&'static str> = None;
    macro_rules! note_job {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(err) => {
                    let message = err.to_string();
                    if !message.contains(DISTILL_CANCELLED_SENTINEL) {
                        let clean = message.trim_start_matches("Error: ").trim().to_string();
                        let saved = if clean.is_empty() {
                            "Margins could not finish the note.".to_string()
                        } else {
                            clean
                        };
                        let _ =
                            session::set_note_failure(&margins_dir, &name, &saved, failed_stage);
                        emit_lifecycle(ProcessingTrack::Note, "failed", "error", &saved, None);
                    }
                    return Err(message);
                }
            }
        };
    }

    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    emit_lifecycle(
        ProcessingTrack::Capture,
        "captured",
        "prepare",
        "Capture saved.",
        Some(0.02),
    );
    emit_lifecycle(
        ProcessingTrack::Note,
        "preparing_context",
        "prepare",
        "Preparing context from marks and capture.",
        Some(0.08),
    );
    let meta = note_job!(session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string()));
    note_job!(verify_capture_ready_for_processing(
        &work_dir,
        &margins_dir,
        &meta
    ));

    let memo_path = session_memo_path(&work_dir, &name);

    let capture_context = note_job!(build_session_capture_context(
        &margins_dir,
        &name,
        &memo_path
    ));
    emit(
        "context",
        &format!(
            "Using desktop capture context ({}).",
            capture_context.summary
        ),
        Some(0.6),
    );

    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    let refresh_transcript = transcript_refresh_requested(force_transcribe, max_speakers);
    let resolved_transcript = note_job!(resolve_process_session_transcript(
        &margins_dir,
        &name,
        force_transcribe,
        max_speakers,
        || {
            failed_stage = Some("transcribe");
            let _ = session::set_processing_state(&margins_dir, &name, "transcribing", None);
            let preparing_message = if refresh_transcript {
                "Refreshing transcript before writing the note..."
            } else {
                "Building full transcript..."
            };
            emit_lifecycle(
                ProcessingTrack::Transcript,
                "preparing",
                "align",
                preparing_message,
                Some(0.3),
            );
            let transcript_started = Instant::now();
            let summary = write_aligned_sidecar(
                &app,
                &work_dir,
                &margins_dir,
                &name,
                &memo_path,
                &settings,
                max_speakers,
            )?;
            emit_lifecycle(
                ProcessingTrack::Transcript,
                "ready",
                "align",
                &format!(
                    "Transcript ready ({summary}, {} ms).",
                    transcript_started.elapsed().as_millis()
                ),
                Some(0.5),
            );
            read_session_transcript_artifact(&margins_dir, &name).ok_or_else(|| {
                "Transcript preparation completed but the transcript artifact could not be loaded."
                    .to_string()
            })
        },
    ));
    if resolved_transcript.source == ProcessSessionTranscriptSource::ReusedCheckpoint {
        let prepared_kind = if resolved_transcript
            .content
            .contains("Transcript source: `live_qualified`")
        {
            "qualified terminal live transcript"
        } else {
            "prepared transcript"
        };
        emit_lifecycle(
            ProcessingTrack::Transcript,
            "ready",
            "align",
            &format!(
                "Using {prepared_kind} ({} transcript entries).",
                transcript_count_in_markdown(&resolved_transcript.content)
            ),
            Some(0.5),
        );
    }
    let aligned_context = resolved_transcript.content;

    // A silent or memo-only capture legitimately has zero spoken entries. As long
    // as we hold a genuine terminal transcript for this session (validated write,
    // correct `Session:` header), that is a valid — if sparse — note source, so we
    // let it distill from marks/memo/title rather than hard-failing. Only bail when
    // there is neither speech nor even a valid terminal artifact to distill from.
    let aligned_transcript_count = transcript_count_in_markdown(&aligned_context);
    if aligned_transcript_count == 0
        && !is_valid_terminal_transcript_checkpoint(&aligned_context, &name)
    {
        failed_stage = Some("transcribe");
        note_job!(Err::<(), String>(
            "No transcript entries were available after transcript preparation. Margins needs a full transcript before writing the note.".to_string(),
        ));
    }
    // Audio is deleted only after the note is saved (see the done state below),
    // so a distillation failure can still fall back to re-transcribing from audio
    // rather than depending solely on the transcript checkpoint.

    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    // Step 5: Distill through the embedded AI agent runtime. This reuses the
    // configured ChatGPT login or API-compatible endpoint and adds context tools
    // that mirror the Margins skill's MCP contract.
    emit(
        "synthesize",
        "Starting connected note distillation...",
        Some(0.6),
    );
    failed_stage = Some("distill");
    let _ = session::set_processing_state(&margins_dir, &name, "distilling", None);
    emit_lifecycle(
        ProcessingTrack::Note,
        "writing",
        "synthesize",
        "Writing note.",
        Some(0.62),
    );
    // Enzyme context + the notes destination are anchored to the active project
    // ROOT. Note-saving joins the inbox subfolder onto this root; enzyme is
    // operated against this root directly.
    let vault_path = vault_root(&settings);
    let (
        (ai_provider, ai_model, ai_api_key, ai_credential_generation),
        (prep_ai_provider, prep_ai_model, prep_ai_api_key, prep_ai_credential_generation),
    ) = note_job!(configure_ai_for_note_job(&settings).await);
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let existing_note_path = if overwrite_existing_note.unwrap_or(false) {
        meta.vault_note_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
    } else {
        None
    };
    let pi_request = pi_distill::PiDistillRequest {
        work_dir: work_dir.clone(),
        margins_dir: margins_dir.clone(),
        trace_dir: trace_dir.clone(),
        session_name: name.clone(),
        memo_path: memo_path.clone(),
        capture_context: capture_context.content,
        aligned_context,
        vault_path,
        note_config,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
        prep_ai_provider,
        prep_ai_model,
        prep_ai_api_key,
        prep_ai_credential_generation,
        skill_path: note_job!(desktop_distill_skill_path(&app)),
        cancel: cancel.clone(),
        resume_session_path: None,
        refine_message: None,
        existing_note_path,
        save_generated_note: true,
    };
    let app_for_pi = app.clone();
    let trace_for_pi = trace_file.clone();
    let steps_for_pi = steps_file.clone();
    let cancel_for_pi_emit = cancel.clone();
    let pi_run_started_at = run_started_at;
    let distill_timeout = distill_timeout();
    let distill_cancel = cancel.clone();
    let join_handle = crate::async_runtime::spawn_blocking(move || {
        pi_distill::run_pi_distill_blocking(pi_request, move |stage, message, progress| {
            if cancel_for_pi_emit.load(Ordering::SeqCst) {
                return;
            }
            let event = ProcessingEvent::timed(&pi_run_started_at, stage, message, progress);
            write_processing_trace(&trace_for_pi, &event);
            write_processing_steps_markdown(&steps_for_pi, &event);
            let _ = app_for_pi.emit("processing-progress", event);
        })
    });
    let join = note_job!(
        match tokio::time::timeout(distill_timeout, join_handle).await {
            Ok(result) => result.map_err(|e| format!("AI note distillation task failed: {e}")),
            Err(_) => {
                distill_cancel.store(true, Ordering::SeqCst);
                Err(format!(
                    "Note generation timed out after {}s",
                    distill_timeout.as_secs()
                ))
            }
        }
    );

    // Treat cancellation as terminal regardless of how the blocking task wound down.
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    let outcome = note_job!(join);

    failed_stage = Some("save");
    emit_lifecycle(
        ProcessingTrack::Note,
        "saving",
        "cleanup",
        "Saving note.",
        Some(0.95),
    );
    if let Some(np) = &outcome.note_path {
        if session::is_session_tombstoned(&margins_dir, &name).map_err(|e| e.to_string())? {
            let _ = delete_file_if_present(np);
            return Err(DISTILL_CANCELLED_SENTINEL.to_string());
        }
        if let Err(err) = session::set_vault_note_path(&margins_dir, &name, np) {
            if session::is_session_tombstoned(&margins_dir, &name).unwrap_or(false) {
                let _ = delete_file_if_present(np);
                return Err(DISTILL_CANCELLED_SENTINEL.to_string());
            }
            let message = err.to_string();
            let _ = session::set_note_failure(&margins_dir, &name, &message, failed_stage);
            emit_lifecycle(ProcessingTrack::Note, "failed", "error", &message, None);
            return Err(message);
        }
        pi_distill::remove_note_draft(&margins_dir, &name);
        pi_distill::remove_note_draft(&trace_dir, &name);
    }
    if session_transcript_artifact_path(&margins_dir, &name).exists() {
        let _ = register_transcript_artifact(&margins_dir, &name);
    }
    if read_session_transcript_artifact(&margins_dir, &name).is_some()
        && session_capture_context_scratch_path(&margins_dir, &name).exists()
    {
        let _ = register_capture_context_scratch_artifact(&margins_dir, &name);
    }
    // Remember the persisted Pi session so the user can refine the note later by
    // resuming this exact conversation.
    persist_pi_session_file(&trace_dir, &name, outcome.session_file.as_deref());

    emit_lifecycle(
        ProcessingTrack::Note,
        "saved",
        "complete",
        "Note saved.",
        Some(1.0),
    );
    if outcome.note_path.is_some() {
        schedule_background_enzyme_refresh(&app, &settings, project_id.as_deref());
    }
    // Note is saved — now it is safe to reclaim the captured audio. Deferring the
    // deletion to this point keeps audio available as a re-transcribe fallback for
    // any earlier failure/retry.
    delete_audio_after_aligned_transcript(&work_dir, &margins_dir, &name, &meta);
    let _ = session::set_processing_state(&margins_dir, &name, "done", None);
    cancel_guard.release();
    Ok(())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn retry_session(
    app: AppHandle,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    process_session(app, name, project_id, Some(false), None, Some(false)).await
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn save_draft_note(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
    overwrite_existing_note: Option<bool>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let meta = session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string())?;
    let draft_path = margins_dir.join(format!("{name}_note_draft.md"));
    let markdown =
        std::fs::read_to_string(&draft_path).map_err(|e| format!("Cannot read draft note: {e}"))?;
    if markdown.trim().is_empty() {
        return Err("Draft note is empty.".to_string());
    }
    let vault_path = vault_root(&settings);
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let existing_note_path = if overwrite_existing_note.unwrap_or(false) {
        meta.vault_note_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
    } else {
        None
    };
    let note_path = pi_distill::save_markdown_note(pi_distill::SaveMarkdownRequest {
        vault_path: vault_path.as_deref(),
        margins_dir: &margins_dir,
        session_name: &name,
        note_config: &note_config,
        existing_note_path: existing_note_path.as_deref(),
        markdown: &markdown,
        grounding: None,
        cancel: None,
    })
    .map_err(|e| format!("failed to save draft note: {e}"))?;
    let note_path = note_path.to_string_lossy().to_string();
    session::set_vault_note_path(&margins_dir, &name, &note_path).map_err(|e| e.to_string())?;
    pi_distill::remove_note_draft(&margins_dir, &name);
    pi_distill::remove_note_draft(&trace_dir, &name);
    Ok(note_path)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn discard_note(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    delete_session_fully(&work_dir, &margins_dir, &name)
}

/// Sidecar path recording the persisted Pi session file for a capture, written
/// after distillation so a later `refine_session` can resume the conversation.
fn pi_session_pointer_path(trace_dir: &Path, name: &str) -> PathBuf {
    trace_dir.join(format!("{name}_pi_session.txt"))
}

fn persist_pi_session_file(trace_dir: &Path, name: &str, session_file: Option<&str>) {
    let pointer = pi_session_pointer_path(trace_dir, name);
    match session_file {
        Some(path) if !path.trim().is_empty() => {
            let _ = std::fs::write(&pointer, path);
        }
        // A fresh distill couldn't be resumed from a stale pointer; clear it.
        _ => {
            let _ = std::fs::remove_file(&pointer);
        }
    }
}

static PI_SESSION_MIGRATION_COUNTER: AtomicU64 = AtomicU64::new(0);

fn validate_migrated_pi_header(json: &str, source: &Path) -> Result<serde_json::Value, String> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|error| {
        format!(
            "Legacy Pi conversation {} has an invalid header: {error}",
            source.display()
        )
    })?;
    if value.get("type").and_then(Value::as_str) != Some("session")
        || value.get("id").and_then(Value::as_str).is_none()
    {
        return Err(format!(
            "Legacy Pi conversation {} has an unrecognized session header",
            source.display()
        ));
    }
    Ok(value)
}

fn migrated_pi_session_matches(target: &Path, expected_jsonl: &str) -> Result<bool, String> {
    let contents = std::fs::read_to_string(target).map_err(|error| {
        format!(
            "failed to read migrated Pi conversation {}: {error}",
            target.display()
        )
    })?;
    for (index, line) in contents.lines().enumerate() {
        serde_json::from_str::<serde_json::Value>(line).map_err(|error| {
            format!(
                "migrated Pi conversation {} has invalid JSONL record {}: {error}",
                target.display(),
                index + 1
            )
        })?;
    }
    Ok(contents == expected_jsonl)
}

fn publish_migrated_pi_session(target: &Path, expected_jsonl: &str) -> Result<(), String> {
    if target.exists() {
        return if migrated_pi_session_matches(target, expected_jsonl)? {
            Ok(())
        } else {
            Err(format!(
                "{} already exists with content that differs from the complete legacy migration",
                target.display()
            ))
        };
    }

    let sequence = PI_SESSION_MIGRATION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let temporary =
        target.with_extension(format!("jsonl.migrating-{}-{sequence}", std::process::id()));
    let write_result = (|| -> Result<(), String> {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("failed to create {}: {error}", temporary.display()))?;
        let mut writer = std::io::BufWriter::new(file);
        writer
            .write_all(expected_jsonl.as_bytes())
            .map_err(|error| format!("failed to write {}: {error}", temporary.display()))?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", temporary.display()))?;
        writer
            .get_ref()
            .sync_all()
            .map_err(|error| format!("failed to sync {}: {error}", temporary.display()))
    })();
    if let Err(error) = write_result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }

    // A hard link publishes the already-synced inode atomically and, unlike
    // rename, can never replace a target won by another migrator.
    let publish_result = std::fs::hard_link(&temporary, target);
    let _ = std::fs::remove_file(&temporary);
    match publish_result {
        Ok(()) => Ok(()),
        Err(error) if target.exists() => {
            if migrated_pi_session_matches(target, expected_jsonl)? {
                Ok(())
            } else {
                Err(format!(
                    "failed to publish {} without replacement: {error}; the concurrent target differs from the complete legacy migration",
                    target.display()
                ))
            }
        }
        Err(error) => Err(format!(
            "failed to publish {} without replacement: {error}",
            target.display()
        )),
    }
}

fn migrate_legacy_pi_session(path: &Path) -> Result<PathBuf, String> {
    let is_legacy_sqlite = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| extension.eq_ignore_ascii_case("sqlite"));
    if !is_legacy_sqlite {
        return Ok(path.to_path_buf());
    }

    let connection = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| {
        format!(
            "Legacy Pi conversation {} could not be opened for migration; the original was retained: {error}",
            path.display()
        )
    })?;
    let header: String = connection
        .query_row(
            "SELECT json FROM pi_session_header LIMIT 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| {
            format!(
                "Legacy Pi conversation {} could not be migrated; the original was retained: {error}",
                path.display()
            )
        })?;
    validate_migrated_pi_header(&header, path)?;
    let mut statement = connection
        .prepare("SELECT json FROM pi_session_entries ORDER BY seq ASC")
        .map_err(|error| {
            format!(
                "Legacy Pi conversation {} could not be migrated; the original was retained: {error}",
                path.display()
            )
        })?;
    let entries = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| {
            format!(
                "Legacy Pi conversation {} could not be migrated; the original was retained: {error}",
                path.display()
            )
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            format!(
                "Legacy Pi conversation {} could not be migrated; the original was retained: {error}",
                path.display()
            )
        })?;
    for entry in &entries {
        serde_json::from_str::<serde_json::Value>(entry).map_err(|error| {
            format!(
                "Legacy Pi conversation {} contains an invalid entry; the original was retained: {error}",
                path.display()
            )
        })?;
    }
    drop(statement);
    drop(connection);

    let target = path.with_extension("jsonl");
    let mut expected_jsonl = String::new();
    expected_jsonl.push_str(&header);
    expected_jsonl.push('\n');
    for entry in &entries {
        expected_jsonl.push_str(entry);
        expected_jsonl.push('\n');
    }
    publish_migrated_pi_session(&target, &expected_jsonl).map_err(|error| {
        format!(
            "Legacy Pi conversation {} could not be migrated; the original was retained: {error}",
            path.display()
        )
    })?;
    Ok(target)
}

fn read_pi_session_file(trace_dir: &Path, name: &str) -> Result<Option<PathBuf>, String> {
    let pointer = pi_session_pointer_path(trace_dir, name);
    let Ok(contents) = std::fs::read_to_string(&pointer) else {
        return Ok(None);
    };
    let raw_path = contents.trim();
    if raw_path.is_empty() {
        return Ok(None);
    }
    let path = PathBuf::from(raw_path);
    if !path.exists() {
        return Ok(None);
    }
    let migrated = migrate_legacy_pi_session(&path)?;
    if migrated != path {
        write_atomic_utf8(&pointer, &migrated.to_string_lossy()).map_err(|error| {
            format!(
                "Pi conversation migrated to {}, but its pointer could not be updated: {error}",
                migrated.display()
            )
        })?;
    }
    Ok(Some(migrated))
}

/// Refine an already-distilled note by resuming its persisted Pi conversation
/// and applying `message` as the next turn. Overwrites the saved note in place.
/// Streams through the same `processing-progress` channel as `process_session`,
/// so the frontend renders it as an active processing turn.
#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn refine_session(
    app: AppHandle,
    name: String,
    message: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("Enter a change to make.".to_string());
    }
    let state = app.state::<Arc<AppState>>();
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);

    // Refine resumes the conversation that produced the note. Both pieces must
    // exist: the persisted Pi session and the saved note we'll overwrite.
    let resume_session_path = read_pi_session_file(&trace_dir, &name)?
        .ok_or_else(|| "This note can't be refined yet. Reprocess it first.".to_string())?;
    let meta = session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string())?;
    let existing_note_path = meta
        .vault_note_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or_else(|| "Save a note before asking for changes.".to_string())?;

    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.distill_cancel.lock().unwrap();
        if let Some(prev) = slot.remove(&name) {
            prev.store(true, Ordering::SeqCst);
        }
        slot.insert(name.clone(), cancel.clone());
    }

    let _ = std::fs::create_dir_all(&margins_dir);
    let _ = std::fs::create_dir_all(&trace_dir);
    // Append refine turns to the same trace files so the provenance log grows
    // rather than resetting.
    let trace_path = trace_dir.join(format!("{}_distill_trace.jsonl", name));
    let trace_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&trace_path)
        .ok()
        .map(|f| Arc::new(Mutex::new(f)));
    let steps_path = margins_dir.join(format!("{}_margins_trace.md", name));
    let steps_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&steps_path)
        .ok()
        .map(|mut f| {
            let _ = writeln!(f, "\n## Refine: {message}\n");
            Arc::new(Mutex::new(f))
        });
    let run_started_at = Instant::now();

    let mut cancel_guard = DistillCancelGuard {
        state: Arc::clone(&*state),
        name: name.clone(),
        owner: cancel.clone(),
        released: false,
    };
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    // Enzyme context + the notes destination are anchored to the active project
    // ROOT. Note-saving joins the inbox subfolder onto this root; enzyme is
    // operated against this root directly.
    let vault_path = vault_root(&settings);
    let (
        (ai_provider, ai_model, ai_api_key, ai_credential_generation),
        (prep_ai_provider, prep_ai_model, prep_ai_api_key, prep_ai_credential_generation),
    ) = configure_ai_for_note_job(&settings).await?;
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let memo_path = session_memo_path(&work_dir, &name);
    let pi_request = pi_distill::PiDistillRequest {
        work_dir: work_dir.clone(),
        margins_dir: margins_dir.clone(),
        trace_dir: trace_dir.clone(),
        session_name: name.clone(),
        memo_path,
        // On resume the conversation already holds the transcript/templates; the
        // refine prompt replaces the heavy first-pass context.
        capture_context: String::new(),
        aligned_context: String::new(),
        vault_path,
        note_config,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
        prep_ai_provider,
        prep_ai_model,
        prep_ai_api_key,
        prep_ai_credential_generation,
        skill_path: desktop_distill_skill_path(&app)?,
        cancel: cancel.clone(),
        resume_session_path: Some(resume_session_path),
        refine_message: Some(message),
        existing_note_path: Some(existing_note_path),
        save_generated_note: true,
    };

    let app_for_pi = app.clone();
    let trace_for_pi = trace_file.clone();
    let steps_for_pi = steps_file.clone();
    let cancel_for_pi_emit = cancel.clone();
    let pi_run_started_at = run_started_at;
    let join = crate::async_runtime::spawn_blocking(move || {
        pi_distill::run_pi_distill_blocking(pi_request, move |stage, message, progress| {
            if cancel_for_pi_emit.load(Ordering::SeqCst) {
                return;
            }
            let event = ProcessingEvent::timed(&pi_run_started_at, stage, message, progress);
            write_processing_trace(&trace_for_pi, &event);
            write_processing_steps_markdown(&steps_for_pi, &event);
            let _ = app_for_pi.emit("processing-progress", event);
        })
    })
    .await
    .map_err(|e| format!("Refine task failed: {e}"))?;

    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }

    let outcome = join?;
    if let Some(np) = &outcome.note_path {
        if session::is_session_tombstoned(&margins_dir, &name).map_err(|e| e.to_string())? {
            let _ = delete_file_if_present(np);
            return Err(DISTILL_CANCELLED_SENTINEL.to_string());
        }
        if let Err(err) = session::set_vault_note_path(&margins_dir, &name, np) {
            if session::is_session_tombstoned(&margins_dir, &name).unwrap_or(false) {
                let _ = delete_file_if_present(np);
                return Err(DISTILL_CANCELLED_SENTINEL.to_string());
            }
            return Err(err.to_string());
        }
    }
    persist_pi_session_file(&trace_dir, &name, outcome.session_file.as_deref());

    emit_refine_complete(&app, &trace_file, &steps_file, &run_started_at);
    if outcome.note_path.is_some() {
        schedule_background_enzyme_refresh(&app, &settings, project_id.as_deref());
    }
    cancel_guard.release();
    Ok(())
}

#[cfg(feature = "tauri-app")]
fn emit_refine_complete(
    app: &AppHandle,
    trace_file: &Option<Arc<Mutex<std::fs::File>>>,
    steps_file: &Option<Arc<Mutex<std::fs::File>>>,
    run_started_at: &Instant,
) {
    let event = ProcessingEvent::timed(run_started_at, "complete", "Refine complete!", Some(1.0));
    write_processing_trace(trace_file, &event);
    write_processing_steps_markdown(steps_file, &event);
    let _ = app.emit("processing-progress", event);
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn reprocess_session_with_people(
    app: AppHandle,
    name: String,
    people: Vec<String>,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let people = normalize_people(people);
    if people.is_empty() {
        return Err("Add at least one person before reprocessing.".to_string());
    }

    let state = app.state::<Arc<AppState>>();
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();

    let vault_path = vault_root(&settings);
    let people_folder = settings.people_folder.clone();
    // Reprocess runs on the same "fast model" that powers live cues, not the
    // heavier distillation model. configure_ai_for_reprocess delegates to the
    // live-cues (backchannel) resolver, so a user's separate cue model / key /
    // base_url is honored identically — base_url rides along inside the resolved
    // provider (see openai_compatible_config).
    let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
        configure_ai_for_reprocess(&settings).await?;

    let request = reprocess::ReprocessRequest {
        session_name: name.clone(),
        people,
        work_dir: work_dir.clone(),
        margins_dir,
        vault_path,
        people_folder,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
    };

    let run_started_at = Instant::now();
    let app_for_emit = app.clone();
    let join = crate::async_runtime::spawn_blocking(move || {
        reprocess::run_reprocess(request, move |stage, message, progress| {
            let event = ProcessingEvent::timed(&run_started_at, stage, message, progress);
            let _ = app_for_emit.emit("processing-progress", event);
        })
    })
    .await
    .map_err(|e| format!("Reprocess task failed: {e}"))?;

    join
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_aligned_content(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    // Keep the command name for API compatibility, but prefer the durable
    // transcript artifact. Fall back to legacy aligned/capture-context files for
    // sessions processed before transcript artifacts existed.
    if let Some(content) = read_session_transcript_artifact(&margins_dir, &name) {
        return Ok(content);
    }
    snapshot_active_live_transcript_if_current(&state, &work_dir, &name)?;
    let memo_path = session_memo_path(&work_dir, &name);
    build_session_capture_context(&margins_dir, &name, &memo_path).map(|context| context.content)
}

fn snapshot_active_live_transcript_if_current(
    state: &Arc<AppState>,
    work_dir: &Path,
    name: &str,
) -> Result<(), String> {
    let request = {
        let guard = state.recording.lock().unwrap();
        let Some(rec) = guard.as_ref() else {
            return Ok(());
        };
        if rec.session_name != name || !same_directory(&rec.work_dir, work_dir) {
            return Ok(());
        }
        let Some(client) = rec.live_backchannel.as_ref().map(|live| live.client()) else {
            return Ok(());
        };
        (
            rec.session_name.clone(),
            rec.work_dir.clone(),
            rec.memo_lines.clone(),
            (Local::now() - rec.start_time).num_milliseconds().max(0) as u64,
            client,
        )
    };

    let (session_name, active_work_dir, memo_lines, end_ms, client) = request;
    persist_live_memo(&active_work_dir, &session_name, &memo_lines)?;
    let snapshot = client.request_context(end_ms)?;
    let margins_dir = active_work_dir.join(".margins");
    transcript_store::append_live_transcript_segment(
        &margins_dir,
        &session_name,
        "snapshot",
        None,
        None,
        &snapshot,
    )?;
    let _ = append_backchannel_trace(
        &margins_dir,
        &session_name,
        json!({
            "kind":"live_transcript_read_snapshot",
            "decoded_until_ms": snapshot.decoded_until_ms,
            "committed_until_ms": snapshot.committed_until_ms,
            "previous_memo_checkpoint_ms": snapshot.previous_memo_checkpoint_ms,
            "transcript_source": snapshot.transcript_source,
            "has_transcript": !snapshot.transcript.trim().is_empty(),
            "has_new_transcript_since_previous_snapshot": !snapshot.new_transcript.trim().is_empty(),
        }),
    );
    Ok(())
}

fn same_directory(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_session_memo(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    std::fs::read_to_string(session_memo_path(&work_dir, &name))
        .map_err(|e| format!("Cannot read memo file: {e}"))
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_vault_note(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    let path = note_path_for_session_or_capture(&margins_dir, &name, &settings)?;
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_session_grounding(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<Option<Value>, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let settings = state.settings.lock().unwrap().clone();
    let margins_dir = settings_trace_margins_dir(&settings, &work_dir);
    let grounding =
        session::get_session_grounding(&margins_dir, &name).map_err(|e| e.to_string())?;
    if grounding.is_empty() {
        Ok(None)
    } else {
        Ok(Some(json!({ "uses": grounding })))
    }
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_distill_trace(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<Vec<ProcessingEvent>, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let settings = state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let path = trace_dir.join(format!("{}_distill_trace.jsonl", name));
    let fallback_path = work_dir
        .join(".margins")
        .join(format!("{}_distill_trace.jsonl", name));
    let path = if path.exists() { path } else { fallback_path };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<ProcessingEvent>(line).ok())
        .collect())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn open_note(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    if matches!(
        editor_preference(settings.editor_command.as_deref()),
        EditorPreference::Obsidian
    ) {
        return open_session_note_in_obsidian(&margins_dir, &name, &settings);
    }
    let path = note_path_for_session_or_capture(&margins_dir, &name, &settings)?;
    open_path_with_editor(&path, settings.editor_command.as_deref())
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn open_note_in_obsidian(
    state: tauri::State<'_, Arc<AppState>>,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = state.settings.lock().unwrap().clone();
    open_session_note_in_obsidian(&margins_dir, &name, &settings)
}

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn open_note_target_in_obsidian(
    state: tauri::State<'_, Arc<AppState>>,
    target: String,
    project_id: Option<String>,
) -> Result<(), String> {
    let clean_target = target
        .trim()
        .trim_start_matches('/')
        .trim_end_matches(".md")
        .trim();
    if clean_target.is_empty() || clean_target.contains('\0') {
        return Err("Linked note target is empty.".to_string());
    }
    let settings = state.settings.lock().unwrap().clone();
    let vault = vault_root_for_project(&settings, project_id.as_deref())?;
    open_obsidian_file_target(&vault, clean_target)
}

fn open_session_note_in_obsidian(
    margins_dir: &std::path::Path,
    name: &str,
    settings: &Settings,
) -> Result<(), String> {
    let vault = settings
        .vault_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(expand_tilde)
        .map(PathBuf::from)
        .ok_or_else(|| "No notes folder is configured.".to_string())?;

    if !vault.join(".obsidian").exists() {
        return Err(
            "Obsidian opening is only available for folders that already contain .obsidian."
                .to_string(),
        );
    }

    let note_path = note_path_for_session_or_capture(margins_dir, name, settings)?;
    let note_abs = note_path.canonicalize().unwrap_or(note_path);
    let vault_abs = vault.canonicalize().unwrap_or(vault);
    let rel = note_abs
        .strip_prefix(&vault_abs)
        .map_err(|_| "The saved note is not inside the configured Obsidian vault.".to_string())?;
    let vault_name = vault_abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Could not determine Obsidian vault name.".to_string())?;
    let file = rel.to_string_lossy().trim_end_matches(".md").to_string();
    let uri = format!(
        "obsidian://open?vault={}&file={}",
        uri_encode(vault_name),
        uri_encode(&file),
    );
    Command::new("open")
        .arg(uri)
        .status()
        .map_err(|e| format!("failed to open Obsidian URI: {e}"))?;
    Ok(())
}

fn open_obsidian_file_target(vault: &Path, file_target: &str) -> Result<(), String> {
    if !vault.join(".obsidian").exists() {
        return Err(
            "Obsidian opening is only available for folders that already contain .obsidian."
                .to_string(),
        );
    }
    let vault_abs = vault.canonicalize().unwrap_or_else(|_| vault.to_path_buf());
    let vault_name = vault_abs
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| "Could not determine Obsidian vault name.".to_string())?;
    let uri = format!(
        "obsidian://open?vault={}&file={}",
        uri_encode(vault_name),
        uri_encode(file_target),
    );
    Command::new("open")
        .arg(uri)
        .status()
        .map_err(|e| format!("failed to open Obsidian URI: {e}"))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn expand_tilde(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    path.to_string()
}

fn settings_trace_margins_dir(settings: &Settings, work_dir: &Path) -> PathBuf {
    settings_work_dir(settings, work_dir).join(".margins")
}

fn existing_path(path: PathBuf) -> Option<PathBuf> {
    path.exists().then_some(path)
}

fn resource_skill_path_from_checkout(relative: &str) -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let desktop_path = manifest.join("resources").join(relative);
    if let Some(path) = existing_path(desktop_path) {
        return Some(path);
    }
    if let Some(path) = existing_path(manifest.join("../..").join(relative)) {
        return Some(path);
    }
    relative.strip_prefix("skills/").and_then(|skill| {
        existing_path(
            manifest
                .join("../../crates/public/margins-workflows/resources/skills")
                .join(skill),
        )
    })
}

#[cfg(feature = "tauri-app")]
fn app_resource_path(app: &AppHandle, relative: impl AsRef<Path>) -> Option<PathBuf> {
    let relative = relative.as_ref();
    app.path()
        .resolve(relative, BaseDirectory::Resource)
        .ok()
        .and_then(existing_path)
        .or_else(|| {
            app.path()
                .resource_dir()
                .ok()
                .map(|dir| dir.join(relative))
                .and_then(existing_path)
        })
}

#[cfg(feature = "tauri-app")]
fn desktop_distill_skill_path(app: &AppHandle) -> Result<PathBuf, String> {
    app_resource_path(app, "skills/margins/hosts/desktop.md")
        .or_else(|| {
            #[cfg(debug_assertions)]
            {
                resource_skill_path_from_checkout("skills/margins/hosts/desktop.md")
            }
            #[cfg(not(debug_assertions))]
            {
                None
            }
        })
        .ok_or_else(|| {
            "Cannot find bundled Margins Desktop distillation host preamble (skills/margins/hosts/desktop.md). Reinstall Margins."
                .to_string()
        })
}

/// Legacy no-op kept for older frontend bundles. Setup handoffs now point to
/// `margins guide workspace-setup`, which is embedded in the CLI sidecar.
#[cfg(feature = "tauri-app")]
#[tauri::command]
async fn install_workspace_skills(_app: AppHandle, project_path: String) -> Result<(), String> {
    crate::async_runtime::spawn_blocking(move || {
        let root = clean_project_root(&project_path)?;
        if !root.exists() {
            return Err(format!("Project folder not found: {}", root.display()));
        }
        Ok(())
    })
    .await
    .map_err(|e| format!("Workspace guide check task failed: {e}"))?
}

#[cfg(not(feature = "tauri-app"))]
pub fn install_workspace_skills_from_checkout(project_path: &str) -> Result<(), String> {
    let root = clean_project_root(project_path)?;
    if !root.exists() {
        return Err(format!("Project folder not found: {}", root.display()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Dev-only: harness snapshot export
// ---------------------------------------------------------------------------

/// When `MARGINS_EXPORT_HARNESS_SNAPSHOT=<path>` is set, dump the real local app
/// state (sanitized settings + the indexed session list + a few sample note
/// bodies) to `<path>` and exit before launching Tauri. The browser UX harness
/// loads this gitignored fixture so previews resemble the actual desktop app.
fn export_harness_snapshot(work_dir: &Path, settings: &Settings, out: &Path) {
    let sessions = match session_index::list_sessions_with_notes(work_dir, settings, None, None) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("margins: harness snapshot failed to list sessions: {e}");
            return;
        }
    };
    let sessions_val = serde_json::to_value(&sessions).unwrap_or(serde_json::Value::Null);

    let mut sanitized = settings.clone();
    sanitized.api_key = sanitized.api_key.as_ref().map(|_| "••••••••".to_string());
    sanitized.backchannel_api_key = sanitized
        .backchannel_api_key
        .as_ref()
        .map(|_| "••••••••".to_string());

    let margins_dir = work_dir.join(".margins");
    let mut samples = serde_json::Map::new();
    if let Some(arr) = sessions_val.as_array() {
        for s in arr {
            if samples.len() >= 3 {
                break;
            }
            let name = match s.get("name").and_then(|v| v.as_str()) {
                Some(n) => n,
                None => continue,
            };
            let vault = s.get("vault_note_path").and_then(|v| v.as_str());
            let mut entry = serde_json::Map::new();
            if let Some(vault) = vault {
                if let Some(body) = read_snapshot_file(work_dir, vault) {
                    entry.insert("note".into(), serde_json::Value::String(body));
                }
            }
            if let Some(body) = read_session_transcript_artifact(&margins_dir, name) {
                entry.insert("aligned".into(), serde_json::Value::String(body));
            }
            if let Some(body) =
                std::fs::read_to_string(margins_dir.join(format!("{name}_memo.md"))).ok()
            {
                entry.insert("memo".into(), serde_json::Value::String(body));
            }
            if !entry.is_empty() {
                samples.insert(name.to_string(), serde_json::Value::Object(entry));
            }
        }
    }

    let snapshot = serde_json::json!({
        "settings": sanitized,
        "sessions": sessions_val,
        "samples": serde_json::Value::Object(samples),
    });
    match serde_json::to_string_pretty(&snapshot) {
        Ok(json) => {
            if let Some(parent) = out.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(out, json) {
                Ok(()) => println!(
                    "margins: wrote harness snapshot ({} sessions) to {}",
                    sessions.len(),
                    out.display()
                ),
                Err(e) => eprintln!("margins: failed to write harness snapshot: {e}"),
            }
        }
        Err(e) => eprintln!("margins: failed to serialize harness snapshot: {e}"),
    }
}

/// Read a note body for the snapshot, resolving `rel` against the vault dir when
/// it is not already an absolute, existing path.
fn read_snapshot_file(work_dir: &Path, rel: &str) -> Option<String> {
    let direct = PathBuf::from(rel);
    if direct.is_file() {
        return std::fs::read_to_string(&direct).ok();
    }
    std::fs::read_to_string(work_dir.join(rel)).ok()
}

// ---------------------------------------------------------------------------
// _impl wrappers — thin bridges between Ctx and the Tauri command bodies.
//
// Each `pub(crate) fn NAME_impl(ctx: &Ctx, ...)` extracts the state/sink from
// `ctx` and delegates to the same logic that the `#[tauri::command]` runs.
// The Tauri commands above remain as-is for the desktop path; the dispatch
// module calls these `_impl` functions for the headless server path (WP2).
//
// Commands that deeply depend on AppHandle (skill-path resolution, audio
// download emission) return an appropriate Err in headless mode.
// ---------------------------------------------------------------------------

/// Thin Ctx helper used by command bodies that need `Arc<AppState>` directly.
/// Pulled from `ctx.state`.
#[inline(always)]
fn state_from_ctx(ctx: &ctx::Ctx) -> &Arc<AppState> {
    &ctx.state
}

// ---- Settings ----

// get_settings_impl and update_settings_impl are already defined above.

pub(crate) async fn register_project_impl(
    ctx: &ctx::Ctx,
    _settings: Settings,
    project: ProjectSource,
) -> Result<RegisterProjectResult, String> {
    let state = state_from_ctx(ctx);
    let root = clean_project_root(&project.path)?;
    if !root.exists() {
        return Err(format!("Project folder not found: {}", root.display()));
    }
    if !root.is_dir() {
        return Err(format!("Project path is not a folder: {}", root.display()));
    }
    let margins_dir = root.join(".margins");
    std::fs::create_dir_all(&margins_dir)
        .map_err(|e| format!("Could not create {}: {e}", margins_dir.display()))?;
    let upsert_input = UpsertProject {
        path: &project.path,
        name: Some(&project.name)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        inbox_folder: Some(&project.inbox_folder)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        people_folder: Some(&project.people_folder)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        readiness: Some(&project.readiness)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
        id_hint: Some(&project.id)
            .filter(|s| !s.trim().is_empty())
            .map(String::as_str),
    };
    margins::project::upsert_project(&upsert_input)
        .map_err(|e| format!("Could not register project: {e}"))?;
    let settings = load_settings_async().await?;
    refresh_backchannel_snapshot_settings(&ctx.state, &settings).await;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(RegisterProjectResult {
        settings: redacted_settings(&settings),
        validation: project_validation(&root),
    })
}

pub(crate) async fn update_project_readiness_impl(
    ctx: &ctx::Ctx,
    id: String,
    readiness: String,
) -> Result<Settings, String> {
    let state = state_from_ctx(ctx);
    let cleaned = readiness.trim();
    if !matches!(cleaned, "ready" | "needs_setup" | "updating" | "error") {
        return Err(format!("Unknown project readiness: {readiness}"));
    }
    let mut settings = state.settings.lock().unwrap().clone();
    let project = settings
        .projects
        .iter_mut()
        .find(|project| project.id == id)
        .ok_or_else(|| format!("Unknown project: {id}"))?;
    project.readiness = cleaned.to_string();
    normalize_settings(&mut settings);
    save_settings_async(&settings).await?;
    let settings = load_settings_async().await?;
    refresh_backchannel_snapshot_settings(&ctx.state, &settings).await;
    *state.settings.lock().unwrap() = settings.clone();
    Ok(redacted_settings(&settings))
}

// ---- AI / Auth ----

pub(crate) async fn get_ai_status_impl() -> Result<AiStatus, String> {
    ai_auth::get_ai_status().await
}

pub(crate) fn get_included_ai_status_impl() -> Result<IncludedAiStatus, String> {
    let included_ready = ai_config::included_ai_key_ready();
    Ok(IncludedAiStatus {
        included_ready,
        message: if included_ready {
            "Included note-making is ready on this Mac.".to_string()
        } else {
            "Included note-making will create a usage-limited key before your first note."
                .to_string()
        },
    })
}

pub(crate) async fn prepare_included_ai_impl() -> Result<IncludedAiStatus, String> {
    ai_config::prepare_included_ai_key().await?;
    get_included_ai_status_impl()
}

pub(crate) async fn sign_in_chatgpt_impl() -> Result<AiStatus, String> {
    ai_auth::sign_in_chatgpt().await
}

/// Single readiness snapshot for all three AI modes, computed from the persisted
/// settings and the two external signals (cached included lease, ChatGPT
/// credential). Lets the settings UI drop its hand-stitched checks. A failed
/// ChatGPT status read degrades to "not ready" rather than failing the call.
pub(crate) async fn get_ai_readiness_impl() -> Result<ai_config::AiReadiness, String> {
    let settings = crate::settings::load_settings();
    let included_ready = ai_config::included_ai_key_ready();
    let chatgpt_ready = ai_auth::get_ai_status()
        .await
        .map(|status| status.chatgpt_authenticated)
        .unwrap_or(false);
    Ok(ai_config::ai_readiness(
        &settings,
        included_ready,
        chatgpt_ready,
    ))
}

/// Display-friendly preview of what the given (unsaved) settings would resolve
/// to per activity. Pure — no network/keychain — so the frontend can call it on
/// every debounced keystroke while the user edits AI settings.
pub(crate) fn preview_ai_resolution_impl(
    settings: Settings,
) -> Result<ai_config::ResolutionPreview, String> {
    ai_config::preview_ai_resolution(&settings)
}

// ---- Vault ----

pub(crate) fn validate_vault_impl(path: String) -> Result<serde_json::Value, String> {
    let p = PathBuf::from(expand_tilde(&path));
    let default_p = PathBuf::from(expand_tilde(&default_vault_path_string()));
    let can_create_default =
        p == default_p && p.parent().map(|parent| parent.exists()).unwrap_or(false);
    let usable = p.exists() || can_create_default;
    Ok(json!({
        "exists": usable,
        "has_obsidian": p.join(".obsidian").exists(),
        "has_recall_index": p.join(".margins").join("recall").join("index.db").exists(),
        "has_margins": p.join(".margins").exists(),
    }))
}

pub(crate) async fn index_vault_impl(path: String) -> Result<serde_json::Value, String> {
    crate::async_runtime::spawn_blocking(move || {
        let p = PathBuf::from(expand_tilde(&path));
        run_enzyme_index_command(&p)?;
        Ok(json!({
            "exists": true,
            "has_obsidian": p.join(".obsidian").exists(),
            "has_recall_index": p.join(".margins").join("recall").join("index.db").exists(),
            "has_margins": p.join(".margins").exists(),
        }))
    })
    .await
    .map_err(|e| format!("index task failed: {e}"))?
}

// ---- Calendar ----

pub(crate) async fn get_calendar_event_suggestion_impl(
    ctx: &ctx::Ctx,
    project_id: Option<String>,
) -> Result<CalendarSuggestionResult, String> {
    let settings = ctx.state.settings.lock().unwrap().clone();
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    crate::async_runtime::spawn_blocking(move || {
        let workspace = resolve_recall_workspace(&work_dir)?;
        calendar_event_suggestion(&workspace, &settings, Local::now())
    })
    .await
    .map_err(|e| format!("Calendar event suggestion task failed: {e}"))?
}

// ---- Session metadata ----

pub(crate) fn update_session_people_impl(
    ctx: &ctx::Ctx,
    name: String,
    people: Vec<String>,
    _project_id: Option<String>,
) -> Result<Vec<String>, String> {
    validate_session_name(&name)?;
    let cleaned = normalize_people(people);
    let work_dir = active_work_dir(&ctx.state);
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    if let Ok(path) = note_path_for_session_or_capture(&margins_dir, &name, &settings) {
        update_note_people_metadata(&path, &cleaned)?;
    }
    session::set_people(&margins_dir, &name, cleaned.clone()).map_err(|e| e.to_string())?;
    ensure_people_files(&settings, &cleaned)?;
    Ok(cleaned)
}

pub(crate) fn update_session_title_impl(
    ctx: &ctx::Ctx,
    name: String,
    title: String,
    _project_id: Option<String>,
) -> Result<UpdateTitleResult, String> {
    validate_session_name(&name)?;
    let cleaned = note_artifacts::clean_frontmatter_scalar(&title);
    if cleaned.chars().count() > 160 {
        return Err("Title must be 160 characters or less".into());
    }
    let work_dir = active_work_dir(&ctx.state);
    let margins_dir = work_dir.join(".margins");
    let saved = session::set_title(
        &margins_dir,
        &name,
        if cleaned.is_empty() {
            None
        } else {
            Some(cleaned.to_string())
        },
    )
    .map_err(|e| e.to_string())?;
    let settings = ctx.state.settings.lock().unwrap().clone();
    let mut vault_note_path = None;
    if let Some(saved_title) = saved.as_deref() {
        if let Ok(current_path) = note_path_for_session_or_capture(&margins_dir, &name, &settings) {
            update_note_title_metadata(&current_path, saved_title)?;
            let renamed_path = rename_note_file_for_title(&current_path, saved_title)?;
            let renamed_path_string = renamed_path.to_string_lossy().to_string();
            let current_path_string = current_path.to_string_lossy().to_string();
            if session::get_session_meta(&margins_dir, &name).is_ok() {
                session::move_session_vault_note_path(
                    &margins_dir,
                    &name,
                    &current_path_string,
                    &renamed_path_string,
                )
                .map_err(|e| e.to_string())?;
            } else {
                session::move_vault_note_path_by_id(
                    &margins_dir,
                    &name,
                    &current_path_string,
                    &renamed_path_string,
                )
                .map_err(|e| e.to_string())?;
            }
            vault_note_path = Some(renamed_path_string);
        }
    }
    Ok(UpdateTitleResult {
        title: saved,
        vault_note_path,
    })
}

pub(crate) fn get_project_files_fingerprint_impl(
    ctx: &ctx::Ctx,
    project_id: Option<String>,
) -> ProjectFilesFingerprint {
    let settings = ctx.state.settings.lock().unwrap().clone();
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let resolved_project_id = project_id
        .filter(|id| !id.trim().is_empty())
        .or_else(|| project_id_for_work_dir(&settings, &work_dir));
    let (inbox_folder, people_folder) =
        folders_for_project(&settings, resolved_project_id.as_deref());
    let mut parts = Vec::new();
    push_path_fingerprint(&mut parts, "root", &work_dir);
    push_path_fingerprint(&mut parts, "margins", &work_dir.join(".margins"));
    push_path_fingerprint(
        &mut parts,
        "sessions",
        &work_dir.join(".margins/sessions.sqlite"),
    );
    let inbox_path = work_dir.join(inbox_folder.trim());
    push_path_fingerprint(&mut parts, "inbox", &inbox_path);
    push_markdown_tree_fingerprints(&mut parts, "inbox-md", &inbox_path);
    push_path_fingerprint(&mut parts, "people", &work_dir.join(people_folder.trim()));
    push_known_note_fingerprints(&mut parts, &work_dir.join(".margins"));
    ProjectFilesFingerprint {
        project_id: resolved_project_id,
        fingerprint: parts.join("|"),
    }
}

// ---- Granola ----

pub(crate) fn survey_granola_import_impl(
    ctx: &ctx::Ctx,
    paths: Vec<String>,
    project_id: Option<String>,
) -> Result<granola_import::GranolaImportSurvey, String> {
    if paths.is_empty() {
        return Err(granola_mcp::typed_error(
            "granola_import_source_required",
            "supplemental_parse",
            "source_missing",
            false,
        ));
    }
    let settings = ctx.state.settings.lock().unwrap().clone();
    let vault_root = vault_root_for_project(&settings, project_id.as_deref()).map_err(|_| {
        granola_mcp::typed_error(
            "granola_workspace_unavailable",
            "binding_validation",
            "workspace_unavailable",
            false,
        )
    })?;
    let (inbox_folder, people_folder) = folders_for_project(&settings, project_id.as_deref());
    granola_import::survey(&paths, &vault_root, &inbox_folder, &people_folder).map_err(|_| {
        granola_mcp::typed_error(
            "granola_import_parse_failed",
            "supplemental_parse",
            "invalid_source",
            false,
        )
    })
}

pub(crate) fn import_granola_files_impl(
    ctx: &ctx::Ctx,
    paths: Vec<String>,
    options: granola_import::GranolaImportOptions,
    project_id: Option<String>,
) -> Result<granola_import::GranolaImportResult, String> {
    if paths.is_empty() {
        return Err(granola_mcp::typed_error(
            "granola_import_source_required",
            "supplemental_parse",
            "source_missing",
            false,
        ));
    }
    let settings = ctx.state.settings.lock().unwrap().clone();
    let vault_root = vault_root_for_project(&settings, project_id.as_deref()).map_err(|_| {
        granola_mcp::typed_error(
            "granola_workspace_unavailable",
            "binding_validation",
            "workspace_unavailable",
            false,
        )
    })?;
    let result = granola_import::import(&paths, &vault_root, &options).map_err(|_| {
        granola_mcp::typed_error(
            "granola_import_parse_failed",
            "supplemental_parse",
            "invalid_source",
            false,
        )
    })?;
    #[cfg(feature = "recall")]
    run_enzyme_index_command(&vault_root)?;
    Ok(result)
}

// ---- Granola MCP ----

pub(crate) fn get_granola_import_status_impl(ctx: &ctx::Ctx) -> granola_mcp::GranolaImportStatus {
    let _ = ctx;
    granola_mcp::authorization_status()
}

pub(crate) async fn authorize_granola_import_impl(
    ctx: &ctx::Ctx,
) -> Result<granola_mcp::GranolaImportStatus, String> {
    let _ = ctx;
    crate::async_runtime::spawn_blocking(granola_mcp::authorize_blocking)
        .await
        .map_err(|_| {
            granola_mcp::typed_error(
                "granola_authorization_task_failed",
                "authorization_task",
                "task_failed",
                false,
            )
        })?
}

pub(crate) fn revoke_granola_import_authorization_impl(
    ctx: &ctx::Ctx,
    requested_account: Option<&str>,
) -> Result<granola_mcp::GranolaImportStatus, String> {
    let _ = ctx;
    let status = granola_mcp::authorization_status();
    let requested_account = requested_account
        .map(margins_workflows::workspace::normalize_granola_account)
        .transpose()
        .map_err(|_| {
            granola_mcp::typed_error(
                "granola_account_invalid",
                "authorization",
                "invalid_account",
                false,
            )
        })?;
    let account = granola_mcp::resolve_authorized_account(&status, requested_account)?;
    granola_mcp::revoke_authorization(&account)
}

// import_granola_mcp_impl is intentionally not provided: the Tauri command body
// emits events via AppHandle during import and uses crate::async_runtime which
// requires the Tauri runtime.  The dispatch router returns a clear error in
// server/web mode (see dispatch.rs).

// ---- Speech models ----

pub(crate) fn cancel_speech_model_download_impl(ctx: &ctx::Ctx) -> Result<(), String> {
    if let Some(cancel) = ctx.state.speech_model_cancel.lock().unwrap().as_ref() {
        cancel.store(true, Ordering::SeqCst);
    }
    Ok(())
}

pub(crate) async fn prepare_speech_models_impl(
    ctx: &ctx::Ctx,
    parakeet_model_dir: Option<String>,
) -> Result<SpeechModelPrepResult, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = ctx.state.speech_model_cancel.lock().unwrap();
        if slot.is_some() {
            return Err("A speech model download is already running.".to_string());
        }
        *slot = Some(cancel.clone());
    }

    let sink_for_worker = Arc::clone(&ctx.sink);
    let mut settings = ctx.state.settings.lock().unwrap().clone();
    if let Some(dir) = parakeet_model_dir
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        settings.parakeet_model_dir = Some(dir);
    }

    let join_result = crate::async_runtime::spawn_blocking(move || {
        prepare_speech_models_blocking(sink_for_worker, settings, cancel)
    })
    .await;
    *ctx.state.speech_model_cancel.lock().unwrap() = None;
    join_result.map_err(|e| format!("Speech model preparation task failed: {e}"))?
}

pub(crate) async fn clear_speech_models_impl(ctx: &ctx::Ctx) -> Result<String, String> {
    cancel_speech_model_download_impl(ctx)?;
    crate::async_runtime::spawn_blocking(clear_speech_models_blocking)
        .await
        .map_err(|e| format!("Clear speech models task failed: {e}"))?
}

pub(crate) async fn probe_speech_models_headless_impl(
    ctx: &ctx::Ctx,
    custom_path: Option<String>,
) -> Result<SpeechModelProbe, String> {
    let settings = ctx.state.settings.lock().unwrap().clone();
    Ok(crate::async_runtime::spawn_blocking(move || {
        probe_speech_models_impl(&settings, custom_path)
    })
    .await
    .map_err(|e| format!("probe_speech_models task failed: {e}"))?)
}

// ---- Devices ----

pub(crate) fn list_devices_impl(ctx: &ctx::Ctx) -> device_registry::DeviceSnapshot {
    (*ctx.state.device_registry.snapshot()).clone()
}

pub(crate) fn test_audio_input_impl(
    ctx: &ctx::Ctx,
    device_uid: Option<String>,
) -> Result<audio_devices::AudioTestResult, String> {
    audio_devices::test_audio_input(Arc::clone(&ctx.state.device_registry), device_uid)
}

pub(crate) fn test_system_audio_tap_impl() -> Result<audio_devices::SystemAudioTestResult, String> {
    audio_devices::test_system_audio_tap()
}

pub(crate) fn open_privacy_pane_impl(pane: String) -> Result<(), String> {
    audio_devices::open_privacy_pane(pane)
}

pub(crate) fn install_cli_tool_impl() -> Result<InstallCliResult, String> {
    let bin_dir = local_bin_dir()?;
    std::fs::create_dir_all(&bin_dir)
        .map_err(|e| format!("Could not create {}: {e}", bin_dir.display()))?;
    let source = find_or_build_cli_binary()?;
    let destination = bin_dir.join(format!("margins{}", std::env::consts::EXE_SUFFIX));
    let result = install_or_use_binary(
        &source,
        &destination,
        "margins",
        margins_cli_has_official_recall,
    )?;
    let action = if result.installed {
        "Installed"
    } else if result.used_existing {
        "Using existing"
    } else {
        "Using"
    };
    Ok(InstallCliResult {
        installed_path: result.path.to_string_lossy().to_string(),
        source_path: source.to_string_lossy().to_string(),
        message: format!(
            "{action} margins terminal command at {}. Add {} to PATH if your shell cannot find it.",
            result.path.display(),
            bin_dir.display()
        ),
    })
}

// ---- Sessions ----

pub(crate) fn list_sessions_impl(
    ctx: &ctx::Ctx,
    project_id: Option<String>,
) -> Result<Vec<session_index::SessionInfoDto>, String> {
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let settings = ctx.state.settings.lock().unwrap().clone();
    let resolved_project_id = project_id.or_else(|| project_id_for_work_dir(&settings, &work_dir));
    let recording_name = ctx
        .state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.session_name.clone())
        .or_else(|| {
            let sessions = ctx.state.web_sessions.lock().unwrap();
            let recording_id = sessions.keys().min()?;
            sessions
                .get(recording_id)
                .map(|session| session.session_name.clone())
        });
    session_index::list_sessions_with_notes(
        &work_dir,
        &settings,
        resolved_project_id.as_deref(),
        recording_name.as_deref(),
    )
}

pub(crate) fn delete_session_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    cancel_distill_for_session(&ctx.state, &name);
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    delete_session_fully(&work_dir, &margins_dir, &name)
}

/// Re-scan the project's notes, purge any sessions whose note file was genuinely
/// deleted (recording + DB rows and all), and return the fresh session list.
/// Renamed/moved notes are recovered by reconciliation, so only truly-removed
/// notes are pruned. Procured capture-notes with missing files are dropped by
/// `list_sessions_impl` itself.
pub(crate) fn reconcile_project_notes_impl(
    ctx: &ctx::Ctx,
    project_id: Option<String>,
) -> Result<Vec<session_index::SessionInfoDto>, String> {
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    let resolved_project_id = project_id
        .clone()
        .or_else(|| project_id_for_work_dir(&settings, &work_dir));
    // Never purge the session that is currently recording.
    let recording_name = ctx
        .state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .map(|r| r.session_name.clone());
    for name in session_index::sessions_with_deleted_notes(
        &work_dir,
        &settings,
        resolved_project_id.as_deref(),
    ) {
        if recording_name.as_deref() == Some(name.as_str()) {
            continue;
        }
        cancel_distill_for_session(&ctx.state, &name);
        let _ = delete_session_fully(&work_dir, &margins_dir, &name);
    }
    list_sessions_impl(ctx, project_id)
}

// ---- Recording ----

const CAPTURE_DEVICE_POLL: Duration = Duration::from_millis(100);
const LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS: u64 = 3 * 60 * 1_000;
const LIVE_TRANSCRIPT_CHECKPOINT_RETRY_MS: u64 = 5_000;
#[cfg(test)]
static TEST_LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS: AtomicU64 = AtomicU64::new(0);

fn live_transcript_checkpoint_interval_ms() -> u64 {
    // The deterministic native CoreML rolling harness is the only caller that
    // can shorten this. The override is not present in release builds, so a
    // user setting or ambient environment cannot relax production cadence.
    #[cfg(test)]
    let interval = TEST_LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS.load(Ordering::Relaxed);
    #[cfg(test)]
    if interval > 0 {
        return interval;
    }
    LIVE_TRANSCRIPT_CHECKPOINT_INTERVAL_MS
}

fn live_transcript_checkpoint_due(capture_ms: u64, last_checkpoint_ms: u64) -> bool {
    capture_ms.saturating_sub(last_checkpoint_ms) >= live_transcript_checkpoint_interval_ms()
}

/// A live decode tail lagging the captured audio by more than this means the
/// end of the meeting was never transcribed live. Because the offline pass
/// reads the same WAV, re-decoding *does* recover this span — so it is worth
/// the cost. Scattered interior loss shows up as decode stalling, i.e. as this
/// tail gap, so a single generous bound covers "we missed real speech".
const CATASTROPHIC_TAIL_GAP_MS: u64 = 15_000;

/// Per-channel live delivery floor below which the transcript has lost enough
/// aggregate speech that offline recovery pays for itself. This is a backstop
/// for pathological capture collapse, not a quality dial: routine backpressure
/// sheds a few scattered samples (well above 0.99) and is left to the
/// distiller, which already treats the live sidecar as possibly-partial.
const CATASTROPHIC_COVERAGE_FLOOR: f64 = 0.90;

struct LiveTranscriptQualification<'a> {
    transcript: &'a str,
    mic_audio_end_ms: u64,
    system_audio_end_ms: u64,
    mic_decoded_until_ms: u64,
    system_decoded_until_ms: u64,
    system_audio_expected: bool,
    system_audio_seen: bool,
    terminal_journal_durable: bool,
    mic_frame_coverage: Option<f64>,
    system_frame_coverage: Option<f64>,
}

/// Reuse the live transcript unless something *catastrophic* makes it
/// materially incomplete. The offline pass transcribes the same WAV, so it only
/// ever recovers speech the live pass skipped; marginal capture imperfections
/// (scattered queue loss, sub-second decode lag, timing skew, silence debt,
/// unverifiable resume gaps) recover nothing a human would notice and are not
/// worth a 20-30s re-transcription. We therefore gate on the three things that
/// actually make offline worthwhile: an empty transcript, a missing expected
/// channel (unrecoverable either way, but never reuse a half-capture), and a
/// large contiguous undecoded tail (real skipped speech). A loose per-channel
/// coverage floor is kept only as a backstop for pathological collapse.
fn live_transcript_reuse_rejections(input: &LiveTranscriptQualification<'_>) -> Vec<String> {
    let mut reasons = Vec::new();
    if input.transcript.trim().is_empty() {
        reasons.push("terminal transcript is empty".to_string());
    }
    if input.system_audio_expected && !input.system_audio_seen {
        reasons.push("required system-audio channel was not observed".to_string());
    }
    if !input.terminal_journal_durable {
        reasons.push("terminal transcript journal append failed".to_string());
    }
    let mic_tail_gap_ms = input
        .mic_audio_end_ms
        .saturating_sub(input.mic_decoded_until_ms);
    if mic_tail_gap_ms > CATASTROPHIC_TAIL_GAP_MS {
        reasons.push(format!(
            "terminal live mic decode never caught up to captured audio (missing final {mic_tail_gap_ms} ms)"
        ));
    }
    if input.system_audio_expected {
        let system_tail_gap_ms = input
            .system_audio_end_ms
            .saturating_sub(input.system_decoded_until_ms);
        if system_tail_gap_ms > CATASTROPHIC_TAIL_GAP_MS {
            reasons.push(format!(
                "terminal live system decode never caught up to captured audio (missing final {system_tail_gap_ms} ms)"
            ));
        }
    }
    // Backstop only: an unrecorded coverage figure is uncertainty, not known
    // loss, so it is tolerated. We reject solely on a pathological collapse.
    if let Some(mic) = input.mic_frame_coverage {
        if mic < CATASTROPHIC_COVERAGE_FLOOR {
            reasons.push(format!(
                "microphone live-queue delivery collapsed ({mic:.3})"
            ));
        }
    }
    if input.system_audio_expected {
        if let Some(system) = input.system_frame_coverage {
            if system < CATASTROPHIC_COVERAGE_FLOOR {
                reasons.push(format!(
                    "system-audio live-queue delivery collapsed ({system:.3})"
                ));
            }
        }
    }
    reasons
}

fn delivered_sample_coverage(accepted: u64, dropped: u64) -> Option<f64> {
    let total = accepted.saturating_add(dropped);
    (total > 0).then(|| accepted as f64 / total as f64)
}

fn transcript_refresh_requested(
    force_transcribe: Option<bool>,
    max_speakers: Option<usize>,
) -> bool {
    force_transcribe.unwrap_or(false) || max_speakers.is_some()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProcessSessionTranscriptSource {
    ReusedCheckpoint,
    RefreshedOffline,
}

struct ProcessSessionTranscript {
    content: String,
    source: ProcessSessionTranscriptSource,
}

fn resolve_process_session_transcript<F>(
    margins_dir: &Path,
    name: &str,
    force_transcribe: Option<bool>,
    max_speakers: Option<usize>,
    refresh_offline: F,
) -> Result<ProcessSessionTranscript, String>
where
    F: FnOnce() -> Result<String, String>,
{
    if !transcript_refresh_requested(force_transcribe, max_speakers) {
        if let Some(content) = read_valid_session_transcript_checkpoint(margins_dir, name) {
            return Ok(ProcessSessionTranscript {
                content,
                source: ProcessSessionTranscriptSource::ReusedCheckpoint,
            });
        }
    }

    refresh_offline().map(|content| ProcessSessionTranscript {
        content,
        source: ProcessSessionTranscriptSource::RefreshedOffline,
    })
}

fn install_capture_device_supervisor(ctx: &ctx::Ctx) {
    let mut guard = ctx.state.recording.lock().unwrap();
    let Some(rec) = guard.as_mut() else {
        return;
    };
    if rec.capture_supervisor_handle.is_some() {
        return;
    }

    let stop = Arc::new(AtomicBool::new(false));
    let supervisor_ctx = ctx::Ctx {
        state: Arc::clone(&ctx.state),
        sink: Arc::clone(&ctx.sink),
    };
    let thread_stop = stop.clone();
    let handle = std::thread::spawn(move || {
        run_capture_device_supervisor(supervisor_ctx, thread_stop);
    });
    rec.capture_supervisor_stop = stop;
    rec.capture_supervisor_handle = Some(handle);
}

fn stop_capture_device_supervisor(rec: &mut RecordingState) -> Option<std::thread::JoinHandle<()>> {
    rec.capture_supervisor_stop.store(true, Ordering::SeqCst);
    rec.capture_supervisor_handle.take()
}

fn run_capture_device_supervisor(ctx: ctx::Ctx, stop: Arc<AtomicBool>) {
    let initial = ctx.state.device_registry.snapshot();
    let mut generation = initial.generation;

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(CAPTURE_DEVICE_POLL);
        let snapshot = ctx.state.device_registry.snapshot();
        if snapshot.generation != generation {
            generation = snapshot.generation;
            let controller = ctx
                .state
                .recording
                .lock()
                .unwrap()
                .as_ref()
                .map(|recording| recording.segment_controller.clone());
            if let Some(controller) = controller {
                // The actor applies T6-T12 against the newest immutable
                // snapshot. Pinned targets therefore ignore default-only
                // changes, and coalesced generations are safe.
                let _ = controller.send_event(recording::SegmentControllerEvent::RegistrySnapshot(
                    snapshot,
                ));
            }
        }

        schedule_live_transcript_persistence(&ctx);
    }
}

fn schedule_live_transcript_persistence(ctx: &ctx::Ctx) {
    let checkpoint = {
        let guard = ctx.state.recording.lock().unwrap();
        let Some(rec) = guard.as_ref() else {
            return;
        };
        if !periodic_live_checkpoint_allowed(
            rec.paused,
            rec.phase,
            rec.segment_controller.snapshot().lifecycle,
        ) {
            return;
        }
        let Some((live_handle, client)) = rec
            .live_backchannel
            .as_ref()
            .map(|live| (live, live.client()))
        else {
            return;
        };
        // A6: skip periodic checkpoint while worker is still warming.
        if !live_handle.is_ready() {
            return;
        }
        let client = client;
        let capture_ms = (Local::now() - rec.start_time).num_milliseconds().max(0) as u64;
        let last_ms = rec.last_live_checkpoint_ms.load(Ordering::Acquire);
        let next_attempt_ms = rec.next_live_checkpoint_attempt_ms.load(Ordering::Acquire);
        let journal_due = live_transcript_checkpoint_due(capture_ms, last_ms);
        let snapshot_due = transcript_store::live_transcript_snapshot_due(
            &rec.work_dir.join(".margins"),
            &rec.session_name,
        );
        if !(journal_due || snapshot_due)
            || capture_ms < next_attempt_ms
            || rec
                .live_checkpoint_in_flight
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
        {
            return;
        }
        Some((
            rec.work_dir.join(".margins"),
            rec.session_name.clone(),
            capture_ms,
            client,
            Arc::clone(&rec.last_live_checkpoint_ms),
            Arc::clone(&rec.next_live_checkpoint_attempt_ms),
            Arc::clone(&rec.live_checkpoint_in_flight),
            journal_due,
        ))
    };

    let Some((
        margins_dir,
        session_name,
        capture_ms,
        client,
        last_ms,
        next_attempt_ms,
        in_flight,
        journal_due,
    )) = checkpoint
    else {
        return;
    };
    std::thread::spawn(move || {
        let result = if journal_due {
            client.checkpoint_memo(capture_ms).and_then(|snapshot| {
                transcript_store::append_live_transcript_segment(
                    &margins_dir,
                    &session_name,
                    "periodic",
                    None,
                    None,
                    &snapshot,
                )?;
                Ok(snapshot)
            })
        } else {
            client.request_context(capture_ms).and_then(|snapshot| {
                transcript_store::write_live_transcript_snapshot(
                    &margins_dir,
                    &session_name,
                    &snapshot,
                    false,
                )?;
                Ok(snapshot)
            })
        }
        .and_then(|snapshot| {
            if journal_due {
                let _ = append_backchannel_trace(
                    &margins_dir,
                    &session_name,
                    json!({
                        "kind": "periodic_transcript_checkpoint",
                        "requested_capture_ms": capture_ms,
                        "decoded_until_ms": snapshot.decoded_until_ms,
                        "committed_until_ms": snapshot.committed_until_ms,
                        "transcript_source": snapshot.transcript_source,
                        "has_transcript": !snapshot.transcript.trim().is_empty(),
                    }),
                );
            }
            Ok(())
        });
        match result {
            Ok(()) => {
                if journal_due {
                    last_ms.store(capture_ms, Ordering::Release);
                }
                next_attempt_ms.store(0, Ordering::Release);
            }
            Err(error) => {
                next_attempt_ms.store(
                    capture_ms.saturating_add(LIVE_TRANSCRIPT_CHECKPOINT_RETRY_MS),
                    Ordering::Release,
                );
                let _ = append_backchannel_trace(
                    &margins_dir,
                    &session_name,
                    json!({
                        "kind": "periodic_transcript_checkpoint_failed",
                        "requested_capture_ms": capture_ms,
                        "error": error,
                    }),
                );
            }
        }
        in_flight.store(false, Ordering::Release);
    });
}

fn periodic_live_checkpoint_allowed(
    paused: bool,
    phase: RecordingPhase,
    lifecycle: recording::SegmentLifecycle,
) -> bool {
    // MicRuntime is intentionally absent: both Active and MicHolding are
    // recording states, so the three-minute checkpoint cadence continues
    // while the writer materializes mic silence and system audio advances.
    !paused
        && phase == RecordingPhase::Recording
        && lifecycle == recording::SegmentLifecycle::Running
}

fn segment_controller_event_sink(
    ctx: &ctx::Ctx,
    margins_dir: PathBuf,
    session_name: String,
) -> recording::CaptureEventSink {
    let sink = Arc::clone(&ctx.sink);
    Arc::new(move |payload| {
        if let Some(trace) = payload.get("trace") {
            let _ = append_backchannel_trace(&margins_dir, &session_name, trace.clone());
        } else {
            sink.emit(CAPTURE_DEVICE_CHANGED_EVENT, payload);
        }
    })
}

fn legacy_capture_device_fallback_event(requested_name: String, device_name: String) -> Value {
    json!({
        "state": "fallback",
        "requested_uid": null,
        "requested_name": requested_name,
        "device_name": device_name,
    })
}

pub(crate) fn start_recording_impl(
    ctx: &ctx::Ctx,
    name: String,
    _device_uid: Option<String>,
    project_id: Option<String>,
) -> Result<String, String> {
    let startup_cancel = Arc::new(AtomicBool::new(false));
    {
        let mut guard = ctx.state.recording_startup_cancel.lock().unwrap();
        if guard.is_some() {
            return Err("Audio capture is already starting.".to_string());
        }
        *guard = Some(Arc::clone(&startup_cancel));
    }
    let result = start_recording_impl_inner(
        ctx,
        name,
        _device_uid,
        project_id,
        Arc::clone(&startup_cancel),
    );
    let mut guard = ctx.state.recording_startup_cancel.lock().unwrap();
    if guard
        .as_ref()
        .is_some_and(|active| Arc::ptr_eq(active, &startup_cancel))
    {
        *guard = None;
    }
    result
}

pub(crate) fn cancel_recording_startup_impl(state: &Arc<AppState>) -> bool {
    let guard = state.recording_startup_cancel.lock().unwrap();
    let Some(cancel) = guard.as_ref() else {
        return false;
    };
    cancel.store(true, Ordering::Release);
    true
}

fn start_recording_impl_inner(
    ctx: &ctx::Ctx,
    name: String,
    _device_uid: Option<String>,
    project_id: Option<String>,
    startup_cancel: Arc<AtomicBool>,
) -> Result<String, String> {
    let state = &ctx.state;
    audio_devices::verify_microphone_authorization()?;
    let snapshot = state.device_registry.snapshot();
    let settings = state.settings.lock().unwrap().clone();
    let missing_legacy_name = settings
        .input_device_uid
        .is_none()
        .then(|| settings.input_device_name.clone())
        .flatten()
        .filter(|name| snapshot.devices.iter().all(|device| device.name != *name));
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir).map_err(|e| e.to_string())?;
    {
        let guard = state.recording.lock().unwrap();
        if guard.is_some() {
            return Err("Already recording".into());
        }
    }
    let name = unique_session_name(&work_dir, &margins_dir, &name);
    // Defensive: reject if the generated name collides with an actively-finalizing session.
    // This is extremely unlikely (same-second same-name) but guards data corruption.
    if state.capture_finalizing.lock().unwrap().contains(&name) {
        return Err(format!(
            "Session name '{name}' is still being finalized. Start again in a moment."
        ));
    }
    let start_time = Local::now();
    let start_instant = std::time::Instant::now();
    let notes_path = format!(".margins/{}.md", name);
    session::create_session(&margins_dir, &name, &start_time, &notes_path)
        .map_err(|e| e.to_string())?;
    if let Ok(workspace) = resolve_recall_workspace(&work_dir) {
        if let Ok(Some(event)) = calendar_event_suggestion(&workspace, &settings, start_time)
            .map(|result| result.suggestion)
        {
            let people = event.people.clone();
            let meta = session::CalendarEventMeta {
                title: event.title,
                start: event.start,
                end: event.end,
                calendar_id: event.calendar_id,
                event_id: event.event_id,
            };
            let _ = session::set_calendar_event(&margins_dir, &name, meta, people.clone());
            let _ = ensure_people_files(&settings, &people);
        }
    }
    let segment_index = 0i64;
    let wav_path = recording_segment_rel_path(&name, segment_index);
    let live_generation = 0u64;
    let live_model_dir = live_backchannel::resolved_live_model_dir(&settings);
    let live_transcription_mode = LiveTranscriptionMode::StereoSplit;
    // A2: build on_ready / on_degraded callbacks that emit frontend events.
    let ctx_for_ready = ctx.clone();
    let ready_session_name = name.clone();
    let on_ready: Option<Box<dyn Fn() + Send>> = Some(Box::new(move || {
        ctx_for_ready.emit(
            "live-transcript-ready",
            serde_json::json!({"session_name": ready_session_name}),
        );
    }));
    let ctx_for_degraded = ctx.clone();
    let degraded_session_name = name.clone();
    let on_degraded: Option<Box<dyn Fn(String) + Send>> = Some(Box::new(move |reason: String| {
        ctx_for_degraded.emit(
            "live-transcript-degraded",
            serde_json::json!({"session_name": degraded_session_name, "reason": reason}),
        );
    }));
    // A4: on start_live_backchannel failure, degrade gracefully (do not delete session).
    let live_backchannel = match live_backchannel::start_live_backchannel_with_callbacks(
        &settings,
        margins_dir.clone(),
        name.clone(),
        live_transcription_mode,
        on_ready,
        on_degraded,
    ) {
        Ok(live) => live,
        Err(error) => {
            let message = error.clone();
            let _ = append_backchannel_trace(
                &margins_dir,
                &name,
                serde_json::json!({
                    "kind": "live_transcript_worker_start_degraded",
                    "reason": "start_failed",
                    "message": message,
                }),
            );
            ctx.emit(
                "live-transcript-degraded",
                serde_json::json!({"session_name": name, "reason": error}),
            );
            None
        }
    };
    if live_backchannel.is_none() {
        let env_model_dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let reason = if env_model_dir.is_some() {
            "fluid_coreml_assets_missing_at_env_model_dir"
        } else {
            "no_fluid_coreml_model_dir"
        };
        let _ = append_backchannel_trace(
            &margins_dir,
            &name,
            json!({"kind":"live_transcript_worker_unavailable","backend":"coreml","reason":reason,
                "model_dir": live_model_dir.as_ref().map(|path| path.display().to_string()),
                "env_model_dir": env_model_dir,
                "configured_parakeet_model_dir": settings.parakeet_model_dir}),
        );
    }
    let live_audio = live_backchannel
        .as_ref()
        .map(|handle| handle.audio_sink(live_generation));
    let event_sink = segment_controller_event_sink(ctx, margins_dir.clone(), name.clone());
    let runtime = match SegmentController::start(SegmentControllerStart {
        registry: Arc::clone(&state.device_registry),
        segment_id: segment_index as u64,
        live_generation,
        input_device_mode: settings.input_device_mode,
        pinned_uid: settings.input_device_uid.clone(),
        session_mic_target: None,
        prior_session_counters: recording::SessionCaptureCounters::default(),
        live_audio,
        wav_path: work_dir.join(&wav_path),
        event_sink: Some(event_sink),
        startup_cancel: Some(startup_cancel),
    }) {
        Ok(runtime) => runtime,
        Err(e) => {
            if let Some(live) = live_backchannel {
                let _ = live.finish_and_join(0);
            }
            let _ = session::delete_session_row(&margins_dir, &name);
            let _ = std::fs::remove_file(work_dir.join(&notes_path));
            let _ = std::fs::remove_file(work_dir.join(&wav_path));
            return Err(e);
        }
    };
    // A4: capture is now running; record how long after function entry we got here.
    let _ = append_backchannel_trace(
        &margins_dir,
        &name,
        serde_json::json!({
            "kind": "live_transcript_capture_first_start",
            "start_time_epoch_ms": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            "capture_start_gap_ms": start_instant.elapsed().as_millis() as u64,
        }),
    );
    if let Err(e) = session::add_segment(&margins_dir, &name, segment_index, &wav_path, 0, None) {
        let _ = runtime.client.discard();
        if let Some(live) = live_backchannel {
            let _ = live.finish_and_join(0);
        }
        let _ = session::delete_session_row(&margins_dir, &name);
        let _ = std::fs::remove_file(work_dir.join(&notes_path));
        let _ = std::fs::remove_file(work_dir.join(&wav_path));
        return Err(e.to_string());
    }
    let initial_device_name =
        capture_state_from_snapshot(&runtime.snapshot).last_good_device_name();
    let telemetry = runtime.telemetry;
    *state.recording.lock().unwrap() = Some(RecordingState {
        session_name: name.clone(),
        work_dir,
        start_time,
        segment_started_at: std::time::Instant::now(),
        segment_index,
        mic_peak: Arc::clone(&telemetry.mic_peak),
        mic_frames: Arc::clone(&telemetry.mic_frames),
        spk_peak: Arc::clone(&telemetry.spk_peak),
        mic_drops: Arc::clone(&telemetry.mic_drops),
        spk_drops: Arc::clone(&telemetry.spk_drops),
        spk_silence: Arc::clone(&telemetry.spk_silence),
        spk_frames: Arc::clone(&telemetry.spk_frames),
        spk_health_frames: Arc::clone(&telemetry.spk_health_frames),
        spk_rate: Arc::clone(&telemetry.spk_rate),
        system_audio_seen: Arc::clone(&telemetry.system_audio_seen),
        system_health_seen: Arc::clone(&telemetry.system_health_seen),
        live_transcription_mode,
        live_backchannel,
        live_generation,
        memo_lines: Vec::new(),
        phase: RecordingPhase::Recording,
        paused: false,
        capture_supervisor_stop: Arc::new(AtomicBool::new(false)),
        capture_supervisor_handle: None,
        last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
        next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
        live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
        segment_controller: runtime.client,
        segment_telemetry: telemetry,
        sealed_diagnostics: SegmentDiagnostics::default(),
    });
    // The controller emitted its initial Active/Holding (and any UID fallback)
    // before returning. Preserve the legacy-name-only migration notice too.
    if let Some(requested_name) = missing_legacy_name {
        ctx.emit(
            CAPTURE_DEVICE_CHANGED_EVENT,
            legacy_capture_device_fallback_event(requested_name, initial_device_name),
        );
    }
    install_capture_device_supervisor(ctx);
    ai_config::schedule_capture_ai_preflight(settings, "native");
    Ok(name)
}

pub(crate) async fn stop_recording_impl(ctx: &ctx::Ctx) -> Result<String, String> {
    let finalize_started = Instant::now();
    let state = Arc::clone(&ctx.state);
    let (rec, supervisor) = {
        let mut guard = state.recording.lock().unwrap();
        let mut rec = match guard.take() {
            Some(rec) => rec,
            None if !state.capture_finalizing.lock().unwrap().is_empty() => {
                return Err("Capture is already finalizing.".to_string());
            }
            None => return Err("Not recording".to_string()),
        };
        rec.phase = RecordingPhase::Finalizing;
        state
            .capture_finalizing
            .lock()
            .unwrap()
            .insert(rec.session_name.clone());
        let supervisor = stop_capture_device_supervisor(&mut rec);
        (rec, supervisor)
    };
    if let Some(handle) = supervisor {
        let _ = handle.join();
    }
    let supervisor_join_ms = finalize_started.elapsed().as_millis();
    let finalizing_name = rec.session_name.clone();
    let join_result = crate::async_runtime::spawn_blocking(move || {
        let mut rec = rec;
        let work_dir = rec.work_dir.clone();
        let margins_dir = work_dir.join(".margins");
        let segment_started = Instant::now();
        let was_paused = rec.paused;
        let seal = rec.segment_controller.finish()?;
        if !was_paused {
            session::update_segment_duration(
                &margins_dir,
                &rec.session_name,
                rec.segment_index,
                seal.duration_secs,
            )
            .map_err(|error| error.to_string())?;
            rec.sealed_diagnostics.add_assign(&seal.diagnostics);
        }
        let final_diagnostics = rec.sealed_diagnostics.clone();
        let segment_finalize_ms = segment_started.elapsed().as_millis();
        let live_started = Instant::now();
        let mut live_finish_status = "unavailable";
        let mut live_reuse_rejections = vec!["terminal live transcript unavailable".to_string()];
        let mut qualified_live_transcript = None;
        let mut system_audio_expected = false;
        let mut mic_audio_end_ms = None;
        let mut system_audio_end_ms = None;
        let mut mic_decoded_until_ms = None;
        let mut system_decoded_until_ms = None;
        let mut generation_timeline_verified = None;
        let mut generation_transitions = Vec::new();
        if let Some(live) = rec.live_backchannel.take() {
            // This expectation is frozen when the live worker starts so a
            // settings change cannot rewrite the session's safety contract.
            system_audio_expected = live.system_audio_expected();
            let end_ms = (Local::now() - rec.start_time).num_milliseconds().max(0) as u64;
            match live.finish_and_join(end_ms) {
                Ok(Some(snapshot)) => {
                    mic_audio_end_ms = Some(snapshot.mic_audio_end_ms);
                    system_audio_end_ms = Some(snapshot.system_audio_end_ms);
                    mic_decoded_until_ms = Some(snapshot.mic_decoded_until_ms);
                    system_decoded_until_ms = Some(snapshot.system_decoded_until_ms);
                    generation_timeline_verified = Some(snapshot.generation_timeline_verified);
                    generation_transitions = snapshot.generation_transitions.clone();
                    let terminal_journal_durable =
                        transcript_store::append_live_transcript_segment(
                            &margins_dir,
                            &rec.session_name,
                            "final",
                            None,
                            None,
                            &snapshot.context,
                        )
                        .is_ok();
                    live_finish_status = if terminal_journal_durable {
                        "durable"
                    } else {
                        "journal_write_failed"
                    };
                    let mic_frame_coverage = delivered_sample_coverage(
                        snapshot.context.mic_accepted_samples,
                        snapshot.context.mic_dropped_samples,
                    );
                    let system_frame_coverage = delivered_sample_coverage(
                        snapshot.context.system_accepted_samples,
                        snapshot.context.system_dropped_samples,
                    );
                    // Timeline-contiguity, skew, silence-debt and resume-gap
                    // telemetry are still captured in the finalization trace
                    // below, but no longer gate reuse: they measure capture
                    // imperfection, not skipped speech, and the offline pass
                    // cannot improve on them.
                    live_reuse_rejections =
                        live_transcript_reuse_rejections(&LiveTranscriptQualification {
                            transcript: &snapshot.context.transcript,
                            mic_audio_end_ms: snapshot.mic_audio_end_ms,
                            system_audio_end_ms: snapshot.system_audio_end_ms,
                            mic_decoded_until_ms: snapshot.mic_decoded_until_ms,
                            system_decoded_until_ms: snapshot.system_decoded_until_ms,
                            system_audio_expected,
                            system_audio_seen: rec.system_audio_seen.load(Ordering::Relaxed),
                            terminal_journal_durable,
                            mic_frame_coverage,
                            system_frame_coverage,
                        });
                    if live_reuse_rejections.is_empty() {
                        qualified_live_transcript = Some(snapshot.context.transcript.clone());
                    }
                }
                Ok(None) => live_finish_status = "empty",
                Err(ref e) if e == "aborted_warming" => {
                    live_finish_status = "aborted_warming";
                }
                Err(_) => live_finish_status = "worker_failed",
            }
        }
        let live_finalize_ms = live_started.elapsed().as_millis();
        let artifacts_started = Instant::now();
        let memo_content = export_memo(&rec.memo_lines);
        let notes_path = session_memo_path(&work_dir, &rec.session_name);
        std::fs::write(&notes_path, &memo_content).map_err(|e| e.to_string())?;
        if let Some(transcript) = qualified_live_transcript.as_deref() {
            if let Err(error) = write_qualified_live_transcript_artifact(
                &margins_dir,
                &rec.session_name,
                &memo_content,
                transcript,
            ) {
                // Publishing the optimization must never make Finish fail.
                // Remove any partially-published artifact and let processing
                // build the offline transcript from retained WAV audio.
                let _ = std::fs::remove_file(session_transcript_artifact_path(
                    &margins_dir,
                    &rec.session_name,
                ));
                live_reuse_rejections.push(format!(
                    "qualified live artifact publication failed: {error}"
                ));
                live_finish_status = "artifact_publish_failed";
            }
        }
        write_capture_context_sidecar(&margins_dir, &rec.session_name, &notes_path)?;
        let meta = session::get_session_meta(&margins_dir, &rec.session_name)
            .map_err(|e| e.to_string())?;
        verify_capture_ready_for_processing(&work_dir, &margins_dir, &meta)?;
        let artifact_finalize_ms = artifacts_started.elapsed().as_millis();
        let _ = append_backchannel_trace(
            &margins_dir,
            &rec.session_name,
            json!({
                "kind": "capture_finalization_timing",
                "supervisor_join_ms": supervisor_join_ms,
                "segment_finalize_ms": segment_finalize_ms,
                "live_finalize_ms": live_finalize_ms,
                "artifact_finalize_ms": artifact_finalize_ms,
                "total_ms": finalize_started.elapsed().as_millis(),
                "live_finish_status": live_finish_status,
                "mic_drop_count": rec.mic_drops.load(Ordering::Relaxed),
                "system_drop_count": rec.spk_drops.load(Ordering::Relaxed),
                "system_audio_sample_rate": rec.spk_rate.load(Ordering::Relaxed),
                "system_audio_expected": system_audio_expected,
                "system_audio_seen": rec.system_audio_seen.load(Ordering::Relaxed),
                "timeline_reusable": final_diagnostics.timeline_reusable,
                "mic_gap_ms_total": final_diagnostics.mic_gap_ms_total,
                "mic_switch_count": final_diagnostics.mic_switch_count,
                "stale_controller_events": final_diagnostics.stale_controller_events,
                "mic_writer": final_diagnostics.mic,
                "system_writer": final_diagnostics.system,
                "mic_audio_end_ms": mic_audio_end_ms,
                "system_audio_end_ms": system_audio_end_ms,
                "mic_decoded_until_ms": mic_decoded_until_ms,
                "system_decoded_until_ms": system_decoded_until_ms,
                "generation_timeline_verified": generation_timeline_verified,
                "generation_transitions": generation_transitions,
                "live_reuse_qualified": live_reuse_rejections.is_empty(),
                "live_reuse_rejections": live_reuse_rejections,
            }),
        );
        Ok::<String, String>(rec.session_name.clone())
    })
    .await;
    state
        .capture_finalizing
        .lock()
        .unwrap()
        .remove(&finalizing_name);
    // Re-prewarm after session completes so the next session pays zero load cost.
    // Guard: skip if recording already started again or slot is already occupied.
    {
        let recording_active = state.recording.lock().unwrap().is_some();
        let settings = state.settings.lock().unwrap().clone();
        live_backchannel::schedule_post_session_prewarm(settings, recording_active);
    }
    match join_result {
        Ok(result) => result,
        Err(e) => Err(format!("Recording stop task failed: {e}")),
    }
}

pub(crate) async fn discard_recording_impl(ctx: &ctx::Ctx) -> Result<String, String> {
    let state = Arc::clone(&ctx.state);
    let (rec, supervisor) = {
        let mut guard = state.recording.lock().unwrap();
        let mut rec = match guard.take() {
            Some(rec) => rec,
            None if !state.capture_finalizing.lock().unwrap().is_empty() => {
                return Err("Capture is already finalizing.".to_string());
            }
            None => return Err("Not recording".to_string()),
        };
        rec.phase = RecordingPhase::Finalizing;
        state
            .capture_finalizing
            .lock()
            .unwrap()
            .insert(rec.session_name.clone());
        let supervisor = stop_capture_device_supervisor(&mut rec);
        (rec, supervisor)
    };
    if let Some(handle) = supervisor {
        let _ = handle.join();
    }
    let finalizing_name = rec.session_name.clone();
    let join_result = crate::async_runtime::spawn_blocking(move || {
        let mut rec = rec;
        let work_dir = rec.work_dir.clone();
        let margins_dir = work_dir.join(".margins");
        let name = rec.session_name.clone();
        let _ = rec.segment_controller.discard();
        if let Some(live) = rec.live_backchannel.take() {
            let end_ms = (Local::now() - rec.start_time).num_milliseconds().max(0) as u64;
            let _ = live.finish_and_join(end_ms);
        }
        delete_session_fully(&work_dir, &margins_dir, &name)?;
        Ok::<String, String>(name)
    })
    .await;
    state
        .capture_finalizing
        .lock()
        .unwrap()
        .remove(&finalizing_name);
    // Re-prewarm after discard so the next session pays zero load cost.
    {
        let recording_active = state.recording.lock().unwrap().is_some();
        let settings = state.settings.lock().unwrap().clone();
        live_backchannel::schedule_post_session_prewarm(settings, recording_active);
    }
    match join_result {
        Ok(result) => result,
        Err(e) => Err(format!("Recording discard task failed: {e}")),
    }
}

pub(crate) fn switch_recording_device_impl(
    ctx: &ctx::Ctx,
    device_uid: Option<String>,
) -> Result<RecordingStatus, String> {
    let registry = ctx.state.device_registry.snapshot();
    let to = device_uid
        .as_deref()
        .and_then(|uid| registry.device_by_uid(uid))
        .map(|device| device.name.clone())
        .or_else(|| registry.default_device().map(|device| device.name.clone()))
        .unwrap_or_else(|| "requested microphone".to_string());
    let (controller, from) = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        if rec.paused {
            return Err("Cannot switch microphones while capture is paused.".to_string());
        }
        let snapshot = rec.segment_controller.snapshot();
        (
            rec.segment_controller.clone(),
            capture_state_from_snapshot(&snapshot).last_good_device_name(),
        )
    };

    let report = controller.swap_mic(device_uid.clone()).map_err(|error| {
        format!(
            "Could not switch microphone from {from} to {to}: {error}. Continued recording with {from}."
        )
    })?;

    // T4/T5 persistence happens only after the actor has committed the new
    // capture and retired the old one. Automatic fallback never reaches here.
    if let Some(mode) = report.persisted_mode {
        let mut settings = ctx.state.settings.lock().unwrap();
        settings.input_device_mode = mode;
        match mode {
            InputDeviceMode::FollowDefault => {
                settings.input_device_uid = None;
                settings.input_device_name = None;
            }
            InputDeviceMode::Pinned | InputDeviceMode::Missing => {
                settings.input_device_uid = report.device_uid.clone();
                settings.input_device_name = Some(report.device_name.clone());
            }
        }
        save_settings(&settings).map_err(|error| {
            format!(
                "Microphone switched to {}, but the preference could not be saved: {error}",
                report.device_name
            )
        })?;
    }

    let mut guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    Ok(recording_status_from_state(rec))
}

pub(crate) fn restart_system_audio_capture_impl(ctx: &ctx::Ctx) -> Result<RecordingStatus, String> {
    let controller = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        if rec.paused {
            return Ok(recording_status_from_state(rec));
        }
        rec.segment_controller.clone()
    };
    controller
        .swap_system()
        .map_err(|error| format!("Could not restart computer audio capture: {error}"))?;
    let guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_ref().ok_or("Not recording")?;
    // C16: the segment grace clock and microphone telemetry are untouched.
    // The controller resets only the replacement system lane's health values.
    Ok(recording_status_from_state(rec))
}

pub(crate) fn pause_recording_impl(ctx: &ctx::Ctx) -> Result<RecordingStatus, String> {
    let controller = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        if rec.paused {
            return Ok(recording_status_from_state(rec));
        }
        rec.segment_controller.clone()
    };
    let seal = controller.pause_and_seal()?;
    let mut guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    // The controller has already sealed exactly once. Publish paused state
    // before fallible session bookkeeping so a metadata error cannot leave the
    // UI claiming capture is still running while the controller is Paused.
    rec.sealed_diagnostics.add_assign(&seal.diagnostics);
    rec.paused = true;
    rec.phase = RecordingPhase::Paused;
    let margins_dir = rec.work_dir.join(".margins");
    if let Err(error) = session::update_segment_duration(
        &margins_dir,
        &rec.session_name,
        rec.segment_index,
        seal.duration_secs,
    ) {
        rec.phase = RecordingPhase::NeedsAttention;
        return Err(error.to_string());
    }
    let memo_content = export_memo(&rec.memo_lines);
    let _ = std::fs::write(
        session_memo_path(&rec.work_dir, &rec.session_name),
        memo_content,
    );
    Ok(recording_status_from_state(rec))
}

pub(crate) fn resume_recording_impl(ctx: &ctx::Ctx) -> Result<RecordingStatus, String> {
    let (
        old_controller,
        live_generation,
        segment_index,
        offset_ms,
        wav_path,
        work_dir,
        name,
        prior_session_counters,
    ) = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        if !rec.paused {
            return Ok(recording_status_from_state(rec));
        }
        let segment_index = rec.segment_index + 1;
        (
            rec.segment_controller.clone(),
            rec.live_generation + 1,
            segment_index,
            (Local::now() - rec.start_time).num_milliseconds(),
            recording_segment_rel_path(&rec.session_name, segment_index),
            rec.work_dir.clone(),
            rec.session_name.clone(),
            recording::SessionCaptureCounters {
                mic_drops: rec.mic_drops.load(Ordering::Acquire),
                spk_drops: rec.spk_drops.load(Ordering::Acquire),
                spk_frames: rec.spk_frames.load(Ordering::Acquire),
                system_audio_seen: rec.system_audio_seen.load(Ordering::Acquire),
            },
        )
    };
    let registry_snapshot = ctx.state.device_registry.snapshot();
    let preconditions = old_controller.resume_preconditions(registry_snapshot)?;
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    let live_audio = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        if let Some(live) = rec.live_backchannel.as_ref() {
            // Activate before the new native captures can enqueue. This avoids
            // classifying their startup packets as stale-generation drops.
            live.activate_generation(live_generation, offset_ms.max(0) as u64, "resume");
            Some(live.audio_sink(live_generation))
        } else {
            None
        }
    };
    let runtime = SegmentController::start(SegmentControllerStart {
        registry: Arc::clone(&ctx.state.device_registry),
        segment_id: segment_index as u64,
        live_generation,
        input_device_mode: settings.input_device_mode,
        pinned_uid: settings.input_device_uid.clone(),
        session_mic_target: Some(preconditions.target),
        prior_session_counters,
        live_audio,
        wav_path: work_dir.join(&wav_path),
        event_sink: Some(segment_controller_event_sink(
            ctx,
            margins_dir.clone(),
            name.clone(),
        )),
        startup_cancel: None,
    })
    .map_err(|error| format!("Could not resume capture: {error}"))?;
    if let Err(error) = session::add_segment(
        &margins_dir,
        &name,
        segment_index,
        &wav_path,
        offset_ms,
        None,
    ) {
        let _ = runtime.client.discard();
        return Err(error.to_string());
    }
    // The old paused controller owns no live writer now. Finish it only after
    // the new segment row and runtime are both viable so a failed resume can
    // be retried without losing the PinFallback policy latch.
    let _ = old_controller.finish();

    let mut guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    rec.segment_index = segment_index;
    rec.live_generation = live_generation;
    rec.segment_started_at = Instant::now();
    rec.mic_peak = Arc::clone(&runtime.telemetry.mic_peak);
    rec.mic_frames = Arc::clone(&runtime.telemetry.mic_frames);
    rec.spk_peak = Arc::clone(&runtime.telemetry.spk_peak);
    rec.mic_drops = Arc::clone(&runtime.telemetry.mic_drops);
    rec.spk_drops = Arc::clone(&runtime.telemetry.spk_drops);
    rec.spk_silence = Arc::clone(&runtime.telemetry.spk_silence);
    rec.spk_frames = Arc::clone(&runtime.telemetry.spk_frames);
    rec.spk_health_frames = Arc::clone(&runtime.telemetry.spk_health_frames);
    rec.spk_rate = Arc::clone(&runtime.telemetry.spk_rate);
    rec.system_audio_seen = Arc::clone(&runtime.telemetry.system_audio_seen);
    rec.system_health_seen = Arc::clone(&runtime.telemetry.system_health_seen);
    rec.segment_controller = runtime.client;
    rec.segment_telemetry = runtime.telemetry;
    rec.paused = false;
    rec.phase = RecordingPhase::Recording;
    Ok(recording_status_from_state(rec))
}

pub(crate) async fn set_live_transcription_mode_impl(
    ctx: &ctx::Ctx,
    mode: String,
) -> Result<RecordingStatus, String> {
    let mode = LiveTranscriptionMode::parse(&mode)?;
    let live = {
        let guard = ctx.state.recording.lock().unwrap();
        let rec = guard.as_ref().ok_or("Not recording")?;
        rec.live_backchannel
            .as_ref()
            .ok_or_else(|| "Live transcription is not running for this capture.".to_string())?
            .client()
    };
    crate::async_runtime::spawn_blocking(move || live.set_transcription_mode(mode))
        .await
        .map_err(|e| format!("Live transcription mode task failed: {e}"))??;
    let mut guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    rec.live_transcription_mode = mode;
    Ok(recording_status_from_state(rec))
}

pub(crate) fn get_recording_status_impl(ctx: &ctx::Ctx) -> RecordingStatus {
    let mut guard = ctx.state.recording.lock().unwrap();
    match guard.as_mut() {
        Some(rec) => recording_status_from_state(rec),
        None => idle_recording_status(),
    }
}

// ---- Memo / backchannel ----

pub(crate) fn sync_memo_impl(
    ctx: &ctx::Ctx,
    lines: Vec<MemoLine>,
    session_name: &str,
) -> Result<(), String> {
    let mut guard = ctx.state.recording.lock().unwrap();
    let rec = guard.as_mut().ok_or("Not recording")?;
    ensure_memo_session(session_name, &rec.session_name)?;
    rec.memo_lines = lines;
    Ok(())
}

pub(crate) fn checkpoint_memo_line_impl(
    ctx: &ctx::Ctx,
    lines: Vec<MemoLine>,
    committed_index: usize,
    session_name: &str,
    web_recording_id: Option<&str>,
) -> Result<(), String> {
    if ctx.sink.is_web() {
        let (margins_dir, active_session, memo_secs, memo_text, live_client) = {
            let mut sessions = ctx.state.web_sessions.lock().unwrap();
            let recording_id =
                web_recording_id.ok_or("Hosted memo checkpoint requires recordingId")?;
            let rec = sessions.get_mut(recording_id).ok_or_else(|| {
                format!("Stale memo mutation for {session_name}; capture is not active")
            })?;
            let memo = lines
                .get(committed_index)
                .ok_or("Committed memo index is out of range")?;
            rec.memo_lines = lines.clone();
            persist_live_memo(&rec.work_dir, &rec.session_name, &lines)?;
            (
                rec.work_dir.join(".margins"),
                rec.session_name.clone(),
                memo.created_secs,
                memo.text.clone(),
                rec.live_asr.as_ref().map(|worker| worker.client()),
            )
        };
        if memo_text.trim().is_empty() {
            return Ok(());
        }
        crate::async_runtime::spawn_blocking(move || {
            let memo_time = format_elapsed_for_backchannel(memo_secs);
            let Some(client) = live_client else {
                let _ = append_backchannel_trace(
                    &margins_dir,
                    &active_session,
                    json!({"kind":"memo_transcript_checkpoint_unavailable","memo_index":committed_index,"memo_time":memo_time,"text":memo_text,"reason":"web_live_asr_worker_not_started"}),
                );
                return;
            };
            match client.checkpoint((memo_secs * 1000.0).max(0.0) as u64) {
                Ok(snapshot) => {
                    let durable = transcript_store::append_live_transcript_segment(
                        &margins_dir,
                        &active_session,
                        "checkpoint",
                        Some(committed_index),
                        Some(&memo_time),
                        &snapshot,
                    );
                    let _ = append_backchannel_trace(
                        &margins_dir,
                        &active_session,
                        json!({
                            "kind":"memo_transcript_checkpoint",
                            "memo_index":committed_index,
                            "memo_time":memo_time,
                            "text":memo_text,
                            "decoded_until_ms":snapshot.decoded_until_ms,
                            "committed_until_ms":snapshot.committed_until_ms,
                            "transcript_source":snapshot.transcript_source,
                            "has_transcript":!snapshot.transcript.trim().is_empty(),
                            "durable":durable.is_ok(),
                            "error":durable.err(),
                        }),
                    );
                }
                Err(error) => {
                    let _ = append_backchannel_trace(
                        &margins_dir,
                        &active_session,
                        json!({"kind":"memo_transcript_checkpoint_error","memo_index":committed_index,"memo_time":memo_time,"text":memo_text,"reason":error}),
                    );
                }
            }
        });
        return Ok(());
    }
    // For Tauri path: the full impl is in the #[tauri::command] above.
    // The dispatch router should not call this for the Tauri path.
    Err("checkpoint_memo_line: use the Tauri command path".into())
}

pub(crate) fn request_backchannel_for_memo_impl(
    ctx: &ctx::Ctx,
    lines: Vec<MemoLine>,
    committed_index: usize,
    session_name: String,
    recording_id: &str,
    owner_id: &str,
    client_sent_unix_ms: Option<u128>,
) -> Result<(), String> {
    if ctx.sink.is_web() {
        let command_unix_ms = unix_ms_now();
        let client_sent_unix_ms = client_sent_unix_ms.unwrap_or(command_unix_ms);
        let (session_name, work_dir, memo_secs, memo_text, live_asr) = {
            let mut sessions = ctx.state.web_sessions.lock().unwrap();
            let rec = sessions.get_mut(recording_id).ok_or_else(|| {
                format!("Stale memo mutation for {session_name}; capture is not active")
            })?;
            if owner_id.is_empty() || rec.owner_id != owner_id {
                return Err(format!(
                    "Capture authority for '{}' belongs to another browser operation",
                    rec.session_name
                ));
            }
            if rec.memo_hydrated_owner.as_deref() != Some(owner_id) {
                return Err(
                    "Memo must be hydrated by the capture owner before requesting a backchannel"
                        .to_string(),
                );
            }
            let memo = lines
                .get(committed_index)
                .ok_or("Committed memo index is out of range")?;
            rec.memo_lines = lines.clone();
            persist_live_memo(&rec.work_dir, &rec.session_name, &lines)?;
            (
                rec.session_name.clone(),
                rec.work_dir.clone(),
                memo.created_secs,
                memo.text.clone(),
                rec.live_asr.as_ref().map(|worker| worker.client()),
            )
        };
        if committed_index + 1 != lines.len() || memo_text.trim().is_empty() {
            return Ok(());
        }
        let ctx = ctx.clone();
        crate::async_runtime::spawn(async move {
            run_web_backchannel_request(
                ctx,
                session_name,
                work_dir,
                committed_index,
                memo_secs,
                memo_text,
                lines,
                live_asr,
                command_unix_ms,
                client_sent_unix_ms,
            )
            .await;
        });
        return Ok(());
    }
    Err("request_backchannel_for_memo: use the Tauri command path".into())
}

#[allow(clippy::too_many_arguments)]
async fn run_web_backchannel_request(
    ctx: ctx::Ctx,
    session_name: String,
    work_dir: PathBuf,
    memo_index: usize,
    memo_secs: f64,
    memo_text: String,
    memo_lines: Vec<MemoLine>,
    live_asr: Option<web_live_asr::WebLiveAsrClient>,
    command_unix_ms: u128,
    client_sent_unix_ms: u128,
) {
    let mut timing = BackchannelTiming::new();
    let settings = ctx.state.settings.lock().unwrap().clone();
    timing.settings_ms = timing.mark_stage();
    let vault_path = vault_root(&settings);
    let instructions = settings.distill_instructions.clone();
    let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
        match configure_ai_for_backchannel(&settings).await {
            Ok(config) => config,
            Err(error) => {
                emit_web_backchannel_error(&ctx, &session_name, memo_index, "ai_config", &error);
                return;
            }
        };
    timing.ai_config_ms = timing.mark_stage();
    let state = Arc::clone(&ctx.state);
    let sink = Arc::clone(&ctx.sink);
    let task_session_name = session_name.clone();
    let result = crate::async_runtime::spawn_blocking(move || {
        let session_name = task_session_name;
        let margins_dir = work_dir.join(".margins");
        let memo_time = format_elapsed_for_backchannel(memo_secs);
        let request_id = format!("{session_name}:{memo_index}:{command_unix_ms}");
        let _ = append_backchannel_trace(
            &margins_dir,
            &session_name,
            json!({
                "kind":"backchannel_request_received",
                "unix_ms":command_unix_ms,
                "client_sent_unix_ms":client_sent_unix_ms,
                "client_to_command_ms":command_unix_ms.saturating_sub(client_sent_unix_ms),
                "request_id":request_id,
                "memo_index":memo_index,
            }),
        );
        let snapshot = match live_asr {
            Some(client) => client.checkpoint((memo_secs * 1_000.0).max(0.0) as u64),
            None => Err("Headless live ONNX worker not started".to_string()),
        };
        let (transcript, catalyze_transcript) = match snapshot {
            Ok(snapshot) => {
                let raw = snapshot.transcript.clone();
                let _ = append_backchannel_trace(
                    &margins_dir,
                    &session_name,
                    json!({
                        "kind":"live_transcript_snapshot",
                        "unix_ms":unix_ms_now(),
                        "request_id":request_id,
                        "memo_index":memo_index,
                        "decoded_until_ms":snapshot.decoded_until_ms,
                        "committed_until_ms":snapshot.committed_until_ms,
                        "transcript_source":snapshot.transcript_source,
                        "timing":snapshot.timing.as_ref().map(|value| json!({
                            "command_wait_ms":value.command_wait_ms,
                            "mic_update_ms":value.mic_update_ms,
                            "system_update_ms":value.system_update_ms,
                            "format_ms":value.format_ms,
                        })),
                        "transcript":snapshot.transcript,
                        "new_transcript":snapshot.new_transcript,
                    }),
                );
                (
                    format_live_transcript_prompt_context(&snapshot, &memo_lines, memo_index),
                    raw,
                )
            }
            Err(error) => {
                let _ = append_backchannel_trace(
                    &margins_dir,
                    &session_name,
                    json!({
                        "kind":"live_transcript_unavailable",
                        "unix_ms":unix_ms_now(),
                        "request_id":request_id,
                        "memo_index":memo_index,
                        "reason":error,
                    }),
                );
                (String::new(), String::new())
            }
        };
        timing.transcript_snapshot_ms = timing.mark_stage();
        let request = backchannel_ai::BackchannelAiRequest {
            request_id: request_id.clone(),
            memo_index,
            work_dir: work_dir.clone(),
            margins_dir: margins_dir.clone(),
            session_name: session_name.clone(),
            memo_text,
            memo_time: memo_time.clone(),
            transcript,
            catalyze_transcript,
            vault_path,
            ai_provider,
            ai_model,
            ai_api_key,
            ai_credential_generation,
            instructions,
        };
        timing.prompt_context_trace_ms = timing.mark_stage();
        let CachedBackchannelContext {
            context, cache_hit, ..
        } = prepare_cached_backchannel_context(&state, &request);
        timing.context_prepare_ms = timing.mark_stage();
        timing.context_cache_hit = cache_hit;
        timing.snapshot_store_ms = timing.mark_stage();
        match backchannel_ai::run_backchannel_ai(request, &context) {
            Ok(raw) => {
                let stage_ms = timing.mark_stage();
                timing.model_called = !backchannel_ai::raw_is_local_only(&raw);
                timing.model_ms = if timing.model_called { stage_ms } else { 0 };
                emit_web_backchannel_suggestion(
                    sink.as_ref(),
                    &margins_dir,
                    &session_name,
                    memo_index,
                    &memo_time,
                    raw,
                    &request_id,
                );
                timing.emit_ms = Some(timing.mark_stage());
                append_backchannel_timing_trace(
                    &margins_dir,
                    &session_name,
                    memo_index,
                    &timing,
                    None,
                );
            }
            Err(error) => {
                timing.model_called = true;
                timing.model_ms = timing.mark_stage();
                append_backchannel_timing_trace(
                    &margins_dir,
                    &session_name,
                    memo_index,
                    &timing,
                    Some(&error),
                );
                emit_web_backchannel_error_with_sink(
                    sink.as_ref(),
                    &margins_dir,
                    &session_name,
                    memo_index,
                    "suggest",
                    &error,
                );
            }
        }
    })
    .await;
    if let Err(error) = result {
        emit_web_backchannel_error(&ctx, &session_name, memo_index, "task", &error.to_string());
    }
}

pub(crate) fn steer_backchannel_for_memo_impl(
    ctx: &ctx::Ctx,
    _memo_index: usize,
    _steering: String,
    _previous_suggestion: Option<String>,
    session_name: String,
    recording_id: &str,
    owner_id: &str,
) -> Result<(), String> {
    if ctx.sink.is_web() {
        let sessions = ctx.state.web_sessions.lock().unwrap();
        let rec = sessions.get(recording_id).ok_or_else(|| {
            format!("Stale backchannel steering for {session_name}; capture is not active")
        })?;
        if owner_id.is_empty() || rec.owner_id != owner_id {
            return Err(format!(
                "Capture authority for '{}' belongs to another browser operation",
                rec.session_name
            ));
        }
        if rec.memo_hydrated_owner.as_deref() != Some(owner_id) {
            return Err(
                "Memo must be hydrated by the capture owner before steering a backchannel"
                    .to_string(),
            );
        }
        return Err("backchannel steering is not available in headless server mode".into());
    }
    Err("steer_backchannel_for_memo: use the Tauri command path".into())
}

#[cfg(all(test, feature = "server"))]
mod web_backchannel_authorization_tests {
    use super::*;

    fn fixture(tag: &str) -> (PathBuf, Arc<AppState>, ctx::Ctx, String) {
        let work_dir = std::env::temp_dir().join(format!(
            "margins-web-backchannel-auth-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&work_dir).unwrap();
        let state = build_app_state(work_dir.clone(), load_settings());
        let ctx = ctx::Ctx {
            state: Arc::clone(&state),
            sink: Arc::new(crate::server::events::WsSink::new(8)),
        };
        let started = web_session::start_web_recording(
            &state,
            work_dir.clone(),
            format!("auth-{tag}"),
            "capture-owner".to_string(),
        )
        .unwrap();
        (work_dir, state, ctx, started.recording_id)
    }

    fn lines() -> Vec<MemoLine> {
        vec![
            MemoLine {
                text: "first mark".to_string(),
                created_secs: 1.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
            MemoLine {
                text: "later mark prevents model launch in this authorization test".to_string(),
                created_secs: 2.0,
                edited_secs: None,
                draft_started_secs: None,
                audio_pending_at_mark: false,
                block_ordinal: None,
            },
        ]
    }

    #[test]
    fn observer_cannot_replace_memo_or_launch_web_backchannel() {
        let (work_dir, state, ctx, recording_id) = fixture("observer");
        let session_name = "auth-observer";
        web_session::hydrate_web_recording_memo(&state, &recording_id, Some("capture-owner"))
            .unwrap();

        let error = request_backchannel_for_memo_impl(
            &ctx,
            lines(),
            0,
            session_name.to_string(),
            &recording_id,
            "observer-owner",
            None,
        )
        .unwrap_err();
        assert!(error.contains("another browser operation"), "{error}");
        assert!(state.web_sessions.lock().unwrap()[&recording_id]
            .memo_lines
            .is_empty());

        web_session::discard_web_recording(&state, &recording_id, "capture-owner").unwrap();
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn owner_must_hydrate_before_backchannel_replacement() {
        let (work_dir, state, ctx, recording_id) = fixture("unhydrated");
        let session_name = "auth-unhydrated";

        let error = request_backchannel_for_memo_impl(
            &ctx,
            lines(),
            0,
            session_name.to_string(),
            &recording_id,
            "capture-owner",
            None,
        )
        .unwrap_err();
        assert!(error.contains("must be hydrated"), "{error}");
        assert!(state.web_sessions.lock().unwrap()[&recording_id]
            .memo_lines
            .is_empty());

        web_session::hydrate_web_recording_memo(&state, &recording_id, Some("capture-owner"))
            .unwrap();
        request_backchannel_for_memo_impl(
            &ctx,
            lines(),
            0,
            session_name.to_string(),
            &recording_id,
            "capture-owner",
            None,
        )
        .unwrap();
        assert_eq!(
            state.web_sessions.lock().unwrap()[&recording_id]
                .memo_lines
                .len(),
            2
        );

        web_session::discard_web_recording(&state, &recording_id, "capture-owner").unwrap();
        let _ = std::fs::remove_dir_all(work_dir);
    }
}

pub(crate) fn hydrate_prep_sketch_impl(
    ctx: &ctx::Ctx,
    _lines: Vec<MemoLine>,
    _session_name: String,
    _people: Vec<String>,
    _event_title: Option<String>,
    _block_ordinal: u32,
    _pulled_texts: Vec<String>,
    _meeting_so_far: Option<String>,
) -> Result<(), String> {
    if ctx.sink.is_web() {
        return Err("prep sketch hydration is not available in headless server mode".into());
    }
    Err("hydrate_prep_sketch: use the Tauri command path".into())
}

pub(crate) fn steer_prep_hydration_impl(
    ctx: &ctx::Ctx,
    _session_name: String,
    _block_ordinal: u32,
    _instruction: String,
) -> Result<(), String> {
    if ctx.sink.is_web() {
        return Err("prep steering is not available in headless server mode".into());
    }
    Err("steer_prep_hydration: use the Tauri command path".into())
}

// ---- Processing pipeline ----

pub(crate) async fn process_session_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
    overwrite_existing_note: Option<bool>,
    max_speakers: Option<usize>,
    force_transcribe: Option<bool>,
) -> Result<(), String> {
    if !ctx.sink.is_web()
        && std::env::var_os(hosted_distill_skill::DISTILL_SKILL_PATH_ENV).is_none()
    {
        return Err("process_session: use the Tauri command path in desktop mode".into());
    }
    let state = &ctx.state;
    let skill_path = hosted_distill_skill::resolve_for_state(
        state,
        hosted_distill_skill::HostedDistillCaller::ProcessSession,
    )?;
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    if state
        .recording
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|rec| rec.session_name == name)
    {
        return Err("Capture is still active. Finish capture before writing the note.".to_string());
    }
    if state.capture_finalizing.lock().unwrap().contains(&name) {
        return Err(
            "Capture is still saving audio. Try writing the note again in a moment.".to_string(),
        );
    }
    let settings = state.settings.lock().unwrap().clone();
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.distill_cancel.lock().unwrap();
        if let Some(prev) = slot.remove(&name) {
            prev.store(true, Ordering::SeqCst);
        }
        slot.insert(name.clone(), cancel.clone());
    }
    let _ = std::fs::create_dir_all(&margins_dir);
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let _ = std::fs::create_dir_all(&trace_dir);
    let trace_path = trace_dir.join(format!("{}_distill_trace.jsonl", name));
    let trace_file = std::fs::File::create(&trace_path)
        .ok()
        .map(|file| Arc::new(Mutex::new(file)));
    let steps_path = margins_dir.join(format!("{}_margins_trace.md", name));
    let steps_file = std::fs::File::create(&steps_path).ok().map(|mut file| {
        let _ = writeln!(file, "# Margins trace: {name}\n");
        Arc::new(Mutex::new(file))
    });
    let run_started_at = Instant::now();
    let sink = Arc::clone(&ctx.sink);
    let cancel_c = cancel.clone();
    let trace_for_emit = trace_file.clone();
    let steps_for_emit = steps_file.clone();
    let session_for_emit = name.clone();
    let emit = move |stage: &str, msg: &str, progress: Option<f32>| {
        if cancel_c.load(Ordering::SeqCst) {
            return;
        }
        let event = ProcessingEvent::timed_lifecycle(
            &run_started_at,
            &session_for_emit,
            ProcessingTrack::Note,
            "preparing_context",
            stage,
            msg,
            progress,
        );
        write_processing_trace(&trace_for_emit, &event);
        write_processing_steps_markdown(&steps_for_emit, &event);
        sink.emit(
            "processing-progress",
            serde_json::to_value(&event).unwrap_or_default(),
        );
    };
    let emit_failure = |message: &str| {
        let failed = ProcessingEvent::timed_lifecycle(
            &run_started_at,
            &name,
            ProcessingTrack::Note,
            "failed",
            "error",
            message,
            None,
        );
        write_processing_trace(&trace_file, &failed);
        write_processing_steps_markdown(&steps_file, &failed);
        ctx.sink.emit(
            "processing-progress",
            serde_json::to_value(&failed).unwrap_or_default(),
        );
    };
    macro_rules! server_note_job {
        ($expr:expr) => {
            match $expr {
                Ok(value) => value,
                Err(error) => {
                    let message = error.to_string();
                    emit_failure(&message);
                    return Err(message);
                }
            }
        };
    }
    let mut cancel_guard = DistillCancelGuard {
        state: Arc::clone(state),
        name: name.clone(),
        owner: cancel.clone(),
        released: false,
    };
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let capture_ready = ProcessingEvent::timed_lifecycle(
        &run_started_at,
        &name,
        ProcessingTrack::Capture,
        "captured",
        "prepare",
        "Capture saved.",
        Some(0.02),
    );
    write_processing_trace(&trace_file, &capture_ready);
    write_processing_steps_markdown(&steps_file, &capture_ready);
    ctx.sink.emit(
        "processing-progress",
        serde_json::to_value(&capture_ready).unwrap_or_default(),
    );
    emit(
        "prepare",
        "Preparing context from marks and capture.",
        Some(0.08),
    );
    let meta = server_note_job!(
        session::get_session_meta(&margins_dir, &name).map_err(|error| error.to_string())
    );
    server_note_job!(verify_capture_ready_for_processing(
        &work_dir,
        &margins_dir,
        &meta
    ));
    let memo_path = session_memo_path(&work_dir, &name);
    let capture_context = server_note_job!(build_session_capture_context(
        &margins_dir,
        &name,
        &memo_path
    ));
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let resolved_transcript = server_note_job!(resolve_process_session_transcript(
        &margins_dir,
        &name,
        force_transcribe,
        max_speakers,
        || {
            #[cfg(feature = "parakeet-asr")]
            {
                let event = ProcessingEvent::timed_lifecycle(
                    &run_started_at,
                    &name,
                    ProcessingTrack::Transcript,
                    "preparing",
                    "transcribe",
                    "Building headless transcript from captured audio.",
                    Some(0.3),
                );
                write_processing_trace(&trace_file, &event);
                write_processing_steps_markdown(&steps_file, &event);
                ctx.sink.emit(
                    "processing-progress",
                    serde_json::to_value(&event).unwrap_or_default(),
                );
                let transcript_started = Instant::now();
                let summary =
                    write_aligned_sidecar_headless(&work_dir, &margins_dir, &name, &memo_path)?;
                let event = ProcessingEvent::timed_lifecycle(
                    &run_started_at,
                    &name,
                    ProcessingTrack::Transcript,
                    "ready",
                    "transcribe",
                    &format!(
                        "Transcript ready ({summary}, {} ms).",
                        transcript_started.elapsed().as_millis()
                    ),
                    Some(0.5),
                );
                write_processing_trace(&trace_file, &event);
                write_processing_steps_markdown(&steps_file, &event);
                ctx.sink.emit(
                    "processing-progress",
                    serde_json::to_value(&event).unwrap_or_default(),
                );
                let aligned = read_session_transcript_artifact(&margins_dir, &name).ok_or_else(|| {
                    "Transcript preparation completed but the transcript artifact could not be loaded."
                        .to_string()
                })?;
                if transcript_count_in_markdown(&aligned) == 0 {
                    return Err(
                        "No transcript entries were available after transcript preparation. Margins needs a full transcript before writing the note."
                            .to_string(),
                    );
                }
                Ok(aligned)
            }
            #[cfg(not(feature = "parakeet-asr"))]
            {
                Err("Headless transcription is not enabled in this server build.".to_string())
            }
        },
    ));
    if resolved_transcript.source == ProcessSessionTranscriptSource::ReusedCheckpoint {
        let event = ProcessingEvent::timed_lifecycle(
            &run_started_at,
            &name,
            ProcessingTrack::Transcript,
            "ready",
            "align",
            "Using prepared transcript.",
            Some(0.5),
        );
        write_processing_trace(&trace_file, &event);
        write_processing_steps_markdown(&steps_file, &event);
        ctx.sink.emit(
            "processing-progress",
            serde_json::to_value(&event).unwrap_or_default(),
        );
    }
    let aligned_context = resolved_transcript.content;
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let (
        (ai_provider, ai_model, ai_api_key, ai_credential_generation),
        (prep_ai_provider, prep_ai_model, prep_ai_api_key, prep_ai_credential_generation),
    ) = server_note_job!(configure_ai_for_note_job(&settings).await);
    let vault_path = vault_root(&settings);
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let pi_request = pi_distill::PiDistillRequest {
        work_dir: work_dir.clone(),
        margins_dir: margins_dir.clone(),
        trace_dir: trace_dir.clone(),
        session_name: name.clone(),
        memo_path,
        capture_context: capture_context.content,
        aligned_context,
        vault_path,
        note_config,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
        prep_ai_provider,
        prep_ai_model,
        prep_ai_api_key,
        prep_ai_credential_generation,
        skill_path,
        cancel: cancel.clone(),
        resume_session_path: None,
        refine_message: None,
        existing_note_path: None,
        save_generated_note: true,
    };
    let sink2 = Arc::clone(&ctx.sink);
    let trace_for_pi = trace_file.clone();
    let steps_for_pi = steps_file.clone();
    let session_for_pi = name.clone();
    let run_started_at2 = run_started_at;
    let cancel_c2 = cancel.clone();
    let join = crate::async_runtime::spawn_blocking(move || {
        pi_distill::run_pi_distill_blocking(pi_request, move |stage, message, progress| {
            if cancel_c2.load(Ordering::SeqCst) {
                return;
            }
            let (track, phase) = match stage {
                "transcript" => (ProcessingTrack::Transcript, "ready"),
                "complete" => (ProcessingTrack::Note, "saving"),
                _ => (ProcessingTrack::Note, "writing"),
            };
            let event = ProcessingEvent::timed_lifecycle(
                &run_started_at2,
                &session_for_pi,
                track,
                phase,
                stage,
                message,
                progress,
            );
            write_processing_trace(&trace_for_pi, &event);
            // This helper intentionally excludes note_stream/transcript rows,
            // so raw streamed token text remains JSONL-only.
            write_processing_steps_markdown(&steps_for_pi, &event);
            sink2.emit(
                "processing-progress",
                serde_json::to_value(&event).unwrap_or_default(),
            );
        })
    })
    .await;
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let outcome = match join {
        Ok(Ok(outcome)) => outcome,
        Ok(Err(message)) => {
            emit_failure(&message);
            return Err(message);
        }
        Err(error) => {
            let message = format!("Process session task failed: {error}");
            emit_failure(&message);
            return Err(message);
        }
    };
    if let Some(np) = outcome.note_path.as_deref() {
        if session::is_session_tombstoned(&margins_dir, &name).map_err(|e| e.to_string())? {
            let _ = delete_file_if_present(np);
            return Err(DISTILL_CANCELLED_SENTINEL.to_string());
        }
    }
    persist_pi_session_file(&trace_dir, &name, outcome.session_file.as_deref());
    let complete = ProcessingEvent::timed_lifecycle(
        &run_started_at,
        &name,
        ProcessingTrack::Note,
        "saved",
        "complete",
        "Note saved.",
        Some(1.0),
    );
    write_processing_trace(&trace_file, &complete);
    write_processing_steps_markdown(&steps_file, &complete);
    ctx.sink.emit(
        "processing-progress",
        serde_json::to_value(&complete).unwrap_or_default(),
    );
    cancel_guard.release();
    Ok(())
}

pub(crate) async fn retry_session_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    process_session_impl(ctx, name, project_id, Some(false), None, Some(false)).await
}

pub(crate) async fn refine_session_impl(
    ctx: &ctx::Ctx,
    name: String,
    message: String,
    project_id: Option<String>,
) -> Result<(), String> {
    if !ctx.sink.is_web()
        && std::env::var_os(hosted_distill_skill::DISTILL_SKILL_PATH_ENV).is_none()
    {
        return Err("refine_session: use the Tauri command path in desktop mode".into());
    }
    validate_session_name(&name)?;
    let message = message.trim().to_string();
    if message.is_empty() {
        return Err("Enter a change to make.".to_string());
    }
    let state = &ctx.state;
    let skill_path = hosted_distill_skill::resolve_for_state(
        state,
        hosted_distill_skill::HostedDistillCaller::RefineSession,
    )?;
    let work_dir = work_dir_for_project_id(state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let resume_session_path = read_pi_session_file(&trace_dir, &name)?
        .ok_or_else(|| "This note can't be refined yet. Reprocess it first.".to_string())?;
    let meta = session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string())?;
    let existing_note_path = meta
        .vault_note_path
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .ok_or_else(|| "Save a note before asking for changes.".to_string())?;
    let cancel = Arc::new(AtomicBool::new(false));
    {
        let mut slot = state.distill_cancel.lock().unwrap();
        if let Some(prev) = slot.remove(&name) {
            prev.store(true, Ordering::SeqCst);
        }
        slot.insert(name.clone(), cancel.clone());
    }
    let _ = std::fs::create_dir_all(&margins_dir);
    let run_started_at = Instant::now();
    let mut cancel_guard = DistillCancelGuard {
        state: Arc::clone(state),
        name: name.clone(),
        owner: cancel.clone(),
        released: false,
    };
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let (
        (ai_provider, ai_model, ai_api_key, ai_credential_generation),
        (prep_ai_provider, prep_ai_model, prep_ai_api_key, prep_ai_credential_generation),
    ) = configure_ai_for_note_job(&settings).await?;
    let vault_path = vault_root(&settings);
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let memo_path = session_memo_path(&work_dir, &name);
    let pi_request = pi_distill::PiDistillRequest {
        work_dir: work_dir.clone(),
        margins_dir: margins_dir.clone(),
        trace_dir: trace_dir.clone(),
        session_name: name.clone(),
        memo_path,
        capture_context: String::new(),
        aligned_context: String::new(),
        vault_path,
        note_config,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
        prep_ai_provider,
        prep_ai_model,
        prep_ai_api_key,
        prep_ai_credential_generation,
        skill_path,
        cancel: cancel.clone(),
        resume_session_path: Some(resume_session_path),
        refine_message: Some(message),
        existing_note_path: Some(existing_note_path),
        save_generated_note: true,
    };
    let sink = Arc::clone(&ctx.sink);
    let run_started_at2 = run_started_at;
    let cancel_c = cancel.clone();
    let join = crate::async_runtime::spawn_blocking(move || {
        pi_distill::run_pi_distill_blocking(pi_request, move |stage, message, progress| {
            if cancel_c.load(Ordering::SeqCst) {
                return;
            }
            let event = ProcessingEvent::timed(&run_started_at2, stage, message, progress);
            sink.emit(
                "processing-progress",
                serde_json::to_value(&event).unwrap_or_default(),
            );
        })
    })
    .await
    .map_err(|e| format!("Refine task failed: {e}"))?;
    if cancel_guard.is_cancelled() {
        return Err(DISTILL_CANCELLED_SENTINEL.to_string());
    }
    let outcome = join?;
    if let Some(np) = outcome.note_path.as_deref() {
        if session::is_session_tombstoned(&margins_dir, &name).map_err(|e| e.to_string())? {
            let _ = delete_file_if_present(np);
            return Err(DISTILL_CANCELLED_SENTINEL.to_string());
        }
    }
    cancel_guard.release();
    Ok(())
}

pub(crate) async fn reprocess_session_with_people_impl(
    ctx: &ctx::Ctx,
    name: String,
    people: Vec<String>,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let people = normalize_people(people);
    if people.is_empty() {
        return Err("Add at least one person before reprocessing.".to_string());
    }
    let state = &ctx.state;
    let work_dir = work_dir_for_project_id(state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();
    let vault_path = vault_root(&settings);
    let people_folder = settings.people_folder.clone();
    let (ai_provider, ai_model, ai_api_key, ai_credential_generation) =
        configure_ai_for_reprocess(&settings).await?;
    let request = reprocess::ReprocessRequest {
        session_name: name.clone(),
        people,
        work_dir: work_dir.clone(),
        margins_dir,
        vault_path,
        people_folder,
        ai_provider,
        ai_model,
        ai_api_key,
        ai_credential_generation,
    };
    let run_started_at = Instant::now();
    let sink = Arc::clone(&ctx.sink);
    let join = crate::async_runtime::spawn_blocking(move || {
        reprocess::run_reprocess(request, move |stage, message, progress| {
            let event = ProcessingEvent::timed(&run_started_at, stage, message, progress);
            sink.emit(
                "processing-progress",
                serde_json::to_value(&event).unwrap_or_default(),
            );
        })
    })
    .await
    .map_err(|e| format!("Reprocess task failed: {e}"))?;
    join
}

pub(crate) fn save_draft_note_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
    overwrite_existing_note: Option<bool>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let state = &ctx.state;
    let work_dir = work_dir_for_project_id(state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    ensure_session_not_tombstoned(&margins_dir, &name)?;
    let settings = state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let meta = session::get_session_meta(&margins_dir, &name).map_err(|e| e.to_string())?;
    let draft_path = margins_dir.join(format!("{name}_note_draft.md"));
    let markdown =
        std::fs::read_to_string(&draft_path).map_err(|e| format!("Cannot read draft note: {e}"))?;
    if markdown.trim().is_empty() {
        return Err("Draft note is empty.".to_string());
    }
    let vault_path = vault_root(&settings);
    let people_candidates =
        session_index::recent_people_candidates(&work_dir, &settings, project_id.as_deref());
    let note_config = pi_distill::NoteConfig {
        inbox_folder: settings.inbox_folder.clone(),
        people_folder: settings.people_folder.clone(),
        created_date_format: settings.created_date_format.clone(),
        note_filename_template: settings.note_filename_template.clone(),
        person_note_template: settings.person_note_template.clone(),
        distill_instructions: settings.distill_instructions.clone(),
        people: meta.people.clone(),
        people_candidates,
        event_title: meta
            .title
            .clone()
            .or_else(|| meta.calendar_event.as_ref().map(|e| e.title.clone())),
        event_start: meta
            .calendar_event
            .as_ref()
            .and_then(|e| e.start.clone())
            .or_else(|| Some(meta.start_time.clone())),
    };
    let existing_note_path = if overwrite_existing_note.unwrap_or(false) {
        meta.vault_note_path
            .as_deref()
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .filter(|p| p.exists())
    } else {
        None
    };
    let note_path = pi_distill::save_markdown_note(pi_distill::SaveMarkdownRequest {
        vault_path: vault_path.as_deref(),
        margins_dir: &margins_dir,
        session_name: &name,
        note_config: &note_config,
        existing_note_path: existing_note_path.as_deref(),
        markdown: &markdown,
        grounding: None,
        cancel: None,
    })
    .map_err(|e| format!("failed to save draft note: {e}"))?;
    let note_path = note_path.to_string_lossy().to_string();
    session::set_vault_note_path(&margins_dir, &name, &note_path).map_err(|e| e.to_string())?;
    pi_distill::remove_note_draft(&margins_dir, &name);
    pi_distill::remove_note_draft(&trace_dir, &name);
    Ok(note_path)
}

pub(crate) fn discard_note_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    delete_session_fully(&work_dir, &margins_dir, &name)
}

pub(crate) fn cancel_process_session_impl(
    ctx: &ctx::Ctx,
    name: String,
    _project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    if let Some(flag) = ctx.state.distill_cancel.lock().unwrap().get(&name) {
        flag.store(true, Ordering::SeqCst);
    }
    Ok(())
}

pub(crate) fn clear_session_note_error_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    session::clear_note_error(&margins_dir, &name).map_err(|e| e.to_string())
}

pub(crate) async fn import_audio_file_impl(
    ctx: &ctx::Ctx,
    path: String,
    max_speakers: Option<usize>,
    project_id: Option<String>,
) -> Result<String, String> {
    if ctx.sink.is_web() {
        return Err("import_audio_file is not available in headless server mode".into());
    }
    Err("import_audio_file: use the Tauri command path in desktop mode".into())
}

// ---- Read / view ----

pub(crate) fn get_aligned_content_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    if let Some(content) = read_session_transcript_artifact(&margins_dir, &name) {
        return Ok(content);
    }
    snapshot_active_live_transcript_if_current(&ctx.state, &work_dir, &name)?;
    let memo_path = session_memo_path(&work_dir, &name);
    build_session_capture_context(&margins_dir, &name, &memo_path).map(|context| context.content)
}

pub(crate) fn get_session_memo_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    std::fs::read_to_string(session_memo_path(&work_dir, &name))
        .map_err(|e| format!("Cannot read memo file: {e}"))
}

pub(crate) fn get_vault_note_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<String, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    let path = note_path_for_session_or_capture(&margins_dir, &name, &settings)?;
    std::fs::read_to_string(path).map_err(|e| e.to_string())
}

pub(crate) fn get_session_grounding_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<Option<Value>, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let settings = ctx.state.settings.lock().unwrap().clone();
    let margins_dir = settings_trace_margins_dir(&settings, &work_dir);
    let grounding =
        session::get_session_grounding(&margins_dir, &name).map_err(|e| e.to_string())?;
    if grounding.is_empty() {
        Ok(None)
    } else {
        Ok(Some(json!({ "uses": grounding })))
    }
}

pub(crate) fn get_distill_trace_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<Vec<ProcessingEvent>, String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let settings = ctx.state.settings.lock().unwrap().clone();
    let trace_dir = settings_trace_margins_dir(&settings, &work_dir);
    let path = trace_dir.join(format!("{}_distill_trace.jsonl", name));
    let fallback_path = work_dir
        .join(".margins")
        .join(format!("{}_distill_trace.jsonl", name));
    let path = if path.exists() { path } else { fallback_path };
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str::<ProcessingEvent>(line).ok())
        .collect())
}

pub(crate) fn open_note_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    if matches!(
        editor_preference(settings.editor_command.as_deref()),
        EditorPreference::Obsidian
    ) {
        return open_session_note_in_obsidian(&margins_dir, &name, &settings);
    }
    let path = note_path_for_session_or_capture(&margins_dir, &name, &settings)?;
    open_path_with_editor(&path, settings.editor_command.as_deref())
}

pub(crate) fn open_note_in_obsidian_impl(
    ctx: &ctx::Ctx,
    name: String,
    project_id: Option<String>,
) -> Result<(), String> {
    validate_session_name(&name)?;
    let work_dir = work_dir_for_project_id(&ctx.state, project_id.as_deref());
    let margins_dir = work_dir.join(".margins");
    let settings = ctx.state.settings.lock().unwrap().clone();
    open_session_note_in_obsidian(&margins_dir, &name, &settings)
}

pub(crate) fn open_note_target_in_obsidian_impl(
    ctx: &ctx::Ctx,
    target: String,
    project_id: Option<String>,
) -> Result<(), String> {
    let clean_target = target
        .trim()
        .trim_start_matches('/')
        .trim_end_matches(".md")
        .trim();
    if clean_target.is_empty() || clean_target.contains('\0') {
        return Err("Linked note target is empty.".to_string());
    }
    let settings = ctx.state.settings.lock().unwrap().clone();
    let vault = vault_root_for_project(&settings, project_id.as_deref())?;
    open_obsidian_file_target(&vault, clean_target)
}

// ---- Capabilities ----

#[derive(Serialize)]
pub(crate) struct CapabilitiesResult {
    pub system_audio: bool,
    pub obsidian: bool,
    pub cli_install: bool,
    pub file_dialogs: bool,
    pub live_transcription: bool,
}

pub(crate) fn get_capabilities_impl(ctx: &ctx::Ctx) -> CapabilitiesResult {
    if ctx.sink.is_web() {
        CapabilitiesResult {
            system_audio: false,
            obsidian: false,
            cli_install: false,
            file_dialogs: false,
            live_transcription: false,
        }
    } else {
        let settings = ctx.state.settings.lock().unwrap().clone();
        let live_transcription = live_backchannel::resolved_live_model_dir(&settings).is_some();
        CapabilitiesResult {
            system_audio: true,
            obsidian: true,
            cli_install: true,
            file_dialogs: true,
            live_transcription,
        }
    }
}

// ---- Tauri shims for get_capabilities ----

#[cfg(feature = "tauri-app")]
#[tauri::command]
fn get_capabilities(state: tauri::State<'_, Arc<AppState>>) -> CapabilitiesResult {
    let ctx = ctx::Ctx::no_emit(Arc::clone(&*state));
    get_capabilities_impl(&ctx)
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[cfg(feature = "tauri-app")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    configure_pi_agent_dir();
    let settings = load_settings();
    let startup_prewarm_settings = settings.clone();
    let fallback_work_dir = default_work_dir();
    let work_dir = settings_work_dir(&settings, &fallback_work_dir);
    // Do NOT create the vault / notes destination at app open. Materializing a
    // folder under ~/Documents triggers the macOS "Documents Folder" TCC prompt
    // on launch — we defer all folder creation to the first capture press (see
    // start_recording_impl_inner), so the app never asks for permissions just by
    // opening. Existing vaults already exist; fresh runs stay empty until capture.

    if let Ok(out) = std::env::var("MARGINS_EXPORT_HARNESS_SNAPSHOT") {
        export_harness_snapshot(&work_dir, &settings, Path::new(&out));
        return;
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    if event.state() == ShortcutState::Pressed {
                        let _ = app.emit("global-capture-toggle", ());
                    }
                })
                .build(),
        )
        .setup(move |app| {
            apply_macos_window_material(app);
            if let Some(state) = app.try_state::<Arc<AppState>>() {
                if let Err(error) = desktop_live::start_loopback_api(Arc::clone(&*state)) {
                    eprintln!("margins: could not start local companion service: {error}");
                }
            }
            let toggle = Shortcut::new(Some(Modifiers::SUPER | Modifiers::SHIFT), Code::KeyN);
            if let Err(e) = app.global_shortcut().register(toggle) {
                eprintln!("margins: could not register global shortcut CmdShiftN (another app may have claimed it): {e}");
            }
            open_devtools_if_requested(app);
            live_backchannel::prewarm_live_models(startup_prewarm_settings.clone());
            let device_state = Arc::clone(&*app.state::<Arc<AppState>>());
            let device_app = app.handle().clone();
            let registry = Arc::clone(&device_state.device_registry);
            std::thread::spawn(move || {
                device_registry::run_device_watcher(registry, move |snapshot| {
                    {
                        let mut settings = device_state.settings.lock().unwrap();
                        if settings::migrate_input_device_uid(&mut settings, &snapshot.devices) {
                            if let Err(error) = save_settings(&settings) {
                                eprintln!("margins: could not persist microphone UID migration: {error}");
                            }
                        }
                    }
                    let _ = device_app.emit(device_registry::DEVICES_CHANGED_EVENT, &*snapshot);
                });
            });
            // ---------------------------------------------------------------------------
            // Settings-file watcher: emit "settings-changed" when settings.json changes.
            //
            // This is the primary mechanism for the CLI (`margins projects add`) to push
            // a new project into the running desktop without restart. We watch the *parent
            // directory* non-recursively and filter events to the settings.json filename
            // because many editors/CLI tools write by rename (write tmp → rename over
            // target), which only produces a create/rename event on the parent dir, not on
            // the file itself.
            //
            // Emitting after our own save_settings writes is intentional and harmless —
            // the frontend re-fetch of getSettings() is idempotent, and suppressing
            // self-writes would require IPC plumbing that adds more complexity than it
            // saves. A debounce (~250 ms) coalesces rapid rename+create bursts.
            // ---------------------------------------------------------------------------
            let spath = settings_path();
            if let Some(parent) = spath.parent().map(|p| p.to_owned()) {
                let settings_filename = spath.file_name().map(|n| n.to_owned());
                let app_handle = app.handle().clone();
                std::thread::spawn(move || {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let mut watcher: RecommendedWatcher = match notify::Watcher::new(
                        tx,
                        notify::Config::default()
                            .with_poll_interval(Duration::from_millis(300)),
                    ) {
                        Ok(w) => w,
                        Err(e) => {
                            eprintln!("margins: settings watcher init failed: {e}");
                            return;
                        }
                    };
                    if let Err(e) = watcher.watch(&parent, RecursiveMode::NonRecursive) {
                        eprintln!("margins: settings watcher watch failed: {e}");
                        return;
                    }
                    // Keep the watcher alive; channel drives the event loop.
                    let mut last_emit = std::time::Instant::now()
                        .checked_sub(Duration::from_secs(10))
                        .unwrap_or_else(std::time::Instant::now);
                    // Seed last_content so the first watcher fire after our own
                    // save_settings write is recognized as unchanged and suppressed.
                    let mut last_content: Option<String> =
                        std::fs::read_to_string(&spath).ok();
                    loop {
                        match rx.recv() {
                            Ok(Ok(event)) => {
                                // Only react to create/modify events for settings.json.
                                let is_relevant = matches!(
                                    event.kind,
                                    EventKind::Create(_) | EventKind::Modify(_)
                                ) && event.paths.iter().any(|p| {
                                    p.file_name() == settings_filename.as_deref()
                                });
                                if !is_relevant {
                                    continue;
                                }
                                // Content-compare gate: read the file and skip the
                                // AppState overwrite + emit when the file is byte-for-byte
                                // identical to what we last processed. This suppresses
                                // spurious watcher fires caused by our own save_settings
                                // writes (atomic rename triggers a filesystem event even
                                // when the content is the same). Must run BEFORE the
                                // time-debounce so that a CLI write landing within the
                                // 250ms window is never silently dropped.
                                let current_content = std::fs::read_to_string(&spath).ok();
                                if current_content == last_content {
                                    continue;
                                }
                                // Time-debounce: coalesce rapid bursts of genuinely-changed
                                // content (e.g. two distinct CLI writes within 250ms) to
                                // avoid stampeding the frontend. This gate runs AFTER the
                                // content-compare so it only ever suppresses a changed-content
                                // event when another changed-content event already fired
                                // recently — it cannot suppress a write that carries new data
                                // that hasn't been seen yet.
                                if last_emit.elapsed() < Duration::from_millis(250) {
                                    continue;
                                }
                                last_content = current_content;
                                last_emit = std::time::Instant::now();
                                // Re-load settings and update AppState before emitting.
                                // Use try_load_settings so we can skip the overwrite
                                // when the file is transiently unreadable (e.g. the
                                // CLI truncated it mid-write). Clobbering live AppState
                                // with Settings::default() on a parse error would wipe
                                // the user's projects, API keys, and tokens until the
                                // next successful reload.
                                match try_load_settings() {
                                    LoadSettingsResult::Ok(new_settings) => {
                                        if let Some(state) = app_handle.try_state::<Arc<AppState>>() {
                                            let state = Arc::clone(&*state);
                                            let app_handle = app_handle.clone();
                                            crate::async_runtime::spawn(async move {
                                                refresh_backchannel_snapshot_settings(&state, &new_settings).await;
                                                *state.settings.lock().unwrap() = new_settings;
                                                let _ = app_handle.emit("settings-changed", ());
                                            });
                                        } else {
                                            let _ = app_handle.emit("settings-changed", ());
                                        }
                                    }
                                    LoadSettingsResult::Missing => {
                                        // No file yet — treat as a fresh install and notify
                                        // the frontend so it can show the setup flow.
                                        let _ = app_handle.emit("settings-changed", ());
                                    }
                                    LoadSettingsResult::ParseError => {
                                        // Transient read failure (e.g. mid-write truncation).
                                        // Skip AppState overwrite; the next watcher event after
                                        // the write completes will reload cleanly.
                                        eprintln!("margins: settings watcher: skipping reload due to parse error (transient write?)");
                                    }
                                }
                            }
                            Ok(Err(e)) => {
                                eprintln!("margins: settings watcher event error: {e}");
                            }
                            Err(_) => break, // sender dropped — app is shutting down
                        }
                    }
                    // Watcher is dropped here when the thread exits.
                    drop(watcher);
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            let label = window.label();
            if !aux_windows::is_aux_label(label) {
                return;
            }
            if matches!(
                event,
                tauri::WindowEvent::Moved(_)
                    | tauri::WindowEvent::Resized(_)
                    | tauri::WindowEvent::CloseRequested { .. }
            ) {
                aux_windows::remember_placement(
                    window.app_handle(),
                    label,
                    &default_work_dir(),
                );
            }
        })
        .manage(build_app_state(work_dir, settings))
        .invoke_handler(tauri::generate_handler![
            get_settings,
            update_settings,
            update_audio_settings,
            register_project,
            update_project_readiness,
            get_ai_status,
            get_included_ai_status,
            prepare_included_ai,
            sign_in_chatgpt,
            get_ai_readiness,
            preview_ai_resolution,
            count_vault_notes,
            validate_vault,
            resolve_project_subfolder,
            ensure_default_vault,
            index_vault,
            get_calendar_event_suggestion,
            update_session_people,
            update_session_title,
            get_project_files_fingerprint,
            survey_granola_import,
            import_granola_files,
            get_granola_import_status,
            authorize_granola_import,
            revoke_granola_import_authorization,
            import_granola_mcp,
            prepare_speech_models,
            cancel_speech_model_download,
            clear_speech_models,
            probe_speech_models,
            list_devices,
            refresh_devices,
            test_audio_input,
            test_system_audio_tap,
            open_privacy_pane,
            restart_app,
            install_cli_tool,
            install_agent_hooks,
            ensure_cli_tools,
            install_workspace_skills,
            list_sessions,
            delete_session,
            reconcile_project_notes,
            start_recording,
            cancel_recording_startup,
            stop_recording,
            discard_recording,
            switch_recording_device,
            restart_system_audio_capture,
            pause_recording,
            resume_recording,
            set_live_transcription_mode,
            get_recording_status,
            toggle_active_pad,
            append_active_pad_line,
            show_main_window,
            sync_memo,
            checkpoint_memo_line,
            request_backchannel_for_memo,
            steer_backchannel_for_memo,
            hydrate_prep_sketch,
            steer_prep_hydration,
            process_session,
            retry_session,
            refine_session,
            reprocess_session_with_people,
            save_draft_note,
            discard_note,
            cancel_process_session,
            clear_session_note_error,
            import_transcript,
            import_audio_file,
            get_aligned_content,
            get_session_memo,
            get_vault_note,
            get_session_grounding,
            get_distill_trace,
            open_note,
            open_note_in_obsidian,
            open_note_target_in_obsidian,
            get_capabilities,
        ])
        .build(tauri::generate_context!())
        .expect("error while building margins desktop")
        .run(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                desktop_live::shutdown_loopback_api();
            }
        });
}

/// Run the native recorder as a small local service with no application
/// window. Local integrations discover it through the same private V1 file as
/// the desktop compatibility adapter.
#[cfg(feature = "live-runtime")]
pub fn run_live_runtime() -> anyhow::Result<()> {
    configure_pi_agent_dir();
    let settings = load_settings();
    let fallback_work_dir = default_work_dir();
    let work_dir = settings_work_dir(&settings, &fallback_work_dir);
    let state = build_app_state(work_dir, settings.clone());

    let registry = Arc::clone(&state.device_registry);
    std::thread::Builder::new()
        .name("margins-device-watcher".to_string())
        .spawn(move || device_registry::run_device_watcher(registry, |_| {}))?;

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(async move {
            desktop_live::start_cli_loopback_api(Arc::clone(&state)).map_err(anyhow::Error::msg)?;
            live_backchannel::prewarm_live_models(settings);
            eprintln!("margins-live: ready");

            wait_for_live_runtime_shutdown().await?;
            if state.recording.lock().unwrap().is_some() {
                let ctx = ctx::Ctx::no_emit(Arc::clone(&state));
                if let Err(error) = stop_recording_impl(&ctx).await {
                    eprintln!("margins-live: could not finish the active meeting: {error}");
                }
            }
            desktop_live::shutdown_loopback_api();
            Ok(())
        })
}

#[cfg(all(feature = "live-runtime", unix))]
async fn wait_for_live_runtime_shutdown() -> anyhow::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result?,
        _ = terminate.recv() => {},
    }
    Ok(())
}

#[cfg(all(feature = "live-runtime", not(unix)))]
async fn wait_for_live_runtime_shutdown() -> anyhow::Result<()> {
    tokio::signal::ctrl_c().await?;
    Ok(())
}

#[cfg(all(feature = "tauri-app", target_os = "macos"))]
fn apply_macos_window_material(app: &mut tauri::App) {
    use objc2_web_kit::WKWebView;
    use window_vibrancy::{
        apply_liquid_glass, apply_vibrancy, LiquidGlassOptions, NSGlassEffectViewStyle,
        NSVisualEffectMaterial, NSVisualEffectState,
    };

    let Some(window) = app.get_webview_window("main") else {
        return;
    };

    // Decouple the sidebar vibrancy material from the OS appearance: pin the
    // NSWindow to the Aqua (light) appearance so system dark mode no longer
    // tints our light-theme sidebar. The web layer still drives app theme via
    // `data-theme`; this only fixes the native material behind the sidebar.
    if let Ok(ns_window) = window.ns_window() {
        use objc2::runtime::AnyObject;
        use objc2::{class, msg_send};
        use objc2_foundation::NSString;
        unsafe {
            let ns_window = ns_window as *mut AnyObject;
            if !ns_window.is_null() {
                let name = NSString::from_str("NSAppearanceNameAqua");
                let appearance: *mut AnyObject =
                    msg_send![class!(NSAppearance), appearanceNamed: &*name];
                if !appearance.is_null() {
                    let _: () = msg_send![ns_window, setAppearance: appearance];
                }
            }
        }
    }

    let glass_window = window.clone();
    let fallback_window = window.clone();

    if let Err(error) = window.with_webview(move |webview| {
        let webview: &WKWebView = unsafe { &*webview.inner().cast() };
        let options = LiquidGlassOptions::new(NSGlassEffectViewStyle::Sidebar)
            .radius(26.0)
            .opaque(false)
            .content_view(webview);

        if let Err(error) = apply_liquid_glass(&glass_window, options) {
            eprintln!(
                "margins: macOS Liquid Glass unavailable ({error}); falling back to sidebar vibrancy"
            );
            if let Err(fallback_error) = apply_vibrancy(
                &fallback_window,
                NSVisualEffectMaterial::Sidebar,
                Some(NSVisualEffectState::FollowsWindowActiveState),
                None,
            ) {
                eprintln!("margins: could not apply macOS sidebar vibrancy: {fallback_error}");
            }
        }
    }) {
        eprintln!("margins: could not access the WKWebView for Liquid Glass: {error}");
    }

    // Liquid Glass rebuilds the titlebar view hierarchy after Tauri applies
    // `trafficLightPosition`, which resets the visible controls to AppKit's
    // tight default inset. Reapply the intended geometry to the actual native
    // button container after the material is installed.
    position_macos_window_controls(&window);
}

#[cfg(all(feature = "tauri-app", target_os = "macos"))]
fn position_macos_window_controls(window: &tauri::WebviewWindow) {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::{NSPoint, NSRect};

    let Ok(ns_window) = window.ns_window() else {
        return;
    };

    unsafe {
        let ns_window = ns_window as *mut AnyObject;
        if ns_window.is_null() {
            return;
        }

        let close: *mut AnyObject = msg_send![ns_window, standardWindowButton: 0_isize];
        let minimize: *mut AnyObject = msg_send![ns_window, standardWindowButton: 1_isize];
        let zoom: *mut AnyObject = msg_send![ns_window, standardWindowButton: 2_isize];
        if close.is_null() || minimize.is_null() {
            return;
        }

        let close_frame: NSRect = msg_send![close, frame];
        let minimize_frame: NSRect = msg_send![minimize, frame];
        let spacing = minimize_frame.origin.x - close_frame.origin.x;

        for (index, button) in [close, minimize, zoom].into_iter().enumerate() {
            if button.is_null() {
                continue;
            }
            let frame: NSRect = msg_send![button, frame];
            let origin = NSPoint::new(16.0 + index as f64 * spacing, frame.origin.y);
            let _: () = msg_send![button, setFrameOrigin: origin];
        }

        let button_superview: *mut AnyObject = msg_send![close, superview];
        if button_superview.is_null() {
            return;
        }
        let titlebar_container: *mut AnyObject = msg_send![button_superview, superview];
        if titlebar_container.is_null() {
            return;
        }

        let mut container_frame: NSRect = msg_send![titlebar_container, frame];
        container_frame.origin.y -= 6.0;
        container_frame.size.height += 6.0;
        let _: () = msg_send![titlebar_container, setFrame: container_frame];
    }
}

#[cfg(all(feature = "tauri-app", not(target_os = "macos")))]
fn apply_macos_window_material(_app: &mut tauri::App) {}

#[cfg(all(feature = "tauri-app", any(debug_assertions, feature = "devtools")))]
fn open_devtools_if_requested(app: &mut tauri::App) {
    let requested = std::env::var("MARGINS_OPEN_DEVTOOLS")
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false);

    if !requested {
        return;
    }

    if let Some(window) = app.get_webview_window("main") {
        window.open_devtools();
    }
}

#[cfg(all(
    feature = "tauri-app",
    not(any(debug_assertions, feature = "devtools"))
))]
fn open_devtools_if_requested(_app: &mut tauri::App) {}
