use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;

use crate::ctx::EventSink;
#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
use crate::live_backchannel;
use crate::Settings;

#[derive(Serialize)]
pub(crate) struct SpeechModelPrepResult {
    // Legacy field names retained for the existing frontend contract. These now
    // describe the active local transcription assets for this build.
    parakeet_ready: bool,
    parakeet_message: String,
    parakeet_model_dir: Option<String>,
    diarization_ready: bool,
    diarization_message: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct SpeechModelProbe {
    pub transcription: String,
    pub transcription_path: Option<String>,
    pub diarization: String,
    pub download_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
struct SpeechModelProgressEvent {
    stage: String,
    message: String,
    progress: Option<f32>,
}

// ---------------------------------------------------------------------------
// HuggingFace tree API types (internal)
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct HfTreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
    size: Option<u64>,
    lfs: Option<HfLfsInfo>,
}

#[derive(serde::Deserialize)]
struct HfLfsInfo {
    size: u64,
}

impl HfTreeEntry {
    fn blob_size(&self) -> u64 {
        self.lfs
            .as_ref()
            .map(|l| l.size)
            .unwrap_or(self.size.unwrap_or(0))
    }
}

// ---------------------------------------------------------------------------
// HF constants
// ---------------------------------------------------------------------------

const HF_REPO: &str = "FluidInference/parakeet-tdt-0.6b-v2-coreml";
const HF_TREE_API: &str =
    "https://huggingface.co/api/models/FluidInference/parakeet-tdt-0.6b-v2-coreml/tree/main?recursive=true";
const HF_RESOLVE_BASE: &str =
    "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/resolve/main";

/// Build the download URL for a given relative file path in the HF repo.
pub(crate) fn hf_resolve_url(file_path: &str) -> String {
    format!("{}/{}", HF_RESOLVE_BASE, file_path)
}

/// The repo ships several alternate model bundles (ParakeetEncoder*, Melspectrogram*,
/// RNNTJoint, …) totaling ~2.5 GB. Our loader (`has_coreml_assets_in_dir`) only uses
/// these five assets (~464 MB), so restrict every download/size calculation to them —
/// downloading the whole tree would pull >5x the bytes we need.
const REQUIRED_ASSET_DIRS: &[&str] = &[
    "Preprocessor.mlmodelc",
    "Encoder.mlmodelc",
    "Decoder.mlmodelc",
    "JointDecision.mlmodelc",
];
const REQUIRED_ASSET_FILES: &[&str] = &["parakeet_vocab.json"];

/// True if `path` (a repo-relative HF tree path) belongs to a required asset.
fn is_required_asset_path(path: &str) -> bool {
    if REQUIRED_ASSET_FILES.contains(&path) {
        return true;
    }
    let top = path.split('/').next().unwrap_or("");
    REQUIRED_ASSET_DIRS.contains(&top)
}

/// Fetch the HF tree and sum file sizes, returning None on any network error.
pub(crate) fn hf_tree_total_bytes() -> Option<u64> {
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .ok()?;
    let entries: Vec<HfTreeEntry> = client
        .get(HF_TREE_API)
        .send()
        .and_then(|r| r.error_for_status())
        .ok()?
        .json()
        .ok()?;
    Some(
        entries
            .iter()
            .filter(|e| e.entry_type == "file" && is_required_asset_path(&e.path))
            .map(|e| e.blob_size())
            .sum(),
    )
}

// ---------------------------------------------------------------------------
// public clear / prepare
// ---------------------------------------------------------------------------

pub(crate) fn clear_speech_models_blocking() -> Result<String, String> {
    let mut messages: Vec<String> = Vec::new();

    if let Some(polyvoice_dir) = dirs::cache_dir().map(|dir| dir.join("polyvoice").join("models")) {
        if polyvoice_dir.exists() {
            std::fs::remove_dir_all(&polyvoice_dir)
                .map_err(|e| format!("Could not remove speaker model cache: {e}"))?;
            messages.push("Speaker model cache cleared.".to_string());
        }
    }

    // Task 5: also remove the app-managed default FluidAudio CoreML dir —
    // but ONLY the conventional path; never remove an env-override or
    // user-set custom path.
    let env_override = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    if env_override.is_none() {
        let managed = default_managed_fluid_coreml_dir();
        if let Some(dir) = managed {
            if dir.exists() {
                std::fs::remove_dir_all(&dir)
                    .map_err(|e| format!("Could not remove local transcription model: {e}"))?;
                messages.push("Local transcription model removed.".to_string());
            }
        }
    }

    if messages.is_empty() {
        Ok("No local model assets found to remove.".to_string())
    } else {
        Ok(messages.join(" "))
    }
}

/// The one canonical app-managed directory (not env-overridden, not user-set).
/// This is the only path `clear_speech_models_blocking` is allowed to delete.
fn default_managed_fluid_coreml_dir() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    Some(default_managed_fluid_coreml_dir_for_home(&home))
}

