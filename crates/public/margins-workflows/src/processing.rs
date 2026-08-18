//! File transcription and multipart session processing through public ports.

use crate::alignment::render_aligned_markdown;
use crate::transcript_view::transcript_artifact_path;
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local};
use margins_core::{AsrBackend, AsrRequest, DiarizationBackend, DiarizationRequest, SpeakerId};
use margins_media::audio::{
    downmix_to_mono, extract_channel, load_audio_any, resample_mono_linear, write_interleaved_wav,
    AudioBuffer,
};
use margins_media::transcript::{transcript_json, TranscriptWordEntry};
use margins_store::legacy::{self, SESSION_ARTIFACT_KIND_TRANSCRIPT};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::io::Write;
use std::path::Component;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProcessRequest<'a> {
    pub work_dir: &'a Path,
    pub margins_dir: &'a Path,
    pub session_name: &'a str,
    pub speakers: usize,
    pub align_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessResult {
    pub session_name: String,
    pub segment_count: usize,
    pub speakers: usize,
    pub transcript_entries: usize,
    pub transcript_json: PathBuf,
    pub aligned_path: PathBuf,
    pub asr_backend: String,
}

#[derive(Debug, Clone)]
pub struct TranscribeRequest<'a> {
    pub work_dir: &'a Path,
    pub margins_dir: &'a Path,
    pub audio_path: &'a Path,
    pub requested_name: Option<&'a str>,
    pub memo_path: Option<&'a Path>,
    pub speakers: usize,
    pub started_at: DateTime<Local>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TranscribeResult {
    pub session_name: String,
    pub audio_path: PathBuf,
    pub memo_path: PathBuf,
    pub transcript_json: PathBuf,
    pub transcript_path: PathBuf,
    pub duration_secs: f64,
    pub audio_mode: &'static str,
    pub speakers: usize,
    pub asr_backend: String,
    pub transcript_entries: usize,
}

pub fn process_session(
    request: ProcessRequest<'_>,
    asr: &dyn AsrBackend,
    diarization: Option<&dyn DiarizationBackend>,
) -> Result<ProcessResult> {
    validate_speaker_request(request.speakers, diarization)?;
    validate_session_name(request.session_name)?;
    let meta = legacy::get_session_meta(request.margins_dir, request.session_name)?;
    if meta.segments.is_empty() {
        bail!("Session '{}' has no audio segments.", request.session_name);
    }
    // Validate metadata needed to render the result before invoking providers.
    // A malformed session must not replace an existing transcript JSON.
    let session_start = DateTime::parse_from_rfc3339(&meta.start_time)
        .with_context(|| format!("invalid session start time for '{}'", request.session_name))?
        .with_timezone(&Local);
    let transcript_path = request
        .margins_dir
        .join(format!("{}_transcript.json", request.session_name));
    let mut entries = if request.align_only {
        read_transcript_entries(&transcript_path)?
    } else {
        Vec::new()
    };
    let mut speaker_channels = HashMap::<SpeakerId, u32>::new();
    let mut backends = BTreeSet::new();

    if !request.align_only {
        // Buffer the full result before the first transcript/artifact write.
        // Provider failures therefore leave existing durable output unchanged.
        let mut segments = meta.segments.iter().collect::<Vec<_>>();
        segments.sort_by_key(|segment| segment.segment_index);
        for segment in segments {
            let path = resolve_input_path(request.work_dir, &segment.wav_path);
            let audio = load_audio_any(&path)
                .with_context(|| format!("failed to decode session segment {}", path.display()))?;
            let offset = segment.offset_ms.max(0) as u64;
            let (part, _, part_backends) = transcribe_audio_buffer(
                &audio,
                request.speakers,
                asr,
                diarization,
                &mut speaker_channels,
                offset,
            )?;
            backends.extend(part_backends);
            entries.extend(part);
        }
        entries.sort_by_key(|entry| (entry.start_ms, entry.channel));
        replace_file_atomically(
            &transcript_path,
            serde_json::to_string_pretty(&transcript_json(&entries))?.as_bytes(),
        )?;
    }

    let memo_path = resolve_input_path(request.work_dir, &meta.notes_path);
    let memo = std::fs::read_to_string(memo_path).unwrap_or_default();
    let aligned = render_aligned_markdown(request.session_name, &session_start, &memo, &entries);
    let local_aligned_path = request
        .margins_dir
        .join(format!("{}_aligned.md", request.session_name));
    let aligned_path = crate::archive::aligned_output_path(
        request.work_dir,
        request.session_name,
        &local_aligned_path,
    );
    std::fs::create_dir_all(aligned_path.parent().expect("aligned path has parent"))?;
    replace_file_atomically(&aligned_path, aligned.as_bytes())?;
    let local_registry_path = format!(".margins/{}_aligned.md", request.session_name);
    legacy::upsert_session_artifact(
        request.margins_dir,
        request.session_name,
        SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &crate::archive::aligned_registry_path(
            request.work_dir,
            request.session_name,
            &local_registry_path,
        ),
        "durable",
        None,
    )?;
    Ok(ProcessResult {
        session_name: request.session_name.to_string(),
        segment_count: meta.segments.len(),
        speakers: request.speakers,
        transcript_entries: entries.len(),
        transcript_json: transcript_path,
        aligned_path,
        asr_backend: if request.align_only {
            String::new()
        } else {
            backends.into_iter().collect::<Vec<_>>().join(",")
        },
    })
}

