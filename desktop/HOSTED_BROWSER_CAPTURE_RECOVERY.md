# Hosted browser capture recovery

Hosted recording has two independent audio transports:

- `MediaRecorder` WebM chunks are the durable source used to produce the final WAV.
  The normal finalizer is Rust-native: WebM is demuxed in process, Opus is
  decoded in process directly at 16 kHz, and mono samples are streamed to a
  temporary WAV that is synced and atomically renamed into place. It does not
  retain the full decoded recording in memory and does not resolve or spawn
  `ffmpeg`.
- Web Audio Float32 PCM batches are supplementary input for live Parakeet transcription.

The native finalizer accepts at most 128 MiB of WebM. That limit is deliberately
below the upload-storage ceiling because `matroska-demuxer` owns `Frame`
allocation before Margins can inspect and reject an individual packet; the file
limit is therefore the honest pre-packet allocation ceiling. A separate 256 KiB
post-demux Opus packet cap remains in place for malformed blocks.

Set `MARGINS_HOSTED_WEBM_FINALIZER=ffmpeg` only to use the legacy compatibility
fallback during migration. In that mode `FFMPEG_BIN` is strict when set; without
the opt-in, an invalid or absent `FFMPEG_BIN` cannot break ordinary hosted Finish.

Every server allocation receives a random 128-bit `recordingId`. This non-secret ID is the
registry key across project workdirs and is carried in JSON command bodies or the
`X-Margins-Recording-Id` header. The separate browser `ownerId` is a capability: it is carried
only in JSON bodies or `X-Margins-Capture-Owner`, is never returned in status, and is never
written to recovery manifests. Display names are presentation only and must not select a
mutation target.

Failed finalizations write an atomic `*.recovery.json` manifest beside the private recording
artifacts. Startup scans the fallback and configured project workdirs, restores every valid
manifest in stable oldest-first recovery status, and gives it no usable persisted owner.
The manifest is synced to disk in a `finalizing` phase before the server removes the capture
from the active registry or starts the first transcode. It changes to `failed` if any later
stage fails, changes to `cleanupPending` before discard, and is removed only after all final
artifacts and metadata are confirmed ready. A crash at any of those boundaries therefore
reconstructs a scoped recovery rather than orphaning the private WebM.
After restart, a browser must explicitly claim the recording ID and then hydrate its durable
memo before sync, Finish, Discard, or backchannel work. Discard installs/persists cleanup state
before deleting anything; a file or database deletion error leaves the same ID retryable.

Recovery manifests are an internal registry, not a best-effort cache. Startup requires the
filename and JSON recording IDs to be the same canonical lowercase 128-bit value, rejects
duplicate IDs, and validates artifact paths and telemetry counters. Invalid entries are moved
to a deterministic `.quarantine` name and reported instead of being silently selected or
overwriting another recovery.

Browser capture protocol version 2 is negotiated before hosted state initializes. Every
mutation and raw audio request carries `X-Margins-Capture-Protocol: 2`; mixed-version open tabs
receive an explicit reload-required error. Boot always requests the ordered recovery collection,
independently of the ordinary session list's `isRecording` flags. A retained failure does not
block a new Start. The UI selects, claims, hydrates, polls, and mutates one explicit recording ID,
so an active recording and any number of same-name recoveries remain independent.

WebM requests are uploaded sequentially because append order is part of the WebM container.
The queue, each request, and Finish drain are bounded. If one or more chunks fail but the server
successfully finalizes the chunks it received, that session is terminal—not rolled back to a
fictional active capture. The browser logs the explicit subset warning; during capture the
durable-audio warning remains visible from `uploadErrors`.
Each upload also carries a recording-local sequence number. The server accepts only the next
sequence, while an exact retry is idempotent only when its length and fingerprint match. A late
timed-out request therefore cannot append after a newer chunk or append the same bytes twice.
Status exposes server-computed monotonic age values for WebM, live PCM, and owner-heartbeat
freshness; browser wall-clock timestamps remain compatibility fallback data only.

## Remaining browser-runtime verification

Deterministic tests cover ownership, timeouts, ordering, response-loss reconciliation, restart
reconstruction, and failure retention, but they cannot establish physical Chrome audio
delivery. Before hosted rollout, run this exact check in production-like HTTPS Chrome:

1. Start a capture, speak continuously for at least 15 seconds, pause for 5 seconds, resume,
   then speak another 15 seconds.
2. Confirm `/api/audio/chunk` requests contain protocol, capture, owner, and sequence headers,
   remain ordered, and status shows increasing WebM chunk/byte counters after resume.
3. Confirm `/api/live-audio/pcm` contains both capture headers and status shows increasing PCM
   batch/sample counters; verify a Parakeet checkpoint reports non-zero accepted and decoded
   microphone samples plus recognizable words.
4. Reload the owner tab. Before the lease/transport timeout, confirm it says capture is in
   another tab and takeover is unavailable. After both expire, claim the exact recording ID,
   confirm memo hydration, then Finish and play the resulting WAV.
5. Repeat with two projects using the same display name and force both finalizations to fail;
   confirm each recovery opens, retries, and discards independently by recording ID.
6. Throttle or stall one WebM request, click Finish, and confirm the drain deadline returns,
   the server-finalized subset is not shown as actively recording, and the upload-loss warning
   is visible in browser diagnostics.
7. Kill the server separately after the initial `finalizing` manifest is synced, after
   transcode, and after metadata/readiness work; restart each time and confirm the exact ID is
   listed and can be claimed, retried, or discarded. Corrupt a copied manifest and confirm it is
   quarantined and visibly reported without hiding valid recoveries.

Chrome may still suspend `ScriptProcessorNode` callbacks in a background tab despite a live
MediaStream, and autoplay policy can still leave `AudioContext.resume()` dependent on user
activation. Startup failures are surfaced without aborting MediaRecorder, but only the physical
check above can validate those policies for the deployed Chrome/version/origin combination.
