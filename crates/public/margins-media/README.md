# margins-media

Portable audio-file, transcript, and caller-supplied-PCM model adapters for
Margins. The crate decodes, probes, writes, resamples, chunks, and combines
audio; it also owns deterministic transcript transforms and checked media-frame
math.

The default feature set is empty. This crate does not open recording devices,
own capture callbacks or actors, implement capture recovery policy, depend on a
desktop runtime, or expose CPAL, CoreAudio, CIDRE, Tauri, or platform handles.
Optional `parakeet-onnx`, `coreml-asr` (macOS), and
`polyvoice-diarization` features add model loading and inference only. The
`providers::UnavailableAsr` and `providers::UnavailableDiarization` types make
no-feature composition explicit.

```rust
use margins_media::audio::resample_mono_linear;

let output = resample_mono_linear(&[0.0, 0.5, 1.0, 0.5], 4, 2);
assert_eq!(output, vec![0.0, 1.0]);
```