pub fn transcribe_audio(
    request: TranscribeRequest<'_>,
    asr: &dyn AsrBackend,
    diarization: Option<&dyn DiarizationBackend>,
) -> Result<TranscribeResult> {
    validate_speaker_request(request.speakers, diarization)?;
    let source = load_audio_any(request.audio_path)
        .with_context(|| format!("failed to decode {}", request.audio_path.display()))?;
    let mut speaker_channels = HashMap::new();
    // Transcribe before creating the session or any destination file so typed
    // unavailable/model errors cannot leave a partial import.
    let (entries, audio_mode, backends) = transcribe_audio_buffer(
        &source,
        request.speakers,
        asr,
        diarization,
        &mut speaker_channels,
        0,
    )?;
    std::fs::create_dir_all(request.margins_dir)?;
    let stem = request
        .requested_name
        .map(str::to_string)
        .or_else(|| {
            request
                .audio_path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "import".to_string());
    let name = unique_session_name(request.margins_dir, &slugify(&stem))?;
    let memo_rel = format!(".margins/{name}.md");
    let memo_dest = request.margins_dir.join(format!("{name}.md"));
    let audio_rel = format!(".margins/{name}_seg0.wav");
    let audio_dest = request.margins_dir.join(format!("{name}_seg0.wav"));
    let transcript_json_path = request.margins_dir.join(format!("{name}_transcript.json"));
    let local_aligned_path = transcript_artifact_path(request.margins_dir, &name);
    let aligned_path =
        crate::archive::aligned_output_path(request.work_dir, &name, &local_aligned_path);

    let mono = downmix_to_mono(&source)?;
    let mono = resample_mono_linear(&mono, source.sample_rate, 16_000);
    let duration = write_interleaved_wav(&audio_dest, &mono, 16_000, 1)?;
    legacy::create_session(request.margins_dir, &name, &request.started_at, &memo_rel)?;
    legacy::add_segment(request.margins_dir, &name, 0, &audio_rel, 0, Some(duration))?;
    if let Some(path) = request.memo_path {
        std::fs::copy(path, &memo_dest).with_context(|| {
            format!(
                "failed to copy memo {} to {}",
                path.display(),
                memo_dest.display()
            )
        })?;
    } else {
        std::fs::write(
            &memo_dest,
            format!(
                "# {name}\n\nImported from {} on {}.\n",
                request.audio_path.display(),
                request.started_at.format("%Y-%m-%d %H:%M:%S")
            ),
        )?;
    }
    std::fs::write(
        &transcript_json_path,
        serde_json::to_string_pretty(&transcript_json(&entries))?,
    )?;
    std::fs::create_dir_all(aligned_path.parent().expect("artifact path has parent"))?;
    std::fs::write(
        &aligned_path,
        render_imported_transcript(
            &name,
            &request.audio_path.to_string_lossy(),
            &std::fs::read_to_string(&memo_dest).unwrap_or_default(),
            &entries,
        ),
    )?;
    let local_registry_path = format!(".margins/artifacts/{name}/transcript.md");
    legacy::upsert_session_artifact(
        request.margins_dir,
        &name,
        SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &crate::archive::aligned_registry_path(request.work_dir, &name, &local_registry_path),
        "durable",
        None,
    )?;
    Ok(TranscribeResult {
        session_name: name,
        audio_path: audio_dest,
        memo_path: memo_dest,
        transcript_json: transcript_json_path,
        transcript_path: aligned_path,
        duration_secs: duration,
        audio_mode,
        speakers: request.speakers,
        asr_backend: backends.into_iter().collect::<Vec<_>>().join(","),
        transcript_entries: entries.len(),
    })
}

fn transcribe_audio_buffer(
    audio: &AudioBuffer,
    speakers: usize,
    asr: &dyn AsrBackend,
    diarization: Option<&dyn DiarizationBackend>,
    speaker_channels: &mut HashMap<SpeakerId, u32>,
    session_offset_ms: u64,
) -> Result<(Vec<TranscriptWordEntry>, &'static str, BTreeSet<String>)> {
    let mut backends = BTreeSet::new();
    if audio.channels >= 2 && speakers <= 1 {
        let mut entries = Vec::new();
        for channel in 0..audio.channels.min(2) as usize {
            let mono =
                resample_mono_linear(&extract_channel(audio, channel)?, audio.sample_rate, 16_000);
            let result = asr.transcribe(AsrRequest {
                samples: mono,
                sample_rate_hz: 16_000,
                session_offset_ms,
                language: None,
            })?;
            backends.insert(asr.backend_name().to_string());
            entries.extend(result.words.into_iter().map(|word| TranscriptWordEntry {
                channel: channel as u32,
                start_ms: word.start_ms,
                end_ms: word.end_ms,
                text: word.text,
            }));
        }
        entries.sort_by_key(|entry| (entry.start_ms, entry.channel));
        return Ok((entries, "stereo_channels", backends));
    }
    let mono = resample_mono_linear(&downmix_to_mono(audio)?, audio.sample_rate, 16_000);
    let result = asr.transcribe(AsrRequest {
        samples: mono.clone(),
        sample_rate_hz: 16_000,
        session_offset_ms,
        language: None,
    })?;
    backends.insert(asr.backend_name().to_string());
    if speakers <= 1 {
        return Ok((
            result
                .words
                .into_iter()
                .map(|word| TranscriptWordEntry {
                    channel: 0,
                    start_ms: word.start_ms,
                    end_ms: word.end_ms,
                    text: word.text,
                })
                .collect(),
            "mono",
            backends,
        ));
    }
    let diarizer =
        diarization.context("multi-speaker processing requires a diarization backend")?;
    let turns = diarizer.diarize(DiarizationRequest {
        samples: mono,
        sample_rate_hz: 16_000,
        session_offset_ms,
        max_speakers: Some(speakers.try_into().unwrap_or(u16::MAX)),
    })?;
    let mut entries = Vec::new();
    for word in result.words {
        let midpoint = word.start_ms.saturating_add(word.end_ms).saturating_div(2);
        let speaker = turns
            .segments
            .iter()
            .find(|turn| midpoint >= turn.start_ms && midpoint < turn.end_ms)
            .map(|turn| turn.speaker.clone())
            .unwrap_or_else(|| SpeakerId::new("unknown"));
        let next = speaker_channels.len() as u32;
        let channel = *speaker_channels.entry(speaker).or_insert(next);
        entries.push(TranscriptWordEntry {
            channel,
            start_ms: word.start_ms,
            end_ms: word.end_ms,
            text: word.text,
        });
    }
    Ok((entries, "diarized_mono", backends))
}

fn validate_speaker_request(
    speakers: usize,
    diarization: Option<&dyn DiarizationBackend>,
) -> Result<()> {
    if speakers == 0 {
        bail!("speaker count must be at least 1");
    }
    if speakers > 1 && diarization.is_none() {
        bail!("multi-speaker processing requires a diarization backend");
    }
    Ok(())
}

fn validate_session_name(name: &str) -> Result<()> {
    let mut components = Path::new(name).components();
    let single_component = matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(_)), None)
    );
    if name.is_empty() || name.contains(['/', '\\', '\0']) || !single_component {
        bail!("session name must be a single path component");
    }
    Ok(())
}

