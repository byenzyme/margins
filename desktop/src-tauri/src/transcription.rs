use margins::{asr::AsrBackend, audio_info, session};
#[cfg(not(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
)))]
use std::io::{BufRead, BufReader};
#[cfg(not(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
)))]
use std::process::{Command, Stdio};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

use crate::{
    processing_events::{ProcessingEvent, TranscriptEntry},
    Settings,
};

const DEFAULT_TRANSCRIBE_TIMEOUT_SECS: u64 = 600;
const TRANSCRIBE_TIMEOUT_ENV: &str = "MARGINS_TRANSCRIBE_TIMEOUT_SECS";

fn transcribe_timeout() -> Duration {
    timeout_from_env_secs(TRANSCRIBE_TIMEOUT_ENV, DEFAULT_TRANSCRIBE_TIMEOUT_SECS)
}

fn timeout_from_env_secs(name: &str, default_secs: u64) -> Duration {
    let secs = std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|secs| *secs > 0)
        .unwrap_or(default_secs);
    Duration::from_secs(secs)
}

fn recv_transcription_result<T>(
    rx: std::sync::mpsc::Receiver<T>,
    timeout: Duration,
) -> Result<T, String> {
    match rx.recv_timeout(timeout) {
        Ok(value) => Ok(value),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(format!(
            "Transcription timed out after {}s",
            timeout.as_secs()
        )),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err("Transcription worker stopped".to_string())
        }
    }
}

pub(crate) fn transcribe_segments(
    app: &AppHandle,
    name: &str,
    work_dir: &std::path::Path,
    meta: &session::SessionMeta,
    _script_path: &str,
    settings: &Settings,
    max_speakers: Option<usize>,
    force_diarize: bool,
) -> Result<Vec<String>, String> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    return transcribe_segments_with_provider(
        app,
        name,
        work_dir,
        meta,
        settings,
        max_speakers,
        force_diarize,
    );

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    return transcribe_segments_with_provider(
        app,
        name,
        work_dir,
        meta,
        settings,
        max_speakers,
        force_diarize,
    );

    #[cfg(not(any(
        all(feature = "coreml-asr", target_os = "macos"),
        feature = "parakeet-asr"
    )))]
    return transcribe_segments_python(app, name, work_dir, meta, _script_path);
}

#[cfg(not(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
)))]
fn transcribe_segments_python(
    app: &AppHandle,
    name: &str,
    work_dir: &std::path::Path,
    meta: &session::SessionMeta,
    script_path: &str,
) -> Result<Vec<String>, String> {
    let mut transcript_paths = Vec::new();
    let n = meta.segments.len();

    for (i, seg) in meta.segments.iter().enumerate() {
        let pct = (i as f32) / (n as f32) * 0.4;
        emit_processing(
            app,
            "transcribe",
            &format!("Transcribing segment {} of {}...", i + 1, n),
            Some(pct),
        );

        let wav = work_dir.join(&seg.wav_path);
        let out = format!("/tmp/{}_seg{}_transcript.json", name, seg.segment_index);

        let mut child = Command::new("python3")
            .arg(script_path)
            .arg("transcribe")
            .arg(wav.to_str().unwrap())
            .arg("--output")
            .arg(&out)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Transcription failed to start: {}", e))?;

        let app_for_stdout = app.clone();
        let stdout_reader = child.stdout.take().map(|stdout| {
            std::thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    if let Some(entry_json) = line.strip_prefix("ENTRY:") {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(entry_json) {
                            emit_transcript_entry(
                                &app_for_stdout,
                                TranscriptEntry {
                                    channel: val["channel"].as_u64().unwrap_or(0) as u32,
                                    start_ms: val["start_ms"].as_u64().unwrap_or(0),
                                    end_ms: val["end_ms"].as_u64().unwrap_or(0),
                                    text: val["text"].as_str().unwrap_or("").to_string(),
                                },
                            );
                        }
                    } else if line.starts_with("CLEANUP:")
                        || line.starts_with("MERGE:")
                        || line.starts_with("SPLITTING")
                        || line.starts_with("TRANSCRIBING")
                        || line.starts_with("DURATION:")
                    {
                        emit_processing(&app_for_stdout, "transcribe", &line, None);
                    }
                }
            })
        });

        let status = wait_for_transcription_child(&mut child, transcribe_timeout())?;
        if let Some(stdout_reader) = stdout_reader {
            let _ = stdout_reader.join();
        }
        if !status.success() {
            return Err("Transcription failed".to_string());
        }

        transcript_paths.push(out);
    }

    Ok(transcript_paths)
}