fn default_managed_fluid_coreml_dir_for_home(home: &std::path::Path) -> std::path::PathBuf {
    home.join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2")
}

/// The downloader intentionally ignores profile settings: they can select an
/// existing model but must never redirect writes into a profile's configuration
/// tree. An explicit process override is the only isolated-cache opt-in.
fn fluid_coreml_download_target() -> Result<std::path::PathBuf, String> {
    fluid_coreml_download_target_from(
        fluid_coreml_model_dir_override(),
        std::env::var_os("HOME").map(std::path::PathBuf::from),
    )
}

fn fluid_coreml_download_target_from(
    override_dir: Option<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
) -> Result<std::path::PathBuf, String> {
    override_dir
        .or_else(|| home.map(|home| default_managed_fluid_coreml_dir_for_home(&home)))
        .ok_or_else(|| {
            "Could not resolve home directory for the shared FluidAudio model cache.".to_string()
        })
}

fn fluid_coreml_model_dir_override() -> Option<std::path::PathBuf> {
    margins_media::model_registry::env_path("MARGINS_FLUID_COREML_MODEL_DIR")
}

pub(crate) fn prepare_speech_models_blocking(
    sink: Arc<dyn EventSink>,
    settings: Settings,
    cancel: Arc<AtomicBool>,
) -> Result<SpeechModelPrepResult, String> {
    emit_speech_model_progress(
        sink.as_ref(),
        "prepare",
        "Checking local transcription assets...",
        Some(0.0),
    );
    check_speech_model_cancelled(&cancel)?;
    let (mut parakeet_ready, mut parakeet_message, mut parakeet_model_dir) =
        check_transcription_model(&settings);

    // The managed downloader installs FluidAudio CoreML assets. Those assets
    // are useful only to a macOS/CoreML build; a hosted Linux server must use
    // its configured Parakeet ONNX cache instead of downloading ~464 MB it
    // cannot execute.
    if !parakeet_ready && supports_managed_fluid_coreml_download() {
        emit_speech_model_progress(
            sink.as_ref(),
            "transcription",
            "Downloading local transcription model...",
            Some(0.01),
        );
        check_speech_model_cancelled(&cancel)?;
        match download_fluid_coreml_model(sink.as_ref(), &cancel) {
            Ok(dir) => {
                let dir_str = dir.to_string_lossy().to_string();
                parakeet_ready = true;
                parakeet_message = format!("Local transcription model downloaded and installed.");
                parakeet_model_dir = Some(dir_str);
            }
            Err(e) => {
                // propagate cancel as a hard error; other errors leave
                // parakeet_ready=false with the descriptive message.
                if e.contains("canceled") {
                    return Err(e);
                }
                parakeet_message = e;
            }
        }
        // Re-check so the final dir is authoritative.
        if parakeet_ready {
            let (r, m, d) = check_transcription_model(&settings);
            if r {
                parakeet_ready = r;
                parakeet_message = m;
                parakeet_model_dir = d;
            }
        }
    }

    if parakeet_ready {
        // Distinct phase: the download has finished and we are loading/warming
        // the model. Emit a separate "warmup" stage with indeterminate progress
        // so the frontend does not reuse the download percent (which would look
        // like the bar jumping backwards).
        emit_speech_model_progress(sink.as_ref(), "warmup", "Warming up transcription...", None);
        check_speech_model_cancelled(&cancel)?;
        match prepare_transcription_model_now(&settings, &cancel) {
            Ok(message) => {
                parakeet_message = message;
            }
            Err(e) => {
                parakeet_ready = false;
                parakeet_message = e;
            }
        }
    }

    // Task 3: emit indeterminate progress BEFORE diarization load.
    emit_speech_model_progress(
        sink.as_ref(),
        "diarization",
        "Preparing speaker model...",
        None, // None = indeterminate for the frontend
    );
    check_speech_model_cancelled(&cancel)?;
    let (diarization_ready, diarization_message) = prepare_diarization_models_now()?;
    // Task 3: emit completion after diarization.
    emit_speech_model_progress(
        sink.as_ref(),
        "diarization",
        if diarization_ready {
            "Speaker model ready."
        } else {
            "Speaker model not available."
        },
        Some(0.98),
    );
    emit_speech_model_progress(
        sink.as_ref(),
        "complete",
        "Speech models are ready.",
        Some(1.0),
    );

    Ok(SpeechModelPrepResult {
        parakeet_ready,
        parakeet_message,
        parakeet_model_dir,
        diarization_ready,
        diarization_message,
    })
}