/// Replace one durable sidecar without exposing a truncated intermediate file.
/// The temporary file lives beside the destination so persistence is a single
/// filesystem rename and replaces, rather than follows, an existing symlink.
fn replace_file_atomically(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("output path has no parent: {}", path.display()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".margins-write-")
        .tempfile_in(parent)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    staged
        .write_all(contents)
        .with_context(|| format!("failed to stage {}", path.display()))?;
    staged
        .as_file()
        .sync_all()
        .with_context(|| format!("failed to sync staged output for {}", path.display()))?;
    staged
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

pub fn read_transcript_entries(path: &Path) -> Result<Vec<TranscriptWordEntry>> {
    let value: Value = serde_json::from_str(
        &std::fs::read_to_string(path)
            .with_context(|| format!("No existing transcript at {}", path.display()))?,
    )?;
    let words = value
        .get("transcripts")
        .and_then(Value::as_array)
        .and_then(|v| v.first())
        .and_then(|v| v.get("words"))
        .cloned()
        .context("transcript JSON is missing transcripts[0].words")?;
    serde_json::from_value(words).context("transcript JSON words have an unsupported shape")
}

fn resolve_input_path(work_dir: &Path, value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_absolute() {
        path
    } else {
        work_dir.join(path)
    }
}

fn unique_session_name(dir: &Path, base: &str) -> Result<String> {
    for ordinal in 1..1_000 {
        let candidate = if ordinal == 1 {
            base.to_string()
        } else {
            format!("{base}-{ordinal}")
        };
        if !legacy::session_exists(dir, &candidate)?
            && !dir.join(format!("{candidate}.md")).exists()
            && !transcript_artifact_path(dir, &candidate).exists()
        {
            return Ok(candidate);
        }
    }
    Ok(format!("{base}-{}", Local::now().timestamp()))
}

fn slugify(value: &str) -> String {
    let mut out = String::new();
    let mut dash = true;
    for c in value.chars().take(80) {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    let out = out.trim_matches('-');
    if out.is_empty() {
        "import".into()
    } else {
        out.into()
    }
}

fn render_imported_transcript(
    name: &str,
    source: &str,
    memo: &str,
    entries: &[TranscriptWordEntry],
) -> String {
    let mut out = format!("# Transcript\n\nSession: `{name}`\nSource: Imported audio file {source}\nTranscript source: `native-cli`\n\n## Timeline\n\n");
    if entries.is_empty() {
        out.push_str("_No transcript entries were produced._\n");
    } else {
        let mut sorted = entries.to_vec();
        sorted.sort_by_key(|e| (e.start_ms, e.channel));
        for entry in sorted {
            out.push_str(&format!(
                "[{}] {}: {}\n",
                format_timestamp(entry.start_ms),
                if entry.channel == 0 {
                    "speaker"
                } else {
                    "channel"
                },
                entry.text.trim()
            ));
        }
    }
    let lines = memo
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect::<Vec<_>>();
    if !lines.is_empty() {
        out.push_str("\n## Memo / context\n\n");
        for line in lines {
            out.push_str("- memo: ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn format_timestamp(ms: u64) -> String {
    let total = ms / 1_000;
    let h = total / 3_600;
    let m = (total % 3_600) / 60;
    let s = total % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}
