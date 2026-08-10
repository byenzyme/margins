//! macOS-native CoreML scaffold for FluidAudio Parakeet ASR assets.
//!
//! This module intentionally avoids Swift and Python. It loads FluidAudio's
//! compiled `.mlmodelc` bundles directly through `objc2-core-ml` and runs the
//! preprocessor, encoder, and the FluidAudio v2 TDT decoder loop.

use crate::providers::parakeet::{
    merge_word_entries_to_phrases, words_to_transcript_entries, TranscriptWordEntry, WordTiming,
};
use anyhow::{anyhow, bail, Context, Result};
use block2::RcBlock;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::AnyThread;
use objc2_core_ml::{
    MLComputeUnits, MLDictionaryFeatureProvider, MLFeatureProvider, MLFeatureValue, MLModel,
    MLModelConfiguration, MLMultiArray, MLMultiArrayDataType,
};
use objc2_foundation::{NSArray, NSCopying, NSMutableDictionary, NSNumber, NSString, NSURL};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::ptr::NonNull;
use std::slice;
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 16_000;
pub const MAX_MODEL_SAMPLES: usize = 240_000;
pub const SAMPLES_PER_ENCODER_FRAME: usize = 1_280;
const MS_PER_ENCODER_FRAME: u64 = 80;
const MEL_HOP_SAMPLES: usize = 160;
const MEL_CONTEXT_SAMPLES: usize = SAMPLES_PER_ENCODER_FRAME;
const OFFLINE_OVERLAP_SAMPLES: usize = 32_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FluidCoreMlModelVersion {
    V2,
    V3,
}

impl FluidCoreMlModelVersion {
    pub fn from_env_or_dir(model_dir: &Path) -> Self {
        match std::env::var("MARGINS_FLUID_COREML_VERSION")
            .ok()
            .map(|v| v.to_ascii_lowercase())
            .as_deref()
        {
            Some("v3") | Some("3") => Self::V3,
            Some("v2") | Some("2") => Self::V2,
            _ if model_dir.to_string_lossy().contains("v3") => Self::V3,
            _ => Self::V2,
        }
    }

    fn joint_candidates(self) -> &'static [&'static str] {
        match self {
            Self::V2 => &["JointDecision.mlmodelc"],
            Self::V3 => &["JointDecisionv3.mlmodelc", "JointDecision.mlmodelc"],
        }
    }
}

#[derive(Debug, Clone)]
pub struct FluidCoreMlTimings {
    pub model_load: Duration,
    pub last_preprocess: Duration,
    pub last_encoder: Duration,
    pub last_frontend_total: Duration,
}

impl Default for FluidCoreMlTimings {
    fn default() -> Self {
        Self {
            model_load: Duration::ZERO,
            last_preprocess: Duration::ZERO,
            last_encoder: Duration::ZERO,
            last_frontend_total: Duration::ZERO,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FluidCoreMlFrontendOutput {
    pub mel_shape: Vec<usize>,
    pub mel_length: usize,
    pub encoder_shape: Vec<usize>,
    pub encoder_length: usize,
    pub actual_samples: usize,
    pub padded_samples: usize,
    pub timings: FluidCoreMlTimings,
}

#[derive(Debug, Clone)]
pub struct FluidCoreMlWarmupOutput {
    pub frontend_ms: u128,
    pub decode_ms: u128,
    pub total_ms: u128,
    pub encoder_length: usize,
    pub actual_audio_frames: usize,
}

#[derive(Debug, Clone)]
pub struct FluidCoreMlBundle {
    pub root: PathBuf,
    pub preprocessor: PathBuf,
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub joint: PathBuf,
    pub vocabulary: PathBuf,
    pub version: FluidCoreMlModelVersion,
}

impl FluidCoreMlBundle {
    pub fn from_dir(model_dir: impl AsRef<Path>, version: FluidCoreMlModelVersion) -> Result<Self> {
        let root = resolve_model_root(model_dir.as_ref(), version)?;
        let preprocessor = root.join("Preprocessor.mlmodelc");
        let encoder = root.join("Encoder.mlmodelc");
        let decoder = root.join("Decoder.mlmodelc");
        let vocabulary = root.join("parakeet_vocab.json");
        let joint = version
            .joint_candidates()
            .iter()
            .map(|name| root.join(name))
            .find(|path| path.exists())
            .unwrap_or_else(|| root.join(version.joint_candidates()[0]));

        let bundle = Self {
            root,
            preprocessor,
            encoder,
            decoder,
            joint,
            vocabulary,
            version,
        };
        let missing = bundle.missing_files();
        if !missing.is_empty() {
            bail!(
                "FluidAudio CoreML model directory is missing: {}",
                missing.join(", ")
            );
        }
        Ok(bundle)
    }

    pub fn missing_files(&self) -> Vec<String> {
        [
            ("Preprocessor.mlmodelc", &self.preprocessor),
            ("Encoder.mlmodelc", &self.encoder),
            ("Decoder.mlmodelc", &self.decoder),
            ("JointDecision(.v3).mlmodelc", &self.joint),
            ("parakeet_vocab.json", &self.vocabulary),
        ]
        .into_iter()
        .filter(|(_, path)| !path.exists())
        .map(|(name, _)| name.to_string())
        .collect()
    }
}

/// Direct CoreML Parakeet scaffold using FluidAudio model bundles.
pub struct FluidCoreMlAsr {
    bundle: FluidCoreMlBundle,
    preprocessor: CoreMlModel,
    encoder: CoreMlModel,
    decoder: CoreMlModel,
    joint: CoreMlModel,
    vocabulary: Vec<Option<String>>,
    timings: FluidCoreMlTimings,
    frontend_audio_scratch: Vec<f32>,
}

impl std::fmt::Debug for FluidCoreMlAsr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FluidCoreMlAsr")
            .field("bundle", &self.bundle)
            .field("vocabulary_size", &self.vocabulary.len())
            .field("timings", &self.timings)
            .finish_non_exhaustive()
    }
}

impl FluidCoreMlAsr {
    pub fn from_dir(model_dir: impl AsRef<Path>, version: FluidCoreMlModelVersion) -> Result<Self> {
        let started = Instant::now();
        let bundle = FluidCoreMlBundle::from_dir(model_dir, version)?;
        let vocabulary = load_vocabulary(&bundle.vocabulary)?;
        let preprocessor = CoreMlModel::load(&bundle.preprocessor, MLComputeUnits::CPUOnly)
            .with_context(|| format!("failed to load {}", bundle.preprocessor.display()))?;
        let encoder = CoreMlModel::load(&bundle.encoder, MLComputeUnits::CPUAndNeuralEngine)
            .with_context(|| format!("failed to load {}", bundle.encoder.display()))?;
        let decoder = CoreMlModel::load(&bundle.decoder, MLComputeUnits::CPUAndNeuralEngine)
            .with_context(|| format!("failed to load {}", bundle.decoder.display()))?;
        let joint = CoreMlModel::load(&bundle.joint, MLComputeUnits::CPUAndNeuralEngine)
            .with_context(|| format!("failed to load {}", bundle.joint.display()))?;
        Ok(Self {
            bundle,
            preprocessor,
            encoder,
            decoder,
            joint,
            vocabulary,
            timings: FluidCoreMlTimings {
                model_load: started.elapsed(),
                ..Default::default()
            },
            frontend_audio_scratch: Vec::with_capacity(MAX_MODEL_SAMPLES),
        })
    }

    pub fn from_dir_auto(model_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = model_dir.as_ref();
        let version = FluidCoreMlModelVersion::from_env_or_dir(dir);
        Self::from_dir(dir, version)
    }

    pub fn bundle(&self) -> &FluidCoreMlBundle {
        &self.bundle
    }

    pub fn timings(&self) -> &FluidCoreMlTimings {
        &self.timings
    }

    /// Run FluidAudio's CoreML preprocessor and encoder on one mono 16 kHz chunk.
    pub fn run_frontend(&mut self, mono_16k: &[f32]) -> Result<FluidCoreMlFrontendOutput> {
        if mono_16k.is_empty() {
            bail!("cannot run CoreML frontend on empty audio");
        }
        let started = Instant::now();
        let actual_len = prepare_frontend_audio(mono_16k, &mut self.frontend_audio_scratch);
        let padded = &self.frontend_audio_scratch;
        let actual_length = [actual_len as i32];

        let pre_started = Instant::now();
        let pre = self.preprocessor.predict(
            &[
                CoreMlInput::F32 {
                    name: "audio_signal",
                    values: padded,
                    shape: &[1, padded.len()],
                },
                CoreMlInput::I32 {
                    name: "audio_length",
                    values: &actual_length,
                    shape: &[1],
                },
            ],
            &["mel", "mel_length"],
        )?;
        self.timings.last_preprocess = pre_started.elapsed();

        let mel = pre
            .get("mel")
            .ok_or_else(|| anyhow!("preprocessor output `mel` missing"))?;
        let mel_length = pre
            .get("mel_length")
            .and_then(|t| t.data.first())
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as usize;
        let mel_len_i32 = [mel_length as i32];

        let enc_started = Instant::now();
        let enc = self.encoder.predict(
            &[
                CoreMlInput::F32 {
                    name: "mel",
                    values: &mel.data,
                    shape: &mel.shape,
                },
                CoreMlInput::I32 {
                    name: "mel_length",
                    values: &mel_len_i32,
                    shape: &[1],
                },
            ],
            &["encoder", "encoder_length"],
        )?;
        self.timings.last_encoder = enc_started.elapsed();
        self.timings.last_frontend_total = started.elapsed();

        let encoder = enc
            .get("encoder")
            .ok_or_else(|| anyhow!("encoder output `encoder` missing"))?;
        let encoder_length = enc
            .get("encoder_length")
            .and_then(|t| t.data.first())
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as usize;

        Ok(FluidCoreMlFrontendOutput {
            mel_shape: mel.shape.clone(),
            mel_length,
            encoder_shape: encoder.shape.clone(),
            encoder_length,
            actual_samples: actual_len,
            padded_samples: padded.len(),
            timings: self.timings.clone(),
        })
    }

    pub fn warmup_models(&mut self) -> Result<FluidCoreMlWarmupOutput> {
        let started = Instant::now();
        let warmup_audio = vec![0.0f32; SAMPLE_RATE as usize / 10];

        let frontend_started = Instant::now();
        let frontend = self.run_frontend_tensors(&warmup_audio)?;
        let frontend_ms = frontend_started.elapsed().as_millis();

        let decode_started = Instant::now();
        let mut state = TdtDecoderStateRust::new();
        let _ = self.decode_chunk(
            &frontend.encoder,
            frontend.encoder_length,
            frontend.actual_audio_frames,
            &mut state,
            DecodeChunkOptions::default(),
        )?;
        let decode_ms = decode_started.elapsed().as_millis();

        Ok(FluidCoreMlWarmupOutput {
            frontend_ms,
            decode_ms,
            total_ms: started.elapsed().as_millis(),
            encoder_length: frontend.encoder_length,
            actual_audio_frames: frontend.actual_audio_frames,
        })
    }

    fn run_frontend_tensors(&mut self, mono_16k: &[f32]) -> Result<FrontendTensors> {
        if mono_16k.is_empty() {
            bail!("cannot run CoreML frontend on empty audio");
        }
        let started = Instant::now();
        let actual_len = prepare_frontend_audio(mono_16k, &mut self.frontend_audio_scratch);
        let padded = &self.frontend_audio_scratch;
        let actual_length = [actual_len as i32];

        let pre_started = Instant::now();
        let pre = self.preprocessor.predict(
            &[
                CoreMlInput::F32 {
                    name: "audio_signal",
                    values: padded,
                    shape: &[1, padded.len()],
                },
                CoreMlInput::I32 {
                    name: "audio_length",
                    values: &actual_length,
                    shape: &[1],
                },
            ],
            &["mel", "mel_length"],
        )?;
        self.timings.last_preprocess = pre_started.elapsed();

        let mel = pre
            .get("mel")
            .ok_or_else(|| anyhow!("preprocessor output `mel` missing"))?;
        let mel_length = pre
            .get("mel_length")
            .and_then(|t| t.data.first())
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as usize;
        let mel_len_i32 = [mel_length as i32];

        let enc_started = Instant::now();
        let enc = self.encoder.predict(
            &[
                CoreMlInput::F32 {
                    name: "mel",
                    values: &mel.data,
                    shape: &mel.shape,
                },
                CoreMlInput::I32 {
                    name: "mel_length",
                    values: &mel_len_i32,
                    shape: &[1],
                },
            ],
            &["encoder", "encoder_length"],
        )?;
        self.timings.last_encoder = enc_started.elapsed();
        self.timings.last_frontend_total = started.elapsed();

        let encoder = enc
            .get("encoder")
            .ok_or_else(|| anyhow!("encoder output `encoder` missing"))?
            .clone();
        let encoder_length = enc
            .get("encoder_length")
            .and_then(|t| t.data.first())
            .copied()
            .unwrap_or(0.0)
            .max(0.0) as usize;

        Ok(FrontendTensors {
            encoder,
            encoder_length,
            actual_audio_frames: calculate_encoder_frames(mono_16k.len().min(MAX_MODEL_SAMPLES)),
        })
    }