// ---------------------------------------------------------------------------
// Task 2: probe command (no download)
// ---------------------------------------------------------------------------

pub(crate) fn probe_speech_models_impl(
    settings: &Settings,
    custom_path: Option<String>,
) -> SpeechModelProbe {
    let (transcription, transcription_path) = probe_transcription(settings, custom_path);
    let diarization = probe_diarization();
    let download_bytes = supports_managed_fluid_coreml_download()
        .then(hf_tree_total_bytes)
        .flatten(); // best-effort, None on non-CoreML platforms or failure
    SpeechModelProbe {
        transcription,
        transcription_path,
        diarization,
        download_bytes,
    }
}

fn probe_transcription(
    settings: &Settings,
    custom_path: Option<String>,
) -> (String, Option<String>) {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        // custom_path takes priority.
        if let Some(ref cp) = custom_path {
            let p = std::path::PathBuf::from(crate::expand_tilde(cp));
            if has_coreml_assets_in_dir(&p) {
                return ("ready".to_string(), Some(p.to_string_lossy().to_string()));
            } else {
                return ("missing".to_string(), Some(p.to_string_lossy().to_string()));
            }
        }

        return match resolved_fluid_coreml_model_dir(settings) {
            Some(dir) => ("ready".to_string(), Some(dir.to_string_lossy().to_string())),
            None => ("missing".to_string(), None),
        };
    }

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    {
        let selected = custom_path.or_else(|| {
            std::env::var("MARGINS_PARAKEET_MODEL_DIR")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(|| settings.parakeet_model_dir.clone())
        });
        let Some(path) = selected else {
            return ("missing".to_string(), None);
        };
        let dir = std::path::PathBuf::from(crate::expand_tilde(&path));
        let kind = margins_media::model_registry::parakeet_kind_from_env()
            .unwrap_or(margins::asr::AsrModelKind::Tdt);
        let ready = margins::asr::missing_model_files(&dir, kind).is_empty();
        return (
            if ready { "ready" } else { "missing" }.to_string(),
            Some(dir.to_string_lossy().to_string()),
        );
    }

    #[cfg(not(any(
        all(feature = "coreml-asr", target_os = "macos"),
        feature = "parakeet-asr"
    )))]
    {
        let _ = (settings, custom_path);
        ("missing".to_string(), None)
    }
}

fn supports_managed_fluid_coreml_download() -> bool {
    supports_managed_fluid_coreml_download_for(
        cfg!(target_os = "macos"),
        cfg!(feature = "coreml-asr"),
    )
}

fn supports_managed_fluid_coreml_download_for(is_macos: bool, coreml_enabled: bool) -> bool {
    is_macos && coreml_enabled
}

fn probe_diarization() -> String {
    let cache_dir = match dirs::cache_dir() {
        Some(d) => d,
        None => return "missing".to_string(),
    };
    let models = cache_dir.join("polyvoice").join("models");
    if models.join("powerset_fp32.onnx").exists() && models.join("wespeaker_resnet34.onnx").exists()
    {
        "ready".to_string()
    } else {
        "missing".to_string()
    }
}

