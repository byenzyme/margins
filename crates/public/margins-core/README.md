# margins-core

`margins-core` is the dependency-light public contract crate for Margins. It
contains owned IDs and values plus synchronous, in-process ports for capture,
session persistence, events, ASR, and diarization. Versioned meeting-runtime
wire DTOs are re-exported from `margins-meeting-protocol` as
`margins_core::wire`; they are not duplicated here.

The crate has no default features and no native or persistence implementation.
In particular, it does not include microphone/system capture, CPAL, CoreAudio,
CIDRE, Tauri, SQLite, model runtimes, threads, queues, platform handles, or UI
policy. Applications provide those implementations at their composition root.
Public-only builds can install `UnavailableCaptureProvider` and still use the
rest of the contract graph.

```rust
use margins_core::{CaptureProvider, UnavailableCaptureProvider};

let provider = UnavailableCaptureProvider::default();
assert!(!provider.capabilities().available);
```

Run the portable contract lane with:

```text
cargo test -p margins-core --no-default-features --all-targets
```

The root `margins` crate currently exposes this crate as `margins::core` and
provides lossless conversions from its live PCM, ASR-word, and diarization-turn
values. Existing recorder and SQLite/session implementations remain unchanged
and do not implement these ports yet.
