// ---------------------------------------------------------------------------
// web_session — WP3: headless web-capture session state
//
// Manages server-side recording state for browser clients that upload
// webm/opus chunks. On stop, the accumulated WebM/Opus is finalized in
// process to a mono 16 kHz WAV, and the memo is persisted to disk so that
// `process_session` works unchanged.
// ---------------------------------------------------------------------------

use crate::{settings::Settings, AppState, MemoLine};
use margins::session;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

// Chrome can clamp background-tab timers to roughly one minute. Keep the
// heartbeat lease beyond that ordinary suspension window; transport evidence
// remains an independent, shorter proof of a live capture owner.
pub const WEB_OWNER_LEASE_TIMEOUT_MS: u64 = 75_000;
pub const WEB_TRANSPORT_OWNER_EVIDENCE_MS: u64 = 10_000;
pub const HOSTED_CAPTURE_PROTOCOL_VERSION: u8 = 2;

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// ---------------------------------------------------------------------------
// State struct
// ---------------------------------------------------------------------------

/// Per-session recording state for a headless web client.
pub struct WebRecordingState {
    /// Globally unambiguous, non-secret identity used for registry addressing.
    pub recording_id: String,
    pub session_name: String,
    /// Absolute path to the project work directory.
    pub work_dir: PathBuf,
    /// Absolute path to the accumulating webm file.
    pub webm_path: PathBuf,
    /// Open file handle for appending chunks; None if already closed.
    pub file: Option<File>,
    /// Wall-clock start time used to compute elapsed_secs.
    pub started_at: SystemTime,
    /// Memo lines synced from the frontend.
    pub memo_lines: Vec<MemoLine>,
    /// Optional continuously decoding ONNX worker used by the Linux latency harness.
    pub live_asr: Option<crate::web_live_asr::WebLiveAsrHandle>,
    /// Browser MediaRecorder pause state. Audio finalization still happens only on stop.
    pub paused: bool,
    /// Opaque capability created by the browser capture operation. It is never
    /// exposed in status and must match every upload/mutating command.
    pub owner_id: String,
    pub owner_last_heartbeat: Instant,
    pub owner_last_heartbeat_unix_ms: u64,
    /// Only an owner that has first read the server memo may replace it.
    pub memo_hydrated_owner: Option<String>,
    /// Durable finalization failures retain the session for retry/discard.
    pub finalization_error: Option<String>,
    /// Persisted recovery lifecycle. Active captures have no recovery phase.
    recovery_phase: Option<WebRecoveryPhase>,
    /// Ordered durable-upload protocol state. Receipts make client retries
    /// idempotent; a request may append only at `next_webm_sequence`.
    pub next_webm_sequence: u64,
    pub webm_receipts: BTreeMap<u64, (u64, u64)>,
    /// Last capture time durably published to the live transcript journal.
    pub last_live_checkpoint_ms: Arc<AtomicU64>,
    /// Earliest capture time at which a failed checkpoint may be retried.
    pub next_live_checkpoint_attempt_ms: Arc<AtomicU64>,
    /// Bounds hosted status polling to one transcript checkpoint at a time.
    pub live_checkpoint_in_flight: Arc<AtomicBool>,
    /// Transport facts are atomics so status snapshots never need to borrow the
    /// append path's mutable file handle. Counters advance only after the
    /// corresponding write/admission succeeds.
    pub transport: WebCaptureTransportTelemetry,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebRecordingStart {
    pub session_name: String,
    pub recording_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WebRecoveryPhase {
    Finalizing,
    Failed,
    CleanupPending,
}

fn legacy_recovery_phase() -> WebRecoveryPhase {
    WebRecoveryPhase::Failed
}

#[derive(Serialize, Deserialize)]
struct WebRecoveryManifest {
    version: u8,
    #[serde(default = "legacy_recovery_phase")]
    phase: WebRecoveryPhase,
    recording_id: String,
    session_name: String,
    started_at_unix_ms: u64,
    memo_lines: Vec<MemoLine>,
    finalization_error: String,
    webm_chunk_count: u64,
    webm_bytes: u64,
    webm_last_received_unix_ms: Option<u64>,
    live_pcm_batch_count: u64,
    live_pcm_sample_count: u64,
    live_pcm_last_received_unix_ms: Option<u64>,
}

const RECOVERY_MANIFEST_SUFFIX: &str = ".recovery.json";

fn new_recording_id() -> String {
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn recovery_manifest_path(ws: &WebRecordingState) -> PathBuf {
    ws.work_dir
        .join(".margins/recordings")
        .join(format!("{}{}", ws.recording_id, RECOVERY_MANIFEST_SUFFIX))
}

fn remove_if_present(path: &std::path::Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Failed to delete {}: {error}", path.display())),
    }
}

fn sync_webm_before_recovery_manifest(ws: &mut WebRecordingState) -> Result<(), String> {
    ws.file
        .as_mut()
        .ok_or_else(|| "Web recording file is already closed".to_string())?
        .sync_all()
        .map_err(|error| format!("Failed to sync durable browser audio: {error}"))?;
    if let Some(parent) = ws.webm_path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Failed to sync browser audio directory: {error}"))?;
    }
    Ok(())
}

fn persist_recovery_manifest(
    ws: &WebRecordingState,
    phase: WebRecoveryPhase,
    recovery_message: &str,
) -> Result<(), String> {
    let started_at_unix_ms = ws
        .started_at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX);
    let manifest = WebRecoveryManifest {
        version: 2,
        phase,
        recording_id: ws.recording_id.clone(),
        session_name: ws.session_name.clone(),
        started_at_unix_ms,
        memo_lines: ws.memo_lines.clone(),
        finalization_error: recovery_message.to_string(),
        webm_chunk_count: ws.transport.webm_chunk_count.load(Ordering::Acquire),
        webm_bytes: ws.transport.webm_bytes.load(Ordering::Acquire),
        webm_last_received_unix_ms: nonzero_timestamp(
            ws.transport
                .webm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
        live_pcm_batch_count: ws.transport.live_pcm_batch_count.load(Ordering::Acquire),
        live_pcm_sample_count: ws.transport.live_pcm_sample_count.load(Ordering::Acquire),
        live_pcm_last_received_unix_ms: nonzero_timestamp(
            ws.transport
                .live_pcm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
    };
    let path = recovery_manifest_path(ws);
    let temp = path.with_extension("recovery.json.tmp");
    let encoded = serde_json::to_vec_pretty(&manifest).map_err(|error| error.to_string())?;
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| format!("Failed to persist hosted recovery: {error}"))?;
    file.write_all(&encoded)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("Failed to sync hosted recovery: {error}"))?;
    drop(file);
    std::fs::rename(&temp, &path)
        .map_err(|error| format!("Failed to publish hosted recovery: {error}"))?;
    if let Some(parent) = path.parent() {
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("Failed to sync hosted recovery directory: {error}"))?;
    }
    Ok(())
}

fn valid_recording_id(recording_id: &str) -> bool {
    recording_id.len() == 32
        && recording_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn open_regular_file_no_follow(path: &std::path::Path) -> Result<File, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if metadata.file_type().is_symlink() {
        return Err(format!("{} is a symbolic link", path.display()));
    }
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }

    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|error| {
        format!(
            "could not open {} without following links: {error}",
            path.display()
        )
    })?;
    if !file
        .metadata()
        .map_err(|error| format!("could not inspect opened {}: {error}", path.display()))?
        .is_file()
    {
        return Err(format!("{} did not open as a regular file", path.display()));
    }
    Ok(file)
}

fn quarantine_recovery_manifest(path: &std::path::Path, reason: &str) {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let mut ordinal = 0_u32;
    let quarantine_path = loop {
        let suffix = if ordinal == 0 {
            ".quarantine".to_string()
        } else {
            format!(".quarantine.{ordinal}")
        };
        let candidate = path.with_file_name(format!("{file_name}{suffix}"));
        if !candidate.exists() {
            break candidate;
        }
        ordinal = ordinal.saturating_add(1);
    };
    match std::fs::rename(path, &quarantine_path) {
        Ok(()) => eprintln!(
            "margins: quarantined malformed hosted recovery {}: {reason}",
            quarantine_path.display()
        ),
        Err(error) => eprintln!(
            "margins: could not quarantine malformed hosted recovery {} ({reason}): {error}",
            path.display()
        ),
    }
}

fn validate_recovery_manifest(
    path: &std::path::Path,
    recordings_dir: &std::path::Path,
    manifest: &WebRecoveryManifest,
) -> Result<PathBuf, String> {
    if manifest.version != 1 && manifest.version != 2 {
        return Err(format!("unsupported manifest version {}", manifest.version));
    }
    if !valid_recording_id(&manifest.recording_id) {
        return Err("recording ID is not canonical lowercase hexadecimal".to_string());
    }
    let expected_name = format!("{}{}", manifest.recording_id, RECOVERY_MANIFEST_SUFFIX);
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err("manifest filename does not match its recording ID".to_string());
    }
    crate::validate_session_name(&manifest.session_name)?;
    let durable_counts_empty = manifest.webm_chunk_count == 0 && manifest.webm_bytes == 0;
    let durable_counts_present = manifest.webm_chunk_count > 0 && manifest.webm_bytes > 0;
    if !durable_counts_empty && !durable_counts_present {
        return Err("WebM chunk/byte counters are inconsistent".to_string());
    }
    if manifest.webm_chunk_count == 0 && manifest.webm_last_received_unix_ms.is_some() {
        return Err("empty WebM telemetry has a last-received timestamp".to_string());
    }
    if manifest.webm_chunk_count > 0 && manifest.webm_last_received_unix_ms.is_none() {
        return Err("non-empty WebM telemetry has no last-received timestamp".to_string());
    }
    let pcm_counts_empty =
        manifest.live_pcm_batch_count == 0 && manifest.live_pcm_sample_count == 0;
    let pcm_counts_present =
        manifest.live_pcm_batch_count > 0 && manifest.live_pcm_sample_count > 0;
    if !pcm_counts_empty && !pcm_counts_present {
        return Err("live PCM batch/sample counters are inconsistent".to_string());
    }
    if manifest.live_pcm_batch_count == 0 && manifest.live_pcm_last_received_unix_ms.is_some() {
        return Err("empty live PCM telemetry has a last-received timestamp".to_string());
    }
    if manifest.live_pcm_batch_count > 0 && manifest.live_pcm_last_received_unix_ms.is_none() {
        return Err("non-empty live PCM telemetry has no last-received timestamp".to_string());
    }

    let webm_path = recordings_dir.join(format!("{}_upload.webm", manifest.session_name));
    if manifest.phase != WebRecoveryPhase::CleanupPending {
        if !durable_counts_present {
            return Err("a finalization recovery has no durable WebM counters".to_string());
        }
        let webm = open_regular_file_no_follow(&webm_path)
            .map_err(|error| format!("durable WebM artifact is unavailable: {error}"))?;
        let metadata = webm
            .metadata()
            .map_err(|error| format!("durable WebM artifact is unavailable: {error}"))?;
        if metadata.len() != manifest.webm_bytes {
            return Err(
                "durable WebM artifact length does not match its committed counter".to_string(),
            );
        }
    } else {
        match std::fs::symlink_metadata(&webm_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err("cleanup-pending WebM artifact is a symbolic link".to_string());
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cleanup-pending WebM artifact is unavailable: {error}"
                ));
            }
        }
    }
    Ok(webm_path)
}

fn recovery_work_dirs(fallback: &std::path::Path, settings: &Settings) -> Vec<PathBuf> {
    let mut candidates = vec![fallback.to_path_buf()];
    candidates.extend(
        settings
            .projects
            .iter()
            .map(|project| PathBuf::from(crate::expand_tilde(&project.path))),
    );
    let mut dirs = candidates
        .into_iter()
        .filter_map(|path| match std::fs::canonicalize(&path) {
            Ok(canonical) => Some(canonical),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                eprintln!(
                    "margins: skipping hosted recovery workdir {}: {error}",
                    path.display()
                );
                None
            }
        })
        .collect::<Vec<_>>();
    dirs.sort();
    dirs.dedup();
    dirs
}