// ---------------------------------------------------------------------------
// Task 1: FluidAudio CoreML downloader
// ---------------------------------------------------------------------------

pub(crate) fn download_fluid_coreml_model(
    sink: &dyn EventSink,
    cancel: &Arc<AtomicBool>,
) -> Result<std::path::PathBuf, String> {
    check_speech_model_cancelled(cancel)?;
    margins_media::model_registry::prepare_model_with_cancel(
        margins_media::model_registry::ModelKind::CoreMl,
        &|message, progress| {
            emit_speech_model_progress(sink, "transcription", &message, progress);
        },
        &|| cancel.load(Ordering::SeqCst),
    )
    .map_err(|error| format!("{error:#}"))
}

/// Check whether `dir` contains all required CoreML assets.
/// Mirrors live_backchannel::has_coreml_assets but available here for use
/// in the downloader verify step.
pub(crate) fn has_coreml_assets_in_dir(dir: &std::path::Path) -> bool {
    margins_media::model_registry::coreml::valid_model(dir)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn check_transcription_model(settings: &Settings) -> (bool, String, Option<String>) {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        if let Some(path) = resolved_fluid_coreml_model_dir(settings) {
            let path = path.to_string_lossy().to_string();
            return (
                true,
                format!("FluidAudio CoreML transcription assets found in {path}."),
                Some(path),
            );
        }
        return check_fluid_coreml_model();
    }

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    return check_parakeet_onnx_model(settings);

    #[cfg(not(any(
        all(feature = "coreml-asr", target_os = "macos"),
        feature = "parakeet-asr"
    )))]
    return (
        false,
        "FluidAudio CoreML transcription assets were not found.".to_string(),
        None,
    );
}

/// Report whether this process can actually construct the configured ASR
/// backend. This is deliberately stricter than a Cargo feature check: hosted
/// clients use it during capability negotiation before starting a device.
pub(crate) fn transcription_runtime_available(settings: &Settings) -> bool {
    let (model_ready, _, _) = check_transcription_model(settings);
    if !model_ready {
        return false;
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    return true;

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    return margins::asr::parakeet::ParakeetAsr::runtime_available().is_ok();

    #[cfg(not(any(
        all(feature = "coreml-asr", target_os = "macos"),
        feature = "parakeet-asr"
    )))]
    false
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn check_fluid_coreml_model() -> (bool, String, Option<String>) {
    (
        false,
        "FluidAudio CoreML transcription assets were not found. Install FluidAudio models or set MARGINS_FLUID_COREML_MODEL_DIR.".to_string(),
        None,
    )
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn check_parakeet_onnx_model(settings: &Settings) -> (bool, String, Option<String>) {
    match margins_media::model_registry::resolve_parakeet_model_with_fallback(
        settings.parakeet_model_dir.as_deref(),
    ) {
        Ok(Some((dir, _))) => {
            let path = dir.to_string_lossy().to_string();
            (
                true,
                format!("Parakeet ONNX transcription assets found in {path}."),
                Some(path),
            )
        }
        Ok(None) => (
            false,
            "Parakeet ONNX assets were not found. Set MARGINS_PARAKEET_MODEL_DIR.".to_string(),
            None,
        ),
        Err(error) => (false, error.to_string(), None),
    }
}

fn check_speech_model_cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::SeqCst) {
        Err("Model preparation canceled.".to_string())
    } else {
        Ok(())
    }
}

pub(crate) fn emit_speech_model_progress(
    sink: &dyn EventSink,
    stage: &str,
    message: &str,
    progress: Option<f32>,
) {
    sink.emit(
        "speech-model-progress",
        serde_json::to_value(SpeechModelProgressEvent {
            stage: stage.to_string(),
            message: message.to_string(),
            progress,
        })
        .unwrap_or(serde_json::Value::Null),
    );
}

fn resolved_fluid_coreml_model_dir(settings: &Settings) -> Option<std::path::PathBuf> {
    margins_media::model_registry::resolve_coreml_dir_with_fallback(
        settings.parakeet_model_dir.as_deref(),
    )
}