#[cfg(not(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
)))]
fn wait_for_transcription_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let started = std::time::Instant::now();
    loop {
        match child
            .try_wait()
            .map_err(|e| format!("Transcription process error: {e}"))?
        {
            Some(status) => return Ok(status),
            None if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Transcription timed out after {}s",
                    timeout.as_secs()
                ));
            }
            None => std::thread::sleep(Duration::from_millis(200)),
        }
    }
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
enum OfflineAsrProvider {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    CoreMl(margins::coreml_asr::FluidCoreMlAsr),
    #[cfg(feature = "parakeet-asr")]
    Parakeet(margins::asr::parakeet::ParakeetAsr),
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
impl OfflineAsrProvider {
    fn name(&self) -> &'static str {
        match self {
            #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
            Self::CoreMl(_) => "FluidAudio CoreML",
            #[cfg(feature = "parakeet-asr")]
            Self::Parakeet(_) => "Parakeet ONNX",
        }
    }

    fn transcribe_words(
        &mut self,
        mono_16k: &[f32],
    ) -> anyhow::Result<Vec<margins::asr::WordTiming>> {
        match self {
            #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
            Self::CoreMl(asr) => asr.transcribe_words(mono_16k),
            #[cfg(feature = "parakeet-asr")]
            Self::Parakeet(asr) => asr.transcribe_words(mono_16k),
        }
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn coreml_timings(&self) -> Option<&margins::coreml_asr::FluidCoreMlTimings> {
        match self {
            Self::CoreMl(asr) => Some(asr.timings()),
            #[cfg(feature = "parakeet-asr")]
            Self::Parakeet(_) => None,
        }
    }
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
fn transcribe_segments_with_provider(
    app: &AppHandle,
    name: &str,
    work_dir: &std::path::Path,
    meta: &session::SessionMeta,
    settings: &Settings,
    max_speakers: Option<usize>,
    force_diarize: bool,
) -> Result<Vec<String>, String> {
    let asr = OfflineAsrWorker::start(settings)?;
    emit_processing(
        app,
        "transcribe",
        &format!("Loaded {} transcription models.", asr.name()),
        Some(0.08),
    );

    let single_speaker_requested = max_speakers == Some(1);
    // Speaker labeling is auto-derived from the recording's channel reality rather
    // than a user toggle. It only applies to single-track (mono) segments, which
    // lack a second channel to separate speakers by; ordinary stereo captures (mic
    // on one channel, system audio on the other) are already speaker-separated and
    // never diarize. "Multi-speaker intent" = anything other than an explicit
    // single-speaker request; imports (`force_diarize`) and the env override force
    // it on. (`settings.rust_diarization_enabled` is retained for back-compat
    // deserialization but no longer drives this decision.)
    #[cfg(feature = "rust-diarization")]
    let diar_allowed = !single_speaker_requested
        && (force_diarize
            || max_speakers.map_or(true, |n| n > 1)
            || std::env::var("MARGINS_POLYVOICE_DIARIZE").ok().as_deref() == Some("1"));
    #[cfg(not(feature = "rust-diarization"))]
    let _ = (single_speaker_requested, force_diarize);

    // Loaded lazily on the first mono segment so stereo captures never pay the
    // model-load cost. `diarizer_load_failed` makes the non-fatal fallback sticky.
    #[cfg(feature = "rust-diarization")]
    let mut diarizer: Option<margins::diarization::polyvoice_backend::PolyvoiceDiarization> = None;
    #[cfg(feature = "rust-diarization")]
    let mut diarizer_load_failed = false;

    let mut transcript_paths = Vec::new();
    let n = meta.segments.len();
    for (i, seg) in meta.segments.iter().enumerate() {
        emit_processing(
            app,
            "transcribe",
            &format!(
                "Transcribing segment {} of {} with {}...",
                i + 1,
                n,
                asr.name()
            ),
            Some(0.1 + (i as f32) / (n as f32) * 0.35),
        );
        let wav = work_dir.join(&seg.wav_path);
        let info = audio_info::probe(&wav).map_err(|e| format!("Audio probe failed: {e}"))?;

        let entries;

        #[cfg(feature = "rust-diarization")]
        {
            if info.channels < 2 && diar_allowed && !diarizer_load_failed {
                if diarizer.is_none() {
                    emit_processing(app, "transcribe", "Loading speaker models...", None);
                    // Diarization failure is non-fatal — fall back to single-speaker
                    // transcription and continue rather than hard-failing the session.
                    match margins::diarization::polyvoice_backend::PolyvoiceDiarization::with_speaker_count(
                        max_speakers,
                    ) {
                        Ok(d) => diarizer = Some(d),
                        Err(e) => {
                            eprintln!("[aside] Speaker diarization model unavailable, continuing without speaker labels: {e}");
                            emit_processing(
                                app,
                                "transcribe",
                                "Speaker labels unavailable — continuing without speaker identification.",
                                None,
                            );
                            diarizer_load_failed = true;
                        }
                    }
                }
                if let Some(diarizer) = diarizer.as_mut() {
                    use margins::asr::{merge_and_dedupe_entries, TranscriptWordEntry};
                    use margins::diarization::{assign_speakers_to_words, DiarizationBackend};

                    let mono_16k = margins::audio_pipeline::mono_16k_from_wav(&wav)
                        .map_err(|e| format!("Mono downmix failed: {e}"))?;
                    let response = asr
                        .transcribe_words(mono_16k.clone())
                        .map_err(|e| format!("{} transcription failed: {e}", asr.name()))?;
                    let words = response.words;
                    let segments = diarizer
                        .diarize(&mono_16k)
                        .map_err(|e| format!("Polyvoice diarization failed: {e}"))?;
                    let assigned = assign_speakers_to_words(&words, &segments);
                    emit_processing(
                        app,
                        "transcribe",
                        &format!(
                            "Diarized {} words into {} speaker turns.",
                            words.len(),
                            segments.len()
                        ),
                        None,
                    );
                    let word_entries: Vec<TranscriptWordEntry> = assigned
                        .into_iter()
                        .map(|w| TranscriptWordEntry {
                            channel: w.channel,
                            start_ms: w.start_ms,
                            end_ms: w.end_ms,
                            text: format!(" {}", w.text.trim()),
                        })
                        .collect();
                    entries = merge_and_dedupe_entries(word_entries, 2_000);

                    for entry in &entries {
                        emit_transcript_entry(
                            app,
                            TranscriptEntry {
                                channel: entry.channel,
                                start_ms: entry.start_ms,
                                end_ms: entry.end_ms,
                                text: entry.text.clone(),
                            },
                        );
                    }

                    let out = transcript_output_path(name, seg.segment_index);
                    let mut json = margins::asr::transcript_json(&entries);
                    json["channel_semantics"] = serde_json::json!("diarized_speaker");
                    std::fs::write(&out, serde_json::to_string_pretty(&json).unwrap())
                        .map_err(|e| format!("Failed to write transcript JSON: {e}"))?;
                    transcript_paths.push(out);
                    continue;
                }
            }
        }

        // Non-diarized path: stereo (mic/system split) or diarization disabled.
        let transcript = transcribe_wav_to_transcript_with_provider(&asr, &wav)
            .map_err(|e| format!("{} transcription failed: {e}", asr.name()))?;
        for (channel, words) in &transcript.channel_word_counts {
            #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
            let detail = if let Some(timings) = transcript.coreml_timings.as_ref() {
                format!(
                    "{} channel {channel}: {} words, load {:?}, pre {:?}, enc {:?}.",
                    asr.name(),
                    words,
                    timings.model_load,
                    timings.last_preprocess,
                    timings.last_encoder
                )
            } else {
                format!("{} channel {channel}: {} words.", asr.name(), words)
            };
            #[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
            let detail = format!("{} channel {channel}: {} words.", asr.name(), words);
            emit_processing(app, "transcribe", &detail, None);
        }

        entries = margins::asr::merge_and_dedupe_entries(transcript.entries, 2_000);
        for entry in &entries {
            emit_transcript_entry(
                app,
                TranscriptEntry {
                    channel: entry.channel,
                    start_ms: entry.start_ms,
                    end_ms: entry.end_ms,
                    text: entry.text.clone(),
                },
            );
        }

        let out = transcript_output_path(name, seg.segment_index);
        let mut json = margins::asr::transcript_json(&entries);
        if info.channels >= 2 {
            json["channel_semantics"] = serde_json::json!("recording_channel");
        }
        std::fs::write(&out, serde_json::to_string_pretty(&json).unwrap())
            .map_err(|e| format!("Failed to write transcript JSON: {e}"))?;
        transcript_paths.push(out);
    }

    Ok(transcript_paths)
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
fn load_offline_asr_provider(settings: &Settings) -> Result<OfflineAsrProvider, String> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        let model_dir =
            crate::live_backchannel::resolved_live_model_dir(settings).ok_or_else(|| {
                "Install FluidAudio CoreML model assets or set MARGINS_FLUID_COREML_MODEL_DIR."
                    .to_string()
            })?;
        let asr = margins::coreml_asr::FluidCoreMlAsr::from_dir_auto(&model_dir)
            .map_err(|e| format!("Failed to load FluidAudio CoreML models: {e}"))?;
        return Ok(OfflineAsrProvider::CoreMl(asr));
    }

    #[cfg(all(
        not(all(feature = "coreml-asr", target_os = "macos")),
        feature = "parakeet-asr"
    ))]
    {
        let (model_dir, kind) = resolved_parakeet_onnx_model(settings)?;
        let asr = margins::asr::parakeet::ParakeetAsr::from_dir(&model_dir, kind)
            .map_err(|e| format!("Failed to load Parakeet ONNX models: {e}"))?;
        Ok(OfflineAsrProvider::Parakeet(asr))
    }
}