pub fn reconstruct_failed_web_sessions(
    fallback: &std::path::Path,
    settings: &Settings,
) -> BTreeMap<String, WebRecordingState> {
    let mut recovered = BTreeMap::new();
    for work_dir in recovery_work_dirs(fallback, settings) {
        let recordings_dir = work_dir.join(".margins/recordings");
        let Ok(recordings_metadata) = std::fs::symlink_metadata(&recordings_dir) else {
            continue;
        };
        if recordings_metadata.file_type().is_symlink() || !recordings_metadata.is_dir() {
            eprintln!(
                "margins: refusing hosted recovery directory that is not a direct directory: {}",
                recordings_dir.display()
            );
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&recordings_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(RECOVERY_MANIFEST_SUFFIX))
            {
                continue;
            }
            let bytes = match open_regular_file_no_follow(&path).and_then(|mut file| {
                let mut bytes = Vec::new();
                file.read_to_end(&mut bytes)
                    .map_err(|error| format!("could not read JSON: {error}"))?;
                Ok(bytes)
            }) {
                Ok(bytes) => bytes,
                Err(error) => {
                    quarantine_recovery_manifest(&path, &error);
                    continue;
                }
            };
            let manifest = match serde_json::from_slice::<WebRecoveryManifest>(&bytes) {
                Ok(manifest) => manifest,
                Err(error) => {
                    quarantine_recovery_manifest(&path, &format!("invalid JSON: {error}"));
                    continue;
                }
            };
            let webm_path = match validate_recovery_manifest(&path, &recordings_dir, &manifest) {
                Ok(path) => path,
                Err(error) => {
                    quarantine_recovery_manifest(&path, &error);
                    continue;
                }
            };
            if recovered.contains_key(&manifest.recording_id) {
                quarantine_recovery_manifest(
                    &path,
                    "duplicate recording ID already reconstructed from an earlier workdir",
                );
                continue;
            }
            let transport = WebCaptureTransportTelemetry::default();
            transport
                .webm_chunk_count
                .store(manifest.webm_chunk_count, Ordering::Release);
            transport
                .webm_bytes
                .store(manifest.webm_bytes, Ordering::Release);
            transport.webm_last_received_unix_ms.store(
                manifest.webm_last_received_unix_ms.unwrap_or(0),
                Ordering::Release,
            );
            transport
                .live_pcm_batch_count
                .store(manifest.live_pcm_batch_count, Ordering::Release);
            transport
                .live_pcm_sample_count
                .store(manifest.live_pcm_sample_count, Ordering::Release);
            transport.live_pcm_last_received_unix_ms.store(
                manifest.live_pcm_last_received_unix_ms.unwrap_or(0),
                Ordering::Release,
            );
            let started_at =
                UNIX_EPOCH + std::time::Duration::from_millis(manifest.started_at_unix_ms);
            let recording_id = manifest.recording_id.clone();
            recovered.insert(
                recording_id.clone(),
                WebRecordingState {
                    recording_id,
                    session_name: manifest.session_name,
                    work_dir: work_dir.clone(),
                    webm_path,
                    file: None,
                    started_at,
                    memo_lines: manifest.memo_lines,
                    live_asr: None,
                    paused: false,
                    owner_id: new_recording_id(),
                    owner_last_heartbeat: Instant::now()
                        - std::time::Duration::from_millis(WEB_OWNER_LEASE_TIMEOUT_MS + 1),
                    owner_last_heartbeat_unix_ms: 0,
                    memo_hydrated_owner: None,
                    finalization_error: Some(manifest.finalization_error),
                    recovery_phase: Some(manifest.phase),
                    next_webm_sequence: manifest.webm_chunk_count,
                    webm_receipts: BTreeMap::new(),
                    last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                    next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                    live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                    transport,
                },
            );
        }
    }
    recovered
}

#[derive(Default)]
pub struct WebCaptureTransportTelemetry {
    webm_chunk_count: AtomicU64,
    webm_bytes: AtomicU64,
    webm_last_received_unix_ms: AtomicU64,
    webm_last_received_monotonic_ms: AtomicU64,
    live_pcm_batch_count: AtomicU64,
    live_pcm_sample_count: AtomicU64,
    live_pcm_last_received_unix_ms: AtomicU64,
    live_pcm_last_received_monotonic_ms: AtomicU64,
}

impl WebCaptureTransportTelemetry {
    fn record_webm_chunk(&self, byte_count: u64) {
        self.webm_bytes
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(byte_count))
            })
            .ok();
        self.webm_chunk_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
        self.webm_last_received_unix_ms
            .store(unix_time_ms(), Ordering::Release);
        self.webm_last_received_monotonic_ms
            .store(monotonic_time_ms(), Ordering::Release);
    }

    fn record_live_pcm_batch(&self, sample_count: u64) {
        self.live_pcm_sample_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(sample_count))
            })
            .ok();
        self.live_pcm_batch_count
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_add(1))
            })
            .ok();
        self.live_pcm_last_received_unix_ms
            .store(unix_time_ms(), Ordering::Release);
        self.live_pcm_last_received_monotonic_ms
            .store(monotonic_time_ms(), Ordering::Release);
    }
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn monotonic_time_ms() -> u64 {
    static PROCESS_EPOCH: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    PROCESS_EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
        .saturating_add(1)
}

fn nonzero_timestamp(value: u64) -> Option<u64> {
    (value != 0).then_some(value)
}

fn transport_age_ms(monotonic_received_ms: u64, unix_received_ms: u64) -> Option<u64> {
    if monotonic_received_ms > 0 {
        Some(monotonic_time_ms().saturating_sub(monotonic_received_ms))
    } else {
        nonzero_timestamp(unix_received_ms).map(|received| unix_time_ms().saturating_sub(received))
    }
}

// ---------------------------------------------------------------------------
// Chunk append
// ---------------------------------------------------------------------------

/// Append a browser-uploaded webm/opus chunk to the in-progress webm file.
///
/// Called by the WP2 HTTP route for every incoming chunk.  The signature is
/// fixed so WP2 can wire it without touching this file.
pub fn handle_audio_chunk(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    sequence: u64,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("Audio chunk must not be empty".to_string());
    }
    let mut map = state.web_sessions.lock().unwrap();
    let ws = map
        .get_mut(recording_id)
        .ok_or_else(|| format!("No active web recording for ID '{recording_id}'"))?;
    ensure_owner(ws, owner_id)?;
    let fingerprint = webm_chunk_fingerprint(bytes);
    if sequence < ws.next_webm_sequence {
        return match ws.webm_receipts.get(&sequence) {
            Some(&(length, prior_fingerprint))
                if length == bytes.len() as u64 && prior_fingerprint == fingerprint =>
            {
                Ok(())
            }
            _ => Err(format!(
                "WebM chunk sequence {sequence} was already committed with different content"
            )),
        };
    }
    if sequence > ws.next_webm_sequence {
        return Err(format!(
            "Out-of-order WebM chunk {sequence}; expected {}",
            ws.next_webm_sequence
        ));
    }
    let file = ws
        .file
        .as_mut()
        .ok_or("Web recording file is already closed")?;
    file.write_all(bytes)
        .map_err(|e| format!("Failed to write audio chunk: {e}"))?;
    file.sync_data()
        .map_err(|e| format!("Failed to durably sync audio chunk: {e}"))?;
    ws.webm_receipts
        .insert(sequence, (bytes.len() as u64, fingerprint));
    ws.next_webm_sequence = ws.next_webm_sequence.saturating_add(1);
    ws.transport.record_webm_chunk(bytes.len() as u64);
    Ok(())
}

fn webm_chunk_fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

/// Inject raw little-endian f32 PCM into the headless live-ASR worker.
pub fn handle_live_pcm_chunk(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    channel: margins::recorder::LiveAudioChannel,
    sample_rate: u32,
    bytes: &[u8],
) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("Live PCM body must not be empty".to_string());
    }
    if bytes.len() % 4 != 0 {
        return Err("Live PCM body length must be divisible by four".to_string());
    }
    let map = state.web_sessions.lock().unwrap();
    let ws = map
        .get(recording_id)
        .ok_or_else(|| format!("No active web recording for ID '{recording_id}'"))?;
    ensure_owner(ws, owner_id)?;
    let client = ws
        .live_asr
        .as_ref()
        .ok_or("Headless live ASR is not configured; build with parakeet-asr-dynamic and set MARGINS_PARAKEET_MODEL_DIR")?
        .client();
    let samples = bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect::<Vec<_>>();
    // inject() is nonblocking. Keeping the session-map lock through admission
    // establishes a strict happens-before edge with stop/removal without a
    // permit that could outlive the session.
    let sample_count = samples.len() as u64;
    let result = client.inject(channel, sample_rate, samples);
    if result.is_ok() {
        let ws = map
            .get(recording_id)
            .expect("session remains locked through live PCM admission");
        ws.transport.record_live_pcm_batch(sample_count);
    }
    drop(map);
    result
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

/// Create a new `WebRecordingState` for `session_name`, register it in the
/// session index, and insert it into `state.web_sessions`.
///
/// `work_dir` must already be resolved by the caller (via
/// `work_dir_for_project_id`); `name` must already be uniquified.
pub fn start_web_recording(
    state: &Arc<AppState>,
    work_dir: PathBuf,
    name: String,
    owner_id: String,
) -> Result<WebRecordingStart, String> {
    use chrono::Local;

    if owner_id.trim().is_empty() {
        return Err("Hosted recording owner ID must not be empty".to_string());
    }

    // Hold the map lock through allocation so simultaneous hosted start
    // requests cannot each create a different uniquely named active session.
    let mut map = state.web_sessions.lock().unwrap();
    if let Some(active) = map.values().min_by_key(|state| &state.recording_id) {
        return Err(format!(
            "Hosted recording '{}' is already active; finish or discard it before starting another",
            active.session_name
        ));
    }

    let margins_dir = work_dir.join(".margins");
    std::fs::create_dir_all(&margins_dir).map_err(|e| e.to_string())?;

    // Create recordings dir
    let recordings_dir = margins_dir.join("recordings");
    std::fs::create_dir_all(&recordings_dir).map_err(|e| e.to_string())?;

    // Register session in the session index
    let notes_path = format!(".margins/{}.md", name);
    let start_time = Local::now();
    session::create_session(&margins_dir, &name, &start_time, &notes_path)
        .map_err(|e| e.to_string())?;

    // Register segment 0 in the index (duration unknown until stop)
    let wav_path = format!(".margins/recordings/{name}_seg0.wav");
    session::add_segment(&margins_dir, &name, 0, &wav_path, 0, None).map_err(|e| e.to_string())?;

    // Create the webm accumulation file
    let webm_path = margins_dir
        .join("recordings")
        .join(format!("{name}_upload.webm"));
    let file = File::create(&webm_path)
        .map_err(|e| format!("Failed to create webm file {}: {e}", webm_path.display()))?;

    let live_asr = match crate::web_live_asr::start(margins_dir.clone(), name.clone()) {
        Ok(live) => live,
        Err(error) => {
            drop(file);
            let _ = std::fs::remove_file(&webm_path);
            let _ = session::delete_session_row(&margins_dir, &name);
            return Err(error);
        }
    };
    let owner_last_heartbeat_unix_ms = unix_time_ms();
    // Random IDs are collision-resistant; checking both live registries also
    // makes a generated collision with any reconstructed recovery impossible
    // to publish in this server process.
    let recording_id = loop {
        let candidate = new_recording_id();
        let collides_with_recovery = state
            .failed_web_sessions
            .lock()
            .unwrap()
            .contains_key(&candidate);
        if !map.contains_key(&candidate) && !collides_with_recovery {
            break candidate;
        }
    };
    let ws = WebRecordingState {
        recording_id: recording_id.clone(),
        session_name: name.clone(),
        work_dir,
        webm_path,
        file: Some(file),
        started_at: SystemTime::now(),
        memo_lines: Vec::new(),
        live_asr,
        paused: false,
        owner_id,
        owner_last_heartbeat: Instant::now(),
        owner_last_heartbeat_unix_ms,
        memo_hydrated_owner: None,
        finalization_error: None,
        recovery_phase: None,
        next_webm_sequence: 0,
        webm_receipts: BTreeMap::new(),
        last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
        next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
        live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
        transport: WebCaptureTransportTelemetry::default(),
    };

    map.insert(recording_id.clone(), ws);
    drop(map);
    let settings = state.settings.lock().unwrap().clone();
    crate::ai_config::schedule_capture_ai_preflight(settings, "web");
    Ok(WebRecordingStart {
        session_name: name,
        recording_id,
    })
}

pub fn set_web_recording_paused(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    paused: bool,
) -> Result<crate::recording::RecordingStatus, String> {
    let mut map = state.web_sessions.lock().unwrap();
    let ws = map
        .get_mut(recording_id)
        .ok_or_else(|| format!("No active web recording for ID '{recording_id}'"))?;
    ensure_owner(ws, owner_id)?;
    ws.paused = paused;
    Ok(web_recording_status(ws))
}

fn ensure_owner(ws: &WebRecordingState, owner_id: &str) -> Result<(), String> {
    if !owner_id.is_empty() && ws.owner_id == owner_id {
        Ok(())
    } else {
        Err(format!(
            "Capture authority for '{}' belongs to another browser operation",
            ws.session_name
        ))
    }
}

fn owner_lease_active(ws: &WebRecordingState) -> bool {
    ws.owner_last_heartbeat.elapsed().as_millis() <= WEB_OWNER_LEASE_TIMEOUT_MS as u128
}

fn transport_owner_evidence_fresh(ws: &WebRecordingState) -> bool {
    let now = monotonic_time_ms();
    let last = ws
        .transport
        .webm_last_received_monotonic_ms
        .load(Ordering::Acquire)
        .max(
            ws.transport
                .live_pcm_last_received_monotonic_ms
                .load(Ordering::Acquire),
        );
    last > 0 && now.saturating_sub(last) <= WEB_TRANSPORT_OWNER_EVIDENCE_MS
}

pub fn heartbeat_web_recording(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
) -> Result<(), String> {
    let refresh = |ws: &mut WebRecordingState| {
        ensure_owner(ws, owner_id)?;
        ws.owner_last_heartbeat = Instant::now();
        ws.owner_last_heartbeat_unix_ms = unix_time_ms();
        Ok(())
    };
    {
        let mut active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get_mut(recording_id) {
            return refresh(ws);
        }
    }
    let mut failed = state.failed_web_sessions.lock().unwrap();
    failed
        .get_mut(recording_id)
        .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))
        .and_then(refresh)
}

pub fn claim_web_recording_recovery(
    state: &Arc<AppState>,
    recording_id: &str,
    new_owner_id: String,
) -> Result<(), String> {
    if new_owner_id.trim().is_empty() {
        return Err("Recovery owner ID must not be empty".to_string());
    }
    let claim = |ws: &mut WebRecordingState| {
        if ws.owner_id == new_owner_id {
            // The first response may be lost after the server accepted the claim.
            // Retrying the same opaque capability is idempotent and refreshes it.
            ws.owner_last_heartbeat = Instant::now();
            ws.owner_last_heartbeat_unix_ms = unix_time_ms();
            return Ok(());
        }
        if owner_lease_active(ws) || transport_owner_evidence_fresh(ws) {
            return Err(format!(
                "Capture '{}' still has a live browser owner or fresh audio transport",
                ws.session_name
            ));
        }
        ws.owner_id = new_owner_id.clone();
        ws.owner_last_heartbeat = Instant::now();
        ws.owner_last_heartbeat_unix_ms = unix_time_ms();
        ws.memo_hydrated_owner = None;
        Ok(())
    };
    {
        let mut active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get_mut(recording_id) {
            return claim(ws);
        }
    }
    let mut failed = state.failed_web_sessions.lock().unwrap();
    failed
        .get_mut(recording_id)
        .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))
        .and_then(claim)
}