#[cfg(test)]
fn resolved_fluid_coreml_model_dir_from(
    settings: &Settings,
    override_dir: Option<std::path::PathBuf>,
    default_dir: Option<std::path::PathBuf>,
) -> Option<std::path::PathBuf> {
    if let Some(dir) = override_dir {
        return has_coreml_assets_in_dir(&dir).then_some(dir);
    }

    settings
        .parakeet_model_dir
        .as_deref()
        .map(str::trim)
        .filter(|dir| !dir.is_empty())
        .map(crate::expand_tilde)
        .map(std::path::PathBuf::from)
        .filter(|dir| has_coreml_assets_in_dir(dir))
        .or(default_dir)
}

#[cfg(test)]
fn default_fluid_coreml_model_dir() -> Option<std::path::PathBuf> {
    margins_media::model_registry::resolve_coreml_dir()
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn prepare_transcription_model_now(
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let model_dir = live_backchannel::resolved_live_model_dir(settings).ok_or_else(|| {
        "FluidAudio CoreML transcription assets were not found. Install FluidAudio models or set MARGINS_FLUID_COREML_MODEL_DIR.".to_string()
    })?;
    let mut asr =
        margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(&model_dir).map_err(|error| {
            format!("Could not load FluidAudio CoreML transcription model: {error}")
        })?;
    check_speech_model_cancelled(cancel)?;
    let warmup = asr.warmup_models().map_err(|error| {
        format!("Could not warm FluidAudio CoreML transcription model: {error}")
    })?;
    Ok(format!(
        "FluidAudio CoreML transcription model loaded and warmed from {} (frontend {} ms, decode {} ms).",
        model_dir.display(), warmup.frontend_ms, warmup.decode_ms
    ))
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn prepare_transcription_model_now(
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let (dir, kind) = margins_media::model_registry::resolve_parakeet_model_with_fallback(
        settings.parakeet_model_dir.as_deref(),
    )
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "Set MARGINS_PARAKEET_MODEL_DIR to a Parakeet ONNX model folder.".to_string())?;
    check_speech_model_cancelled(cancel)?;
    let _asr = margins_media::providers::parakeet::ParakeetOnnxBackend::from_dir(&dir, kind)
        .map_err(|error| format!("Could not load Parakeet ONNX transcription model: {error}"))?;
    Ok(format!(
        "Parakeet ONNX transcription model loaded from {}.",
        dir.display()
    ))
}

#[cfg(not(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
)))]
fn prepare_transcription_model_now(
    _settings: &Settings,
    _cancel: &AtomicBool,
) -> Result<String, String> {
    Ok(
        "FluidAudio CoreML transcription assets are downloaded. This headless build cannot run macOS CoreML inference.".to_string(),
    )
}

#[cfg(feature = "rust-diarization")]
fn prepare_diarization_models_now() -> Result<(bool, String), String> {
    // Task 3: non-fatal — diarization failure does not block transcription.
    match margins::diarization::polyvoice_backend::PolyvoiceDiarization::from_default_registry() {
        Ok(_) => Ok((
            true,
            "Speaker diarization model is downloaded and ready.".to_string(),
        )),
        Err(e) => {
            eprintln!("[aside] Speaker diarization model could not be loaded: {e}");
            Ok((
                false,
                format!("Speaker diarization model could not be loaded: {e}"),
            ))
        }
    }
}