#[cfg(all(
    not(all(feature = "coreml-asr", target_os = "macos")),
    feature = "parakeet-asr"
))]
fn resolved_parakeet_onnx_model(
    settings: &Settings,
) -> Result<(std::path::PathBuf, margins::asr::AsrModelKind), String> {
    let dir = std::env::var("MARGINS_PARAKEET_MODEL_DIR")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| settings.parakeet_model_dir.clone())
        .map(|path| crate::expand_tilde(&path))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| {
            "Set MARGINS_PARAKEET_MODEL_DIR or choose a Parakeet ONNX model folder.".to_string()
        })?;

    let kind = match std::env::var("MARGINS_PARAKEET_MODEL_KIND")
        .unwrap_or_else(|_| "tdt".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "ctc" => margins::asr::AsrModelKind::Ctc,
        "tdt" | "tdt-v3" | "v3" => margins::asr::AsrModelKind::Tdt,
        other => {
            return Err(format!(
                "Unknown MARGINS_PARAKEET_MODEL_KIND `{other}`; use `tdt` or `ctc`."
            ));
        }
    };

    let missing = margins::asr::missing_model_files(&dir, kind);
    if !missing.is_empty() {
        return Err(format!(
            "Parakeet ONNX model folder {} is missing: {}",
            dir.display(),
            missing.join(", ")
        ));
    }

    Ok((dir, kind))
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
struct OfflineAsrWorker {
    name: &'static str,
    command_tx: std::sync::mpsc::Sender<OfflineAsrCommand>,
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
enum OfflineAsrCommand {
    Transcribe {
        mono_16k: Vec<f32>,
        response_tx: std::sync::mpsc::Sender<Result<OfflineAsrResponse, String>>,
    },
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
struct OfflineAsrResponse {
    words: Vec<margins::asr::WordTiming>,
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    coreml_timings: Option<margins::coreml_asr::FluidCoreMlTimings>,
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
impl OfflineAsrWorker {
    fn start(settings: &Settings) -> Result<Self, String> {
        let settings = settings.clone();
        let timeout = transcribe_timeout();
        let (init_tx, init_rx) = std::sync::mpsc::sync_channel(1);
        let (command_tx, command_rx) = std::sync::mpsc::channel();

        std::thread::Builder::new()
            .name("margins-offline-transcribe".to_string())
            .spawn(move || {
                let mut provider = match load_offline_asr_provider(&settings) {
                    Ok(provider) => {
                        let name = provider.name();
                        let _ = init_tx.send(Ok(name));
                        provider
                    }
                    Err(err) => {
                        let _ = init_tx.send(Err(err));
                        return;
                    }
                };

                while let Ok(command) = command_rx.recv() {
                    match command {
                        OfflineAsrCommand::Transcribe {
                            mono_16k,
                            response_tx,
                        } => {
                            let result = provider
                                .transcribe_words(&mono_16k)
                                .map(|words| {
                                    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
                                    {
                                        OfflineAsrResponse {
                                            words,
                                            coreml_timings: provider.coreml_timings().cloned(),
                                        }
                                    }
                                    #[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
                                    {
                                        OfflineAsrResponse { words }
                                    }
                                })
                                .map_err(|e| e.to_string());
                            let _ = response_tx.send(result);
                        }
                    }
                }
            })
            .map_err(|e| format!("Transcription worker failed to start: {e}"))?;

        let name = recv_transcription_result(init_rx, timeout)??;
        Ok(Self { name, command_tx })
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn transcribe_words(&self, mono_16k: Vec<f32>) -> Result<OfflineAsrResponse, String> {
        let timeout = transcribe_timeout();
        let (response_tx, response_rx) = std::sync::mpsc::channel();
        self.command_tx
            .send(OfflineAsrCommand::Transcribe {
                mono_16k,
                response_tx,
            })
            .map_err(|_| "Transcription worker stopped".to_string())?;

        // FluidAudio/CoreML can deadlock inside the native transcription call.
        // This bounds the session failure; the underlying worker thread may
        // continue running until the native call returns.
        recv_transcription_result(response_rx, timeout)?
    }
}

#[cfg(test)]
fn run_test_transcription_timeout(timeout: Duration) -> Result<(), String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        std::thread::sleep(timeout + Duration::from_millis(50));
        let _ = tx.send(());
    });
    recv_transcription_result(rx, timeout)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_env_uses_positive_override() {
        std::env::set_var("MARGINS_TEST_TIMEOUT_SECS", "7");
        assert_eq!(
            timeout_from_env_secs("MARGINS_TEST_TIMEOUT_SECS", 600),
            Duration::from_secs(7)
        );
        std::env::remove_var("MARGINS_TEST_TIMEOUT_SECS");
    }