    fn transcribe_long_form_token_windows(&mut self, mono_16k: &[f32]) -> Result<Vec<TokenWindow>> {
        let mut merged = Vec::new();
        let mut chunk_start = 0usize;
        while chunk_start < mono_16k.len() {
            debug_assert_eq!(chunk_start % SAMPLES_PER_ENCODER_FRAME, 0);
            let chunk_end = (chunk_start + offline_chunk_samples()).min(mono_16k.len());
            if chunk_end <= chunk_start {
                break;
            }

            let context_samples = if chunk_start == 0 {
                0
            } else {
                MEL_CONTEXT_SAMPLES.min(chunk_start)
            };
            let context_start = chunk_start - context_samples;
            let chunk_with_context = &mono_16k[context_start..chunk_end];
            let frontend = self.run_frontend_tensors(chunk_with_context)?;
            let actual_audio_frames = calculate_encoder_frames(chunk_end - chunk_start);
            let mut state = TdtDecoderStateRust::new();
            let hyp = self.decode_chunk(
                &frontend.encoder,
                frontend.encoder_length,
                actual_audio_frames,
                &mut state,
                DecodeChunkOptions {
                    is_last_chunk: chunk_end >= mono_16k.len(),
                    global_frame_offset: chunk_start / SAMPLES_PER_ENCODER_FRAME,
                    context_frame_adjustment: context_samples / SAMPLES_PER_ENCODER_FRAME,
                    ..Default::default()
                },
            )?;
            let chunk_tokens = hypothesis_token_windows(&hyp);
            merged = merge_token_windows(&merged, &chunk_tokens, offline_overlap_frames());

            if chunk_end >= mono_16k.len() {
                break;
            }
            chunk_start = chunk_start.saturating_add(offline_chunk_stride_samples());
        }
        Ok(merged)
    }

    fn decode_chunk(
        &mut self,
        encoder: &CoreMlTensor,
        encoder_sequence_length: usize,
        actual_audio_frames: usize,
        state: &mut TdtDecoderStateRust,
        options: DecodeChunkOptions,
    ) -> Result<TdtHypothesisRust> {
        if self.bundle.version != FluidCoreMlModelVersion::V2 {
            bail!(
                "FluidAudio CoreML {:?} decoder is not ported in Rust yet; v2 is supported",
                self.bundle.version
            );
        }
        if encoder_sequence_length <= 1 {
            return Ok(TdtHypothesisRust::default());
        }

        let frames = EncoderFrames::new(encoder, encoder_sequence_length, ENCODER_HIDDEN_SIZE)?;
        let effective_sequence_length = encoder_sequence_length
            .min(actual_audio_frames)
            .min(frames.count);
        if effective_sequence_length == 0 {
            return Ok(TdtHypothesisRust::default());
        }

        if state.last_token.is_none() && state.predictor_output.is_none() {
            state.reset_arrays();
        }
        if state.predictor_output.is_none() && state.last_token.is_none() {
            let primed = self.run_decoder(BLANK_ID_V2, state)?;
            state.h = primed.h_out.clone();
            state.c = primed.c_out.clone();
            state.predictor_output = Some(primed.decoder);
        }

        let mut hyp = TdtHypothesisRust {
            last_token: state.last_token,
            ..Default::default()
        };
        let mut time_index = options
            .initial_time_index_override
            .unwrap_or(options.context_frame_adjustment);
        let last_timestep = effective_sequence_length - 1;
        let mut safe_time_index = 0usize;
        let mut active = time_index < effective_sequence_length;
        let mut time_index_current_label: usize;
        let mut last_emission_timestamp: Option<usize> = None;
        let mut emissions_at_this_timestamp = 0usize;
        let mut tokens_processed = 0usize;
        let mut encoder_step = vec![0.0f32; ENCODER_HIDDEN_SIZE];
        let mut decoder_step = vec![0.0f32; DECODER_HIDDEN_SIZE];

        while active {
            let label_for_decoder = hyp.last_token.unwrap_or(BLANK_ID_V2);
            let decoder_result = if let Some(cached) = state.predictor_output.clone() {
                DecoderStepOutput {
                    decoder: cached,
                    h_out: state.h.clone(),
                    c_out: state.c.clone(),
                }
            } else {
                self.run_decoder(label_for_decoder, state)?
            };
            normalize_decoder_projection_into(&decoder_result.decoder, &mut decoder_step)?;
            frames.copy_frame_into(safe_time_index, &mut encoder_step);
            let mut decision = self.run_joint(&encoder_step, &decoder_step)?;
            decision.probability = clamp_probability(decision.probability);

            let mut label = decision.token;
            let mut score = decision.probability;
            let mut duration = map_duration_bin(decision.duration_bin)?;
            let mut blank = label == BLANK_ID_V2;
            let current_time_index = time_index;
            if !blank
                && duration == 0
                && last_emission_timestamp == Some(current_time_index)
                && emissions_at_this_timestamp >= 1
            {
                duration = 1;
            }
            if blank && duration == 0 {
                duration = 1;
            }

            time_index_current_label = time_index;
            time_index = time_index.saturating_add(duration);
            safe_time_index = time_index.min(last_timestep);
            active = time_index < effective_sequence_length;
            let mut advance = active && blank;

            while advance {
                time_index_current_label = time_index;
                frames.copy_frame_into(safe_time_index, &mut encoder_step);
                let mut inner = self.run_joint(&encoder_step, &decoder_step)?;
                inner.probability = clamp_probability(inner.probability);
                label = inner.token;
                score = inner.probability;
                duration = map_duration_bin(inner.duration_bin)?;
                blank = label == BLANK_ID_V2;
                if blank && duration == 0 {
                    duration = 1;
                }
                time_index = time_index.saturating_add(duration);
                safe_time_index = time_index.min(last_timestep);
                active = time_index < effective_sequence_length;
                advance = active && blank;
            }

            if active && label != BLANK_ID_V2 {
                tokens_processed += 1;
                if tokens_processed > MAX_TOKENS_PER_CHUNK {
                    break;
                }
                let emission_timestamp = time_index_current_label + options.global_frame_offset;
                if should_emit_token(emission_timestamp, options.emit_tokens_after_global_frame) {
                    hyp.tokens.push(label);
                    hyp.timestamps.push(emission_timestamp);
                    hyp.confidences.push(score);
                    hyp.durations.push(duration);
                }
                hyp.last_token = Some(label);

                let step = self.run_decoder(
                    label,
                    &TdtDecoderStateRust {
                        h: decoder_result.h_out,
                        c: decoder_result.c_out,
                        last_token: state.last_token,
                        predictor_output: None,
                    },
                )?;
                state.h = step.h_out.clone();
                state.c = step.c_out.clone();
                state.predictor_output = Some(step.decoder);

                if last_emission_timestamp == Some(time_index_current_label) {
                    emissions_at_this_timestamp += 1;
                } else {
                    last_emission_timestamp = Some(time_index_current_label);
                    emissions_at_this_timestamp = 1;
                }
                if emissions_at_this_timestamp >= MAX_SYMBOLS_PER_STEP {
                    time_index = (time_index + 1).min(last_timestep);
                    safe_time_index = time_index.min(last_timestep);
                    emissions_at_this_timestamp = 0;
                    last_emission_timestamp = None;
                }
            }
            active = time_index < effective_sequence_length;
        }

        if options.is_last_chunk {
            let mut additional_steps = 0usize;
            let mut consecutive_blanks = 0usize;
            let mut last_token = hyp.last_token.unwrap_or(BLANK_ID_V2);
            let mut final_time_index = time_index;
            while additional_steps < MAX_SYMBOLS_PER_STEP
                && consecutive_blanks < CONSECUTIVE_BLANK_LIMIT
            {
                let decoder_result = if let Some(cached) = state.predictor_output.clone() {
                    DecoderStepOutput {
                        decoder: cached,
                        h_out: state.h.clone(),
                        c_out: state.c.clone(),
                    }
                } else {
                    self.run_decoder(last_token, state)?
                };
                normalize_decoder_projection_into(&decoder_result.decoder, &mut decoder_step)?;
                let choices = [
                    final_time_index.min(frames.count - 1),
                    effective_sequence_length
                        .saturating_sub(1)
                        .min(frames.count - 1),
                    effective_sequence_length
                        .saturating_sub(2)
                        .min(frames.count - 1),
                ];
                let frame_index = choices[additional_steps % choices.len()];
                frames.copy_frame_into(frame_index, &mut encoder_step);
                let mut decision = self.run_joint(&encoder_step, &decoder_step)?;
                decision.probability = clamp_probability(decision.probability);
                let duration = map_duration_bin(decision.duration_bin)?;
                if decision.token == BLANK_ID_V2 {
                    consecutive_blanks += 1;
                } else {
                    consecutive_blanks = 0;
                    let final_timestamp = final_time_index.min(effective_sequence_length - 1)
                        + options.global_frame_offset;
                    if should_emit_token(final_timestamp, options.emit_tokens_after_global_frame) {
                        hyp.tokens.push(decision.token);
                        hyp.timestamps.push(final_timestamp);
                        hyp.confidences.push(decision.probability);
                        hyp.durations.push(duration);
                    }
                    hyp.last_token = Some(decision.token);
                    let step = self.run_decoder(
                        decision.token,
                        &TdtDecoderStateRust {
                            h: decoder_result.h_out,
                            c: decoder_result.c_out,
                            last_token: state.last_token,
                            predictor_output: None,
                        },
                    )?;
                    state.h = step.h_out.clone();
                    state.c = step.c_out.clone();
                    state.predictor_output = Some(step.decoder);
                    last_token = decision.token;
                }
                final_time_index =
                    (final_time_index + duration.max(1)).min(effective_sequence_length);
                additional_steps += 1;
            }
            state.predictor_output = None;
        }

        state.last_token = hyp.last_token;
        if let Some(last) = hyp.last_token {
            if PUNCTUATION_TOKENS.contains(&last) {
                state.predictor_output = None;
            }
        }

        Ok(hyp)
    }

    fn run_decoder(&self, token: usize, state: &TdtDecoderStateRust) -> Result<DecoderStepOutput> {
        let target = [token as i32];
        let target_length = [1i32];
        let out = self.decoder.predict(
            &[
                CoreMlInput::I32 {
                    name: "targets",
                    values: &target,
                    shape: &[1, 1],
                },
                CoreMlInput::I32 {
                    name: "target_length",
                    values: &target_length,
                    shape: &[1],
                },
                CoreMlInput::F32 {
                    name: "h_in",
                    values: &state.h,
                    shape: &[DECODER_LAYERS, 1, DECODER_HIDDEN_SIZE],
                },
                CoreMlInput::F32 {
                    name: "c_in",
                    values: &state.c,
                    shape: &[DECODER_LAYERS, 1, DECODER_HIDDEN_SIZE],
                },
            ],
            &["decoder", "h_out", "c_out"],
        )?;
        Ok(DecoderStepOutput {
            decoder: out
                .get("decoder")
                .ok_or_else(|| anyhow!("decoder output missing"))?
                .clone(),
            h_out: out
                .get("h_out")
                .ok_or_else(|| anyhow!("decoder h_out missing"))?
                .data
                .clone(),
            c_out: out
                .get("c_out")
                .ok_or_else(|| anyhow!("decoder c_out missing"))?
                .data
                .clone(),
        })
    }

    fn run_joint(
        &self,
        encoder_step: &[f32],
        decoder_step: &[f32],
    ) -> Result<TdtJointDecisionRust> {
        let out = self.joint.predict(
            &[
                CoreMlInput::F32 {
                    name: "encoder_step",
                    values: encoder_step,
                    shape: &[1, ENCODER_HIDDEN_SIZE, 1],
                },
                CoreMlInput::F32 {
                    name: "decoder_step",
                    values: decoder_step,
                    shape: &[1, DECODER_HIDDEN_SIZE, 1],
                },
            ],
            &["token_id", "token_prob", "duration"],
        )?;
        Ok(TdtJointDecisionRust {
            token: tensor_scalar(&out, "token_id")? as usize,
            probability: tensor_scalar(&out, "token_prob")?,
            duration_bin: tensor_scalar(&out, "duration")? as usize,
        })
    }
}

impl FluidCoreMlAsr {
    pub fn transcribe_words(&mut self, mono_16k: &[f32]) -> Result<Vec<WordTiming>> {
        if mono_16k.is_empty() {
            return Ok(Vec::new());
        }
        if is_digital_silence(mono_16k) {
            return Ok(Vec::new());
        }
        if self.bundle.version != FluidCoreMlModelVersion::V2 {
            bail!(
                "FluidAudio CoreML {:?} decoder is not ported in Rust yet; v2 is supported",
                self.bundle.version
            );
        }

        let token_windows = if mono_16k.len() <= MAX_MODEL_SAMPLES {
            let mut state = TdtDecoderStateRust::new();
            let frontend = self.run_frontend_tensors(mono_16k)?;
            let hyp = self.decode_chunk(
                &frontend.encoder,
                frontend.encoder_length,
                frontend.actual_audio_frames,
                &mut state,
                DecodeChunkOptions {
                    is_last_chunk: true,
                    ..Default::default()
                },
            )?;
            hypothesis_token_windows(&hyp)
        } else {
            self.transcribe_long_form_token_windows(mono_16k)?
        };

        Ok(token_windows_to_word_timings(
            &token_windows,
            &self.vocabulary,
        ))
    }
}

/// Public `margins-core` port adapter for offline CoreML inference.
///
/// The adapter stores only an explicit model bundle path, making the service
/// `Send + Sync`; each request creates a CoreML model instance on the calling
/// thread and consumes only the supplied PCM.
#[derive(Debug, Clone)]
pub struct CoreMlAsrBackend {
    model_dir: PathBuf,
    version: FluidCoreMlModelVersion,
}

impl CoreMlAsrBackend {
    pub fn from_dir(model_dir: impl AsRef<Path>, version: FluidCoreMlModelVersion) -> Result<Self> {
        let bundle = FluidCoreMlBundle::from_dir(model_dir, version)?;
        Ok(Self {
            model_dir: bundle.root,
            version,
        })
    }