pub fn hydrate_web_recording_memo(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: Option<&str>,
) -> Result<Vec<MemoLine>, String> {
    let hydrate = |ws: &mut WebRecordingState| {
        if let Some(owner_id) = owner_id.filter(|value| !value.is_empty()) {
            ensure_owner(ws, owner_id)?;
            ws.memo_hydrated_owner = Some(owner_id.to_string());
        }
        Ok(ws.memo_lines.clone())
    };
    {
        let mut active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get_mut(recording_id) {
            return hydrate(ws);
        }
    }
    let mut failed = state.failed_web_sessions.lock().unwrap();
    failed
        .get_mut(recording_id)
        .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))
        .and_then(hydrate)
}

pub fn sync_web_recording_memo(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    lines: Vec<MemoLine>,
) -> Result<(), String> {
    let replace = |ws: &mut WebRecordingState, lines: Vec<MemoLine>| {
        ensure_owner(ws, owner_id)?;
        if ws.recovery_phase == Some(WebRecoveryPhase::CleanupPending) {
            return Err(
                "Hosted capture cleanup is pending; retry Discard instead of changing its memo"
                    .to_string(),
            );
        }
        if ws.memo_hydrated_owner.as_deref() != Some(owner_id) {
            return Err(
                "Memo must be hydrated from the active capture before it can be replaced"
                    .to_string(),
            );
        }
        crate::persist_live_memo(&ws.work_dir, &ws.session_name, &lines)?;
        ws.memo_lines = lines;
        Ok(())
    };
    {
        let mut active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get_mut(recording_id) {
            return replace(ws, lines);
        }
    }
    let mut failed = state.failed_web_sessions.lock().unwrap();
    failed
        .get_mut(recording_id)
        .ok_or_else(|| {
            format!("Stale memo mutation for recording {recording_id}; capture is not retained")
        })
        .and_then(|ws| {
            replace(ws, lines)?;
            ws.recovery_phase = Some(WebRecoveryPhase::Failed);
            let message = ws
                .finalization_error
                .as_deref()
                .unwrap_or("Hosted finalization is recoverable.");
            persist_recovery_manifest(ws, WebRecoveryPhase::Failed, message)
        })
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebNotepadSnapshot {
    pub text: String,
    pub revision: String,
}

fn web_notepad_snapshot(lines: &[MemoLine]) -> WebNotepadSnapshot {
    let document = margins::core::TimedMemoDocument::from_committed(lines.to_vec());
    WebNotepadSnapshot {
        text: document.plain_text(),
        revision: document.revision(),
    }
}

pub fn get_web_recording_notepad(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
) -> Result<WebNotepadSnapshot, String> {
    let active = state.web_sessions.lock().unwrap();
    let ws = active
        .get(recording_id)
        .ok_or_else(|| format!("No active web recording for ID '{recording_id}'"))?;
    ensure_owner(ws, owner_id)?;
    let authority =
        margins::session::SqliteWorkspaceAuthorityStorage::open(ws.work_dir.join(".margins"))
            .map_err(|error| error.to_string())?;
    let memo = authority
        .memo(&ws.session_name)
        .map_err(|error| error.to_string())?;
    Ok(web_notepad_snapshot(&memo.lines))
}

/// Reconcile the whole hosted notepad through the same timestamp-preserving
/// model used by the TUI and local live bridge.
pub fn update_web_recording_notepad(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    expected_revision: &str,
    text: &str,
) -> Result<WebNotepadSnapshot, String> {
    let update = |ws: &mut WebRecordingState| {
        ensure_owner(ws, owner_id)?;
        let authority =
            margins::session::SqliteWorkspaceAuthorityStorage::open(ws.work_dir.join(".margins"))
                .map_err(|error| error.to_string())?;
        let current_memo = authority
            .memo(&ws.session_name)
            .map_err(|error| error.to_string())?;
        let current = web_notepad_snapshot(&current_memo.lines);
        if current.revision != expected_revision {
            return Err(
                "The notepad changed somewhere else. Review the latest text and try again."
                    .to_string(),
            );
        }
        let elapsed = ws.started_at.elapsed().unwrap_or_default().as_secs_f64();
        let observed_at_ms = (elapsed * 1000.0).round().max(0.0) as u64;
        let request_id = format!(
            "web-{}",
            sha256_hex(format!("{}\0{}\0{text}", ws.owner_id, expected_revision).as_bytes())
        );
        let receipt = authority
            .update_memo(
                &ws.session_name,
                &ws.owner_id,
                &request_id,
                expected_revision,
                observed_at_ms,
                ws.paused,
                text,
            )
            .map_err(|error| error.to_string())?;
        ws.memo_lines = receipt.lines;
        Ok(web_notepad_snapshot(&ws.memo_lines))
    };
    let mut active = state.web_sessions.lock().unwrap();
    active
        .get_mut(recording_id)
        .ok_or_else(|| format!("No active web recording for ID '{recording_id}'"))
        .and_then(update)
}

pub fn active_web_recording_status(
    state: &Arc<AppState>,
) -> Option<crate::recording::RecordingStatus> {
    let recoveries = web_recording_recoveries(state);
    let (status, checkpoint) = {
        let map = state.web_sessions.lock().unwrap();
        if map.is_empty() {
            drop(map);
            let failed = state.failed_web_sessions.lock().unwrap();
            let selected_id = recoveries.first()?.recording_id.as_str();
            return failed.get(selected_id).map(|ws| {
                let mut status = web_recording_status(ws);
                status.web_recoveries = recoveries;
                status
            });
        }
        let selected_id = map.keys().min()?;
        let ws = map.get(selected_id)?;
        let mut status = web_recording_status(ws);
        status.web_recoveries = recoveries;
        let capture_ms = ws.started_at.elapsed().unwrap_or_default().as_millis() as u64;
        let last_ms = ws.last_live_checkpoint_ms.load(Ordering::Acquire);
        let next_attempt_ms = ws.next_live_checkpoint_attempt_ms.load(Ordering::Acquire);
        let journal_due = crate::live_transcript_checkpoint_due(capture_ms, last_ms);
        let snapshot_due = crate::transcript_store::live_transcript_snapshot_due(
            &ws.work_dir.join(".margins"),
            &ws.session_name,
        );
        let checkpoint = ws
            .live_asr
            .as_ref()
            .filter(|worker| {
                !ws.paused
                    && worker.client().is_ready()
                    && (journal_due || snapshot_due)
                    && capture_ms >= next_attempt_ms
                    && ws
                        .live_checkpoint_in_flight
                        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok()
            })
            .map(|worker| {
                (
                    ws.work_dir.join(".margins"),
                    ws.session_name.clone(),
                    capture_ms,
                    worker.client(),
                    Arc::clone(&ws.last_live_checkpoint_ms),
                    Arc::clone(&ws.next_live_checkpoint_attempt_ms),
                    Arc::clone(&ws.live_checkpoint_in_flight),
                    journal_due,
                )
            });
        (status, checkpoint)
    };

    if let Some((
        margins_dir,
        session_name,
        capture_ms,
        client,
        last_ms,
        next_attempt_ms,
        in_flight,
        journal_due,
    )) = checkpoint
    {
        std::thread::spawn(move || {
            let result = if journal_due {
                client.checkpoint(capture_ms).and_then(|snapshot| {
                    crate::transcript_store::append_live_transcript_segment(
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
                    crate::transcript_store::write_live_transcript_snapshot(
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
                    let _ = crate::append_backchannel_trace(
                        &margins_dir,
                        &session_name,
                        serde_json::json!({
                            "kind": "periodic_transcript_checkpoint",
                            "lane": "web",
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
                        capture_ms.saturating_add(crate::LIVE_TRANSCRIPT_CHECKPOINT_RETRY_MS),
                        Ordering::Release,
                    );
                    let _ = crate::append_backchannel_trace(
                        &margins_dir,
                        &session_name,
                        serde_json::json!({
                            "kind": "periodic_transcript_checkpoint_failed",
                            "lane": "web",
                            "requested_capture_ms": capture_ms,
                            "error": error,
                        }),
                    );
                }
            }
            in_flight.store(false, Ordering::Release);
        });
    }

    Some(status)
}

pub fn web_recording_recoveries(
    state: &Arc<AppState>,
) -> Vec<crate::recording::WebRecordingRecoveryStatus> {
    let failed = state.failed_web_sessions.lock().unwrap();
    let mut recoveries = failed
        .values()
        .map(|ws| crate::recording::WebRecordingRecoveryStatus {
            recording_id: ws.recording_id.clone(),
            session_name: ws.session_name.clone(),
            elapsed_secs: ws.started_at.elapsed().unwrap_or_default().as_secs_f64(),
            finalization_error: ws.finalization_error.clone(),
            owner_lease_active: owner_lease_active(ws),
            recovery_phase: ws.recovery_phase.unwrap_or(WebRecoveryPhase::Failed),
        })
        .collect::<Vec<_>>();
    recoveries.sort_by(|left, right| {
        right
            .elapsed_secs
            .total_cmp(&left.elapsed_secs)
            .then_with(|| left.recording_id.cmp(&right.recording_id))
    });
    recoveries
}

fn web_recording_status(ws: &WebRecordingState) -> crate::recording::RecordingStatus {
    crate::recording::RecordingStatus {
        is_recording: true,
        paused: ws.paused,
        session_name: Some(ws.session_name.clone()),
        web_recording_id: Some(ws.recording_id.clone()),
        web_recoveries: Vec::new(),
        elapsed_secs: ws.started_at.elapsed().unwrap_or_default().as_secs_f64(),
        input_device_name: None,
        capture_device: crate::recording::CaptureDeviceState::active("Web capture"),
        // The server cannot observe browser MediaStream samples. The HTTP
        // adapter overlays its local Web Audio meter on this unknown value.
        mic_level: None,
        mic_audio_frame_count: 0,
        spk_level: 0.0,
        mic_drop_count: 0,
        spk_drop_count: 0,
        mic_gap_ms_total: 0,
        mic_switch_count: 0,
        timeline_reusable: true,
        speaker_silence_secs: 0.0,
        system_audio_expected: false,
        system_audio_frame_count: 0,
        system_audio_observed: false,
        system_audio_seen: false,
        tap_status: "not_expected".to_string(),
        tap_warning: None,
        live_transcription_mode: crate::live_backchannel::LiveTranscriptionMode::StereoSplit
            .as_str()
            .to_string(),
        capture_phase: if ws.finalization_error.is_some() {
            "needs_attention"
        } else if ws.paused {
            "paused"
        } else {
            "recording"
        }
        .to_string(),
        webm_chunk_count: Some(ws.transport.webm_chunk_count.load(Ordering::Acquire)),
        webm_bytes: Some(ws.transport.webm_bytes.load(Ordering::Acquire)),
        webm_last_received_unix_ms: nonzero_timestamp(
            ws.transport
                .webm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
        webm_last_received_age_ms: transport_age_ms(
            ws.transport
                .webm_last_received_monotonic_ms
                .load(Ordering::Acquire),
            ws.transport
                .webm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
        live_pcm_batch_count: Some(ws.transport.live_pcm_batch_count.load(Ordering::Acquire)),
        live_pcm_sample_count: Some(ws.transport.live_pcm_sample_count.load(Ordering::Acquire)),
        live_pcm_last_received_unix_ms: nonzero_timestamp(
            ws.transport
                .live_pcm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
        live_pcm_last_received_age_ms: transport_age_ms(
            ws.transport
                .live_pcm_last_received_monotonic_ms
                .load(Ordering::Acquire),
            ws.transport
                .live_pcm_last_received_unix_ms
                .load(Ordering::Acquire),
        ),
        live_pcm_configured: Some(ws.live_asr.is_some()),
        // Browser and server wall clocks can differ. Publishing the server's
        // sampling clock lets the frontend calculate transport age without
        // treating client/server skew as a stale upload.
        web_transport_server_unix_ms: Some(unix_time_ms()),
        web_owner_lease_active: Some(owner_lease_active(ws)),
        web_owner_last_heartbeat_unix_ms: Some(ws.owner_last_heartbeat_unix_ms),
        web_owner_heartbeat_age_ms: Some(
            ws.owner_last_heartbeat
                .elapsed()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        ),
        web_owner_lease_timeout_ms: Some(WEB_OWNER_LEASE_TIMEOUT_MS),
        web_finalization_error: ws.finalization_error.clone(),
        transcript_watermark: crate::transcript_store::read_live_transcript_watermark(
            &ws.work_dir.join(".margins"),
            &ws.session_name,
        ),
    }
}

// ---------------------------------------------------------------------------
// Stop
// ---------------------------------------------------------------------------

/// Stop a web recording: close the webm, transcode to WAV, write the memo,
/// update the session index with the segment duration, and write the capture
/// context sidecar.
///
/// Returns the session name on success.
pub fn stop_web_recording(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
) -> Result<String, String> {
    stop_web_recording_with(
        state,
        recording_id,
        owner_id,
        |webm, wav| finalize_webm_to_wav(&webm, &wav),
        None,
    )
}

fn stop_web_recording_with<Transcode>(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
    transcode: Transcode,
    post_transcode_error: Option<String>,
) -> Result<String, String>
where
    Transcode: FnOnce(PathBuf, PathBuf) -> Result<(), String> + Send,
{
    let stop_started = Instant::now();
    // Removing A releases the singleton active slot so B may start while A's
    // expensive finalization runs. A failure is retained in the separate
    // recovery map and can therefore never displace B.
    let mut ws = take_web_recording_for_finalization(state, recording_id, owner_id)?;
    let end_ms = ws.started_at.elapsed().unwrap_or_default().as_millis() as u64;
    append_capture_finish_trace(
        &ws.work_dir,
        &ws.session_name,
        serde_json::json!({"kind":"backend_stop_requested"}),
    );
    append_capture_finish_trace(
        &ws.work_dir,
        &ws.session_name,
        serde_json::json!({
            "kind":"live_pcm_input_closed",
            "admission_close_ms":stop_started.elapsed().as_millis(),
        }),
    );
    let work_dir = &ws.work_dir;
    let margins_dir = work_dir.join(".margins");
    let name = &ws.session_name;
    let wav_path = margins_dir
        .join("recordings")
        .join(format!("{name}_seg0.wav"));
    let live_asr = ws.live_asr.take();
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"final_live_asr_requested",
            "end_ms":end_ms,
            "available":live_asr.is_some(),
        }),
    );
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"durable_audio_transcode_requested",
            "finalizer":crate::webm_opus::HostedWebmFinalizer::from_env().name(),
        }),
    );
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({"kind":"capture_finalization_parallel_started"}),
    );

    let transcode_webm_path = ws.webm_path.clone();
    let transcode_wav_path = wav_path.clone();
    let branches = run_joined_branches(
        move || match live_asr {
            Some(live_asr) => live_asr.finish(end_ms).map(Some),
            None => Ok(None),
        },
        move || transcode(transcode_webm_path, transcode_wav_path),
    );
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"durable_audio_transcode_completed",
            "elapsed_ms":branches.right.elapsed_ms,
            "completed_unix_ms":branches.right.completed_unix_ms,
            "ok":branches.right.result.is_ok(),
            "error":branches.right.result.as_ref().err(),
        }),
    );

    // Qualification is deliberately after the join: neither transcript reuse
    // nor publication is decided while durable audio readiness is unknown.
    let mut qualified_live_transcript = None;
    let mut journal_written = false;
    let mut reuse_qualified = false;
    let mut decoded_until_ms = None;
    let mut command_wait_ms = None;
    let mut decode_ms = None;
    let mut format_ms = None;
    let mut mic_accepted_samples = None;
    let mut system_accepted_samples = None;
    let mut mic_dropped_samples = None;
    let mut system_dropped_samples = None;
    let mut mic_decoded_samples = None;
    let mut system_decoded_samples = None;
    if let Ok(Some(snapshot)) = &branches.left.result {
        journal_written = crate::transcript_store::append_live_transcript_segment(
            &margins_dir,
            name,
            "final",
            None,
            None,
            &snapshot.context,
        )
        .is_ok();
        reuse_qualified = terminal_web_live_transcript_is_reusable(snapshot, journal_written);
        decoded_until_ms = Some(snapshot.context.decoded_until_ms);
        mic_accepted_samples = Some(snapshot.context.mic_accepted_samples);
        system_accepted_samples = Some(snapshot.context.system_accepted_samples);
        mic_dropped_samples = Some(snapshot.context.mic_dropped_samples);
        system_dropped_samples = Some(snapshot.context.system_dropped_samples);
        mic_decoded_samples = Some(snapshot.mic_decoded_samples);
        system_decoded_samples = Some(snapshot.system_decoded_samples);
        if let Some(timing) = snapshot.context.timing.as_ref() {
            command_wait_ms = Some(timing.command_wait_ms);
            decode_ms = Some(timing.mic_update_ms + timing.system_update_ms);
            format_ms = Some(timing.format_ms);
        }
        if reuse_qualified {
            qualified_live_transcript = Some(snapshot.context.transcript.clone());
        }
    }
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"final_live_asr_completed",
            "elapsed_ms":branches.left.elapsed_ms,
            "completed_unix_ms":branches.left.completed_unix_ms,
            "available":branches.left.result.as_ref().ok().map_or(false, Option::is_some),
            "ok":branches.left.result.is_ok(),
            "error":branches.left.result.as_ref().err(),
            "journal_written":journal_written,
            "reuse_qualified":reuse_qualified,
            "decoded_until_ms":decoded_until_ms,
            "mic_accepted_samples":mic_accepted_samples,
            "system_accepted_samples":system_accepted_samples,
            "mic_dropped_samples":mic_dropped_samples,
            "system_dropped_samples":system_dropped_samples,
            "mic_decoded_samples":mic_decoded_samples,
            "system_decoded_samples":system_decoded_samples,
            "command_wait_ms":command_wait_ms,
            "decode_ms":decode_ms,
            "format_ms":format_ms,
        }),
    );
    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"capture_finalization_parallel_joined",
            "join_elapsed_ms":branches.join_elapsed_ms,
            "live_asr_elapsed_ms":branches.left.elapsed_ms,
            "transcode_elapsed_ms":branches.right.elapsed_ms,
            "live_asr_ok":branches.left.result.is_ok(),
            "transcode_ok":branches.right.result.is_ok(),
        }),
    );

    // Terminal ASR failure remains a soft fallback. Durable audio failure is
    // still fatal, but only after the ASR branch has joined and any successful
    // terminal snapshot has been journaled as before.
    if let Err(error) = branches.right.result {
        let message = retain_failed_web_finalization(state, recording_id, ws, &error);
        return Err(message);
    }
    if let Some(error) = post_transcode_error {
        let message = retain_failed_web_finalization(state, recording_id, ws, &error);
        return Err(message);
    }

    macro_rules! retain_on_error {
        ($result:expr) => {
            match $result {
                Ok(value) => value,
                Err(error) => {
                    let message =
                        retain_failed_web_finalization(state, recording_id, ws, &error.to_string());
                    return Err(message);
                }
            }
        };
    }

    // Compute duration by probing the WAV. Probe failure must remain
    // recoverable rather than silently publishing a zero-duration segment.
    let duration_secs = retain_on_error!(probe_wav_duration(&wav_path));

    // Update segment duration in session index
    retain_on_error!(
        session::update_segment_duration(&margins_dir, name, 0, duration_secs)
            .map_err(|e| e.to_string())
    );

    // Commit the final memo through the same revisioned authority used while
    // capture was live; it owns the Markdown projection.
    let authority = retain_on_error!(margins::session::SqliteWorkspaceAuthorityStorage::open(
        work_dir.join(".margins")
    )
    .map_err(|error| error.to_string()));
    let latest_memo = retain_on_error!(authority.memo(name).map_err(|error| error.to_string()));
    ws.memo_lines = latest_memo.lines;
    let memo_content = crate::recording::export_memo(&ws.memo_lines);
    retain_on_error!(crate::persist_live_memo(work_dir, name, &ws.memo_lines));
    let notes_path = crate::session_memo_path(work_dir, name);

    if let Some(transcript) = qualified_live_transcript.as_deref() {
        let publication = crate::write_qualified_headless_live_transcript_artifact(
            &margins_dir,
            name,
            &memo_content,
            transcript,
        );
        let published = publication.is_ok();
        if !published {
            // Reuse is only an optimization. Retain the WAV and force the
            // normal offline path if publishing the checkpoint fails.
            let _ =
                std::fs::remove_file(crate::session_transcript_artifact_path(&margins_dir, name));
        }
        append_capture_finish_trace(
            work_dir,
            name,
            serde_json::json!({
                "kind":"terminal_live_transcript_checkpoint",
                "published":published,
                "error":publication.err(),
            }),
        );
    }

    // Write capture context sidecar (best-effort)
    let _ = crate::write_capture_context_sidecar(&margins_dir, name, &notes_path);

    // Validate that process_session will accept this session
    let meta =
        retain_on_error!(session::get_session_meta(&margins_dir, name).map_err(|e| e.to_string()));
    retain_on_error!(crate::verify_capture_ready_for_processing(
        work_dir,
        &margins_dir,
        &meta,
    ));

    retain_on_error!(session::mark_session_ended(&margins_dir, name));

    append_capture_finish_trace(
        work_dir,
        name,
        serde_json::json!({
            "kind":"backend_stop_completed",
            "elapsed_ms":stop_started.elapsed().as_millis(),
        }),
    );

    // The session is now confirmed process-ready. Clear the recovery marker
    // first; a crash after this point can leave only a redundant source WebM,
    // never a recovery manifest whose source was already deleted.
    if let Err(error) = remove_if_present(&recovery_manifest_path(&ws)) {
        let message = retain_failed_web_finalization(
            state,
            recording_id,
            ws,
            &format!(
                "final output is ready, but the recovery marker could not be cleared: {error}"
            ),
        );
        return Err(message);
    }
    let _ = std::fs::remove_file(&ws.webm_path);

    Ok(name.clone())
}

