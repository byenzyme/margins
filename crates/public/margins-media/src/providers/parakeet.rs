use serde::{Deserialize, Serialize};
use std::path::Path;

pub use crate::transcript::{
    merge_and_dedupe_entries, merge_word_entries_to_phrases, transcript_json,
    words_to_transcript_entries, TranscriptWordEntry, WordTiming,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AsrModelKind {
    /// NVIDIA Parakeet CTC. Best when punctuation is not required; word timing
    /// mode is the natural output.
    Ctc,
    /// NVIDIA Parakeet TDT v3 ONNX. Keeps punctuation/capitalization and uses
    /// the `vocab.txt` model asset directly, without a tokenizer/CLI process.
    Tdt,
}

#[cfg(feature = "parakeet-onnx")]
pub mod parakeet {
    use super::{AsrModelKind, WordTiming};
    use anyhow::{anyhow, bail, Context, Result};
    #[cfg(feature = "parakeet-onnx-dynamic")]
    use libloading::Library;
    use ndarray::{Array1, Array2, Array3};
    use ort::session::builder::GraphOptimizationLevel;
    use ort::session::Session;
    use std::f32::consts::PI;
    #[cfg(feature = "parakeet-onnx-dynamic")]
    use std::ffi::CStr;
    use std::io::{BufRead, BufReader};
    use std::path::{Path, PathBuf};

    /// Rust-native Parakeet TDT ONNX adapter that preserves Margins's critical
    /// word-level timing contract without shelling out to a speech CLI.
    ///
    /// The caller supplies mono 16 kHz f32 PCM; the adapter never opens a
    /// recording device.
    pub struct ParakeetAsr {
        encoder: Session,
        decoder_joint: Session,
        vocab: Vocabulary,
    }

    // Parakeet's ONNX attention overflows on very long inputs, so long audio is
    // windowed into overlapping chunks and stitched back together. 15 s matches
    // the CoreML backend's per-chunk size and sits safely under Parakeet's limit.
    const PARAKEET_CHUNK_SAMPLES: usize = 240_000; // 15 s @ 16 kHz
    const PARAKEET_OVERLAP_SAMPLES: usize = 16_000; // 1 s overlap so boundary words survive
    const SAMPLE_RATE: usize = 16_000;
    const FEATURE_SIZE: usize = 128;
    const HOP_LENGTH: usize = 160;
    const N_FFT: usize = 512;
    const WIN_LENGTH: usize = 400;
    const PREEMPHASIS: f32 = 0.97;
    const ENCODER_STRIDE: usize = 8;

    impl ParakeetAsr {
        pub fn from_dir(model_dir: impl AsRef<Path>, kind: AsrModelKind) -> Result<Self> {
            if kind != AsrModelKind::Tdt {
                bail!("Margins's in-process Windows ASR currently supports Parakeet TDT ONNX only");
            }
            let model_dir = model_dir.as_ref();
            let vocab = Vocabulary::from_file(model_dir.join("vocab.txt"))?;
            let encoder = load_session(&find_encoder(model_dir)?)?;
            let decoder_joint = load_session(&find_decoder_joint(model_dir)?)?;
            Ok(Self {
                encoder,
                decoder_joint,
                vocab,
            })
        }

        /// Transcribe a single (length-bounded) chunk, returning word timings
        /// shifted by `offset_ms` into the full stream.
        fn transcribe_chunk_words(
            &mut self,
            mono_16k: &[f32],
            offset_ms: u64,
        ) -> Result<Vec<WordTiming>> {
            let features = extract_features(mono_16k)?;
            let (encoder_out, _encoder_len) = self.run_encoder(&features)?;
            let tokens = self.greedy_decode(&encoder_out)?;
            Ok(group_tokens_to_words(&decode_tokens(&self.vocab, &tokens))
                .into_iter()
                .filter(|word| !word.text.trim().is_empty())
                .map(|word| WordTiming {
                    start_ms: offset_ms + seconds_to_ms(word.start),
                    end_ms: offset_ms + seconds_to_ms(word.end),
                    text: word.text,
                })
                .collect())
        }

        fn run_encoder(&mut self, features: &Array2<f32>) -> Result<(Array3<f32>, i64)> {
            let time_steps = features.shape()[0];
            let feature_size = features.shape()[1];
            let input = features
                .t()
                .to_shape((1, feature_size, time_steps))
                .context("failed to reshape Parakeet encoder input")?
                .to_owned();
            let input_length = Array1::from_vec(vec![time_steps as i64]);

            let outputs = self.encoder.run(ort::inputs!(
                "audio_signal" => ort::value::Value::from_array(input)?,
                "length" => ort::value::Value::from_array(input_length)?
            ))?;

            let (shape, data) = outputs["outputs"]
                .try_extract_tensor::<f32>()
                .context("failed to extract Parakeet encoder output")?;
            let (_, lens_data) = outputs["encoded_lengths"]
                .try_extract_tensor::<i64>()
                .context("failed to extract Parakeet encoder length")?;
            let dims = shape.as_ref();
            if dims.len() != 3 {
                bail!("expected 3D Parakeet encoder output, got {dims:?}");
            }

            let out = Array3::from_shape_vec(
                (dims[0] as usize, dims[1] as usize, dims[2] as usize),
                data.to_vec(),
            )
            .context("failed to materialize Parakeet encoder output")?;
            Ok((out, lens_data[0]))
        }

        fn greedy_decode(&mut self, encoder_out: &Array3<f32>) -> Result<Vec<TimedTokenId>> {
            let encoder_dim = encoder_out.shape()[1];
            let time_steps = encoder_out.shape()[2];
            let vocab_size = self.vocab.size();
            let blank_id = vocab_size.saturating_sub(1);
            let max_tokens_per_step = 10;

            let mut state_h = Array3::<f32>::zeros((2, 1, 640));
            let mut state_c = Array3::<f32>::zeros((2, 1, 640));
            let mut out = Vec::new();
            let mut t = 0usize;
            let mut emitted_tokens = 0usize;
            let mut last_emitted_token = blank_id as i32;

            while t < time_steps {
                let frame = encoder_out.slice(ndarray::s![0, .., t]).to_owned();
                let frame = frame
                    .to_shape((1, encoder_dim, 1))
                    .context("failed to reshape Parakeet decoder frame")?
                    .to_owned();
                let targets = Array2::from_shape_vec((1, 1), vec![last_emitted_token])
                    .context("failed to create Parakeet target tensor")?;

                let outputs = self.decoder_joint.run(ort::inputs!(
                    "encoder_outputs" => ort::value::Value::from_array(frame)?,
                    "targets" => ort::value::Value::from_array(targets)?,
                    "target_length" => ort::value::Value::from_array(Array1::from_vec(vec![1i32]))?,
                    "input_states_1" => ort::value::Value::from_array(state_h.clone())?,
                    "input_states_2" => ort::value::Value::from_array(state_c.clone())?
                ))?;

                let (_, logits_data) = outputs["outputs"]
                    .try_extract_tensor::<f32>()
                    .context("failed to extract Parakeet decoder logits")?;
                let token_id = logits_data
                    .iter()
                    .take(vocab_size)
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx)
                    .unwrap_or(blank_id);
                let duration_step = logits_data
                    .iter()
                    .skip(vocab_size)
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
                    .map(|(idx, _)| idx)
                    .unwrap_or(0);

                if token_id != blank_id {
                    if let Ok((shape, data)) =
                        outputs["output_states_1"].try_extract_tensor::<f32>()
                    {
                        let dims = shape.as_ref();
                        state_h = Array3::from_shape_vec(
                            (dims[0] as usize, dims[1] as usize, dims[2] as usize),
                            data.to_vec(),
                        )
                        .context("failed to update Parakeet decoder state_h")?;
                    }
                    if let Ok((shape, data)) =
                        outputs["output_states_2"].try_extract_tensor::<f32>()
                    {
                        let dims = shape.as_ref();
                        state_c = Array3::from_shape_vec(
                            (dims[0] as usize, dims[1] as usize, dims[2] as usize),
                            data.to_vec(),
                        )
                        .context("failed to update Parakeet decoder state_c")?;
                    }

                    out.push(TimedTokenId {
                        id: token_id,
                        frame: t,
                    });
                    last_emitted_token = token_id as i32;
                    emitted_tokens += 1;
                }

                if duration_step > 0 {
                    t += duration_step;
                    emitted_tokens = 0;
                } else if token_id == blank_id || emitted_tokens >= max_tokens_per_step {
                    t += 1;
                    emitted_tokens = 0;
                }
            }

            Ok(out)
        }
    }

    impl ParakeetAsr {
        pub fn transcribe_words(&mut self, mono_16k: &[f32]) -> Result<Vec<WordTiming>> {
            if mono_16k.len() <= PARAKEET_CHUNK_SAMPLES {
                return self.transcribe_chunk_words(mono_16k, 0);
            }

            // Overlapping windows; each chunk owns only its "core" so words in a
            // shared overlap region aren't emitted twice (split at the midpoint).
            let hop = PARAKEET_CHUNK_SAMPLES - PARAKEET_OVERLAP_SAMPLES;
            let half_overlap_ms = (PARAKEET_OVERLAP_SAMPLES as u64 * 1000) / (2 * 16_000);
            let mut out: Vec<WordTiming> = Vec::new();
            let mut start = 0usize;
            while start < mono_16k.len() {
                let end = (start + PARAKEET_CHUNK_SAMPLES).min(mono_16k.len());
                let offset_ms = (start as u64 * 1000) / 16_000;
                let chunk_end_ms = (end as u64 * 1000) / 16_000;
                let is_first = start == 0;
                let is_last = end >= mono_16k.len();
                let core_start_ms = if is_first {
                    0
                } else {
                    offset_ms + half_overlap_ms
                };
                let core_end_ms = if is_last {
                    u64::MAX
                } else {
                    chunk_end_ms - half_overlap_ms
                };

                for w in self.transcribe_chunk_words(&mono_16k[start..end], offset_ms)? {
                    let mid = (w.start_ms + w.end_ms) / 2;
                    if mid >= core_start_ms && mid < core_end_ms {
                        out.push(w);
                    }
                }
                if is_last {
                    break;
                }
                start += hop;
            }
            Ok(out)
        }
    }

    fn seconds_to_ms(seconds: f64) -> u64 {
        (seconds.max(0.0) * 1000.0).round() as u64
    }

    fn load_session(path: &Path) -> Result<Session> {
        ensure_dynamic_ort_runtime()?;
        Session::builder()
            .map_err(|e| anyhow!("failed to create ONNX session builder: {e}"))?
            .with_optimization_level(GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("failed to configure ONNX graph optimization: {e}"))?
            .with_intra_threads(4)
            .map_err(|e| anyhow!("failed to configure ONNX intra-op threads: {e}"))?
            .with_inter_threads(1)
            .map_err(|e| anyhow!("failed to configure ONNX inter-op threads: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow!("failed to load ONNX model {}: {e}", path.display()))
    }

    /// `ort` normally loads its dynamic runtime lazily from `Session::builder`.
    /// If that lookup fails, its error construction re-enters ORT's global
    /// `OnceLock` and can wait indefinitely. Validate the explicit runtime
    /// before touching any other ORT API so a bad environment fails normally.
    fn ensure_dynamic_ort_runtime() -> Result<()> {
        #[cfg(feature = "parakeet-onnx-dynamic")]
        {
            let runtime = dynamic_ort_runtime_path()?;
            validate_dynamic_ort_library(&runtime)?;
            // Commit the library before Session::builder reaches ORT's lazy
            // setup path. The first call wins; subsequent calls are harmless.
            ort::init_from(&runtime)
                .map_err(|e| {
                    anyhow!(
                        "{}",
                        dynamic_ort_error(&format!("could not initialize it: {e}"))
                    )
                })?
                .commit();
        }
        Ok(())
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn dynamic_ort_runtime_path() -> Result<PathBuf> {
        dynamic_ort_runtime_path_from_value(
            std::env::var_os("ORT_DYLIB_PATH").filter(|value| !value.is_empty()),
        )
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn dynamic_ort_runtime_path_from_value(value: Option<std::ffi::OsString>) -> Result<PathBuf> {
        let value = value.ok_or_else(|| anyhow!(dynamic_ort_error("is not set")))?;
        let path = PathBuf::from(value);
        let metadata = std::fs::metadata(&path).map_err(|e| {
            anyhow!(
                "{}",
                dynamic_ort_error(&format!("does not name a readable file ({e})"))
            )
        })?;
        if !metadata.is_file() {
            bail!("{}", dynamic_ort_error("does not name a file"));
        }
        Ok(path)
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn validate_dynamic_ort_library(path: &Path) -> Result<()> {
        // This direct dlopen does not enter ORT, so a missing, malformed, or
        // wrong-architecture file cannot poison/re-enter ORT's lazy loader.
        let library = unsafe { Library::new(path) }.map_err(|e| {
            anyhow!(
                "{}",
                dynamic_ort_error(&format!("could not load {} ({e})", path.display()))
            )
        })?;
        let get_api_base: libloading::Symbol<
            unsafe extern "C" fn() -> *const ort::sys::OrtApiBase,
        > = unsafe { library.get(b"OrtGetApiBase") }.map_err(|_| {
            anyhow!(
                "{}",
                dynamic_ort_error(&format!(
                    "{} is not an ONNX Runtime library (missing OrtGetApiBase)",
                    path.display()
                ))
            )
        })?;
        let api_base = unsafe { get_api_base() };
        let version = dynamic_ort_version_from_api_base(api_base)?;
        validate_dynamic_ort_version(&version)
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn dynamic_ort_version_from_api_base(api_base: *const ort::sys::OrtApiBase) -> Result<String> {
        if api_base.is_null() {
            bail!("{}", dynamic_ort_error("returned a null OrtGetApiBase"));
        }
        let get_version = unsafe { (*api_base).GetVersionString };
        let version = unsafe { get_version() };
        if version.is_null() {
            bail!(
                "{}",
                dynamic_ort_error("returned a null ONNX Runtime version string")
            );
        }
        Ok(unsafe { CStr::from_ptr(version) }
            .to_string_lossy()
            .into_owned())
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn validate_dynamic_ort_version(version: &str) -> Result<()> {
        let minor = version
            .split('.')
            .nth(1)
            .and_then(|part| part.parse::<u32>().ok());
        if minor.is_none_or(|minor| minor < ort::MINOR_VERSION) {
            bail!(
                "{}",
                dynamic_ort_error(&format!(
                    "reports version {version}; expected ONNX Runtime 1.{}.x or newer",
                    ort::MINOR_VERSION
                ))
            );
        }
        Ok(())
    }

    #[cfg(feature = "parakeet-onnx-dynamic")]
    fn dynamic_ort_error(problem: &str) -> String {
        format!(
            "ONNX Runtime dynamic loading {problem}. Set ORT_DYLIB_PATH to an absolute path to the official ONNX Runtime 1.{}.x library (for example libonnxruntime.so or libonnxruntime.dylib).",
            ort::MINOR_VERSION
        )
    }

    #[cfg(all(test, feature = "parakeet-onnx-dynamic"))]
    mod dynamic_runtime_tests {
        use super::*;

        #[test]
        fn missing_runtime_path_is_actionable_before_ort_initializes() {
            let error = dynamic_ort_runtime_path_from_value(None)
                .unwrap_err()
                .to_string();
            assert!(error.contains("ORT_DYLIB_PATH"));
            assert!(error.contains("1.24"));
        }

        #[test]
        fn nonexistent_runtime_path_is_actionable_before_ort_initializes() {
            let path = std::env::temp_dir().join("margins-ort-runtime-does-not-exist");
            let error = dynamic_ort_runtime_path_from_value(Some(path.into_os_string()))
                .unwrap_err()
                .to_string();
            assert!(error.contains("ORT_DYLIB_PATH"));
            assert!(error.contains("readable file"));
        }

        #[test]
        fn valid_runtime_initializes_when_provided() {
            let Some(path) = std::env::var_os("ORT_DYLIB_PATH") else {
                eprintln!("skipping: ORT_DYLIB_PATH is not set");
                return;
            };
            let path = PathBuf::from(path);
            validate_dynamic_ort_library(&path).unwrap();
            ensure_dynamic_ort_runtime().unwrap();
        }

        #[test]
        fn too_old_runtime_version_is_actionable() {
            let error = validate_dynamic_ort_version("1.23.0")
                .unwrap_err()
                .to_string();
            assert!(error.contains("1.24"));
            assert!(error.contains("reports version 1.23.0"));
        }

        #[test]
        fn null_api_base_is_actionable() {
            let error = dynamic_ort_version_from_api_base(std::ptr::null())
                .unwrap_err()
                .to_string();
            assert!(error.contains("null OrtGetApiBase"));
        }

        #[cfg(target_os = "macos")]
        #[test]
        fn non_ort_dylib_is_actionable() {
            let error = validate_dynamic_ort_library(Path::new("/usr/lib/libSystem.B.dylib"))
                .unwrap_err()
                .to_string();
            assert!(error.contains("ORT_DYLIB_PATH"));
            assert!(error.contains("missing OrtGetApiBase"));
        }
    }

    fn find_encoder(dir: &Path) -> Result<PathBuf> {
        find_first_existing(
            dir,
            &[
                "encoder-model.int8.onnx",
                "encoder-model.onnx",
                "encoder.onnx",
            ],
            "encoder",
        )
    }

    fn find_decoder_joint(dir: &Path) -> Result<PathBuf> {
        find_first_existing(
            dir,
            &[
                "decoder_joint-model.int8.onnx",
                "decoder_joint-model.onnx",
                "decoder_joint.onnx",
            ],
            "decoder_joint",
        )
    }

    fn find_first_existing(dir: &Path, candidates: &[&str], label: &str) -> Result<PathBuf> {
        candidates
            .iter()
            .map(|candidate| dir.join(candidate))
            .find(|path| path.exists())
            .ok_or_else(|| anyhow!("no Parakeet {label} ONNX model found in {}", dir.display()))
    }

    #[derive(Debug, Clone)]
    struct Vocabulary {
        id_to_token: Vec<String>,
    }

    impl Vocabulary {
        fn from_file(path: impl AsRef<Path>) -> Result<Self> {
            let file = std::fs::File::open(path.as_ref())
                .with_context(|| format!("failed to open {}", path.as_ref().display()))?;
            let mut id_to_token = Vec::new();
            for line in BufReader::new(file).lines() {
                let line = line?;
                let Some((token, id)) = line.split_once(' ') else {
                    continue;
                };
                let id: usize = id.parse().context("invalid Parakeet vocab token id")?;
                if id >= id_to_token.len() {
                    id_to_token.resize(id + 1, String::new());
                }
                id_to_token[id] = token.to_string();
            }
            if id_to_token.is_empty() {
                bail!("Parakeet vocab is empty");
            }
            Ok(Self { id_to_token })
        }

        fn size(&self) -> usize {
            self.id_to_token.len()
        }

        fn token(&self, id: usize) -> Option<&str> {
            self.id_to_token.get(id).map(String::as_str)
        }
    }

    #[derive(Debug, Clone)]
    struct TimedTokenId {
        id: usize,
        frame: usize,
    }

    #[derive(Debug, Clone)]
    struct TimedToken {
        text: String,
        start: f64,
        end: f64,
    }

    fn decode_tokens(vocab: &Vocabulary, tokens: &[TimedTokenId]) -> Vec<TimedToken> {
        let mut out = Vec::new();
        let mut full_text = String::new();
        for (i, token) in tokens.iter().enumerate() {
            let Some(raw) = vocab.token(token.id) else {
                continue;
            };
            if raw.starts_with('<') && raw.ends_with('>') && raw != "<unk>" {
                continue;
            }
            let mut text = raw.replace('▁', " ");
            if !full_text.is_empty()
                && !text.starts_with(' ')
                && text.chars().all(|c| c.is_ascii_digit())
            {
                let trailing_letters = full_text
                    .chars()
                    .rev()
                    .take_while(|c| c.is_alphabetic())
                    .count();
                let last_char = full_text.chars().last();
                let is_article_a = trailing_letters == 1 && last_char == Some('a');
                if trailing_letters > 1 || is_article_a {
                    text.insert(0, ' ');
                }
            }

            full_text.push_str(&text);
            let start =
                token.frame as f64 * ENCODER_STRIDE as f64 * HOP_LENGTH as f64 / SAMPLE_RATE as f64;
            let end_frame = tokens
                .get(i + 1)
                .map(|next| next.frame)
                .unwrap_or(token.frame + 1);
            let end =
                end_frame as f64 * ENCODER_STRIDE as f64 * HOP_LENGTH as f64 / SAMPLE_RATE as f64;
            out.push(TimedToken { text, start, end });
        }
        out
    }

    fn group_tokens_to_words(tokens: &[TimedToken]) -> Vec<TimedToken> {
        let mut words = Vec::new();
        let mut current = String::new();
        let mut current_start = 0.0;
        let mut last_word_lower = String::new();

        for (i, token) in tokens.iter().enumerate() {
            if token.text.trim().is_empty() {
                push_word(
                    &mut words,
                    &mut current,
                    current_start,
                    token.end,
                    &mut last_word_lower,
                );
                continue;
            }

            let stripped = token.text.trim_start_matches('▁').trim_start_matches(' ');
            let is_punctuation =
                !token.text.is_empty() && token.text.chars().all(|c| c.is_ascii_punctuation());
            let starts_word = (token.text.starts_with(' ')
                || token.text.starts_with('▁')
                || is_punctuation
                || i == 0)
                && !stripped.starts_with('\'')
                && !stripped.starts_with('-');

            if starts_word && !current.is_empty() {
                push_word(
                    &mut words,
                    &mut current,
                    current_start,
                    tokens[i - 1].end,
                    &mut last_word_lower,
                );
            }
            if current.is_empty() {
                current_start = token.start;
            }
            current.push_str(stripped);
        }

        if !current.is_empty() {
            let end = tokens.last().map(|t| t.end).unwrap_or(current_start);
            push_word(
                &mut words,
                &mut current,
                current_start,
                end,
                &mut last_word_lower,
            );
        }

        words
    }

    fn push_word(
        words: &mut Vec<TimedToken>,
        current: &mut String,
        start: f64,
        end: f64,
        last_word_lower: &mut String,
    ) {
        if current.is_empty() {
            return;
        }
        let lower = current.to_lowercase();
        if lower != *last_word_lower {
            words.push(TimedToken {
                text: std::mem::take(current),
                start,
                end,
            });
            *last_word_lower = lower;
        } else {
            current.clear();
        }
    }

    fn extract_features(audio: &[f32]) -> Result<Array2<f32>> {
        let audio = apply_preemphasis(audio, PREEMPHASIS);
        let spectrogram = stft(&audio, N_FFT, HOP_LENGTH, WIN_LENGTH)?;
        let mel_filterbank = create_mel_filterbank(N_FFT, FEATURE_SIZE, SAMPLE_RATE);
        let mel_spectrogram = mel_filterbank.dot(&spectrogram);
        let log_zero_guard = 2.0f32.powi(-24);
        let mut features = mel_spectrogram
            .mapv(|x| (x + log_zero_guard).ln())
            .t()
            .to_owned();
        let frames = features.shape()[0];
        let feature_count = features.shape()[1];
        if frames <= 1 {
            return Ok(features);
        }
        for feature in 0..feature_count {
            let mut column = features.column_mut(feature);
            let mean = column.iter().sum::<f32>() / frames as f32;
            let variance =
                column.iter().map(|x| (*x - mean).powi(2)).sum::<f32>() / (frames as f32 - 1.0);
            let std = variance.sqrt() + 1e-5;
            for value in column.iter_mut() {
                *value = (*value - mean) / std;
            }
        }
        Ok(features)
    }

    fn apply_preemphasis(audio: &[f32], coef: f32) -> Vec<f32> {
        if audio.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(audio.len());
        out.push(audio[0]);
        for i in 1..audio.len() {
            out.push(audio[i] - coef * audio[i - 1]);
        }
        out
    }

    fn stft(
        audio: &[f32],
        n_fft: usize,
        hop_length: usize,
        win_length: usize,
    ) -> Result<Array2<f32>> {
        use realfft::RealFftPlanner;

        let pad = n_fft / 2;
        let mut padded = vec![0.0; pad];
        padded.extend_from_slice(audio);
        padded.resize(padded.len() + pad, 0.0);

        let window = hann_window(win_length);
        let frames = (padded.len() - n_fft) / hop_length + 1;
        let bins = n_fft / 2 + 1;
        let mut spectrogram = Array2::<f32>::zeros((bins, frames));
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(n_fft);
        let mut input = vec![0.0; n_fft];
        let mut output = fft.make_output_vec();
        let mut scratch = fft.make_scratch_vec();

        for frame in 0..frames {
            let start = frame * hop_length;
            input.fill(0.0);
            for i in 0..win_length.min(padded.len() - start) {
                input[i] = padded[start + i] * window[i];
            }
            fft.process_with_scratch(&mut input, &mut output, &mut scratch)
                .map_err(|e| anyhow!("Parakeet FFT failed: {e}"))?;
            for bin in 0..bins {
                spectrogram[[bin, frame]] = output[bin].norm_sqr();
            }
        }

        Ok(spectrogram)
    }

    fn hann_window(window_length: usize) -> Vec<f32> {
        (0..window_length)
            .map(|i| 0.5 - 0.5 * ((2.0 * PI * i as f32) / (window_length as f32 - 1.0)).cos())
            .collect()
    }

    const F_SP: f64 = 200.0 / 3.0;
    const MIN_LOG_HZ: f64 = 1000.0;
    const MIN_LOG_MEL: f64 = MIN_LOG_HZ / F_SP;
    const LOG_STEP: f64 = 0.06875177742094912;

    fn hz_to_mel_slaney(hz: f64) -> f64 {
        if hz < MIN_LOG_HZ {
            hz / F_SP
        } else {
            MIN_LOG_MEL + (hz / MIN_LOG_HZ).ln() / LOG_STEP
        }
    }

    fn mel_to_hz_slaney(mel: f64) -> f64 {
        if mel < MIN_LOG_MEL {
            mel * F_SP
        } else {
            MIN_LOG_HZ * ((mel - MIN_LOG_MEL) * LOG_STEP).exp()
        }
    }

    fn create_mel_filterbank(n_fft: usize, n_mels: usize, sample_rate: usize) -> Array2<f32> {
        let bins = n_fft / 2 + 1;
        let mut filterbank = Array2::<f32>::zeros((n_mels, bins));
        let fmax = sample_rate as f64 / 2.0;
        let mel_min = hz_to_mel_slaney(0.0);
        let mel_max = hz_to_mel_slaney(fmax);
        let mel_points: Vec<f64> = (0..=n_mels + 1)
            .map(|i| {
                mel_to_hz_slaney(mel_min + (mel_max - mel_min) * i as f64 / (n_mels + 1) as f64)
            })
            .collect();
        let fft_freqs: Vec<f64> = (0..bins)
            .map(|i| i as f64 * sample_rate as f64 / n_fft as f64)
            .collect();
        let fdiff: Vec<f64> = mel_points.windows(2).map(|w| w[1] - w[0]).collect();

        for mel in 0..n_mels {
            for (bin, &freq) in fft_freqs.iter().enumerate() {
                let lower = (freq - mel_points[mel]) / fdiff[mel];
                let upper = (mel_points[mel + 2] - freq) / fdiff[mel + 1];
                filterbank[[mel, bin]] = 0.0f64.max(lower.min(upper)) as f32;
            }
        }
        for mel in 0..n_mels {
            let enorm = 2.0 / (mel_points[mel + 2] - mel_points[mel]);
            for bin in 0..bins {
                filterbank[[mel, bin]] *= enorm as f32;
            }
        }
        filterbank
    }
}

#[cfg(feature = "parakeet-onnx")]
pub use parakeet::ParakeetAsr;

/// Thread-safe public port adapter around the offline Parakeet ONNX decoder.
#[cfg(feature = "parakeet-onnx")]
pub struct ParakeetOnnxBackend {
    inner: std::sync::Mutex<ParakeetAsr>,
}

#[cfg(feature = "parakeet-onnx")]
impl ParakeetOnnxBackend {
    pub fn from_dir(model_dir: impl AsRef<Path>, kind: AsrModelKind) -> anyhow::Result<Self> {
        Ok(Self {
            inner: std::sync::Mutex::new(ParakeetAsr::from_dir(model_dir, kind)?),
        })
    }
}

#[cfg(feature = "parakeet-onnx")]
impl margins_core::AsrBackend for ParakeetOnnxBackend {
    fn backend_name(&self) -> &'static str {
        "parakeet-onnx"
    }

    fn transcribe(
        &self,
        request: margins_core::AsrRequest,
    ) -> Result<margins_core::AsrResult, margins_core::TranscriptError> {
        if request.sample_rate_hz != 16_000 || request.samples.is_empty() {
            return Err(margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::InvalidAudio,
                message: "Parakeet ONNX requires non-empty mono 16 kHz f32 PCM".into(),
                retryable: false,
            });
        }
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::Internal,
                message: "Parakeet ONNX backend lock was poisoned".into(),
                retryable: false,
            })?;
        let words = inner.transcribe_words(&request.samples).map_err(|error| {
            margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::InferenceFailed,
                message: error.to_string(),
                retryable: false,
            }
        })?;
        Ok(margins_core::AsrResult {
            words: words
                .into_iter()
                .map(|word| margins_core::TranscriptWord {
                    start_ms: word.start_ms.saturating_add(request.session_offset_ms),
                    end_ms: word.end_ms.saturating_add(request.session_offset_ms),
                    text: word.text,
                    speaker: None,
                    confidence_per_mille: None,
                })
                .collect(),
            detected_language: None,
        })
    }
}

#[cfg(not(feature = "parakeet-onnx"))]
pub fn parakeet_feature_error() -> anyhow::Error {
    anyhow::anyhow!("Rust Parakeet ASR is not enabled; build with feature `parakeet-onnx`")
}

pub fn expected_model_files(kind: AsrModelKind) -> &'static [&'static str] {
    match kind {
        AsrModelKind::Ctc => &["model.onnx", "model.onnx_data", "tokenizer.json"],
        // Margins defaults to the int8 TDT layout, but the in-process ONNX
        // backend also accepts the full encoder/decoder filenames.
        AsrModelKind::Tdt => &[
            "encoder-model.int8.onnx",
            "decoder_joint-model.int8.onnx",
            "vocab.txt",
        ],
    }
}

pub fn missing_model_files(model_dir: impl AsRef<Path>, kind: AsrModelKind) -> Vec<String> {
    let model_dir = model_dir.as_ref();
    match kind {
        AsrModelKind::Ctc => expected_model_files(kind)
            .iter()
            .filter(|file| !model_dir.join(file).exists())
            .map(|file| (*file).to_string())
            .collect(),
        AsrModelKind::Tdt => missing_tdt_model_files(model_dir),
    }
}

fn missing_tdt_model_files(model_dir: &Path) -> Vec<String> {
    let mut missing = Vec::new();
    let has_int8_encoder = model_dir.join("encoder-model.int8.onnx").exists();
    let has_full_encoder = model_dir.join("encoder-model.onnx").exists()
        && model_dir.join("encoder-model.onnx.data").exists();
    if !has_int8_encoder && !has_full_encoder {
        missing.push("encoder-model.int8.onnx".to_string());
    }

    let has_decoder = model_dir.join("decoder_joint-model.int8.onnx").exists()
        || model_dir.join("decoder_joint-model.onnx").exists();
    if !has_decoder {
        missing.push("decoder_joint-model.int8.onnx".to_string());
    }

    if !model_dir.join("vocab.txt").exists() {
        missing.push("vocab.txt".to_string());
    }

    missing
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "parakeet-onnx")]
    use std::path::PathBuf;

    #[test]
    fn validates_expected_model_files() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = missing_model_files(tmp.path(), AsrModelKind::Tdt);
        assert!(missing.contains(&"encoder-model.int8.onnx".to_string()));
        assert!(missing.contains(&"decoder_joint-model.int8.onnx".to_string()));
        assert!(missing.contains(&"vocab.txt".to_string()));
    }

    #[test]
    fn accepts_quantized_tdt_model_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("encoder-model.int8.onnx"), b"").unwrap();
        std::fs::write(tmp.path().join("decoder_joint-model.int8.onnx"), b"").unwrap();
        std::fs::write(tmp.path().join("vocab.txt"), b"").unwrap();
        assert!(missing_model_files(tmp.path(), AsrModelKind::Tdt).is_empty());
    }

    #[test]
    fn accepts_full_tdt_model_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("encoder-model.onnx"), b"").unwrap();
        std::fs::write(tmp.path().join("encoder-model.onnx.data"), b"").unwrap();
        std::fs::write(tmp.path().join("decoder_joint-model.onnx"), b"").unwrap();
        std::fs::write(tmp.path().join("vocab.txt"), b"").unwrap();
        assert!(missing_model_files(tmp.path(), AsrModelKind::Tdt).is_empty());
    }

    #[test]
    fn offsets_words_into_entries() {
        let entries = words_to_transcript_entries(
            &[WordTiming {
                start_ms: 10,
                end_ms: 25,
                text: "hello".into(),
            }],
            1,
            100,
        );
        assert_eq!(entries[0].channel, 1);
        assert_eq!(entries[0].start_ms, 110);
        assert_eq!(entries[0].end_ms, 125);
        assert_eq!(entries[0].text, " hello");
    }

    #[test]
    fn merges_word_entries_into_phrases() {
        let entries = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: " hello".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 250,
                end_ms: 400,
                text: " world".into(),
            },
            TranscriptWordEntry {
                channel: 1,
                start_ms: 300,
                end_ms: 500,
                text: " other".into(),
            },
        ];
        let phrases = merge_word_entries_to_phrases(entries, 500);
        assert_eq!(phrases.len(), 2);
        assert_eq!(phrases[0].text, " hello world");
        assert_eq!(phrases[1].channel, 1);
    }

    #[test]
    fn merge_and_dedupe_collapses_filler_twice() {
        let entries = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: " yeah".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 120,
                end_ms: 220,
                text: " yeah,".into(),
            },
        ];

        let phrases = merge_and_dedupe_entries(entries, 2_000);
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0].text, " yeah");
        assert_eq!(phrases[0].end_ms, 220);
    }

    #[test]
    fn merge_and_dedupe_preserves_non_filler_twice() {
        let entries = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: " very".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 120,
                end_ms: 220,
                text: " very".into(),
            },
        ];

        let phrases = merge_and_dedupe_entries(entries, 2_000);
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0].text, " very very");
        assert_eq!(phrases[0].end_ms, 220);
    }

    #[test]
    fn merge_and_dedupe_collapses_non_filler_three_times() {
        let entries = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: " very".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 120,
                end_ms: 220,
                text: " very".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 240,
                end_ms: 340,
                text: " very".into(),
            },
        ];

        let phrases = merge_and_dedupe_entries(entries, 2_000);
        assert_eq!(phrases.len(), 1);
        assert_eq!(phrases[0].text, " very");
        assert_eq!(phrases[0].end_ms, 340);
    }

    #[test]
    fn merge_and_dedupe_does_not_collapse_cross_channel() {
        let entries = vec![
            TranscriptWordEntry {
                channel: 0,
                start_ms: 0,
                end_ms: 100,
                text: " yeah".into(),
            },
            TranscriptWordEntry {
                channel: 1,
                start_ms: 120,
                end_ms: 220,
                text: " yeah".into(),
            },
            TranscriptWordEntry {
                channel: 0,
                start_ms: 240,
                end_ms: 340,
                text: " yeah".into(),
            },
        ];

        let phrases = merge_and_dedupe_entries(entries, 2_000);
        assert_eq!(phrases.len(), 3);
        assert_eq!(phrases[0].channel, 0);
        assert_eq!(phrases[1].channel, 1);
        assert_eq!(phrases[2].channel, 0);
    }

    #[cfg(feature = "parakeet-onnx")]
    #[test]
    #[ignore = "requires local Parakeet TDT ONNX assets and MARGINS_PARAKEET_SMOKE_WAV"]
    fn smoke_transcribes_wav_with_parakeet_onnx() {
        let model_dir = std::env::var_os("MARGINS_PARAKEET_MODEL_DIR")
            .map(PathBuf::from)
            .expect("MARGINS_PARAKEET_MODEL_DIR must point to Parakeet ONNX assets");
        let wav = std::env::var_os("MARGINS_PARAKEET_SMOKE_WAV")
            .map(PathBuf::from)
            .expect("MARGINS_PARAKEET_SMOKE_WAV must point to a WAV fixture");
        let mono_16k = crate::audio::mono_16k_from_wav(&wav).unwrap();
        let mut asr = parakeet::ParakeetAsr::from_dir(&model_dir, AsrModelKind::Tdt).unwrap();
        let words = asr.transcribe_words(&mono_16k).unwrap();
        eprintln!(
            "Parakeet ONNX wav smoke: {} words: {:?}",
            words.len(),
            words
        );
    }
}