#[cfg(not(feature = "rust-diarization"))]
fn prepare_diarization_models_now() -> Result<(bool, String), String> {
    Ok((
        false,
        "This desktop build does not include the experimental diarization model.".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_coreml_download_is_never_offered_to_a_hosted_linux_server() {
        assert!(!supports_managed_fluid_coreml_download_for(false, true));
        assert!(!supports_managed_fluid_coreml_download_for(false, false));
        assert!(supports_managed_fluid_coreml_download_for(true, true));
    }

    /// Task 6(a): HF resolve-URL construction.
    #[test]
    fn test_hf_resolve_url_construction() {
        let url = hf_resolve_url("Preprocessor.mlmodelc/coremldata.bin");
        assert_eq!(
            url,
            "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/resolve/main/Preprocessor.mlmodelc/coremldata.bin"
        );
    }

    /// Real-network smoke test for the download path: repo layout, the
    /// `is_required_asset_path` filter, resolve-URL construction, the byte
    /// stream, and the per-file size guard. `#[ignore]`d so normal CI stays
    /// offline; this is where the DEFERRED clean-machine download validation
    /// belongs. Run it on a box with no pre-installed model via:
    ///   cargo test -p margins-desktop real_download_smoke -- --ignored --nocapture
    /// To do the full 464 MB run, loop `required` downloading each to a temp
    /// dir and assert `has_coreml_assets_in_dir` after.
    #[test]
    #[ignore]
    fn real_download_smoke() {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();
        let entries: Vec<HfTreeEntry> = client
            .get(HF_TREE_API)
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .json()
            .unwrap();
        let required: Vec<&HfTreeEntry> = entries
            .iter()
            .filter(|e| e.entry_type == "file" && is_required_asset_path(&e.path))
            .collect();
        // Repo-drift guard: every required asset represented, variants excluded.
        for dir in REQUIRED_ASSET_DIRS {
            assert!(
                required.iter().any(|e| e.path.starts_with(dir)),
                "required dir {dir} missing from live repo"
            );
        }
        assert!(required.iter().any(|e| e.path == "parakeet_vocab.json"));
        assert!(
            !required
                .iter()
                .any(|e| e.path.starts_with("ParakeetEncoder")),
            "alternate bundle leaked through the filter"
        );
        let total: u64 = required.iter().map(|e| e.blob_size()).sum();
        assert!(
            (400_000_000..=600_000_000).contains(&total),
            "unexpected required-asset total: {total} bytes"
        );

        // Download the smallest required file and verify size == declared —
        // exercises the resolve URL + byte stream + truncation guard without
        // pulling the full 464 MB.
        let small = required
            .iter()
            .filter(|e| e.blob_size() > 0)
            .min_by_key(|e| e.blob_size())
            .expect("at least one sized required file");
        let bytes = client
            .get(hf_resolve_url(&small.path))
            .send()
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .unwrap();
        assert_eq!(
            bytes.len() as u64,
            small.blob_size(),
            "size mismatch for {}",
            small.path
        );
    }

    #[test]
    fn test_is_required_asset_path() {
        // Required: the five assets the loader consumes (dirs + vocab).
        assert!(is_required_asset_path("parakeet_vocab.json"));
        assert!(is_required_asset_path(
            "Preprocessor.mlmodelc/coremldata.bin"
        ));
        assert!(is_required_asset_path(
            "Encoder.mlmodelc/weights/weight.bin"
        ));
        assert!(is_required_asset_path("Decoder.mlmodelc/model.mil"));
        assert!(is_required_asset_path(
            "JointDecision.mlmodelc/metadata.json"
        ));
        // Excluded: alternate bundles and repo metadata that must NOT be downloaded.
        assert!(!is_required_asset_path(
            "ParakeetEncoder.mlmodelc/weights/weight.bin"
        ));
        assert!(!is_required_asset_path(
            "ParakeetEncoder_v2.mlmodelc/coremldata.bin"
        ));
        assert!(!is_required_asset_path(
            "ParakeetEncoder_4bit_par.mlmodelc/model.mil"
        ));
        assert!(!is_required_asset_path(
            "Melspectrogram_v2.mlmodelc/coremldata.bin"
        ));
        assert!(!is_required_asset_path("RNNTJoint.mlmodelc/model.mil"));
        assert!(!is_required_asset_path("README.md"));
        assert!(!is_required_asset_path("config.json"));
        assert!(!is_required_asset_path(".gitattributes"));
    }

    #[test]
    fn test_hf_resolve_url_top_level_file() {
        let url = hf_resolve_url("parakeet_vocab.json");
        assert_eq!(
            url,
            "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v2-coreml/resolve/main/parakeet_vocab.json"
        );
    }

    /// Task 6(b): has_coreml_assets_in_dir / atomic-rename guard logic
    /// using a fake directory structure. Fully hermetic — no network.
    #[test]
    fn test_has_coreml_assets_in_dir_missing() {
        let tmp = std::env::temp_dir().join(format!("speech_models_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // empty dir — should fail
        assert!(!has_coreml_assets_in_dir(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_has_coreml_assets_in_dir_complete() {
        let tmp = std::env::temp_dir().join(format!(
            "speech_models_test_complete_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Create required asset stubs.
        for name in &[
            "Preprocessor.mlmodelc",
            "Encoder.mlmodelc",
            "Decoder.mlmodelc",
            "JointDecision.mlmodelc",
        ] {
            std::fs::create_dir_all(tmp.join(name)).unwrap();
        }
        std::fs::write(tmp.join("parakeet_vocab.json"), b"{}").unwrap();

        assert!(has_coreml_assets_in_dir(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_has_coreml_assets_in_dir_partial() {
        let tmp =
            std::env::temp_dir().join(format!("speech_models_test_partial_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // Only some assets — Encoder missing.
        for name in &[
            "Preprocessor.mlmodelc",
            "Decoder.mlmodelc",
            "JointDecision.mlmodelc",
        ] {
            std::fs::create_dir_all(tmp.join(name)).unwrap();
        }
        std::fs::write(tmp.join("parakeet_vocab.json"), b"{}").unwrap();

        assert!(!has_coreml_assets_in_dir(&tmp));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Atomic rename guard: simulate temp dir -> final dir rename behaviour.
    #[test]
    fn test_atomic_rename_guard() {
        let base =
            std::env::temp_dir().join(format!("speech_models_rename_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();

        let tmp = base.join("downloading");
        let final_dir = base.join("installed");

        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("marker.txt"), b"ok").unwrap();

        // Rename should work.
        std::fs::rename(&tmp, &final_dir).unwrap();
        assert!(final_dir.join("marker.txt").exists());
        assert!(!tmp.exists());

        // If final_dir exists already, we should be able to move it aside then rename.
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("marker2.txt"), b"ok2").unwrap();
        let old = base.join("installed_old");
        std::fs::rename(&final_dir, &old).unwrap();
        std::fs::rename(&tmp, &final_dir).unwrap();
        assert!(final_dir.join("marker2.txt").exists());

        let _ = std::fs::remove_dir_all(&base);
    }

    fn write_coreml_asset_stubs(dir: &std::path::Path) {
        for name in [
            "Preprocessor.mlmodelc",
            "Encoder.mlmodelc",
            "Decoder.mlmodelc",
            "JointDecision.mlmodelc",
        ] {
            std::fs::create_dir_all(dir.join(name)).unwrap();
        }
        std::fs::write(dir.join("parakeet_vocab.json"), b"{}").unwrap();
    }

    #[test]
    fn coreml_env_override_wins_over_profile_setting_and_shared_cache() {
        let root = std::env::temp_dir().join(format!(
            "speech_models_override_precedence_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let override_dir = root.join("isolated");
        let profile_dir = root.join("profile-selected");
        let shared_dir = root.join("shared");
        write_coreml_asset_stubs(&override_dir);
        write_coreml_asset_stubs(&profile_dir);
        write_coreml_asset_stubs(&shared_dir);

        let mut settings = Settings::default();
        settings.parakeet_model_dir = Some(profile_dir.to_string_lossy().into_owned());
        let resolved = resolved_fluid_coreml_model_dir_from(
            &settings,
            Some(override_dir.clone()),
            Some(shared_dir),
        );
        assert_eq!(resolved, Some(override_dir));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn profile_setting_cannot_redirect_shared_cache_downloads() {
        let root = std::env::temp_dir().join(format!(
            "speech_models_download_target_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let profile_selected_dir = root.join("profile-selected");
        let expected_shared = default_managed_fluid_coreml_dir_for_home(&home);
        let mut settings = Settings::default();
        settings.parakeet_model_dir = Some(profile_selected_dir.to_string_lossy().into_owned());

        let target = fluid_coreml_download_target_from(None, Some(home)).unwrap();
        assert_eq!(target, expected_shared);
        assert_ne!(
            target,
            std::path::PathBuf::from(settings.parakeet_model_dir.unwrap()),
            "a profile setting may select an existing model but never becomes a downloader target"
        );
        assert_eq!(
            fluid_coreml_download_target_from(Some(profile_selected_dir.clone()), None),
            Ok(profile_selected_dir)
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