fn take_web_recording_for_finalization(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
) -> Result<WebRecordingState, String> {
    {
        let mut active = state.web_sessions.lock().unwrap();
        if let Some(current) = active.get_mut(recording_id) {
            ensure_owner(current, owner_id)?;
            if current.transport.webm_bytes.load(Ordering::Acquire) == 0 {
                return Err(
                    "No durable browser audio has reached the server. This partial capture cannot be finished; explicitly discard it before starting another recording."
                        .to_string(),
                );
            }
            // The recovery marker must never promise bytes that have only
            // reached the process page cache. Sync the WebM and its directory
            // entry before publishing the first-stop manifest.
            sync_webm_before_recovery_manifest(current)?;
            let interrupted = "Hosted finalization was interrupted before completion. Retry Finish or explicitly discard this capture.";
            let previous_error = current.finalization_error.replace(interrupted.to_string());
            let previous_phase = current.recovery_phase.replace(WebRecoveryPhase::Finalizing);
            if let Err(error) =
                persist_recovery_manifest(current, WebRecoveryPhase::Finalizing, interrupted)
            {
                current.finalization_error = previous_error;
                current.recovery_phase = previous_phase;
                return Err(error);
            }
            let mut ws = active
                .remove(recording_id)
                .expect("session existed while singleton map remained locked");
            ws.file.take();
            return Ok(ws);
        }
    }

    let mut failed = state.failed_web_sessions.lock().unwrap();
    let current = failed
        .get_mut(recording_id)
        .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))?;
    ensure_owner(current, owner_id)?;
    if current.recovery_phase == Some(WebRecoveryPhase::CleanupPending) {
        return Err(
            "Hosted capture cleanup is pending; retry Discard instead of Finish".to_string(),
        );
    }
    let interrupted = "Hosted finalization was interrupted before completion. Retry Finish or explicitly discard this capture.";
    let previous_error = current.finalization_error.replace(interrupted.to_string());
    let previous_phase = current.recovery_phase.replace(WebRecoveryPhase::Finalizing);
    if let Err(error) =
        persist_recovery_manifest(current, WebRecoveryPhase::Finalizing, interrupted)
    {
        current.finalization_error = previous_error;
        current.recovery_phase = previous_phase;
        return Err(error);
    }
    let mut ws = failed
        .remove(recording_id)
        .expect("failed session existed while recovery map remained locked");
    ws.file.take();
    Ok(ws)
}

fn retain_failed_web_finalization(
    state: &Arc<AppState>,
    recording_id: &str,
    mut ws: WebRecordingState,
    error: &str,
) -> String {
    let message = format!(
        "Durable browser audio could not be finalized: {error}. The partial WebM is preserved; retry Finish or explicitly discard this capture."
    );
    ws.file.take();
    ws.finalization_error = Some(message.clone());
    ws.recovery_phase = Some(WebRecoveryPhase::Failed);
    ws.owner_last_heartbeat = Instant::now();
    ws.owner_last_heartbeat_unix_ms = unix_time_ms();
    let persistence_error =
        persist_recovery_manifest(&ws, WebRecoveryPhase::Failed, &message).err();
    state
        .failed_web_sessions
        .lock()
        .unwrap()
        .insert(recording_id.to_string(), ws);
    if let Some(error) = persistence_error {
        return format!(
            "{message} Recovery metadata could not be persisted across restart: {error}"
        );
    }
    message
}

fn terminal_web_live_transcript_is_reusable(
    snapshot: &crate::web_live_asr::WebLiveAsrTerminalSnapshot,
    journal_written: bool,
) -> bool {
    let context = &snapshot.context;
    journal_written
        && context.transcript_source == "web_parakeet_onnx"
        && !context.transcript.trim().is_empty()
        && context.hypothesis_transcript.trim().is_empty()
        && context
            .mic_accepted_samples
            .max(context.system_accepted_samples)
            > 0
        && channel_reuse_qualified(
            context.mic_accepted_samples,
            context.mic_dropped_samples,
            snapshot.mic_decoded_samples,
        )
        && channel_reuse_qualified(
            context.system_accepted_samples,
            context.system_dropped_samples,
            snapshot.system_decoded_samples,
        )
        && context.committed_until_ms >= context.decoded_until_ms
}

fn channel_reuse_qualified(
    accepted_samples: u64,
    dropped_samples: u64,
    decoded_samples: u64,
) -> bool {
    dropped_samples == 0 && decoded_samples >= accepted_samples
}

fn append_capture_finish_trace(
    work_dir: &std::path::Path,
    session_name: &str,
    mut event: serde_json::Value,
) {
    let Some(object) = event.as_object_mut() else {
        return;
    };
    object.entry("unix_ms").or_insert_with(|| {
        serde_json::Value::from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        )
    });
    let margins_dir = work_dir.join(".margins");
    let _ = std::fs::create_dir_all(&margins_dir);
    let path = margins_dir.join(format!("{session_name}_capture_finish_trace.jsonl"));
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{event}");
    }
}

// ---------------------------------------------------------------------------
// Discard
// ---------------------------------------------------------------------------