    pub fn from_dir_auto(model_dir: impl AsRef<Path>) -> Result<Self> {
        let model_dir = model_dir.as_ref();
        Self::from_dir(
            model_dir,
            FluidCoreMlModelVersion::from_env_or_dir(model_dir),
        )
    }
}

impl margins_core::AsrBackend for CoreMlAsrBackend {
    fn backend_name(&self) -> &'static str {
        "coreml"
    }

    fn transcribe(
        &self,
        request: margins_core::AsrRequest,
    ) -> std::result::Result<margins_core::AsrResult, margins_core::TranscriptError> {
        if request.sample_rate_hz != SAMPLE_RATE || request.samples.is_empty() {
            return Err(margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::InvalidAudio,
                message: "CoreML ASR requires non-empty mono 16 kHz f32 PCM".into(),
                retryable: false,
            });
        }
        let mut backend =
            FluidCoreMlAsr::from_dir(&self.model_dir, self.version).map_err(|error| {
                margins_core::TranscriptError {
                    code: margins_core::TranscriptErrorCode::ModelLoadFailed,
                    message: error.to_string(),
                    retryable: false,
                }
            })?;
        let words = backend
            .transcribe_words(&request.samples)
            .map_err(|error| margins_core::TranscriptError {
                code: margins_core::TranscriptErrorCode::InferenceFailed,
                message: error.to_string(),
                retryable: false,
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

#[derive(Debug, Clone, PartialEq)]
pub struct CoreMlWavTranscript {
    pub entries: Vec<TranscriptWordEntry>,
    pub channel_word_counts: Vec<(u32, usize)>,
}

/// Transcribe a WAV through the Rust-direct FluidAudio CoreML path without any
/// Tauri `AppHandle`, progress emission, alignment, cleanup, or distillation.
///
/// Margins-created stereo recordings are treated as channel 0 = mic and channel
/// 1 = system audio. More than two channels are intentionally ignored for the
/// first v2-only desktop path; imported/long-form/diarized parity remains a
/// follow-up.
pub fn transcribe_wav_to_transcript(
    model_dir: impl AsRef<Path>,
    wav_path: impl AsRef<Path>,
) -> Result<CoreMlWavTranscript> {
    let mut asr = FluidCoreMlAsr::from_dir_auto(model_dir)?;
    transcribe_wav_to_transcript_with_asr(&mut asr, wav_path)
}

pub fn transcribe_wav_to_transcript_with_asr(
    asr: &mut FluidCoreMlAsr,
    wav_path: impl AsRef<Path>,
) -> Result<CoreMlWavTranscript> {
    let wav_path = wav_path.as_ref();
    let audio = crate::audio::load_wav(wav_path)
        .with_context(|| format!("failed to load WAV {}", wav_path.display()))?;
    let mut entries = Vec::new();
    let mut channel_word_counts = Vec::new();

    if audio.channels >= 2 {
        for channel in 0..2usize {
            let source = crate::audio::extract_channel(&audio, channel)?;
            let mono_16k =
                crate::audio::resample_mono_linear(&source, audio.sample_rate, SAMPLE_RATE);
            let words = asr.transcribe_words(&mono_16k)?;
            channel_word_counts.push((channel as u32, words.len()));
            entries.extend(words_to_transcript_entries(&words, channel as u32, 0));
        }
    } else {
        let mono = crate::audio::downmix_to_mono(&audio)?;
        let mono_16k = crate::audio::resample_mono_linear(&mono, audio.sample_rate, SAMPLE_RATE);
        let words = asr.transcribe_words(&mono_16k)?;
        channel_word_counts.push((0, words.len()));
        entries.extend(words_to_transcript_entries(&words, 0, 0));
    }

    Ok(CoreMlWavTranscript {
        entries: merge_word_entries_to_phrases(entries, 2_000),
        channel_word_counts,
    })
}

fn resolve_model_root(input: &Path, version: FluidCoreMlModelVersion) -> Result<PathBuf> {
    let expanded = expand_tilde(input);
    if expanded.join("Preprocessor.mlmodelc").exists() {
        return Ok(expanded);
    }

    let preferred = match version {
        FluidCoreMlModelVersion::V2 => "parakeet-tdt-0.6b-v2",
        FluidCoreMlModelVersion::V3 => "parakeet-tdt-0.6b-v3",
    };
    let candidate = expanded.join(preferred);
    if candidate.join("Preprocessor.mlmodelc").exists() {
        return Ok(candidate);
    }

    for child in ["parakeet-tdt-0.6b-v2", "parakeet-tdt-0.6b-v3"] {
        let candidate = expanded.join(child);
        if candidate.join("Preprocessor.mlmodelc").exists() {
            return Ok(candidate);
        }
    }

    Ok(expanded)
}

fn expand_tilde(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    path.to_path_buf()
}

#[cfg(test)]
fn pad_frontend_audio(samples: &[f32]) -> (Vec<f32>, usize) {
    let mut padded = Vec::with_capacity(MAX_MODEL_SAMPLES);
    let actual_len = prepare_frontend_audio(samples, &mut padded);
    (padded, actual_len)
}

fn prepare_frontend_audio(samples: &[f32], padded: &mut Vec<f32>) -> usize {
    let capped_len = samples.len().min(MAX_MODEL_SAMPLES);
    let frame_aligned_len =
        capped_len.div_ceil(SAMPLES_PER_ENCODER_FRAME) * SAMPLES_PER_ENCODER_FRAME;
    let actual_len = frame_aligned_len.min(MAX_MODEL_SAMPLES).max(capped_len);
    padded.clear();
    padded.extend_from_slice(&samples[..capped_len]);
    if padded.len() < actual_len {
        padded.resize(actual_len, 0.0);
    }
    padded.resize(MAX_MODEL_SAMPLES, 0.0);
    actual_len
}

fn offline_chunk_stride_samples() -> usize {
    offline_chunk_samples().saturating_sub(OFFLINE_OVERLAP_SAMPLES)
}

fn offline_chunk_samples() -> usize {
    ((MAX_MODEL_SAMPLES - MEL_CONTEXT_SAMPLES - MEL_HOP_SAMPLES) / SAMPLES_PER_ENCODER_FRAME)
        * SAMPLES_PER_ENCODER_FRAME
}

fn offline_overlap_frames() -> usize {
    OFFLINE_OVERLAP_SAMPLES / SAMPLES_PER_ENCODER_FRAME
}

const ENCODER_HIDDEN_SIZE: usize = 1024;
const DECODER_HIDDEN_SIZE: usize = 640;
const DECODER_LAYERS: usize = 2;
const BLANK_ID_V2: usize = 1024;
const MAX_SYMBOLS_PER_STEP: usize = 10;
const MAX_TOKENS_PER_CHUNK: usize = 150;
const CONSECUTIVE_BLANK_LIMIT: usize = 5;
const DURATION_BINS_V2: [usize; 5] = [0, 1, 2, 3, 4];
const PUNCTUATION_TOKENS: [usize; 3] = [7883, 7952, 7948];

fn calculate_encoder_frames(samples: usize) -> usize {
    samples.div_ceil(SAMPLES_PER_ENCODER_FRAME)
}

fn load_vocabulary(path: &Path) -> Result<Vec<Option<String>>> {
    let data = std::fs::read(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value =
        serde_json::from_slice(&data).context("failed to parse parakeet_vocab.json")?;
    match value {
        Value::Array(items) => Ok(items
            .into_iter()
            .map(|item| item.as_str().map(ToOwned::to_owned))
            .collect()),
        Value::Object(map) => {
            let max_id = map.keys().filter_map(|key| key.parse::<usize>().ok()).max();
            let mut vocab = vec![None; max_id.map(|id| id + 1).unwrap_or(0)];
            for (key, value) in map {
                if let (Ok(id), Some(token)) = (key.parse::<usize>(), value.as_str()) {
                    if id >= vocab.len() {
                        vocab.resize(id + 1, None);
                    }
                    vocab[id] = Some(token.to_string());
                }
            }
            Ok(vocab)
        }
        _ => bail!("parakeet_vocab.json must be a JSON array or object"),
    }
}

#[derive(Debug, Clone)]
pub struct CoreMlStreamingConfig {
    pub step_ms: u64,
    pub overlap_ms: u64,
    pub stability_lag_ms: u64,
    pub checkpoint_interval_ms: u64,
}

impl Default for CoreMlStreamingConfig {
    fn default() -> Self {
        Self {
            step_ms: 1_000,
            overlap_ms: 2_000,
            stability_lag_ms: 1_500,
            checkpoint_interval_ms: 5_000,
        }
    }
}

impl CoreMlStreamingConfig {
    fn normalized(&self) -> Self {
        Self {
            step_ms: self.step_ms.max(1).min(max_model_window_ms()),
            overlap_ms: self.overlap_ms,
            stability_lag_ms: self.stability_lag_ms,
            checkpoint_interval_ms: self.checkpoint_interval_ms,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CoreMlStreamingTimings {
    pub audio_slice: Duration,
    pub frontend: Duration,
    pub decode: Duration,
    pub total: Duration,
    pub audio_ms: u64,
    pub words_decoded: usize,
    pub words_committed: usize,
    pub words_hypothesis: usize,
    pub checkpoint_restored: bool,
    pub checkpoint_created: bool,
}

impl CoreMlStreamingTimings {
    fn accumulate(&mut self, other: &Self) {
        self.audio_slice += other.audio_slice;
        self.frontend += other.frontend;
        self.decode += other.decode;
        self.total += other.total;
        self.audio_ms += other.audio_ms;
        self.words_decoded += other.words_decoded;
        self.words_committed = other.words_committed;
        self.words_hypothesis = other.words_hypothesis;
        self.checkpoint_restored |= other.checkpoint_restored;
        self.checkpoint_created |= other.checkpoint_created;
    }
}

#[derive(Debug, Clone)]
pub struct StreamingTranscriptUpdate {
    pub committed: Vec<WordTiming>,
    pub hypothesis: Vec<WordTiming>,
    pub decoded_until_ms: u64,
    pub committed_until_ms: u64,
    pub timings: CoreMlStreamingTimings,
}

#[derive(Debug, Clone)]
pub struct RollingAudioBuffer {
    sample_rate: u32,
    base_sample_index: u64,
    samples: VecDeque<f32>,
}

impl RollingAudioBuffer {
    pub fn new(sample_rate: u32) -> Self {
        Self {
            sample_rate,
            base_sample_index: 0,
            samples: VecDeque::new(),
        }
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn base_sample_index(&self) -> u64 {
        self.base_sample_index
    }

    pub fn append(&mut self, samples: &[f32]) {
        self.samples.extend(samples.iter().copied());
    }

    pub fn slice(&self, start_ms: u64, end_ms: u64) -> Vec<f32> {
        if end_ms <= start_ms || self.samples.is_empty() {
            return Vec::new();
        }

        let requested_start = ms_to_sample_index(start_ms, self.sample_rate);
        let requested_end = ms_to_sample_index(end_ms, self.sample_rate);
        let available_start = self.base_sample_index;
        let available_end = self.base_sample_index + self.samples.len() as u64;
        let start = requested_start.max(available_start).min(available_end);
        let end = requested_end.max(available_start).min(available_end);
        if end <= start {
            return Vec::new();
        }

        let offset = (start - self.base_sample_index) as usize;
        let len = (end - start) as usize;
        self.samples
            .iter()
            .skip(offset)
            .take(len)
            .copied()
            .collect()
    }

    pub fn trim_before(&mut self, ms: u64) {
        let target = ms_to_sample_index(ms, self.sample_rate);
        if target <= self.base_sample_index {
            return;
        }
        let remove = (target - self.base_sample_index).min(self.samples.len() as u64) as usize;
        self.samples.drain(..remove);
        self.base_sample_index += remove as u64;
    }

    pub fn duration_ms(&self) -> u64 {
        samples_to_ms(self.samples.len() as u64, self.sample_rate)
    }

    pub fn end_ms(&self) -> u64 {
        samples_to_ms(
            self.base_sample_index + self.samples.len() as u64,
            self.sample_rate,
        )
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct AsrCheckpoint {
    time_ms: u64,
    frame_index: usize,
    state: TdtDecoderStateRust,
    committed_word_len: usize,
    committed_until_ms: u64,
    decoded_until_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DecodePlan {
    start_ms: u64,
    restore_checkpoint: Option<usize>,
    kind: DecodePlanKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DecodePlanKind {
    Initial,
    OverlapReplay,
    ConservativeNoOverlap,
}

/// Long-lived streaming TDT session for one mono channel.
///
/// Live backchanneling should keep this session alive for the whole recording:
/// append new samples, call `update_until` for cadence/memo-triggered reads,
/// and reserve `finish_until` for recording stop. Memo commits are consumers of
/// transcript state, not ASR boundaries, so they must not reset this state.
pub struct CoreMlAsrSession {
    asr: FluidCoreMlAsr,
    state: TdtDecoderStateRust,
    audio: RollingAudioBuffer,
    checkpoints: VecDeque<AsrCheckpoint>,
    committed_words: Vec<WordTiming>,
    hypothesis_words: Vec<WordTiming>,
    transcript_token_windows: Vec<TokenWindow>,
    decoded_until_ms: u64,
    committed_until_ms: u64,
    terminal: bool,
    config: CoreMlStreamingConfig,
}

impl std::fmt::Debug for CoreMlAsrSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreMlAsrSession")
            .field("audio_end_ms", &self.audio.end_ms())
            .field("checkpoints", &self.checkpoints.len())
            .field("committed_words", &self.committed_words.len())
            .field("hypothesis_words", &self.hypothesis_words.len())
            .field(
                "transcript_token_windows",
                &self.transcript_token_windows.len(),
            )
            .field("decoded_until_ms", &self.decoded_until_ms)
            .field("committed_until_ms", &self.committed_until_ms)
            .field("terminal", &self.terminal)
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl CoreMlAsrSession {
    pub fn new(asr: FluidCoreMlAsr, config: CoreMlStreamingConfig) -> Self {
        let initial_state = TdtDecoderStateRust::new();
        let mut checkpoints = VecDeque::new();
        checkpoints.push_back(AsrCheckpoint {
            time_ms: 0,
            frame_index: 0,
            state: initial_state.clone(),
            committed_word_len: 0,
            committed_until_ms: 0,
            decoded_until_ms: 0,
        });
        Self {
            asr,
            state: initial_state,
            audio: RollingAudioBuffer::new(SAMPLE_RATE),
            checkpoints,
            committed_words: Vec::new(),
            hypothesis_words: Vec::new(),
            transcript_token_windows: Vec::new(),
            decoded_until_ms: 0,
            committed_until_ms: 0,
            terminal: false,
            config: config.normalized(),
        }
    }

    pub fn append_audio(&mut self, mono_16k: &[f32]) {
        self.audio.append(mono_16k);
    }

    pub fn update_until(&mut self, end_ms: u64) -> Result<StreamingTranscriptUpdate> {
        if self.terminal {
            bail!("cannot update a CoreML ASR session after finish_until; create a new session");
        }
        let target_ms = end_ms.min(self.audio.end_ms());
        let decode_target_ms = aligned_streaming_target_ms(target_ms);
        let update_started = Instant::now();
        let mut timings = CoreMlStreamingTimings::default();

        while self.decoded_until_ms < decode_target_ms {
            let next_end = self.next_decode_end(decode_target_ms);
            if next_end <= self.decoded_until_ms {
                break;
            }
            let mut step_timings = self.decode_update_step(next_end)?;
            step_timings.words_committed = self.committed_words.len();
            step_timings.words_hypothesis = self.hypothesis_words.len();
            timings.accumulate(&step_timings);
        }

        timings.total = update_started.elapsed();
        Ok(StreamingTranscriptUpdate {
            committed: self.committed_words.clone(),
            hypothesis: self.hypothesis_words.clone(),
            decoded_until_ms: self.decoded_until_ms,
            committed_until_ms: self.committed_until_ms,
            timings,
        })
    }

    /// Finalize decoding up to `end_ms` and allow the TDT decoder to flush
    /// trailing symbols. Ordinary progressive `update_until` deliberately does
    /// not use last-chunk finalization because more audio may still arrive.
    ///
    /// `finish_until` is terminal for the current session. Unlike progressive
    /// updates, the final target may be an arbitrary millisecond in the middle
    /// of an encoder frame so the final tail can be consumed. Such a terminal
    /// partial-frame decode is intentionally not checkpointed.
    pub fn finish_until(&mut self, end_ms: u64) -> Result<StreamingTranscriptUpdate> {
        if self.terminal {
            bail!("CoreML ASR session has already been finalized");
        }
        let target_ms = end_ms.min(self.audio.end_ms());
        if self.decoded_until_ms < target_ms {
            self.update_until(target_ms)?;
        }

        let update_started = Instant::now();
        let original_decoded_until_ms = self.decoded_until_ms;
        let plan = select_decode_plan(
            self.decoded_until_ms,
            &self.checkpoints,
            &self.config,
            target_ms,
        );
        let checkpoint_restored = if let Some(index) = plan.restore_checkpoint {
            self.restore_checkpoint(index);
            true
        } else {
            false
        };

        let mut span = if plan.start_ms < target_ms {
            self.decode_span(plan.start_ms, target_ms, true)?
        } else {
            DecodeSpanResult {
                timings: CoreMlStreamingTimings::default(),
                token_windows: Vec::new(),
            }
        };
        self.decoded_until_ms = target_ms.max(original_decoded_until_ms);
        let final_words = self.merge_span_token_windows(std::mem::take(&mut span.token_windows));
        self.reconcile_final_words(final_words);
        let checkpoint_created = self.maybe_create_checkpoint();
        self.terminal = true;

        span.timings.total = update_started.elapsed();
        span.timings.checkpoint_restored = checkpoint_restored;
        span.timings.checkpoint_created = checkpoint_created;
        span.timings.words_committed = self.committed_words.len();
        span.timings.words_hypothesis = self.hypothesis_words.len();

        Ok(StreamingTranscriptUpdate {
            committed: self.committed_words.clone(),
            hypothesis: self.hypothesis_words.clone(),
            decoded_until_ms: self.decoded_until_ms,
            committed_until_ms: self.committed_until_ms,
            timings: span.timings,
        })
    }

    pub fn committed_words(&self) -> &[WordTiming] {
        &self.committed_words
    }

    pub fn hypothesis_words(&self) -> &[WordTiming] {
        &self.hypothesis_words
    }

    pub fn decoded_until_ms(&self) -> u64 {
        self.decoded_until_ms
    }

    pub fn committed_until_ms(&self) -> u64 {
        self.committed_until_ms
    }

    pub fn checkpoints_len(&self) -> usize {
        self.checkpoints.len()
    }

    pub fn asr_mut(&mut self) -> &mut FluidCoreMlAsr {
        &mut self.asr
    }

    fn next_decode_end(&self, target_ms: u64) -> u64 {
        next_decode_end_for(self.decoded_until_ms, target_ms, &self.config)
    }

    fn decode_update_step(&mut self, decode_end_ms: u64) -> Result<CoreMlStreamingTimings> {
        debug_assert!(is_encoder_frame_aligned_ms(decode_end_ms));
        let step_started = Instant::now();
        let plan = select_decode_plan(
            self.decoded_until_ms,
            &self.checkpoints,
            &self.config,
            decode_end_ms,
        );
        let checkpoint_restored = if let Some(index) = plan.restore_checkpoint {
            self.restore_checkpoint(index);
            true
        } else {
            false
        };

        let mut span = self.decode_span(plan.start_ms, decode_end_ms, false)?;
        self.decoded_until_ms = decode_end_ms;
        let decoded_words = self.merge_span_token_windows(std::mem::take(&mut span.token_windows));
        self.reconcile_words(decoded_words, decode_end_ms);
        let checkpoint_created = self.maybe_create_checkpoint();
        self.maintain_bounded_retention();

        span.timings.total = step_started.elapsed();
        span.timings.checkpoint_restored = checkpoint_restored;
        span.timings.checkpoint_created = checkpoint_created;
        span.timings.words_committed = self.committed_words.len();
        span.timings.words_hypothesis = self.hypothesis_words.len();
        Ok(span.timings)
    }

    fn decode_span(
        &mut self,
        start_ms: u64,
        end_ms: u64,
        is_final: bool,
    ) -> Result<DecodeSpanResult> {
        let audio_started = Instant::now();
        let audio = self.audio.slice(start_ms, end_ms);
        let audio_slice = audio_started.elapsed();
        let audio_ms = end_ms.saturating_sub(start_ms);

        if audio.is_empty() || is_digital_silence(&audio) {
            return Ok(DecodeSpanResult {
                timings: CoreMlStreamingTimings {
                    audio_slice,
                    audio_ms,
                    ..Default::default()
                },
                token_windows: Vec::new(),
            });
        }
        if audio.len() > MAX_MODEL_SAMPLES {
            bail!(
                "streaming CoreML decode span exceeded model window: {} samples > {}",
                audio.len(),
                MAX_MODEL_SAMPLES
            );
        }

        let frontend_started = Instant::now();
        let frontend = self.asr.run_frontend_tensors(&audio)?;
        let frontend_elapsed = frontend_started.elapsed();

        let decode_started = Instant::now();
        if !is_encoder_frame_aligned_ms(start_ms) {
            bail!("streaming CoreML decode start must be frame-aligned: {start_ms}ms");
        }
        let global_frame_offset = ms_to_encoder_frame_floor(start_ms);
        let hyp = self.asr.decode_chunk(
            &frontend.encoder,
            frontend.encoder_length,
            frontend.actual_audio_frames,
            &mut self.state,
            DecodeChunkOptions {
                is_last_chunk: is_final,
                global_frame_offset,
                ..Default::default()
            },
        )?;
        let decode_elapsed = decode_started.elapsed();

        let token_windows = hypothesis_token_windows(&hyp);
        let words = token_windows_to_word_timings(&token_windows, &self.asr.vocabulary);
        Ok(DecodeSpanResult {
            timings: CoreMlStreamingTimings {
                audio_slice,
                frontend: frontend_elapsed,
                decode: decode_elapsed,
                audio_ms,
                words_decoded: words.len(),
                ..Default::default()
            },
            token_windows,
        })
    }

    fn merge_span_token_windows(&mut self, token_windows: Vec<TokenWindow>) -> Vec<WordTiming> {
        if !token_windows.is_empty() {
            self.transcript_token_windows = merge_token_windows(
                &self.transcript_token_windows,
                &token_windows,
                ms_to_encoder_frame_floor(self.config.overlap_ms).max(1),
            );
        }
        token_windows_to_word_timings(&self.transcript_token_windows, &self.asr.vocabulary)
    }

    fn reconcile_words(&mut self, decoded_words: Vec<WordTiming>, decode_end_ms: u64) {
        self.reconcile_words_with_lag(decoded_words, decode_end_ms, self.config.stability_lag_ms);
    }

    fn reconcile_words_with_lag(
        &mut self,
        decoded_words: Vec<WordTiming>,
        decode_end_ms: u64,
        stability_lag_ms: u64,
    ) {
        let reconciled = reconcile_streaming_words(
            &self.committed_words,
            self.committed_until_ms,
            decoded_words,
            decode_end_ms,
            stability_lag_ms,
        );
        self.committed_words = reconciled.committed;
        self.hypothesis_words = reconciled.hypothesis;
        self.committed_until_ms = reconciled.committed_until_ms;
    }

    fn reconcile_final_words(&mut self, decoded_words: Vec<WordTiming>) {
        // Finalization flush can produce token durations that extend slightly
        // past the physical audio end. At terminal EOF there is no reason to
        // keep those tail words unstable/hypothesis-only: commit all decoded
        // non-duplicates so final consumers do not drop the last word.
        self.reconcile_words_with_lag(decoded_words, u64::MAX, 0);
    }

    fn restore_checkpoint(&mut self, index: usize) {
        let checkpoint = self.checkpoints[index].clone();
        self.state = checkpoint.state;
        // Committed transcript is append-only. Replaying overlap from an older
        // checkpoint rebuilds decoder state; text reconciliation filters words
        // that are already committed instead of rolling committed text back.
        self.hypothesis_words.clear();
        self.decoded_until_ms = checkpoint.decoded_until_ms;
    }

    fn maybe_create_checkpoint(&mut self) -> bool {
        let interval = self.config.checkpoint_interval_ms;
        if interval == 0 || self.decoded_until_ms == 0 {
            return false;
        }
        if !is_encoder_frame_aligned_ms(self.decoded_until_ms) {
            // Finalization may decode a terminal partial-frame tail (for
            // example `finish_until(3000)`, where the last complete Parakeet
            // encoder frame is 2960ms). Do not checkpoint that state: replay
            // checkpoints must stay frame-aligned so future overlap starts have
            // exact frame offsets.
            return false;
        }
        let current_frame = ms_to_encoder_frame_floor(self.decoded_until_ms);
        let last_checkpoint_frame = self
            .checkpoints
            .back()
            .map(|checkpoint| checkpoint.frame_index)
            .unwrap_or(0);
        if !checkpoint_interval_elapsed(last_checkpoint_frame, current_frame, interval) {
            return false;
        }
        self.checkpoints.push_back(AsrCheckpoint {
            time_ms: self.decoded_until_ms,
            frame_index: ms_to_encoder_frame(self.decoded_until_ms),
            state: self.state.clone(),
            committed_word_len: self.committed_words.len(),
            committed_until_ms: self.committed_until_ms,
            decoded_until_ms: self.decoded_until_ms,
        });
        true
    }

    fn maintain_bounded_retention(&mut self) {
        prune_unusable_checkpoints(
            &mut self.checkpoints,
            self.decoded_until_ms,
            max_model_window_ms(),
        );
        let audio_floor_ms = self
            .checkpoints
            .front()
            .map(|checkpoint| checkpoint.time_ms)
            .unwrap_or(self.decoded_until_ms.saturating_sub(self.config.overlap_ms));
        self.audio.trim_before(audio_floor_ms);
        prune_token_windows_before(
            &mut self.transcript_token_windows,
            self.committed_until_ms
                .saturating_sub(self.config.overlap_ms),
        );
    }
}

pub struct StereoCoreMlAsrSession {
    pub mic: CoreMlAsrSession,
    pub system: CoreMlAsrSession,
}

impl StereoCoreMlAsrSession {
    pub fn new(
        mic_asr: FluidCoreMlAsr,
        system_asr: FluidCoreMlAsr,
        config: CoreMlStreamingConfig,
    ) -> Self {
        Self {
            mic: CoreMlAsrSession::new(mic_asr, config.clone()),
            system: CoreMlAsrSession::new(system_asr, config),
        }
    }
}

struct DecodeSpanResult {
    timings: CoreMlStreamingTimings,
    token_windows: Vec<TokenWindow>,
}

struct ReconciledWords {
    committed: Vec<WordTiming>,
    hypothesis: Vec<WordTiming>,
    committed_until_ms: u64,
}

fn select_decode_plan(
    decoded_until_ms: u64,
    checkpoints: &VecDeque<AsrCheckpoint>,
    config: &CoreMlStreamingConfig,
    decode_end_ms: u64,
) -> DecodePlan {
    if decoded_until_ms == 0 {
        return DecodePlan {
            start_ms: 0,
            restore_checkpoint: None,
            kind: DecodePlanKind::Initial,
        };
    }

    let max_window_ms = max_model_window_ms();
    let desired_start_ms = decoded_until_ms.saturating_sub(config.overlap_ms);
    let wants_overlap = desired_start_ms < decoded_until_ms;
    if wants_overlap {
        if let Some(index) = latest_checkpoint_index(checkpoints, |checkpoint| {
            checkpoint.time_ms <= desired_start_ms
                && decode_end_ms.saturating_sub(checkpoint.time_ms) <= max_window_ms
        }) {
            return DecodePlan {
                start_ms: checkpoints[index].time_ms,
                restore_checkpoint: Some(index),
                kind: DecodePlanKind::OverlapReplay,
            };
        }

        // Conservative correctness fallback: if no checkpoint exists at/before
        // overlap start within the model window, do not decode overlapped audio
        // from live end-state. Continue with strictly new audio only.
        return DecodePlan {
            start_ms: decoded_until_ms,
            restore_checkpoint: None,
            kind: DecodePlanKind::ConservativeNoOverlap,
        };
    }

    DecodePlan {
        start_ms: decoded_until_ms,
        restore_checkpoint: None,
        kind: DecodePlanKind::ConservativeNoOverlap,
    }
}

fn next_decode_end_for(
    decoded_until_ms: u64,
    target_ms: u64,
    config: &CoreMlStreamingConfig,
) -> u64 {
    let decoded_frame = ms_to_encoder_frame_floor(decoded_until_ms);
    let target_frame = ms_to_encoder_frame_floor(target_ms);
    if target_frame <= decoded_frame {
        return decoded_until_ms;
    }

    let step_frames = duration_ms_to_encoder_frames_floor(effective_step_ms(config)).max(1);
    let mut next_frame = decoded_frame.saturating_add(step_frames).min(target_frame);

    if config.checkpoint_interval_ms > 0 {
        let checkpoint_frames =
            duration_ms_to_encoder_frames_floor(config.checkpoint_interval_ms).max(1);
        let next_checkpoint_frame =
            ((decoded_frame / checkpoint_frames) + 1).saturating_mul(checkpoint_frames);
        next_frame = next_frame.min(next_checkpoint_frame).max(decoded_frame + 1);
    }

    encoder_frame_to_ms(next_frame)
}

fn effective_step_ms(config: &CoreMlStreamingConfig) -> u64 {
    let mut step_ms = config.step_ms.max(1).min(max_model_window_ms());
    if config.checkpoint_interval_ms > 0 {
        step_ms = step_ms.min(config.checkpoint_interval_ms);
    }
    step_ms.max(1)
}

fn latest_checkpoint_index(
    checkpoints: &VecDeque<AsrCheckpoint>,
    mut predicate: impl FnMut(&AsrCheckpoint) -> bool,
) -> Option<usize> {
    checkpoints
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, checkpoint)| predicate(checkpoint).then_some(index))
}

fn checkpoint_interval_elapsed(
    last_checkpoint_frame: usize,
    current_frame: usize,
    interval_ms: u64,
) -> bool {
    if interval_ms == 0 {
        return false;
    }
    let interval_frames = duration_ms_to_encoder_frames_floor(interval_ms).max(1);
    current_frame.saturating_sub(last_checkpoint_frame) >= interval_frames
}

fn prune_unusable_checkpoints(
    checkpoints: &mut VecDeque<AsrCheckpoint>,
    decoded_until_ms: u64,
    max_window_ms: u64,
) {
    let floor_ms = decoded_until_ms.saturating_sub(max_window_ms);
    while checkpoints.len() > 1
        && checkpoints
            .front()
            .is_some_and(|checkpoint| checkpoint.time_ms < floor_ms)
    {
        checkpoints.pop_front();
    }
}

fn prune_token_windows_before(token_windows: &mut Vec<TokenWindow>, before_ms: u64) {
    let before_frame = ms_to_encoder_frame_floor(before_ms);
    let keep_from = token_windows
        .iter()
        .position(|window| window.timestamp >= before_frame)
        .unwrap_or(token_windows.len());
    if keep_from > 0 {
        token_windows.drain(..keep_from);
    }
}

fn reconcile_streaming_words(
    committed: &[WordTiming],
    committed_until_ms: u64,
    decoded_words: Vec<WordTiming>,
    decode_end_ms: u64,
    stability_lag_ms: u64,
) -> ReconciledWords {
    let cutoff_ms = decode_end_ms.saturating_sub(stability_lag_ms);
    let mut next_committed = committed.to_vec();
    let mut hypothesis = Vec::new();

    for word in decoded_words
        .into_iter()
        .filter(|word| word.end_ms > committed_until_ms)
    {
        if is_duplicate_of_committed_tail(&word, &next_committed) {
            continue;
        }

        if word.end_ms <= cutoff_ms {
            next_committed.push(word);
        } else {
            hypothesis.push(word);
        }
    }

    let next_committed_until = next_committed
        .last()
        .map(|word| word.end_ms)
        .unwrap_or(committed_until_ms)
        .max(committed_until_ms);

    ReconciledWords {
        committed: next_committed,
        hypothesis,
        committed_until_ms: next_committed_until,
    }
}

fn is_duplicate_of_committed_tail(word: &WordTiming, committed: &[WordTiming]) -> bool {
    committed
        .iter()
        .rev()
        .take(12)
        .any(|existing| duplicate_words(existing, word))
}

fn duplicate_words(a: &WordTiming, b: &WordTiming) -> bool {
    normalize_word(&a.text) == normalize_word(&b.text)
        && timestamps_nearly_equal(a.start_ms, b.start_ms, 250)
        && timestamps_nearly_equal(a.end_ms, b.end_ms, 250)
}

fn normalize_word(text: &str) -> String {
    text.trim()
        .trim_matches(|ch: char| ch.is_ascii_punctuation())
        .to_ascii_lowercase()
}

fn timestamps_nearly_equal(a: u64, b: u64, tolerance_ms: u64) -> bool {
    a.abs_diff(b) <= tolerance_ms
}

fn max_model_window_ms() -> u64 {
    samples_to_ms(MAX_MODEL_SAMPLES as u64, SAMPLE_RATE)
}

fn aligned_streaming_target_ms(target_ms: u64) -> u64 {
    encoder_frame_to_ms(ms_to_encoder_frame_floor(target_ms))
}

fn ms_to_sample_index(ms: u64, sample_rate: u32) -> u64 {
    ((ms as u128 * sample_rate as u128) / 1_000) as u64
}

fn samples_to_ms(samples: u64, sample_rate: u32) -> u64 {
    ((samples as u128 * 1_000) / sample_rate as u128) as u64
}

fn ms_to_encoder_frame(ms: u64) -> usize {
    ms_to_encoder_frame_floor(ms)
}

fn ms_to_encoder_frame_floor(ms: u64) -> usize {
    (ms_to_sample_index(ms, SAMPLE_RATE) as usize) / SAMPLES_PER_ENCODER_FRAME
}

fn duration_ms_to_encoder_frames_floor(ms: u64) -> usize {
    ms_to_encoder_frame_floor(ms)
}

fn encoder_frame_to_ms(frame: usize) -> u64 {
    (frame as u64).saturating_mul(MS_PER_ENCODER_FRAME)
}

fn is_encoder_frame_aligned_ms(ms: u64) -> bool {
    ms == encoder_frame_to_ms(ms_to_encoder_frame_floor(ms))
}

struct FrontendTensors {
    encoder: CoreMlTensor,
    encoder_length: usize,
    actual_audio_frames: usize,
}

#[derive(Debug, Clone)]
struct TdtDecoderStateRust {
    h: Vec<f32>,
    c: Vec<f32>,
    last_token: Option<usize>,
    predictor_output: Option<CoreMlTensor>,
}

impl TdtDecoderStateRust {
    fn new() -> Self {
        Self {
            h: vec![0.0; DECODER_LAYERS * DECODER_HIDDEN_SIZE],
            c: vec![0.0; DECODER_LAYERS * DECODER_HIDDEN_SIZE],
            last_token: None,
            predictor_output: None,
        }
    }

    fn reset_arrays(&mut self) {
        self.h.fill(0.0);
        self.c.fill(0.0);
    }
}

#[derive(Debug, Default)]
struct TdtHypothesisRust {
    tokens: Vec<usize>,
    timestamps: Vec<usize>,
    confidences: Vec<f32>,
    durations: Vec<usize>,
    last_token: Option<usize>,
}

#[derive(Debug, Clone, Copy, Default)]
struct DecodeChunkOptions {
    is_last_chunk: bool,
    global_frame_offset: usize,
    context_frame_adjustment: usize,
    emit_tokens_after_global_frame: Option<usize>,
    initial_time_index_override: Option<usize>,
}

#[derive(Debug, Clone, PartialEq)]
struct TokenWindow {
    token: usize,
    timestamp: usize,
    confidence: f32,
    duration: usize,
}

#[derive(Debug, Clone)]
struct DecoderStepOutput {
    decoder: CoreMlTensor,
    h_out: Vec<f32>,
    c_out: Vec<f32>,
}

#[derive(Debug)]
struct TdtJointDecisionRust {
    token: usize,
    probability: f32,
    duration_bin: usize,
}

struct EncoderFrames<'a> {
    tensor: &'a CoreMlTensor,
    count: usize,
    hidden_size: usize,
    hidden_axis: usize,
    time_axis: usize,
}

impl<'a> EncoderFrames<'a> {
    fn new(
        tensor: &'a CoreMlTensor,
        valid_length: usize,
        expected_hidden_size: usize,
    ) -> Result<Self> {
        if tensor.shape.len() != 3 {
            bail!("invalid encoder output rank: {:?}", tensor.shape);
        }
        if tensor.shape[0] != 1 {
            bail!("unsupported encoder batch dimension: {}", tensor.shape[0]);
        }
        let axis1_matches_hidden = tensor.shape[1] == expected_hidden_size;
        let axis2_matches_hidden = tensor.shape[2] == expected_hidden_size;
        if !axis1_matches_hidden && !axis2_matches_hidden {
            bail!(
                "encoder hidden size mismatch: shape={:?}, expected {}",
                tensor.shape,
                expected_hidden_size
            );
        }
        let hidden_axis = if axis1_matches_hidden { 1 } else { 2 };
        let time_axis = if axis1_matches_hidden { 2 } else { 1 };
        let count = valid_length.min(tensor.shape[time_axis]);
        if count == 0 {
            bail!("encoder output has no valid frames");
        }
        Ok(Self {
            tensor,
            count,
            hidden_size: expected_hidden_size,
            hidden_axis,
            time_axis,
        })
    }

    #[cfg(test)]
    fn frame(&self, index: usize) -> Vec<f32> {
        debug_assert!(index < self.count);
        let mut out = vec![0.0; self.hidden_size];
        self.copy_frame_into(index, &mut out);
        out
    }

    fn copy_frame_into(&self, index: usize, out: &mut [f32]) {
        debug_assert!(index < self.count);
        debug_assert!(out.len() >= self.hidden_size);
        if self.hidden_axis == 1 && self.time_axis == 2 {
            let frames = self.tensor.shape[2];
            for (hidden, value) in out.iter_mut().take(self.hidden_size).enumerate() {
                *value = self.tensor.data[hidden * frames + index];
            }
        } else {
            let hidden_size = self.tensor.shape[2];
            let base = index * hidden_size;
            out[..self.hidden_size]
                .copy_from_slice(&self.tensor.data[base..base + self.hidden_size]);
        }
    }
}

fn normalize_decoder_projection_into(projection: &CoreMlTensor, out: &mut [f32]) -> Result<()> {
    if out.len() < DECODER_HIDDEN_SIZE {
        bail!(
            "decoder projection output buffer too small: {}, expected {}",
            out.len(),
            DECODER_HIDDEN_SIZE
        );
    }
    if projection.shape.len() != 3 || projection.shape[0] != 1 {
        bail!("invalid decoder projection shape: {:?}", projection.shape);
    }
    if projection.shape[1] == DECODER_HIDDEN_SIZE && projection.shape[2] == 1 {
        out[..DECODER_HIDDEN_SIZE].copy_from_slice(&projection.data[..DECODER_HIDDEN_SIZE]);
        return Ok(());
    }
    if projection.shape[1] == 1 && projection.shape[2] == DECODER_HIDDEN_SIZE {
        out[..DECODER_HIDDEN_SIZE].copy_from_slice(&projection.data[..DECODER_HIDDEN_SIZE]);
        return Ok(());
    }
    bail!(
        "decoder projection hidden size mismatch: shape={:?}, expected {}",
        projection.shape,
        DECODER_HIDDEN_SIZE
    )
}

fn tensor_scalar(outputs: &HashMap<String, CoreMlTensor>, name: &str) -> Result<f32> {
    outputs
        .get(name)
        .and_then(|tensor| tensor.data.first())
        .copied()
        .ok_or_else(|| anyhow!("missing scalar CoreML output `{name}`"))
}

fn map_duration_bin(bin: usize) -> Result<usize> {
    DURATION_BINS_V2
        .get(bin)
        .copied()
        .ok_or_else(|| anyhow!("duration bin index out of range: {bin}"))
}

fn clamp_probability(value: f32) -> f32 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn should_emit_token(timestamp: usize, emit_after: Option<usize>) -> bool {
    emit_after.is_none_or(|emit_after| timestamp >= emit_after)
}

fn hypothesis_token_windows(hyp: &TdtHypothesisRust) -> Vec<TokenWindow> {
    hyp.tokens
        .iter()
        .copied()
        .zip(hyp.timestamps.iter().copied())
        .zip(hyp.confidences.iter().copied())
        .zip(hyp.durations.iter().copied().chain(std::iter::repeat(0)))
        .map(|(((token, timestamp), confidence), duration)| TokenWindow {
            token,
            timestamp,
            confidence,
            duration,
        })
        .collect()
}

#[cfg(test)]
fn tokens_to_word_timings(
    token_ids: &[usize],
    timestamps: &[usize],
    _confidences: &[f32],
    durations: &[usize],
    vocabulary: &[Option<String>],
) -> Vec<WordTiming> {
    let windows: Vec<_> = token_ids
        .iter()
        .copied()
        .zip(timestamps.iter().copied())
        .zip(durations.iter().copied().chain(std::iter::repeat(0)))
        .map(|((token, timestamp), duration)| TokenWindow {
            token,
            timestamp,
            confidence: 0.0,
            duration,
        })
        .collect();
    token_windows_to_word_timings(&windows, vocabulary)
}

fn token_windows_to_word_timings(
    token_windows: &[TokenWindow],
    vocabulary: &[Option<String>],
) -> Vec<WordTiming> {
    let mut combined = token_windows.to_vec();
    combined.sort_by_key(|window| window.timestamp);

    let mut words = Vec::new();
    let mut current = String::new();
    let mut word_start = 0u64;
    let mut word_end = 0u64;

    for window in combined {
        let Some(raw_token) = vocabulary
            .get(window.token)
            .and_then(|token| token.as_ref())
        else {
            continue;
        };
        if is_special_token(raw_token) {
            continue;
        }

        let starts_new_word = is_word_boundary(raw_token) || current.is_empty();
        if starts_new_word && !current.trim().is_empty() {
            words.push(WordTiming {
                start_ms: word_start,
                end_ms: word_end.max(word_start + 1),
                text: current.trim().to_string(),
            });
            current.clear();
        }

        let start_ms = frame_to_ms(window.timestamp);
        let token_end_ms = frame_to_ms(window.timestamp + window.duration.max(1)).max(start_ms + 1);
        if starts_new_word {
            current.push_str(&strip_word_boundary_prefix(raw_token));
            word_start = start_ms;
        } else {
            current.push_str(raw_token);
        }
        word_end = token_end_ms;
    }

    if !current.trim().is_empty() {
        words.push(WordTiming {
            start_ms: word_start,
            end_ms: word_end.max(word_start + 1),
            text: current.trim().to_string(),
        });
    }
    words
}

fn merge_token_windows(
    left: &[TokenWindow],
    right: &[TokenWindow],
    overlap_frames: usize,
) -> Vec<TokenWindow> {
    if left.is_empty() {
        return right.to_vec();
    }
    if right.is_empty() {
        return left.to_vec();
    }

    let left_end = left
        .last()
        .map(|token| token.timestamp.saturating_add(token.duration.max(1)))
        .unwrap_or(0);
    let right_start = right
        .first()
        .map(|token| token.timestamp)
        .unwrap_or(left_end);
    if left_end <= right_start {
        let mut out = Vec::with_capacity(left.len() + right.len());
        out.extend_from_slice(left);
        out.extend_from_slice(right);
        return out;
    }

    let tolerance = (overlap_frames / 2).max(1);
    let overlap_left: Vec<(usize, &TokenWindow)> = left
        .iter()
        .enumerate()
        .filter(|(_, token)| {
            token.timestamp.saturating_add(token.duration.max(1))
                > right_start.saturating_sub(overlap_frames)
        })
        .collect();
    let overlap_right: Vec<(usize, &TokenWindow)> = right
        .iter()
        .enumerate()
        .filter(|(_, token)| token.timestamp < left_end.saturating_add(overlap_frames))
        .collect();

    if overlap_left.len() < 2 || overlap_right.len() < 2 {
        return merge_token_windows_by_midpoint(left, right, left_end, right_start);
    }

    let minimum_pairs = (overlap_left.len() / 2).max(1);
    let contiguous_pairs = contiguous_token_matches(&overlap_left, &overlap_right, tolerance);
    if contiguous_pairs.len() >= minimum_pairs {
        return merge_token_windows_using_matches(
            &contiguous_pairs,
            &overlap_left,
            &overlap_right,
            left,
            right,
        );
    }

    let lcs_pairs = lcs_token_matches(&overlap_left, &overlap_right, tolerance);
    if lcs_pairs.is_empty() {
        return merge_token_windows_by_midpoint(left, right, left_end, right_start);
    }

    merge_token_windows_using_matches(&lcs_pairs, &overlap_left, &overlap_right, left, right)
}

fn token_windows_match(left: &TokenWindow, right: &TokenWindow, tolerance_frames: usize) -> bool {
    left.token == right.token && left.timestamp.abs_diff(right.timestamp) < tolerance_frames
}

fn contiguous_token_matches(
    left: &[(usize, &TokenWindow)],
    right: &[(usize, &TokenWindow)],
    tolerance_frames: usize,
) -> Vec<(usize, usize)> {
    let mut best_start = None;
    let mut best_len = 0usize;
    for left_start in 0..left.len() {
        for right_start in 0..right.len() {
            let mut len = 0usize;
            while left_start + len < left.len()
                && right_start + len < right.len()
                && token_windows_match(
                    left[left_start + len].1,
                    right[right_start + len].1,
                    tolerance_frames,
                )
            {
                len += 1;
            }
            if len > best_len {
                best_len = len;
                best_start = Some((left_start, right_start));
            }
        }
    }
    let Some((left_start, right_start)) = best_start else {
        return Vec::new();
    };
    (0..best_len)
        .map(|offset| (left_start + offset, right_start + offset))
        .collect()
}

fn lcs_token_matches(
    left: &[(usize, &TokenWindow)],
    right: &[(usize, &TokenWindow)],
    tolerance_frames: usize,
) -> Vec<(usize, usize)> {
    let mut dp = vec![vec![0usize; right.len() + 1]; left.len() + 1];
    for i in (0..left.len()).rev() {
        for j in (0..right.len()).rev() {
            dp[i][j] = if token_windows_match(left[i].1, right[j].1, tolerance_frames) {
                1 + dp[i + 1][j + 1]
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        if token_windows_match(left[i].1, right[j].1, tolerance_frames) {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn merge_token_windows_using_matches(
    matches: &[(usize, usize)],
    overlap_left: &[(usize, &TokenWindow)],
    overlap_right: &[(usize, &TokenWindow)],
    left: &[TokenWindow],
    right: &[TokenWindow],
) -> Vec<TokenWindow> {
    let left_indices: Vec<usize> = matches
        .iter()
        .map(|(left_index, _)| overlap_left[*left_index].0)
        .collect();
    let right_indices: Vec<usize> = matches
        .iter()
        .map(|(_, right_index)| overlap_right[*right_index].0)
        .collect();
    let mut result = Vec::with_capacity(left.len() + right.len());

    if let Some(first_left) = left_indices.first().copied() {
        result.extend_from_slice(&left[..first_left]);
    }

    for idx in 0..matches.len() {
        let left_index = left_indices[idx];
        let right_index = right_indices[idx];
        result.push(left[left_index].clone());

        if idx + 1 >= matches.len() {
            continue;
        }

        let next_left_index = left_indices[idx + 1];
        let next_right_index = right_indices[idx + 1];
        let gap_left = if next_left_index > left_index + 1 {
            &left[(left_index + 1)..next_left_index]
        } else {
            &[]
        };
        let gap_right = if next_right_index > right_index + 1 {
            &right[(right_index + 1)..next_right_index]
        } else {
            &[]
        };
        if gap_right.len() > gap_left.len() {
            result.extend_from_slice(gap_right);
        } else {
            result.extend_from_slice(gap_left);
        }
    }

    if let Some(last_right) = right_indices.last().copied() {
        if last_right + 1 < right.len() {
            result.extend_from_slice(&right[(last_right + 1)..]);
        }
    }
    result
}

fn merge_token_windows_by_midpoint(
    left: &[TokenWindow],
    right: &[TokenWindow],
    left_end: usize,
    right_start: usize,
) -> Vec<TokenWindow> {
    let cutoff = (left_end + right_start) / 2;
    let mut out = Vec::with_capacity(left.len() + right.len());
    out.extend(
        left.iter()
            .filter(|token| token.timestamp < cutoff)
            .cloned(),
    );
    out.extend(
        right
            .iter()
            .filter(|token| token.timestamp >= cutoff)
            .cloned(),
    );
    out
}

fn frame_to_ms(frame: usize) -> u64 {
    ((frame as f64) * (SAMPLES_PER_ENCODER_FRAME as f64) / (SAMPLE_RATE as f64) * 1000.0).round()
        as u64
}

fn is_word_boundary(token: &str) -> bool {
    token.starts_with('▁') || token.starts_with(' ')
}

fn strip_word_boundary_prefix(token: &str) -> String {
    token
        .strip_prefix('▁')
        .or_else(|| token.strip_prefix(' '))
        .unwrap_or(token)
        .to_string()
}

fn is_special_token(token: &str) -> bool {
    token.is_empty()
        || matches!(token, "<blank>" | "<pad>" | "<s>" | "</s>" | "<unk>")
        || (token.starts_with('<') && token.ends_with('>'))
}

fn is_digital_silence(samples: &[f32]) -> bool {
    samples.iter().all(|sample| sample.abs() <= f32::EPSILON)
}

struct CoreMlModel {
    model: Retained<MLModel>,
    noop_deallocator: RcBlock<dyn Fn(NonNull<c_void>)>,
    input_dict: RefCell<Retained<NSMutableDictionary<NSString, AnyObject>>>,
}

impl CoreMlModel {
    fn load(path: &Path, compute_units: MLComputeUnits) -> Result<Self> {
        let model = load_model(path, compute_units)?;
        Ok(Self {
            model,
            noop_deallocator: RcBlock::new(|_: NonNull<c_void>| {}),
            input_dict: RefCell::new(NSMutableDictionary::new()),
        })
    }

    fn predict(
        &self,
        inputs: &[CoreMlInput<'_>],
        output_names: &[&str],
    ) -> Result<HashMap<String, CoreMlTensor>> {
        autoreleasepool(|_| {
            let input_dict = self.input_dict.borrow_mut();
            input_dict.removeAllObjects();
            let mut arrays = Vec::with_capacity(inputs.len());

            for input in inputs {
                let (name, array) = match input {
                    CoreMlInput::F32 {
                        name,
                        values,
                        shape,
                    } => (
                        *name,
                        multi_array_f32(values, shape, &self.noop_deallocator)?,
                    ),
                    CoreMlInput::I32 {
                        name,
                        values,
                        shape,
                    } => (
                        *name,
                        multi_array_i32(values, shape, &self.noop_deallocator)?,
                    ),
                };
                let key = NSString::from_str(name);
                let key_copy: &ProtocolObject<dyn NSCopying> = ProtocolObject::from_ref(&*key);
                insert_input_feature(&input_dict, key_copy, &array);
                arrays.push(array);
            }

            let provider = build_feature_provider(&input_dict)?;
            let output = predict_features(&self.model, ProtocolObject::from_ref(&*provider))?;
            let mut out = HashMap::with_capacity(output_names.len());
            for output_name in output_names {
                let key = NSString::from_str(output_name);
                let array = output_multi_array(&output, &key, output_name)?;
                let (data, shape) = extract_output(&array)?;
                out.insert((*output_name).to_string(), CoreMlTensor { data, shape });
            }
            Ok(out)
        })
    }
}

impl std::fmt::Debug for CoreMlModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreMlModel").finish_non_exhaustive()
    }
}

enum CoreMlInput<'a> {
    F32 {
        name: &'static str,
        values: &'a [f32],
        shape: &'a [usize],
    },
    I32 {
        name: &'static str,
        values: &'a [i32],
        shape: &'a [usize],
    },
}

#[derive(Debug, Clone)]
struct CoreMlTensor {
    data: Vec<f32>,
    shape: Vec<usize>,
}

fn load_model(path: &Path, compute_units: MLComputeUnits) -> Result<Retained<MLModel>> {
    let path_str = NSString::from_str(&path.to_string_lossy());
    let url = NSURL::fileURLWithPath_isDirectory(&path_str, true);
    unsafe {
        let config = MLModelConfiguration::new();
        config.setComputeUnits(compute_units);
        config.setAllowLowPrecisionAccumulationOnGPU(true);
        MLModel::modelWithContentsOfURL_configuration_error(&url, &config)
    }
    .map_err(|error| anyhow!("CoreML failed to load model: {error}"))
}

fn insert_input_feature(
    input_dict: &NSMutableDictionary<NSString, AnyObject>,
    key_copy: &ProtocolObject<dyn NSCopying>,
    multi_array: &MLMultiArray,
) {
    unsafe {
        let feature_value = MLFeatureValue::featureValueWithMultiArray(multi_array);
        input_dict.setObject_forKey(feature_value_as_any_object(&feature_value), key_copy);
    }
}

fn build_feature_provider(
    input_dict: &NSMutableDictionary<NSString, AnyObject>,
) -> Result<Retained<MLDictionaryFeatureProvider>> {
    unsafe {
        MLDictionaryFeatureProvider::initWithDictionary_error(
            MLDictionaryFeatureProvider::alloc(),
            input_dict,
        )
    }
    .map_err(|error| anyhow!("CoreML feature provider failed: {error}"))
}

fn predict_features(
    model: &MLModel,
    input_ref: &ProtocolObject<dyn MLFeatureProvider>,
) -> Result<Retained<ProtocolObject<dyn MLFeatureProvider>>> {
    unsafe { model.predictionFromFeatures_error(input_ref) }
        .map_err(|error| anyhow!("CoreML prediction failed: {error}"))
}

fn output_multi_array(
    output: &ProtocolObject<dyn MLFeatureProvider>,
    output_key: &NSString,
    output_name: &str,
) -> Result<Retained<MLMultiArray>> {
    let feature = unsafe { output.featureValueForName(output_key) }
        .ok_or_else(|| anyhow!("missing CoreML output `{output_name}`"))?;
    unsafe { feature.multiArrayValue() }
        .ok_or_else(|| anyhow!("CoreML output `{output_name}` was not an MLMultiArray"))
}

fn feature_value_as_any_object(feature_value: &MLFeatureValue) -> &AnyObject {
    unsafe { &*(feature_value as *const MLFeatureValue).cast::<AnyObject>() }
}

fn ns_number_array(values: &[usize]) -> Retained<NSArray<NSNumber>> {
    let numbers: Vec<Retained<NSNumber>> = values
        .iter()
        .copied()
        .map(|value| NSNumber::new_isize(value as isize))
        .collect();
    NSArray::from_retained_slice(&numbers)
}

fn contiguous_strides(shape: &[usize]) -> Vec<usize> {
    let mut strides = vec![1usize; shape.len()];
    for index in (0..shape.len().saturating_sub(1)).rev() {
        strides[index] = strides[index + 1] * shape[index + 1];
    }
    strides
}

fn multi_array_f32(
    values: &[f32],
    shape: &[usize],
    deallocator: &RcBlock<dyn Fn(NonNull<c_void>)>,
) -> Result<Retained<MLMultiArray>> {
    multi_array(
        values.as_ptr().cast::<c_void>() as *mut c_void,
        shape,
        MLMultiArrayDataType::Float32,
        deallocator,
    )
}

fn multi_array_i32(
    values: &[i32],
    shape: &[usize],
    deallocator: &RcBlock<dyn Fn(NonNull<c_void>)>,
) -> Result<Retained<MLMultiArray>> {
    multi_array(
        values.as_ptr().cast::<c_void>() as *mut c_void,
        shape,
        MLMultiArrayDataType::Int32,
        deallocator,
    )
}

fn multi_array(
    ptr: *mut c_void,
    shape: &[usize],
    data_type: MLMultiArrayDataType,
    deallocator: &RcBlock<dyn Fn(NonNull<c_void>)>,
) -> Result<Retained<MLMultiArray>> {
    let ptr = NonNull::new(ptr).ok_or_else(|| anyhow!("input tensor had null data pointer"))?;
    let ns_shape = ns_number_array(shape);
    let ns_strides = ns_number_array(&contiguous_strides(shape));
    unsafe {
        MLMultiArray::initWithDataPointer_shape_dataType_strides_deallocator_error(
            MLMultiArray::alloc(),
            ptr,
            &ns_shape,
            data_type,
            &ns_strides,
            Some(deallocator),
        )
    }
    .map_err(|error| anyhow!("failed to create CoreML MLMultiArray: {error}"))
}

#[allow(deprecated)]
fn extract_output(array: &MLMultiArray) -> Result<(Vec<f32>, Vec<usize>)> {
    let (count, ptr, dtype, shape, strides) = unsafe {
        (
            array.count() as usize,
            array.dataPointer(),
            array.dataType(),
            array.shape(),
            array.strides(),
        )
    };
    let shape: Vec<usize> = (0..shape.len())
        .map(|index| shape.objectAtIndex(index).as_isize() as usize)
        .collect();
    let strides: Vec<isize> = (0..strides.len())
        .map(|index| strides.objectAtIndex(index).as_isize())
        .collect();
    let data = match dtype {
        MLMultiArrayDataType::Float32 => {
            read_output(ptr.as_ptr() as *const f32, count, &shape, &strides)?
        }
        MLMultiArrayDataType::Int32 => {
            read_output(ptr.as_ptr() as *const i32, count, &shape, &strides)?
                .into_iter()
                .map(|value| value as f32)
                .collect()
        }
        MLMultiArrayDataType::Float16 => {
            read_output(ptr.as_ptr() as *const u16, count, &shape, &strides)?
                .into_iter()
                .map(f16_to_f32)
                .collect()
        }
        _ => bail!("unsupported CoreML output dtype: {dtype:?}"),
    };
    Ok((data, shape))
}

fn read_output<T: Copy>(
    ptr: *const T,
    count: usize,
    shape: &[usize],
    strides: &[isize],
) -> Result<Vec<T>> {
    if shape.len() != strides.len() {
        bail!("shape/stride rank mismatch: shape={shape:?} strides={strides:?}");
    }
    if contiguous_strides(shape)
        .into_iter()
        .map(|s| s as isize)
        .eq(strides.iter().copied())
    {
        if count == 0 {
            return Ok(Vec::new());
        }
        return Ok(unsafe { slice::from_raw_parts(ptr, count) }.to_vec());
    }

    let total = shape.iter().product::<usize>();
    let mut out = Vec::with_capacity(total);
    for linear in 0..total {
        let mut rem = linear;
        let mut offset = 0isize;
        for axis in (0..shape.len()).rev() {
            let dim = shape[axis];
            let idx = rem % dim;
            rem /= dim;
            offset += idx as isize * strides[axis];
        }
        out.push(unsafe { *ptr.offset(offset) });
    }
    Ok(out)
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = ((bits & 0x8000) as u32) << 16;
    let exp = ((bits & 0x7c00) >> 10) as u32;
    let frac = (bits & 0x03ff) as u32;
    let f32_bits = if exp == 0 {
        if frac == 0 {
            sign
        } else {
            let mut frac_norm = frac;
            let mut exp_adjust = 0i32;
            while (frac_norm & 0x0400) == 0 {
                frac_norm <<= 1;
                exp_adjust -= 1;
            }
            frac_norm &= 0x03ff;
            let exp32 = (127 - 15 + 1 + exp_adjust) as u32;
            sign | (exp32 << 23) | (frac_norm << 13)
        }
    } else if exp == 0x1f {
        sign | 0x7f80_0000 | (frac << 13)
    } else {
        let exp32 = exp + (127 - 15);
        sign | (exp32 << 23) | (frac << 13)
    };
    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pads_to_fluid_frontend_window() {
        let (padded, actual) = pad_frontend_audio(&vec![0.0; 16_001]);
        assert_eq!(padded.len(), MAX_MODEL_SAMPLES);
        assert_eq!(actual % SAMPLES_PER_ENCODER_FRAME, 0);
        assert!(actual >= 16_001);
    }

    #[test]
    fn converts_sentencepiece_tokens_to_word_timings() {
        let vocab = vec![
            Some("<unk>".to_string()),
            Some("▁hel".to_string()),
            Some("lo".to_string()),
            Some("▁world".to_string()),
            Some("!".to_string()),
        ];
        let words =
            tokens_to_word_timings(&[1, 2, 3, 4], &[0, 1, 4, 5], &[], &[1, 2, 1, 1], &vocab);
        assert_eq!(
            words,
            vec![
                WordTiming {
                    start_ms: 0,
                    end_ms: 240,
                    text: "hello".into()
                },
                WordTiming {
                    start_ms: 320,
                    end_ms: 480,
                    text: "world!".into()
                },
            ]
        );
    }

    #[test]
    fn extracts_encoder_frames_from_hidden_time_layout() {
        let tensor = CoreMlTensor {
            data: vec![1.0, 2.0, 3.0, 10.0, 20.0, 30.0],
            shape: vec![1, 2, 3],
        };
        let frames = EncoderFrames::new(&tensor, 3, 2).unwrap();
        assert_eq!(frames.frame(1), vec![2.0, 20.0]);
    }

    #[test]
    fn rolling_audio_buffer_append_slice_and_trim() {
        let mut buffer = RollingAudioBuffer::new(SAMPLE_RATE);
        buffer.append(&[1.0; 16_000]);
        buffer.append(&[2.0; 8_000]);

        assert_eq!(buffer.duration_ms(), 1_500);
        assert_eq!(buffer.end_ms(), 1_500);
        assert_eq!(buffer.slice(250, 750).len(), 8_000);
        assert!(buffer.slice(250, 750).iter().all(|sample| *sample == 1.0));
        let crossing = buffer.slice(900, 1_100);
        assert_eq!(crossing.len(), 3_200);
        assert!(crossing[..1_600].iter().all(|sample| *sample == 1.0));
        assert!(crossing[1_600..].iter().all(|sample| *sample == 2.0));

        buffer.trim_before(1_000);
        assert_eq!(buffer.base_sample_index(), 16_000);
        assert_eq!(buffer.duration_ms(), 500);
        assert_eq!(buffer.slice(0, 1_500).len(), 8_000);
    }

    #[test]
    fn checkpoint_selection_restores_before_overlap_and_avoids_one_second_context() {
        let checkpoints = checkpoint_times(&[0, 5_000, 10_000, 15_000]);
        let config = CoreMlStreamingConfig {
            overlap_ms: 2_000,
            ..Default::default()
        };

        let plan = select_decode_plan(15_000, &checkpoints, &config, 16_000);

        assert_eq!(plan.kind, DecodePlanKind::OverlapReplay);
        assert_eq!(plan.start_ms, 10_000);
        let checkpoint = &checkpoints[plan.restore_checkpoint.unwrap()];
        let desired_overlap_start = 13_000;
        assert!(checkpoint.time_ms <= desired_overlap_start);
        assert!(16_000 - checkpoint.time_ms <= max_model_window_ms());
        assert!(16_000 - plan.start_ms > 1_000);
        assert_eq!(
            checkpoint.frame_index,
            ms_to_encoder_frame(checkpoint.time_ms)
        );
    }

    #[test]
    fn checkpoint_selection_never_uses_checkpoint_after_overlap_start() {
        let checkpoints = checkpoint_times(&[0, 15_000]);
        let config = CoreMlStreamingConfig {
            overlap_ms: 2_000,
            ..Default::default()
        };

        let plan = select_decode_plan(15_000, &checkpoints, &config, 16_000);

        assert_eq!(plan.kind, DecodePlanKind::ConservativeNoOverlap);
        assert_eq!(plan.start_ms, 15_000);
        assert!(plan.restore_checkpoint.is_none());
    }

    #[test]
    fn checkpoint_interval_caps_internal_progressive_steps() {
        let config = CoreMlStreamingConfig {
            step_ms: 15_000,
            checkpoint_interval_ms: 5_000,
            ..Default::default()
        }
        .normalized();

        let mut decoded = 0;
        let mut ends = Vec::new();
        while decoded < aligned_streaming_target_ms(15_000) {
            decoded = next_decode_end_for(decoded, 15_000, &config);
            ends.push(decoded);
        }

        assert_eq!(ends, vec![4_960, 9_920, 14_880, 14_960]);
        assert!(ends.iter().all(|end| is_encoder_frame_aligned_ms(*end)));
    }

    #[test]
    fn streaming_step_boundaries_floor_to_encoder_frames() {
        let config = CoreMlStreamingConfig::default().normalized();

        assert_eq!(aligned_streaming_target_ms(1_000), 960);
        assert_eq!(next_decode_end_for(0, 1_000, &config), 960);
        assert_eq!(ms_to_encoder_frame_floor(1_000), 12);
        assert_eq!(encoder_frame_to_ms(ms_to_encoder_frame_floor(1_000)), 960);

        let mut decoded = 0;
        let mut ends = Vec::new();
        while decoded < aligned_streaming_target_ms(5_000) {
            decoded = next_decode_end_for(decoded, 5_000, &config);
            ends.push(decoded);
        }
        assert_eq!(ends.last().copied(), Some(4_960));
        assert!(ends.iter().all(|end| is_encoder_frame_aligned_ms(*end)));
        assert_eq!(ms_to_encoder_frame_floor(5_000), 62);
        assert_eq!(encoder_frame_to_ms(ms_to_encoder_frame_floor(5_000)), 4_960);
    }

    #[test]
    fn zero_overlap_streaming_config_uses_strict_continuation_without_overlap_replay() {
        let config = CoreMlStreamingConfig {
            overlap_ms: 0,
            ..Default::default()
        }
        .normalized();
        assert_eq!(config.overlap_ms, 0);

        let checkpoints = checkpoint_times(&[0, 4_960]);
        let plan = select_decode_plan(4_960, &checkpoints, &config, 5_920);
        assert_eq!(plan.kind, DecodePlanKind::ConservativeNoOverlap);
        assert_eq!(plan.start_ms, 4_960);
        assert!(plan.restore_checkpoint.is_none());
    }

    #[test]
    fn offline_long_form_stride_is_frame_aligned_below_model_window() {
        let chunk = offline_chunk_samples();
        let stride = offline_chunk_stride_samples();
        assert_eq!(chunk % SAMPLES_PER_ENCODER_FRAME, 0);
        assert_eq!(stride % SAMPLES_PER_ENCODER_FRAME, 0);
        assert!(chunk < MAX_MODEL_SAMPLES);
        assert!(stride < MAX_MODEL_SAMPLES);
        assert_eq!(chunk, 238_080);
        assert_eq!(chunk / SAMPLES_PER_ENCODER_FRAME, 186);
        assert_eq!(stride, 206_080);
        assert_eq!(stride / SAMPLES_PER_ENCODER_FRAME, 161);
        assert_eq!(offline_overlap_frames(), 25);
        assert_eq!(samples_to_ms(chunk as u64, SAMPLE_RATE), 14_880);
        assert_eq!(samples_to_ms(stride as u64, SAMPLE_RATE), 12_880);
    }

    #[test]
    fn token_window_merge_dedupes_shifted_overlap() {
        let left = vec![tw(1, 100), tw(2, 110), tw(3, 120), tw(4, 130)];
        let right = vec![tw(2, 111), tw(3, 121), tw(4, 131), tw(5, 145)];

        let merged = merge_token_windows(&left, &right, 25);

        assert_eq!(token_ids(&merged), vec![1, 2, 3, 4, 5]);
        assert_eq!(merged[1].timestamp, 110);
    }

    #[test]
    fn token_window_merge_midpoint_drops_overlap_alternates() {
        let left = vec![tw(1, 100), tw(90, 110), tw(3, 120)];
        let right = vec![tw(91, 112), tw(92, 121), tw(4, 140)];

        let merged = merge_token_windows(&left, &right, 25);

        assert_eq!(token_ids(&merged), vec![1, 90, 92, 4]);
    }

    #[test]
    fn checkpoint_interval_uses_frame_counts_not_wall_clock_drift() {
        let interval_ms = 5_000;
        let frame_4960 = ms_to_encoder_frame_floor(4_960);
        let frame_5000 = ms_to_encoder_frame_floor(5_000);
        assert_eq!(frame_4960, 62);
        assert_eq!(frame_5000, 62);
        assert!(checkpoint_interval_elapsed(0, frame_4960, interval_ms));
        assert!(checkpoint_interval_elapsed(0, frame_5000, interval_ms));
        assert!(!checkpoint_interval_elapsed(0, 61, interval_ms));
    }

    #[test]
    fn checkpoint_pruning_drops_entries_outside_model_window() {
        let mut checkpoints = checkpoint_times(&[0, 5_000, 10_000, 15_000]);

        prune_unusable_checkpoints(&mut checkpoints, 20_000, 12_000);

        let retained: Vec<u64> = checkpoints
            .iter()
            .map(|checkpoint| checkpoint.time_ms)
            .collect();
        assert_eq!(retained, vec![10_000, 15_000]);
    }

    #[test]
    fn checkpoint_pruning_keeps_at_least_one_checkpoint() {
        let mut checkpoints = checkpoint_times(&[0]);

        prune_unusable_checkpoints(&mut checkpoints, 60_000, 12_000);

        assert_eq!(checkpoints.len(), 1);
        assert_eq!(checkpoints[0].time_ms, 0);
    }

    #[test]
    fn rolling_audio_trim_floor_preserves_oldest_retained_checkpoint_audio() {
        let mut buffer = RollingAudioBuffer::new(SAMPLE_RATE);
        buffer.append(&vec![1.0; SAMPLE_RATE as usize * 20]);
        let mut checkpoints = checkpoint_times(&[0, 5_000, 10_000, 15_000]);

        prune_unusable_checkpoints(&mut checkpoints, 20_000, 12_000);
        buffer.trim_before(checkpoints.front().unwrap().time_ms);

        assert_eq!(buffer.base_sample_index(), 160_000);
        assert_eq!(buffer.end_ms(), 20_000);
        assert_eq!(buffer.slice(10_000, 12_000).len(), 32_000);
        assert!(buffer.slice(9_000, 10_000).is_empty());
    }

    #[test]
    fn token_window_pruning_keeps_overlap_before_committed_cutoff() {
        let mut windows = vec![tw(1, 10), tw(2, 20), tw(3, 30), tw(4, 40)];

        prune_token_windows_before(&mut windows, encoder_frame_to_ms(25));

        assert_eq!(token_ids(&windows), vec![3, 4]);
    }

    #[test]
    fn unaligned_terminal_tail_is_not_checkpoint_aligned() {
        assert_eq!(aligned_streaming_target_ms(3_000), 2_960);
        assert!(!is_encoder_frame_aligned_ms(3_000));
        assert!(is_encoder_frame_aligned_ms(2_960));
    }

    #[test]
    fn reconcile_is_append_only_and_replaces_hypothesis_conservatively() {
        let committed = vec![word(0, 300, "yes"), word(800, 1_000, "yes")];
        let decoded = vec![
            word(20, 320, "YES,"),
            word(1_150, 1_300, "yes"),
            word(3_700, 3_900, "tail"),
        ];

        let out = reconcile_streaming_words(&committed, 1_000, decoded, 4_000, 1_500);

        assert_eq!(out.committed[..2], committed[..]);
        assert_eq!(out.committed[2], word(1_150, 1_300, "yes"));
        assert_eq!(out.hypothesis, vec![word(3_700, 3_900, "tail")]);
        assert_eq!(out.committed_until_ms, 1_300);
    }

    #[test]
    fn fake_overlap_replay_matches_continuous_state_not_live_double_feed() {
        #[derive(Clone, Debug, PartialEq, Eq)]
        struct FakeState(Vec<u64>);

        fn replay(mut state: FakeState, start_s: u64, end_s: u64) -> FakeState {
            state.0.extend(start_s..end_s);
            state
        }

        let continuous = replay(FakeState(Vec::new()), 0, 16);
        let state_at_5 = replay(FakeState(Vec::new()), 0, 5);
        let state_at_10 = replay(state_at_5.clone(), 5, 10);
        let live_at_15 = replay(state_at_10.clone(), 10, 15);

        let checkpoints = checkpoint_times(&[0, 5_000, 10_000, 15_000]);
        let config = CoreMlStreamingConfig {
            overlap_ms: 2_000,
            ..Default::default()
        };
        let plan = select_decode_plan(15_000, &checkpoints, &config, 16_000);
        assert_eq!(plan.start_ms, 10_000);

        let replayed_from_checkpoint = replay(state_at_10, 10, 16);
        let double_fed_live_state = replay(live_at_15, 13, 16);

        assert_eq!(replayed_from_checkpoint, continuous);
        assert_ne!(double_fed_live_state, continuous);
        assert_eq!(
            double_fed_live_state.0,
            vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 13, 14, 15]
        );
    }

    fn checkpoint_times(times_ms: &[u64]) -> VecDeque<AsrCheckpoint> {
        times_ms
            .iter()
            .copied()
            .map(|time_ms| AsrCheckpoint {
                time_ms,
                frame_index: ms_to_encoder_frame(time_ms),
                state: TdtDecoderStateRust::new(),
                committed_word_len: 0,
                committed_until_ms: 0,
                decoded_until_ms: time_ms,
            })
            .collect()
    }

    fn word(start_ms: u64, end_ms: u64, text: &str) -> WordTiming {
        WordTiming {
            start_ms,
            end_ms,
            text: text.to_string(),
        }
    }

    fn tw(token: usize, timestamp: usize) -> TokenWindow {
        TokenWindow {
            token,
            timestamp,
            confidence: 0.9,
            duration: 1,
        }
    }

    fn token_ids(windows: &[TokenWindow]) -> Vec<usize> {
        windows.iter().map(|window| window.token).collect()
    }

    #[test]
    #[ignore = "requires local FluidAudio Parakeet CoreML assets"]
    fn smoke_loads_and_transcribes_silence() {
        let dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2")
                })
            })
            .expect("HOME or MARGINS_FLUID_COREML_MODEL_DIR must be set");
        if !dir.join("Preprocessor.mlmodelc").exists() {
            eprintln!(
                "skipping: {} does not contain FluidAudio CoreML assets",
                dir.display()
            );
            return;
        }
        let mut asr = FluidCoreMlAsr::from_dir(&dir, FluidCoreMlModelVersion::V2).unwrap();
        let audio = vec![0.0f32; SAMPLE_RATE as usize / 2];
        let out = asr.run_frontend(&audio).unwrap();
        let words = asr.transcribe_words(&audio).unwrap();
        eprintln!(
            "CoreML smoke: load={:?} pre={:?} enc={:?} mel={:?} encoder={:?} len={} words={:?}",
            out.timings.model_load,
            out.timings.last_preprocess,
            out.timings.last_encoder,
            out.mel_shape,
            out.encoder_shape,
            out.encoder_length,
            words
        );
        assert_eq!(out.padded_samples, MAX_MODEL_SAMPLES);
        assert_eq!(out.encoder_shape.first().copied(), Some(1));
        assert!(out.encoder_length > 0);
    }

    #[test]
    #[ignore = "requires local FluidAudio Parakeet CoreML assets"]
    fn smoke_streaming_silence_updates_without_words() {
        let dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2")
                })
            })
            .expect("HOME or MARGINS_FLUID_COREML_MODEL_DIR must be set");
        if !dir.join("Preprocessor.mlmodelc").exists() {
            eprintln!(
                "skipping: {} does not contain FluidAudio CoreML assets",
                dir.display()
            );
            return;
        }

        let asr = FluidCoreMlAsr::from_dir(&dir, FluidCoreMlModelVersion::V2).unwrap();
        let mut session = CoreMlAsrSession::new(asr, CoreMlStreamingConfig::default());
        session.append_audio(&vec![0.0f32; SAMPLE_RATE as usize * 3]);
        let update = session.update_until(1_000).unwrap();
        assert!(update.committed.is_empty());
        assert!(update.hypothesis.is_empty());
        let update = session.finish_until(3_000).unwrap();
        assert!(update.committed.is_empty());
        assert!(update.hypothesis.is_empty());
        assert_eq!(update.decoded_until_ms, 3_000);
        assert_eq!(session.decoded_until_ms(), 3_000);
    }

    #[test]
    #[ignore = "requires local FluidAudio Parakeet CoreML assets and MARGINS_COREML_SMOKE_WAV"]
    fn smoke_transcribes_wav_from_env() {
        let model_dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2")
                })
            })
            .expect("HOME or MARGINS_FLUID_COREML_MODEL_DIR must be set");
        let wav = match std::env::var_os("MARGINS_COREML_SMOKE_WAV") {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("skipping: MARGINS_COREML_SMOKE_WAV is not set");
                return;
            }
        };
        if !model_dir.join("Preprocessor.mlmodelc").exists() {
            eprintln!(
                "skipping: {} does not contain FluidAudio CoreML assets",
                model_dir.display()
            );
            return;
        }
        let audio = crate::audio::mono_16k_from_wav(&wav).unwrap();
        let mut asr = FluidCoreMlAsr::from_dir(&model_dir, FluidCoreMlModelVersion::V2).unwrap();
        let words = asr.transcribe_words(&audio).unwrap();
        eprintln!("CoreML wav smoke: {} words: {:?}", words.len(), words);
    }

    #[test]
    #[ignore = "requires macOS, FluidAudio CoreML assets, and stereo MARGINS_COREML_SMOKE_WAV"]
    fn smoke_transcribes_stereo_wav_through_transcription_only_seam() {
        let model_dir = std::env::var("MARGINS_FLUID_COREML_MODEL_DIR")
            .ok()
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home)
                        .join("Library/Application Support/FluidAudio/Models/parakeet-tdt-0.6b-v2")
                })
            })
            .expect("HOME or MARGINS_FLUID_COREML_MODEL_DIR must be set");
        let wav = match std::env::var_os("MARGINS_COREML_SMOKE_WAV") {
            Some(path) => PathBuf::from(path),
            None => {
                eprintln!("skipping: MARGINS_COREML_SMOKE_WAV is not set");
                return;
            }
        };
        if !model_dir.join("Preprocessor.mlmodelc").exists() {
            eprintln!(
                "skipping: {} does not contain FluidAudio CoreML assets",
                model_dir.display()
            );
            return;
        }
        let wav_info = crate::audio::load_wav(&wav).unwrap();
        if wav_info.channels < 2 {
            eprintln!(
                "skipping: {} is not stereo ({} channel(s))",
                wav.display(),
                wav_info.channels
            );
            return;
        }

        let transcript = transcribe_wav_to_transcript(&model_dir, &wav).unwrap();
        eprintln!(
            "CoreML stereo seam smoke: counts={:?}, entries={:?}",
            transcript.channel_word_counts, transcript.entries
        );
        assert!(transcript
            .channel_word_counts
            .iter()
            .any(|(channel, _)| *channel == 0));
        assert!(transcript
            .channel_word_counts
            .iter()
            .any(|(channel, _)| *channel == 1));
    }
}