    #[test]
    fn transcription_timeout_wrapper_returns_timeout_error() {
        let err = run_test_transcription_timeout(Duration::from_millis(10)).unwrap_err();
        assert_eq!(err, "Transcription timed out after 0s");
    }
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
struct OfflineTranscript {
    entries: Vec<margins::asr::TranscriptWordEntry>,
    channel_word_counts: std::collections::BTreeMap<u32, usize>,
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    coreml_timings: Option<margins::coreml_asr::FluidCoreMlTimings>,
}

#[cfg(any(
    all(feature = "coreml-asr", target_os = "macos"),
    feature = "parakeet-asr"
))]
fn transcribe_wav_to_transcript_with_provider(
    asr: &OfflineAsrWorker,
    wav_path: &std::path::Path,
) -> Result<OfflineTranscript, String> {
    let info = audio_info::probe(wav_path).map_err(|e| e.to_string())?;
    let mut entries = Vec::new();
    let mut channel_word_counts = std::collections::BTreeMap::new();
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    let mut coreml_timings = None;

    if info.channels >= 2 {
        for channel in 0..info.channels {
            let mono_16k = margins::audio_pipeline::wav_channel_16k(wav_path, channel as usize)
                .map_err(|e| e.to_string())?;
            let response = asr.transcribe_words(mono_16k)?;
            #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
            {
                coreml_timings = response.coreml_timings.clone();
            }
            let words = response.words;
            channel_word_counts.insert(channel as u32, words.len());
            entries.extend(margins::asr::words_to_transcript_entries(
                &words,
                channel as u32,
                0,
            ));
        }
    } else {
        let mono_16k =
            margins::audio_pipeline::mono_16k_from_wav(wav_path).map_err(|e| e.to_string())?;
        let response = asr.transcribe_words(mono_16k)?;
        #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
        {
            coreml_timings = response.coreml_timings.clone();
        }
        let words = response.words;
        channel_word_counts.insert(0, words.len());
        entries.extend(margins::asr::words_to_transcript_entries(&words, 0, 0));
    }

    Ok(OfflineTranscript {
        entries,
        channel_word_counts,
        #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
        coreml_timings,
    })
}

fn transcript_output_path(name: &str, segment_index: i64) -> String {
    std::env::temp_dir()
        .join(format!("{name}_seg{segment_index}_transcript.json"))
        .to_string_lossy()
        .to_string()
}

fn emit_processing(app: &AppHandle, stage: &str, message: &str, progress: Option<f32>) {
    let _ = app.emit(
        "processing-progress",
        ProcessingEvent::new(stage, message, progress),
    );
}

fn emit_transcript_entry(app: &AppHandle, entry: TranscriptEntry) {
    let _ = app.emit(
        "processing-progress",
        ProcessingEvent::with_entry("transcript", "", None, None, Some(entry), None, None, None),
    );
}