/// Discard a web recording: close and delete the webm and all partial
/// artifacts, remove the session from the index.
pub fn discard_web_recording(
    state: &Arc<AppState>,
    recording_id: &str,
    owner_id: &str,
) -> Result<String, String> {
    let mut ws = {
        let active_match = {
            let mut active = state.web_sessions.lock().unwrap();
            if let Some(current) = active.get(recording_id) {
                ensure_owner(current, owner_id)?;
                active.remove(recording_id)
            } else {
                None
            }
        };
        match active_match {
            Some(ws) => ws,
            None => {
                let mut failed = state.failed_web_sessions.lock().unwrap();
                let current = failed
                    .get(recording_id)
                    .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))?;
                ensure_owner(current, owner_id)?;
                failed
                    .remove(recording_id)
                    .expect("failed session existed while recovery map remained locked")
            }
        }
    };
    ws.file.take();

    if let Some(live_asr) = ws.live_asr.take() {
        let end_ms = ws.started_at.elapsed().unwrap_or_default().as_millis() as u64;
        let _ = live_asr.finish(end_ms);
    }

    let margins_dir = ws.work_dir.join(".margins");
    let name = ws.session_name.clone();

    ws.finalization_error = Some(
        "Hosted capture cleanup is pending; retry Discard to remove its private artifacts."
            .to_string(),
    );
    ws.recovery_phase = Some(WebRecoveryPhase::CleanupPending);
    // Publish cleanup authority before touching any artifact. A partial delete
    // can then be retried in-process or reconstructed after restart.
    let cleanup_message = ws
        .finalization_error
        .as_deref()
        .expect("cleanup state always carries a recovery message");
    let persistence_result =
        persist_recovery_manifest(&ws, WebRecoveryPhase::CleanupPending, cleanup_message);
    state
        .failed_web_sessions
        .lock()
        .unwrap()
        .insert(recording_id.to_string(), ws);
    persistence_result?;

    let mut failed = state.failed_web_sessions.lock().unwrap();
    let retained = failed
        .get(recording_id)
        .expect("cleanup authority was installed before artifact deletion");

    remove_if_present(&retained.webm_path)?;

    // Delete any partial WAV
    let wav_path = margins_dir
        .join("recordings")
        .join(format!("{name}_seg0.wav"));
    remove_if_present(&wav_path)?;

    session::delete_session(&margins_dir, &name)
        .map_err(|error| format!("Failed to delete hosted session index/artifacts: {error}"))?;

    remove_if_present(&recovery_manifest_path(retained))?;
    let name = retained.session_name.clone();
    failed.remove(recording_id);

    Ok(name)
}

// ---------------------------------------------------------------------------
// Get status
// ---------------------------------------------------------------------------

/// Return elapsed seconds for the given active web session, or None.
pub fn web_recording_elapsed_secs(state: &Arc<AppState>, recording_id: &str) -> Option<f64> {
    {
        let active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get(recording_id) {
            return Some(ws.started_at.elapsed().unwrap_or_default().as_secs_f64());
        }
    }
    state
        .failed_web_sessions
        .lock()
        .unwrap()
        .get(recording_id)
        .map(|ws| ws.started_at.elapsed().unwrap_or_default().as_secs_f64())
}

pub fn web_recording_status_by_id(
    state: &Arc<AppState>,
    recording_id: &str,
) -> Option<crate::recording::RecordingStatus> {
    {
        let active = state.web_sessions.lock().unwrap();
        if let Some(ws) = active.get(recording_id) {
            let mut status = web_recording_status(ws);
            status.web_recoveries = web_recording_recoveries(state);
            return Some(status);
        }
    }
    let mut status = {
        let failed = state.failed_web_sessions.lock().unwrap();
        failed.get(recording_id).map(web_recording_status)
    }?;
    status.web_recoveries = web_recording_recoveries(state);
    Some(status)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

struct TimedBranch<T> {
    result: Result<T, String>,
    elapsed_ms: u128,
    completed_unix_ms: u64,
}

struct JoinedBranches<L, R> {
    left: TimedBranch<L>,
    right: TimedBranch<R>,
    join_elapsed_ms: u128,
}

fn run_joined_branches<L, R, LeftJob, RightJob>(
    left_job: LeftJob,
    right_job: RightJob,
) -> JoinedBranches<L, R>
where
    L: Send,
    R: Send,
    LeftJob: FnOnce() -> Result<L, String> + Send,
    RightJob: FnOnce() -> Result<R, String> + Send,
{
    let join_started = Instant::now();
    std::thread::scope(|scope| {
        let left_handle = scope.spawn(move || time_branch(left_job));
        let right_handle = scope.spawn(move || time_branch(right_job));

        // Join both handles before interpreting either result. In particular,
        // an early transcode error must not detach terminal ASR work.
        let left_joined = left_handle.join();
        let right_joined = right_handle.join();
        let left = left_joined.unwrap_or_else(|_| TimedBranch {
            result: Err("live ASR finalization branch panicked".to_string()),
            elapsed_ms: join_started.elapsed().as_millis(),
            completed_unix_ms: unix_ms_now(),
        });
        let right = right_joined.unwrap_or_else(|_| TimedBranch {
            result: Err("durable audio transcode branch panicked".to_string()),
            elapsed_ms: join_started.elapsed().as_millis(),
            completed_unix_ms: unix_ms_now(),
        });
        JoinedBranches {
            left,
            right,
            join_elapsed_ms: join_started.elapsed().as_millis(),
        }
    })
}

fn time_branch<T, Job>(job: Job) -> TimedBranch<T>
where
    Job: FnOnce() -> Result<T, String>,
{
    let started = Instant::now();
    let result = job();
    TimedBranch {
        result,
        elapsed_ms: started.elapsed().as_millis(),
        completed_unix_ms: unix_ms_now(),
    }
}

fn unix_ms_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Finalize a hosted WebM/Opus file to mono 16 kHz WAV.
#[allow(dead_code)]
fn finalize_webm_to_wav(webm: &std::path::Path, wav: &std::path::Path) -> Result<(), String> {
    match crate::webm_opus::HostedWebmFinalizer::from_env() {
        crate::webm_opus::HostedWebmFinalizer::Native => {
            crate::webm_opus::finalize_webm_opus_to_wav(webm, wav)
        }
        crate::webm_opus::HostedWebmFinalizer::FfmpegCompatibility => {
            transcode_webm_to_wav_with_ffmpeg(webm, wav)
        }
    }
}

/// Compatibility fallback: transcode a webm/opus file to mono 16 kHz WAV using
/// ffmpeg. This path is opt-in via MARGINS_HOSTED_WEBM_FINALIZER=ffmpeg.
#[allow(dead_code)]
fn transcode_webm_to_wav_with_ffmpeg(
    webm: &std::path::Path,
    wav: &std::path::Path,
) -> Result<(), String> {
    let ffmpeg = crate::ffmpeg::resolve_binary()?;

    if let Some(parent) = wav.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }

    let output = std::process::Command::new(&ffmpeg)
        .args([
            "-y",
            "-i",
            webm.to_str().ok_or("webm path is not valid UTF-8")?,
            "-ar",
            "16000",
            "-ac",
            "1",
            "-f",
            "wav",
            wav.to_str().ok_or("wav path is not valid UTF-8")?,
        ])
        .output()
        .map_err(|e| format!("Failed to launch ffmpeg: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ffmpeg transcoding failed (exit {:?}): {}",
            output.status.code(),
            stderr.trim()
        ));
    }
    Ok(())
}

/// Probe WAV duration by reading the header.  Falls back to 0.0 on error.
#[allow(dead_code)]
fn probe_wav_duration(wav: &std::path::Path) -> Result<f64, String> {
    let info = margins::audio_info::probe(wav).map_err(|e| e.to_string())?;
    Ok(info.duration_secs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_app_state, settings::load_settings};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;

    fn make_test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "margins-web-session-{}-{}",
            tag,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    fn make_state(work_dir: PathBuf) -> Arc<AppState> {
        let settings = load_settings();
        build_app_state(work_dir, settings)
    }

    fn insert_test_recording(
        state: &Arc<AppState>,
        work_dir: &std::path::Path,
        session_name: &str,
        owner_id: &str,
        memo_lines: Vec<MemoLine>,
    ) {
        let margins_dir = work_dir.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        session::create_session(
            &margins_dir,
            session_name,
            &chrono::Local::now(),
            &format!(".margins/{session_name}.md"),
        )
        .unwrap();
        crate::persist_live_memo(work_dir, session_name, &memo_lines).unwrap();
        let recordings_dir = margins_dir.join("recordings");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let webm_path = recordings_dir.join(format!("{session_name}_upload.webm"));
        let file = File::create(&webm_path).unwrap();
        state.web_sessions.lock().unwrap().insert(
            session_name.to_string(),
            WebRecordingState {
                recording_id: session_name.to_string(),
                session_name: session_name.to_string(),
                work_dir: work_dir.to_path_buf(),
                webm_path,
                file: Some(file),
                started_at: SystemTime::now(),
                memo_lines,
                live_asr: None,
                paused: false,
                owner_id: owner_id.to_string(),
                owner_last_heartbeat: Instant::now(),
                owner_last_heartbeat_unix_ms: unix_time_ms(),
                memo_hydrated_owner: None,
                finalization_error: None,
                recovery_phase: None,
                next_webm_sequence: 0,
                webm_receipts: BTreeMap::new(),
                last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                transport: WebCaptureTransportTelemetry::default(),
            },
        );
    }

    fn persist_test_recovery(
        state: &Arc<AppState>,
        work_dir: &std::path::Path,
        session_name: &str,
        recording_id: &str,
    ) -> (PathBuf, PathBuf) {
        insert_test_recording(state, work_dir, session_name, "owner", Vec::new());
        handle_audio_chunk(state, session_name, "owner", 0, b"durable recovery bytes").unwrap();
        let mut ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove(session_name)
            .unwrap();
        ws.recording_id = recording_id.to_string();
        let manifest_path = recovery_manifest_path(&ws);
        let webm_path = ws.webm_path.clone();
        retain_failed_web_finalization(state, recording_id, ws, "test failure");
        (manifest_path, webm_path)
    }

    #[test]
    fn chunk_append_accumulates_bytes() {
        let work_dir = make_test_dir("chunk-append");
        let state = make_state(work_dir.clone());

        // Manually insert a WebRecordingState
        let recordings_dir = work_dir.join(".margins").join("recordings");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let webm_path = recordings_dir.join("test_upload.webm");
        let file = File::create(&webm_path).unwrap();

        {
            let mut map = state.web_sessions.lock().unwrap();
            map.insert(
                "test".to_string(),
                WebRecordingState {
                    recording_id: "test".to_string(),
                    session_name: "test".to_string(),
                    work_dir: work_dir.clone(),
                    webm_path: webm_path.clone(),
                    file: Some(file),
                    started_at: SystemTime::now(),
                    memo_lines: Vec::new(),
                    live_asr: None,
                    paused: false,
                    owner_id: "owner-test".to_string(),
                    owner_last_heartbeat: Instant::now(),
                    owner_last_heartbeat_unix_ms: unix_time_ms(),
                    memo_hydrated_owner: None,
                    finalization_error: None,
                    recovery_phase: None,
                    next_webm_sequence: 0,
                    webm_receipts: BTreeMap::new(),
                    last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                    next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                    live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                    transport: WebCaptureTransportTelemetry::default(),
                },
            );
        }

        // Append two chunks
        handle_audio_chunk(&state, "test", "owner-test", 0, b"hello").unwrap();
        handle_audio_chunk(&state, "test", "owner-test", 1, b" world").unwrap();
        assert!(handle_audio_chunk(&state, "test", "owner-test", 2, b"").is_err());
        assert!(handle_audio_chunk(&state, "test", "another-owner", 2, b"bad").is_err());
        // Exact retry is idempotent; changed content and future sequences are rejected.
        handle_audio_chunk(&state, "test", "owner-test", 0, b"hello").unwrap();
        assert!(handle_audio_chunk(&state, "test", "owner-test", 0, b"other").is_err());
        assert!(handle_audio_chunk(&state, "test", "owner-test", 3, b"future").is_err());

        {
            let map = state.web_sessions.lock().unwrap();
            let status = web_recording_status(map.get("test").unwrap());
            assert_eq!(status.webm_chunk_count, Some(2));
            assert_eq!(status.webm_bytes, Some(11));
            assert!(status.webm_last_received_unix_ms.is_some());
            assert_eq!(status.mic_level, None);
        }

        // Close the file by taking it out
        {
            let mut map = state.web_sessions.lock().unwrap();
            if let Some(ws) = map.get_mut("test") {
                ws.file.take();
            }
        }

        // Verify contents
        let mut buf = Vec::new();
        File::open(&webm_path)
            .unwrap()
            .read_to_end(&mut buf)
            .unwrap();
        assert_eq!(buf, b"hello world");

        // Cleanup
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn stalled_asr_does_not_serialize_durable_audio_status_or_memo() {
        let work_dir = make_test_dir("stalled-asr-isolation");
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "isolated", "owner", Vec::new());
        hydrate_web_recording_memo(&state, "isolated", Some("owner")).unwrap();

        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let worker = crate::web_live_asr::start_stalled_test_worker(
            work_dir.join(".margins"),
            "isolated".to_string(),
            entered_tx,
            release_rx,
        )
        .unwrap();
        state
            .web_sessions
            .lock()
            .unwrap()
            .get_mut("isolated")
            .unwrap()
            .live_asr = Some(worker);
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let barrier = Arc::new(Barrier::new(4));
        let (done_tx, done_rx) = mpsc::channel();
        let audio_state = state.clone();
        let audio_barrier = barrier.clone();
        let audio_done = done_tx.clone();
        let audio = std::thread::spawn(move || {
            audio_barrier.wait();
            let result = handle_audio_chunk(&audio_state, "isolated", "owner", 0, b"durable");
            let _ = audio_done.send(("audio", result.is_ok()));
        });
        let status_state = state.clone();
        let status_barrier = barrier.clone();
        let status_done = done_tx.clone();
        let status = std::thread::spawn(move || {
            status_barrier.wait();
            let result = active_web_recording_status(&status_state).is_some();
            let _ = status_done.send(("status", result));
        });
        let memo_state = state.clone();
        let memo_barrier = barrier.clone();
        let memo = std::thread::spawn(move || {
            memo_barrier.wait();
            let result = sync_web_recording_memo(
                &memo_state,
                "isolated",
                "owner",
                vec![MemoLine {
                    text: "memo while ASR is stalled".to_string(),
                    created_secs: 1.0,
                    edited_secs: None,
                    draft_started_secs: None,
                    audio_pending_at_mark: false,
                    block_ordinal: None,
                }],
            );
            let _ = done_tx.send(("memo", result.is_ok()));
        });
        barrier.wait();

        let mut completed = std::collections::BTreeSet::new();
        for _ in 0..3 {
            let (name, ok) = done_rx.recv_timeout(Duration::from_secs(1)).expect(
                "audio, status, and memo must complete before the stalled ASR loader is released",
            );
            assert!(ok, "{name} failed while ASR was stalled");
            completed.insert(name);
        }
        assert_eq!(completed, ["audio", "memo", "status"].into_iter().collect());
        release_tx.send(()).unwrap();
        audio.join().unwrap();
        status.join().unwrap();
        memo.join().unwrap();
        assert_eq!(
            std::fs::read(
                state.web_sessions.lock().unwrap()["isolated"]
                    .webm_path
                    .clone()
            )
            .unwrap(),
            b"durable"
        );
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn chunk_append_fails_for_unknown_session() {
        let work_dir = make_test_dir("unknown-session");
        let state = make_state(work_dir.clone());

        let err = handle_audio_chunk(&state, "nonexistent", "owner-test", 0, b"data").unwrap_err();
        assert!(err.contains("No active web recording"), "{err}");

        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn chunk_append_fails_after_file_closed() {
        let work_dir = make_test_dir("closed-file");
        let state = make_state(work_dir.clone());

        let recordings_dir = work_dir.join(".margins").join("recordings");
        std::fs::create_dir_all(&recordings_dir).unwrap();
        let webm_path = recordings_dir.join("closed_upload.webm");
        File::create(&webm_path).unwrap();

        {
            let mut map = state.web_sessions.lock().unwrap();
            map.insert(
                "closed".to_string(),
                WebRecordingState {
                    recording_id: "closed".to_string(),
                    session_name: "closed".to_string(),
                    work_dir: work_dir.clone(),
                    webm_path: webm_path.clone(),
                    file: None, // already closed
                    started_at: SystemTime::now(),
                    memo_lines: Vec::new(),
                    live_asr: None,
                    paused: false,
                    owner_id: "owner-closed".to_string(),
                    owner_last_heartbeat: Instant::now(),
                    owner_last_heartbeat_unix_ms: unix_time_ms(),
                    memo_hydrated_owner: None,
                    finalization_error: None,
                    recovery_phase: None,
                    next_webm_sequence: 0,
                    webm_receipts: BTreeMap::new(),
                    last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                    next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                    live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                    transport: WebCaptureTransportTelemetry::default(),
                },
            );
        }

        let err = handle_audio_chunk(&state, "closed", "owner-closed", 0, b"data").unwrap_err();
        assert!(err.contains("already closed"), "{err}");

        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn discarding_named_session_preserves_a_newer_active_session() {
        let work_dir = make_test_dir("scoped-discard");
        let state = make_state(work_dir.clone());
        let recordings_dir = work_dir.join(".margins").join("recordings");
        std::fs::create_dir_all(&recordings_dir).unwrap();

        for name in ["delayed-a", "active-b"] {
            let webm_path = recordings_dir.join(format!("{name}_upload.webm"));
            let file = File::create(&webm_path).unwrap();
            state.web_sessions.lock().unwrap().insert(
                name.to_string(),
                WebRecordingState {
                    recording_id: name.to_string(),
                    session_name: name.to_string(),
                    work_dir: work_dir.clone(),
                    webm_path,
                    file: Some(file),
                    started_at: SystemTime::now(),
                    memo_lines: Vec::new(),
                    live_asr: None,
                    paused: false,
                    owner_id: format!("owner-{name}"),
                    owner_last_heartbeat: Instant::now(),
                    owner_last_heartbeat_unix_ms: unix_time_ms(),
                    memo_hydrated_owner: None,
                    finalization_error: None,
                    recovery_phase: None,
                    next_webm_sequence: 0,
                    webm_receipts: BTreeMap::new(),
                    last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                    next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                    live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                    transport: WebCaptureTransportTelemetry::default(),
                },
            );
        }

        assert_eq!(
            discard_web_recording(&state, "delayed-a", "owner-delayed-a").unwrap(),
            "delayed-a"
        );
        let sessions = state.web_sessions.lock().unwrap();
        assert!(!sessions.contains_key("delayed-a"));
        assert!(sessions.contains_key("active-b"));
        drop(sessions);

        let _ = discard_web_recording(&state, "active-b", "owner-active-b");
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn elapsed_secs_is_non_negative() {
        let work_dir = make_test_dir("elapsed");
        let state = make_state(work_dir.clone());

        {
            let mut map = state.web_sessions.lock().unwrap();
            map.insert(
                "elapsed-test".to_string(),
                WebRecordingState {
                    recording_id: "elapsed-test".to_string(),
                    session_name: "elapsed-test".to_string(),
                    work_dir: work_dir.clone(),
                    webm_path: PathBuf::from("/dev/null"),
                    file: None,
                    started_at: SystemTime::now(),
                    memo_lines: Vec::new(),
                    live_asr: None,
                    paused: false,
                    owner_id: "owner-elapsed".to_string(),
                    owner_last_heartbeat: Instant::now(),
                    owner_last_heartbeat_unix_ms: unix_time_ms(),
                    memo_hydrated_owner: None,
                    finalization_error: None,
                    recovery_phase: None,
                    next_webm_sequence: 0,
                    webm_receipts: BTreeMap::new(),
                    last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
                    next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
                    live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
                    transport: WebCaptureTransportTelemetry::default(),
                },
            );
        }

        let elapsed = web_recording_elapsed_secs(&state, "elapsed-test").unwrap();
        assert!(elapsed >= 0.0);
        assert!(web_recording_elapsed_secs(&state, "missing").is_none());

        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn hosted_status_keeps_unobserved_microphone_level_unknown() {
        let ws = WebRecordingState {
            recording_id: "meter-contract".to_string(),
            session_name: "meter-contract".to_string(),
            work_dir: PathBuf::from("/tmp"),
            webm_path: PathBuf::from("/tmp/meter-contract.webm"),
            file: None,
            started_at: SystemTime::now(),
            memo_lines: Vec::new(),
            live_asr: None,
            paused: false,
            owner_id: "owner-meter".to_string(),
            owner_last_heartbeat: Instant::now(),
            owner_last_heartbeat_unix_ms: unix_time_ms(),
            memo_hydrated_owner: None,
            finalization_error: None,
            recovery_phase: None,
            next_webm_sequence: 0,
            webm_receipts: BTreeMap::new(),
            last_live_checkpoint_ms: Arc::new(AtomicU64::new(0)),
            next_live_checkpoint_attempt_ms: Arc::new(AtomicU64::new(0)),
            live_checkpoint_in_flight: Arc::new(AtomicBool::new(false)),
            transport: WebCaptureTransportTelemetry::default(),
        };

        let status = web_recording_status(&ws);
        assert_eq!(status.mic_level, None);
        assert_eq!(status.mic_audio_frame_count, 0);
        assert_eq!(
            serde_json::to_value(status).unwrap()["mic_level"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn recovery_claim_waits_for_lease_and_transport_then_scopes_cleanup() {
        let work_dir = make_test_dir("owner-recovery");
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "owned", "old-owner", Vec::new());
        {
            let mut sessions = state.web_sessions.lock().unwrap();
            let ws = sessions.get_mut("owned").unwrap();
            ws.owner_last_heartbeat = Instant::now() - std::time::Duration::from_secs(120);
            ws.transport.record_webm_chunk(32);
        }

        let fresh_error =
            claim_web_recording_recovery(&state, "owned", "recovery-owner".to_string())
                .unwrap_err();
        assert!(
            fresh_error.contains("fresh audio transport"),
            "{fresh_error}"
        );

        {
            let sessions = state.web_sessions.lock().unwrap();
            sessions
                .get("owned")
                .unwrap()
                .transport
                .webm_last_received_monotonic_ms
                .store(
                    monotonic_time_ms().saturating_sub(WEB_TRANSPORT_OWNER_EVIDENCE_MS + 1),
                    Ordering::Release,
                );
        }
        claim_web_recording_recovery(&state, "owned", "recovery-owner".to_string()).unwrap();
        // Repeating an accepted capability is safe when the first response was lost.
        claim_web_recording_recovery(&state, "owned", "recovery-owner".to_string()).unwrap();
        assert!(discard_web_recording(&state, "owned", "old-owner").is_err());
        assert_eq!(
            discard_web_recording(&state, "owned", "recovery-owner").unwrap(),
            "owned"
        );
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn memo_must_be_hydrated_before_an_empty_owner_sync_can_replace_it() {
        let work_dir = make_test_dir("memo-hydration");
        let state = make_state(work_dir.clone());
        let durable_line = MemoLine {
            text: "committed before reload".to_string(),
            created_secs: 3.0,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: None,
        };
        insert_test_recording(
            &state,
            &work_dir,
            "memo-owned",
            "memo-owner",
            vec![durable_line.clone()],
        );

        let error =
            sync_web_recording_memo(&state, "memo-owned", "memo-owner", Vec::new()).unwrap_err();
        assert!(error.contains("must be hydrated"), "{error}");
        assert_eq!(
            state.web_sessions.lock().unwrap()["memo-owned"].memo_lines[0].text,
            durable_line.text
        );

        let hydrated =
            hydrate_web_recording_memo(&state, "memo-owned", Some("memo-owner")).unwrap();
        assert_eq!(hydrated.len(), 1);
        assert_eq!(hydrated[0].text, "committed before reload");
        sync_web_recording_memo(&state, "memo-owned", "memo-owner", Vec::new()).unwrap();
        assert!(state.web_sessions.lock().unwrap()["memo-owned"]
            .memo_lines
            .is_empty());

        discard_web_recording(&state, "memo-owned", "memo-owner").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn whole_notepad_updates_reuse_the_shared_timed_memo_model() {
        let work_dir = make_test_dir("whole-notepad");
        let state = make_state(work_dir.clone());
        let original = MemoLine {
            text: "Keep this anchor".to_string(),
            created_secs: 3.0,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: None,
        };
        insert_test_recording(
            &state,
            &work_dir,
            "notepad-owned",
            "notepad-owner",
            vec![original.clone()],
        );
        let before = get_web_recording_notepad(&state, "notepad-owned", "notepad-owner").unwrap();
        let after = update_web_recording_notepad(
            &state,
            "notepad-owned",
            "notepad-owner",
            &before.revision,
            "Keep this anchor\nA new thought",
        )
        .unwrap();
        assert_eq!(after.text, "Keep this anchor\nA new thought");
        let active = state.web_sessions.lock().unwrap();
        assert_eq!(active["notepad-owned"].memo_lines[0], original);
        assert!(active["notepad-owned"].memo_lines[1].created_secs >= 0.0);
        drop(active);
        assert!(update_web_recording_notepad(
            &state,
            "notepad-owned",
            "notepad-owner",
            &before.revision,
            "stale",
        )
        .unwrap_err()
        .contains("changed somewhere else"));
        discard_web_recording(&state, "notepad-owned", "notepad-owner").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn finishing_before_first_webm_is_honest_and_explicitly_discardable() {
        let work_dir = make_test_dir("empty-finish");
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "empty", "empty-owner", Vec::new());

        let error = stop_web_recording(&state, "empty", "empty-owner").unwrap_err();
        assert!(error.contains("No durable browser audio"), "{error}");
        assert!(state.web_sessions.lock().unwrap().contains_key("empty"));
        discard_web_recording(&state, "empty", "empty-owner").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn failed_transcode_state_is_retained_for_retry_or_explicit_discard() {
        let work_dir = make_test_dir("failed-transcode-retained");
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "bad-webm", "recovery-owner", Vec::new());
        let ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove("bad-webm")
            .unwrap();
        let message = retain_failed_web_finalization(
            &state,
            "bad-webm",
            ws,
            "deterministic transcode failure",
        );
        assert!(message.contains("partial WebM is preserved"), "{message}");
        assert!(state.failed_web_sessions.lock().unwrap()["bad-webm"]
            .finalization_error
            .as_deref()
            .unwrap()
            .contains("deterministic transcode failure"));
        state
            .failed_web_sessions
            .lock()
            .unwrap()
            .get_mut("bad-webm")
            .unwrap()
            .owner_last_heartbeat = Instant::now() - std::time::Duration::from_secs(120);
        claim_web_recording_recovery(&state, "bad-webm", "claimed-owner".to_string()).unwrap();
        assert!(discard_web_recording(&state, "bad-webm", "recovery-owner").is_err());
        hydrate_web_recording_memo(&state, "bad-webm", Some("claimed-owner")).unwrap();
        discard_web_recording(&state, "bad-webm", "claimed-owner").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn failed_a_never_reenters_singleton_when_b_starts_during_finalization() {
        for (case, failure) in [
            ("transcode", "deterministic transcode failure"),
            (
                "readiness",
                "deterministic post-transcode readiness failure",
            ),
        ] {
            let work_dir = make_test_dir(&format!("finalize-race-{case}"));
            let state = make_state(work_dir.clone());
            insert_test_recording(&state, &work_dir, "session-a", "owner-a", Vec::new());
            handle_audio_chunk(&state, "session-a", "owner-a", 0, b"durable-audio").unwrap();

            let state_during_finalize = Arc::clone(&state);
            let work_dir_during_finalize = work_dir.clone();
            let transcode_fails = case == "transcode";
            let error = stop_web_recording_with(
                &state,
                "session-a",
                "owner-a",
                move |_webm, _wav| {
                    start_web_recording(
                        &state_during_finalize,
                        work_dir_during_finalize,
                        "session-b".to_string(),
                        "owner-b".to_string(),
                    )
                    .expect("B starts while A finalizes");
                    if transcode_fails {
                        Err("deterministic transcode failure".to_string())
                    } else {
                        Ok(())
                    }
                },
                (case == "readiness").then(|| failure.to_string()),
            )
            .unwrap_err();
            assert!(error.contains(failure), "{case}: {error}");

            let active = state.web_sessions.lock().unwrap();
            assert_eq!(active.len(), 1, "{case}: exactly one active singleton");
            assert!(
                active
                    .values()
                    .any(|session| session.session_name == "session-b"),
                "{case}: B remains active"
            );
            assert!(
                !active.contains_key("session-a"),
                "{case}: A never displaces B"
            );
            drop(active);

            let failed = state.failed_web_sessions.lock().unwrap();
            assert_eq!(failed.len(), 1, "{case}: A retained exactly once");
            assert!(failed["session-a"]
                .finalization_error
                .as_deref()
                .unwrap()
                .contains(failure));
            drop(failed);

            assert!(take_web_recording_for_finalization(&state, "session-a", "observer").is_err());
            let retry = take_web_recording_for_finalization(&state, "session-a", "owner-a")
                .expect("A remains scoped and retryable");
            retain_failed_web_finalization(&state, "session-a", retry, "retry still failed");
            assert_eq!(
                discard_web_recording(&state, "session-a", "owner-a").unwrap(),
                "session-a"
            );
            let session_b_id = state
                .web_sessions
                .lock()
                .unwrap()
                .iter()
                .find_map(|(id, session)| (session.session_name == "session-b").then(|| id.clone()))
                .unwrap();
            discard_web_recording(&state, &session_b_id, "owner-b").unwrap();
            let _ = std::fs::remove_dir_all(&work_dir);
        }
    }

    #[test]
    fn live_pcm_telemetry_counts_only_the_addressed_session() {
        let current = WebCaptureTransportTelemetry::default();
        let other = WebCaptureTransportTelemetry::default();

        current.record_live_pcm_batch(8_000);
        current.record_live_pcm_batch(4_000);

        assert_eq!(current.live_pcm_batch_count.load(Ordering::Acquire), 2);
        assert_eq!(
            current.live_pcm_sample_count.load(Ordering::Acquire),
            12_000
        );
        assert!(
            current
                .live_pcm_last_received_unix_ms
                .load(Ordering::Acquire)
                > 0
        );
        assert_eq!(other.live_pcm_batch_count.load(Ordering::Acquire), 0);
        assert_eq!(other.live_pcm_sample_count.load(Ordering::Acquire), 0);
        assert_eq!(
            other.live_pcm_last_received_unix_ms.load(Ordering::Acquire),
            0
        );
    }

    #[test]
    fn hosted_start_rejects_a_second_active_session() {
        let work_dir = make_test_dir("single-owner");
        let state = make_state(work_dir.clone());

        let first = start_web_recording(
            &state,
            work_dir.clone(),
            "first".to_string(),
            "owner-first".to_string(),
        )
        .unwrap();
        assert_eq!(first.session_name, "first");
        assert_eq!(first.recording_id.len(), 32);
        let error = start_web_recording(
            &state,
            work_dir.clone(),
            "second".to_string(),
            "owner-second".to_string(),
        )
        .unwrap_err();
        assert!(error.contains("'first' is already active"), "{error}");
        let sessions = state.web_sessions.lock().unwrap();
        assert_eq!(sessions.len(), 1);
        assert!(sessions.contains_key(&first.recording_id));
        drop(sessions);

        discard_web_recording(&state, &first.recording_id, "owner-first").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn hosted_writer_and_workspace_service_share_session_title_artifact_and_memo_facts() {
        let work_dir = make_test_dir("workspace-authority-parity");
        let state = make_state(work_dir.clone());
        let workspace_home = work_dir.join("workspace-state");
        let workspace = margins_workflows::workspace::ensure_service_workspace(
            &workspace_home,
            "hosted",
            None,
            &work_dir,
            &work_dir,
        )
        .unwrap();
        let service =
            margins_workflows::workspace_service::WorkspaceService::open("test-host", workspace)
                .unwrap();
        let principal =
            margins_workflows::workspace_service::ServicePrincipal::full("browser-test", "hosted");
        let started = start_web_recording(
            &state,
            work_dir.clone(),
            "browser-session".to_string(),
            "browser-owner".to_string(),
        )
        .unwrap();

        let page = service.sessions(&principal, None, 10).unwrap();
        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].session_id.as_ref(), "browser-session");
        margins::session::set_title(
            &work_dir.join(".margins"),
            "browser-session",
            Some("Canonical title".to_string()),
        )
        .unwrap();
        assert_eq!(
            service.sessions(&principal, None, 10).unwrap().sessions[0]
                .title
                .as_deref(),
            Some("Canonical title")
        );
        let artifact_dir = work_dir.join(".margins/artifacts/browser-session");
        std::fs::create_dir_all(&artifact_dir).unwrap();
        std::fs::write(artifact_dir.join("proof.bin"), b"proof").unwrap();
        margins::session::upsert_session_artifact(
            &work_dir.join(".margins"),
            "browser-session",
            "proof",
            0,
            ".margins/artifacts/browser-session/proof.bin",
            "durable",
            None,
        )
        .unwrap();
        assert_eq!(
            service.artifacts(&principal, "browser-session").unwrap()[0].size_bytes,
            Some(5)
        );

        let first =
            get_web_recording_notepad(&state, &started.recording_id, "browser-owner").unwrap();
        update_web_recording_notepad(
            &state,
            &started.recording_id,
            "browser-owner",
            &first.revision,
            "Browser note",
        )
        .unwrap();
        let shared = service
            .memo(
                &principal,
                &margins_meeting_protocol::SessionId("browser-session".to_string()),
            )
            .unwrap();
        assert_eq!(shared.lines[0].text, "Browser note");
        service
            .update_memo(
                &principal,
                &margins_meeting_protocol::SessionId("browser-session".to_string()),
                &margins_meeting_protocol::WorkspaceMemoUpdateV1 {
                    request_id: "service-edit".to_string(),
                    expected_revision: shared.revision,
                    observed_at_ms: margins_meeting_protocol::SessionMillis(2_000),
                    paused: false,
                    text: "Browser note\nService note".to_string(),
                },
            )
            .unwrap();
        let reflected =
            get_web_recording_notepad(&state, &started.recording_id, "browser-owner").unwrap();
        assert_eq!(reflected.text, "Browser note\nService note");

        discard_web_recording(&state, &started.recording_id, "browser-owner").unwrap();
        let _ = std::fs::remove_dir_all(&work_dir);
    }

    #[test]
    fn terminal_web_transcript_requires_stable_complete_pcm_and_written_journal() {
        let mut snapshot = crate::web_live_asr::WebLiveAsrTerminalSnapshot {
            mic_decoded_samples: 160_000,
            system_decoded_samples: 0,
            context: crate::live_backchannel::LiveTranscriptContext {
                transcript: "[00:01] you (mic): stable words".to_string(),
                previous_transcript: String::new(),
                new_transcript: "[00:01] you (mic): stable words".to_string(),
                committed_transcript: "[00:01] you (mic): stable words".to_string(),
                hypothesis_transcript: String::new(),
                transcript_source: "web_parakeet_onnx".to_string(),
                previous_memo_checkpoint_ms: 0,
                decoded_until_ms: 10_000,
                committed_until_ms: 10_000,
                mic_accepted_samples: 160_000,
                system_accepted_samples: 0,
                mic_dropped_samples: 0,
                system_dropped_samples: 0,
                timing: None,
            },
        };

        assert!(terminal_web_live_transcript_is_reusable(&snapshot, true));
        assert!(!terminal_web_live_transcript_is_reusable(&snapshot, false));

        snapshot.context.hypothesis_transcript = "unstable tail".to_string();
        assert!(!terminal_web_live_transcript_is_reusable(&snapshot, true));
        snapshot.context.hypothesis_transcript.clear();

        snapshot.mic_decoded_samples = 159_999;
        assert!(!terminal_web_live_transcript_is_reusable(&snapshot, true));

        // Unequal present channels qualify independently using exact sample
        // watermarks; the shared millisecond display value is not correctness.
        snapshot.context.decoded_until_ms = 10_000;
        snapshot.context.committed_until_ms = 10_000;
        snapshot.context.mic_accepted_samples = 320_000;
        snapshot.context.system_accepted_samples = 160_003;
        snapshot.mic_decoded_samples = 320_000;
        snapshot.system_decoded_samples = 160_003;
        snapshot
            .context
            .transcript
            .push_str("\n[00:10] System: lagging speech");
        assert!(terminal_web_live_transcript_is_reusable(&snapshot, true));
        snapshot.system_decoded_samples = 160_002;
        assert!(!terminal_web_live_transcript_is_reusable(&snapshot, true));

        snapshot.system_decoded_samples = 160_003;
        snapshot.context.system_dropped_samples = 16_000;
        assert!(!terminal_web_live_transcript_is_reusable(&snapshot, true));

        assert!(!channel_reuse_qualified(537_970, 0, 537_969));
        assert!(channel_reuse_qualified(537_970, 0, 537_970));
        assert!(!channel_reuse_qualified(0, 1, 537_970));
    }

    #[test]
    fn same_display_name_in_different_projects_has_independent_recovery_ids() {
        let root = make_test_dir("same-name-projects");
        let project_a = root.join("project-a");
        let project_b = root.join("project-b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        let state = make_state(project_a.clone());
        let id_a = "11111111111111111111111111111111";
        let id_b = "22222222222222222222222222222222";

        insert_test_recording(&state, &project_a, "same", "owner-a", Vec::new());
        let mut a = state.web_sessions.lock().unwrap().remove("same").unwrap();
        a.recording_id = id_a.to_string();
        a.started_at = SystemTime::now() - std::time::Duration::from_secs(20);
        retain_failed_web_finalization(&state, id_a, a, "failure a");

        insert_test_recording(&state, &project_b, "same", "owner-b", Vec::new());
        let mut b = state.web_sessions.lock().unwrap().remove("same").unwrap();
        b.recording_id = id_b.to_string();
        b.started_at = SystemTime::now() - std::time::Duration::from_secs(10);
        retain_failed_web_finalization(&state, id_b, b, "failure b");

        let recoveries = web_recording_recoveries(&state);
        assert_eq!(recoveries.len(), 2);
        assert_eq!(
            recoveries
                .iter()
                .map(|recovery| recovery.recording_id.as_str())
                .collect::<Vec<_>>(),
            vec![id_a, id_b],
            "multiple failures are exposed oldest-first"
        );
        assert!(recoveries
            .iter()
            .any(|recovery| recovery.recording_id == id_a));
        assert!(recoveries
            .iter()
            .any(|recovery| recovery.recording_id == id_b));
        assert!(recoveries
            .iter()
            .all(|recovery| recovery.session_name == "same"));

        discard_web_recording(&state, id_a, "owner-a").unwrap();
        assert!(!state.failed_web_sessions.lock().unwrap().contains_key(id_a));
        assert!(state.failed_web_sessions.lock().unwrap().contains_key(id_b));
        discard_web_recording(&state, id_b, "owner-b").unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_manifest_reconstructs_after_restart_and_requires_a_fresh_claim() {
        let work_dir = make_test_dir("restart-recovery");
        let id = "abababababababababababababababab";
        let state = make_state(work_dir.clone());
        let memo = MemoLine {
            text: "survives restart".to_string(),
            created_secs: 2.0,
            edited_secs: None,
            draft_started_secs: None,
            audio_pending_at_mark: false,
            block_ordinal: None,
        };
        insert_test_recording(&state, &work_dir, "restart", "old-owner", vec![memo]);
        handle_audio_chunk(&state, "restart", "old-owner", 0, b"durable restart audio").unwrap();
        let mut ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove("restart")
            .unwrap();
        ws.recording_id = id.to_string();
        retain_failed_web_finalization(&state, id, ws, "restart failure");
        drop(state);

        let restarted = make_state(work_dir.clone());
        assert!(restarted
            .failed_web_sessions
            .lock()
            .unwrap()
            .contains_key(id));
        assert!(hydrate_web_recording_memo(&restarted, id, Some("old-owner")).is_err());
        claim_web_recording_recovery(&restarted, id, "new-owner".to_string()).unwrap();
        let hydrated = hydrate_web_recording_memo(&restarted, id, Some("new-owner")).unwrap();
        assert_eq!(hydrated[0].text, "survives restart");
        discard_web_recording(&restarted, id, "new-owner").unwrap();
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn first_stop_persists_finalizing_manifest_before_releasing_active_slot() {
        let work_dir = make_test_dir("first-stop-manifest");
        let id = "edededededededededededededededed";
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "first-stop", "owner", Vec::new());
        handle_audio_chunk(&state, "first-stop", "owner", 0, b"durable first stop").unwrap();
        let mut ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove("first-stop")
            .unwrap();
        ws.recording_id = id.to_string();
        state
            .web_sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), ws);

        let finalizing = take_web_recording_for_finalization(&state, id, "owner").unwrap();
        assert!(!state.web_sessions.lock().unwrap().contains_key(id));
        let manifest_path = recovery_manifest_path(&finalizing);
        let manifest: WebRecoveryManifest =
            serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.phase, WebRecoveryPhase::Finalizing);
        assert_eq!(manifest.recording_id, id);
        drop(finalizing);
        drop(state);

        let restarted = make_state(work_dir.clone());
        assert!(restarted
            .failed_web_sessions
            .lock()
            .unwrap()
            .contains_key(id));
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn failed_first_manifest_write_keeps_active_owner_and_file() {
        let work_dir = make_test_dir("first-stop-manifest-failure");
        let id = "efefefefefefefefefefefefefefefef";
        let state = make_state(work_dir.clone());
        insert_test_recording(&state, &work_dir, "persist-fails", "owner", Vec::new());
        handle_audio_chunk(&state, "persist-fails", "owner", 0, b"durable").unwrap();
        let mut ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove("persist-fails")
            .unwrap();
        ws.recording_id = id.to_string();
        let temp_path = recovery_manifest_path(&ws).with_extension("recovery.json.tmp");
        std::fs::create_dir(&temp_path).unwrap();
        state
            .web_sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), ws);

        let error = match take_web_recording_for_finalization(&state, id, "owner") {
            Ok(_) => panic!("manifest persistence failure must not release the active capture"),
            Err(error) => error,
        };
        assert!(error.contains("persist hosted recovery"), "{error}");
        let active = state.web_sessions.lock().unwrap();
        assert!(active.contains_key(id));
        assert!(active[id].file.is_some());
        drop(active);
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn malformed_manifest_identity_and_artifact_counters_are_quarantined() {
        for (case, mutate) in [("filename", 0_u8), ("counter", 1_u8), ("json", 2_u8)] {
            let work_dir = make_test_dir(&format!("quarantine-{case}"));
            let id = "acacacacacacacacacacacacacacacac";
            let state = make_state(work_dir.clone());
            insert_test_recording(&state, &work_dir, "quarantine", "owner", Vec::new());
            handle_audio_chunk(&state, "quarantine", "owner", 0, b"durable").unwrap();
            let mut ws = state
                .web_sessions
                .lock()
                .unwrap()
                .remove("quarantine")
                .unwrap();
            ws.recording_id = id.to_string();
            state
                .web_sessions
                .lock()
                .unwrap()
                .insert(id.to_string(), ws);
            let finalizing = take_web_recording_for_finalization(&state, id, "owner").unwrap();
            let mut manifest_path = recovery_manifest_path(&finalizing);
            drop(finalizing);
            drop(state);

            match mutate {
                0 => {
                    let wrong = manifest_path.with_file_name(format!(
                        "{}{}",
                        "adadadadadadadadadadadadadadadad", RECOVERY_MANIFEST_SUFFIX
                    ));
                    std::fs::rename(&manifest_path, &wrong).unwrap();
                    manifest_path = wrong;
                }
                1 => {
                    let mut manifest: WebRecoveryManifest =
                        serde_json::from_slice(&std::fs::read(&manifest_path).unwrap()).unwrap();
                    manifest.webm_bytes = manifest.webm_bytes.saturating_add(10_000);
                    std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
                }
                _ => std::fs::write(&manifest_path, b"{not json").unwrap(),
            }

            let restarted = make_state(work_dir.clone());
            assert!(restarted.failed_web_sessions.lock().unwrap().is_empty());
            assert!(!manifest_path.exists());
            let quarantined = std::fs::read_dir(work_dir.join(".margins/recordings"))
                .unwrap()
                .flatten()
                .any(|entry| entry.file_name().to_string_lossy().contains(".quarantine"));
            assert!(quarantined, "{case} manifest should be quarantined");
            let _ = std::fs::remove_dir_all(work_dir);
        }
    }

    #[test]
    fn duplicate_recovery_ids_are_deterministically_quarantined() {
        let root = make_test_dir("duplicate-recovery-id");
        let project_a = root.join("a");
        let project_b = root.join("b");
        std::fs::create_dir_all(&project_a).unwrap();
        std::fs::create_dir_all(&project_b).unwrap();
        let id = "bcbcbcbcbcbcbcbcbcbcbcbcbcbcbcbc";

        for (project, session) in [(&project_a, "duplicate-a"), (&project_b, "duplicate-b")] {
            let state = make_state(project.clone());
            insert_test_recording(&state, project, session, "owner", Vec::new());
            handle_audio_chunk(&state, session, "owner", 0, b"durable duplicate").unwrap();
            let mut ws = state.web_sessions.lock().unwrap().remove(session).unwrap();
            ws.recording_id = id.to_string();
            retain_failed_web_finalization(&state, id, ws, "duplicate failure");
        }

        let mut settings = load_settings();
        settings.projects.clear();
        settings.projects.push(crate::settings::ProjectSource {
            id: "b".to_string(),
            name: "B".to_string(),
            path: project_b.to_string_lossy().into_owned(),
            inbox_folder: "meetings".to_string(),
            people_folder: "people".to_string(),
            readiness: "ready".to_string(),
        });
        let recovered = reconstruct_failed_web_sessions(&project_a, &settings);
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[id].session_name, "duplicate-a");
        let duplicate_quarantined = std::fs::read_dir(project_b.join(".margins/recordings"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".quarantine"));
        assert!(duplicate_quarantined);
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn aliased_recovery_workdirs_scan_once_without_quarantining_the_manifest() {
        use std::os::unix::fs::symlink;

        let root = make_test_dir("recovery-workdir-alias");
        let project = root.join("project");
        let alias = root.join("alias");
        std::fs::create_dir_all(&project).unwrap();
        symlink(&project, &alias).unwrap();
        let state = make_state(project.clone());
        let id = "abababababababababababababababab";
        let (manifest_path, _) = persist_test_recovery(&state, &project, "alias", id);

        let mut settings = load_settings();
        settings.projects.clear();
        settings.projects.push(crate::settings::ProjectSource {
            id: "alias".to_string(),
            name: "Alias".to_string(),
            path: alias.to_string_lossy().into_owned(),
            inbox_folder: "meetings".to_string(),
            people_folder: "people".to_string(),
            readiness: "ready".to_string(),
        });
        let recovered = reconstruct_failed_web_sessions(&project, &settings);
        assert_eq!(recovered.len(), 1);
        assert!(recovered.contains_key(id));
        assert!(manifest_path.exists());
        assert!(!std::fs::read_dir(project.join(".margins/recordings"))
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains(".quarantine")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_recovery_manifest_is_rejected_without_following_its_target() {
        use std::os::unix::fs::symlink;

        let root = make_test_dir("symlink-recovery-manifest");
        let state = make_state(root.clone());
        let id = "cacacacacacacacacacacacacacacaca";
        let (manifest_path, _) = persist_test_recovery(&state, &root, "manifest-link", id);
        let external = root.join("external-recovery.json");
        std::fs::rename(&manifest_path, &external).unwrap();
        symlink(&external, &manifest_path).unwrap();

        let mut settings = load_settings();
        settings.projects.clear();
        let recovered = reconstruct_failed_web_sessions(&root, &settings);
        assert!(recovered.is_empty());
        assert!(
            external.exists(),
            "the symlink target must never be moved or read"
        );
        assert!(!manifest_path.exists());
        assert!(std::fs::symlink_metadata(
            manifest_path.with_file_name(format!("{id}{RECOVERY_MANIFEST_SUFFIX}.quarantine"))
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_webm_artifact_is_rejected_and_manifest_is_quarantined() {
        use std::os::unix::fs::symlink;

        let root = make_test_dir("symlink-recovery-webm");
        let state = make_state(root.clone());
        let id = "cbcbcbcbcbcbcbcbcbcbcbcbcbcbcbcb";
        let (manifest_path, webm_path) = persist_test_recovery(&state, &root, "webm-link", id);
        let external = root.join("external.webm");
        std::fs::rename(&webm_path, &external).unwrap();
        symlink(&external, &webm_path).unwrap();

        let mut settings = load_settings();
        settings.projects.clear();
        let recovered = reconstruct_failed_web_sessions(&root, &settings);
        assert!(recovered.is_empty());
        assert!(external.exists());
        assert!(!manifest_path.exists());
        assert!(manifest_path
            .with_file_name(format!("{id}{RECOVERY_MANIFEST_SUFFIX}.quarantine"))
            .exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn discard_failure_retains_scoped_cleanup_authority() {
        let work_dir = make_test_dir("discard-transaction");
        let state = make_state(work_dir.clone());
        let id = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        insert_test_recording(&state, &work_dir, "cleanup", "cleanup-owner", Vec::new());
        let mut ws = state
            .web_sessions
            .lock()
            .unwrap()
            .remove("cleanup")
            .unwrap();
        ws.recording_id = id.to_string();
        ws.file.take();
        std::fs::remove_file(&ws.webm_path).unwrap();
        std::fs::create_dir(&ws.webm_path).unwrap();
        state
            .web_sessions
            .lock()
            .unwrap()
            .insert(id.to_string(), ws);

        let error = discard_web_recording(&state, id, "cleanup-owner").unwrap_err();
        assert!(error.contains("Failed to delete"), "{error}");
        let failed = state.failed_web_sessions.lock().unwrap();
        assert!(failed.contains_key(id));
        assert_eq!(failed[id].owner_id, "cleanup-owner");
        let manifest: WebRecoveryManifest =
            serde_json::from_slice(&std::fs::read(recovery_manifest_path(&failed[id])).unwrap())
                .unwrap();
        assert_eq!(manifest.phase, WebRecoveryPhase::CleanupPending);
        drop(failed);

        // Restart at the cleanup boundary, claim the reconstructed exact ID,
        // then correct the filesystem problem and complete scoped cleanup.
        let restarted = make_state(work_dir.clone());
        assert_eq!(
            restarted.failed_web_sessions.lock().unwrap()[id].recovery_phase,
            Some(WebRecoveryPhase::CleanupPending)
        );
        claim_web_recording_recovery(&restarted, id, "restart-cleanup-owner".to_string()).unwrap();
        let webm_path = restarted.failed_web_sessions.lock().unwrap()[id]
            .webm_path
            .clone();
        std::fs::remove_dir(&webm_path).unwrap();
        std::fs::File::create(&webm_path).unwrap();
        assert_eq!(
            discard_web_recording(&restarted, id, "restart-cleanup-owner").unwrap(),
            "cleanup"
        );
        assert!(!restarted
            .failed_web_sessions
            .lock()
            .unwrap()
            .contains_key(id));
        let _ = std::fs::remove_dir_all(work_dir);
    }

    #[test]
    fn finalization_branches_start_concurrently_and_join() {
        let barrier = Arc::new(Barrier::new(2));
        let left_barrier = barrier.clone();
        let right_barrier = barrier;
        let joined = run_joined_branches(
            move || {
                left_barrier.wait();
                Ok::<_, String>("transcript")
            },
            move || {
                right_barrier.wait();
                Ok::<_, String>("audio")
            },
        );

        assert_eq!(joined.left.result.unwrap(), "transcript");
        assert_eq!(joined.right.result.unwrap(), "audio");
    }

    #[test]
    fn finalization_joins_both_errors_without_short_circuiting() {
        let completed = Arc::new(AtomicUsize::new(0));
        let left_completed = completed.clone();
        let right_completed = completed.clone();
        let joined = run_joined_branches(
            move || {
                left_completed.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>("asr failed".to_string())
            },
            move || {
                right_completed.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>("transcode failed".to_string())
            },
        );

        assert_eq!(completed.load(Ordering::SeqCst), 2);
        assert_eq!(joined.left.result.unwrap_err(), "asr failed");
        assert_eq!(joined.right.result.unwrap_err(), "transcode failed");
    }

    #[test]
    fn native_webm_failure_preserves_recoverable_upload() {
        let _guard = env_lock().lock().unwrap();
        let _env = EnvGuard::set_invalid_ffmpeg();
        let work_dir = make_test_dir("native-webm-failure");
        let state = make_state(work_dir.clone());
        let start = start_web_recording(
            &state,
            work_dir.clone(),
            "broken-native".to_string(),
            "owner".to_string(),
        )
        .unwrap();
        handle_audio_chunk(&state, &start.recording_id, "owner", 0, b"not a webm").unwrap();

        let error = stop_web_recording(&state, &start.recording_id, "owner").unwrap_err();
        assert!(error.contains("partial WebM is preserved"), "{error}");
        assert!(
            error.contains("parse WebM") || error.contains("Opus"),
            "{error}"
        );
        assert!(!std::path::Path::new("/definitely/not/ffmpeg").exists());

        let failed = state.failed_web_sessions.lock().unwrap();
        let retained = failed.get(&start.recording_id).expect("retained recovery");
        assert!(retained.webm_path.exists());
        assert_eq!(retained.transport.webm_bytes.load(Ordering::Acquire), 10);
        assert!(recovery_manifest_path(retained).exists());
        let _ = std::fs::remove_dir_all(work_dir);
    }

    fn env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    struct EnvGuard {
        ffmpeg_bin: Option<std::ffi::OsString>,
        finalizer: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set_invalid_ffmpeg() -> Self {
            let guard = Self {
                ffmpeg_bin: std::env::var_os("FFMPEG_BIN"),
                finalizer: std::env::var_os("MARGINS_HOSTED_WEBM_FINALIZER"),
            };
            std::env::set_var("FFMPEG_BIN", "/definitely/not/ffmpeg");
            std::env::remove_var("MARGINS_HOSTED_WEBM_FINALIZER");
            guard
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.ffmpeg_bin {
                Some(value) => std::env::set_var("FFMPEG_BIN", value),
                None => std::env::remove_var("FFMPEG_BIN"),
            }
            match &self.finalizer {
                Some(value) => std::env::set_var("MARGINS_HOSTED_WEBM_FINALIZER", value),
                None => std::env::remove_var("MARGINS_HOSTED_WEBM_FINALIZER"),
            }
        }
    }
}
