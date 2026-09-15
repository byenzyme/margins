# Mac audio transport verification

Status: provisional independent verifier record through candidate
`49a6d9a225d65a1ad4188db63017157f49fd2821`. Exact Mac-to-Linux SSH Opus
transport, offline ASR, duration accounting, legacy PCM recovery, and sustained
capture-time headroom pass on this revision. A prior exact candidate exposed a
blocking quadratic spool/request-cadence defect; the failure, repair, and
like-for-like retest are preserved below. Native Pause/Resume/attach on the
current revision still requires a user-owned Terminal process because the BB
execution host has a distinct denied TCC identity.

Verifier thread: `thr_n2qdmhncai`.

## Revision, build, and boundary discipline

- Canonical origin was verified as
  `https://github.com/byenzyme/margins-desktop.git`.
- Worktree:
  `/Users/example/.bb-machines/jpham-server.getbb.app/worktrees/env_jqhw7iz8we/margins`.
  Bootstrap/main was not used as candidate evidence.
- Verification branch `bb/mac-e2e-verify-opus-49a6d9a2` was created at the
  exact fetched candidate object. The source tree was clean before evidence
  collection.
- Mac CLI:
  `/Users/example/Hacks/gmk/margins-cargo-target/debug/margins-private`,
  SHA-256
  `39693825adbf6c5042b9c90370addce1bf2be1df45dcb52af2859d77e48be8ae`.
  `__release-smoke` embeds the full candidate SHA, `dirty=false`, branch
  `bb/mac-e2e-verify-opus-49a6d9a2`, build time
  `2026-09-15T20:47:55Z`, native recorder capture and TUI available, and recall
  unavailable for the intentional audio-only feature slice.
- Matching Linux server SHA-256:
  `0962624c328108c6819840253ffb4ca92ba6ecabae015d628fb295f8d08c0c4e`.
  Matching Linux SSH discovery CLI SHA-256:
  `533087dbd42a37661bebe8a8867d147e074879fcaf2989e76fdce89a937eb427`.
- The isolated server state is
  `/tmp/margins-opus-candidate-9353e00fa` (0700), instance
  `opus-candidate-49a6d9a225d6`, Workspace `mac-e2e`, and listener only
  `127.0.0.1:38871`. Initial PID 2692838 was stopped for the scoped recovery
  test. Current exact `49a6d9a2` PID 4100357 was independently matched through
  `/proc/<pid>/exe` before the sustained run.
- Authenticated capabilities reported protocol 1, mono 16 kHz Opus
  PacketStream preferred, maximum 1 MiB audio body, batch limit 16, 512 MiB
  reserve, ASR available using pinned Parakeet TDT 0.6b v2, and recall
  unavailable.
- `ssh -G bs-server` retained the normal known-host files and strict host check
  policy. The service credential returned by fixed-command discovery was
  consumed in-process and never printed. Tailscale SSH surfaced its normal
  re-authorization URL during readback; it was shown to the user and completed
  without bypassing policy.
- No installed app, primary-checkout change, GitHub push, public deployment,
  release, global network fault, credential change, TCC reset, or user-data
  cleanup was performed.

The verifier read the candidate versions of these documents and used them as
the acceptance map:

- `docs/remote-workspace-implementation.md`
- `docs/capture-refactor-and-testing.md`
- `docs/remote-workspace-implementation-coverage.md`
- `docs/capture-refactor-implementation-coverage.md`

## Exact build and composition commands

```sh
git fetch 'bs-server:/home/example/.bb/worktrees/env_74z8n6ke4k/aside-desktop' \
  49a6d9a225d65a1ad4188db63017157f49fd2821
git switch -c bb/mac-e2e-verify-opus-49a6d9a2 \
  49a6d9a225d65a1ad4188db63017157f49fd2821

scripts/cargo-lane shared -- cargo build -p margins \
  --no-default-features --features audio-capture --bin margins-private

scripts/cargo-lane disposable -- cargo test -p margins \
  --no-default-features --features audio-capture \
  --test private_cli_composition \
  packaged_binary_reports_private_native_composition -- --exact --nocapture
```

The composition test passed 1/1. Evidence binaries used the managed shared
lane; tests and examples used the disposable lane.

## Fixtures and codec-quality matrix

The exact SSH fixture used separate mono signed-16 lanes at 16 kHz:

| Lane | Samples / duration | PCM SHA-256 |
| --- | ---: | --- |
| mic, synthetic known speech | 97,964 / 6.12275 s | `a88e718bd931fc87d6015e5b846082595e1bdb3c4f14ca3cc13ec9f94ec1dc68` |
| system, aligned silence | 97,964 / 6.12275 s | `bf78d8f24d4537b85911acdd2755fcffc48fb22b176289f86ba17cf0bf3fdc5b` |

Expected speech: “Margins verification. Alpha lane four three seven. System
lane nine one three. Stop now.” These are scoped synthetic fixtures, not meeting
or user recordings.

The larger evaluation corpus is owner-only under
`/tmp/margins-opus-eval-thr_n2qdmhncai`; its 74-file manifest is hash-identical
to the scoped Linux copy and has SHA-256
`83c530749b03fbecf7389779c13ea49c08a31bcfa054a265109e9581a9a21767`.
It consists of ephemeral macOS `say` output made only for this evaluation and
is not claimed to be redistributable. No audio or transcript body enters git.

The exact post-commit 28-row pinned-v2 result after 500 ms grouping is
`/tmp/margins-opus-quality-matrix-49a6d9a225d6.json` (0600), 10,354 bytes,
SHA-256
`b90fcb5d19adec7c03f8454b1f4a431d70d2840e17ac37736a06d00209bc00f8`.

- At 24 kbps/lane, normalized WER and configured critical-token hits matched
  PCM in all six speech cases. First/last word boundaries stayed within 80 ms.
- Production 24 kbps PacketStream bytes stayed below the matrix's 15% of PCM
  ceiling for every speech case. The 12.81-second zero lane was 7,243 bytes
  (1.77% of 409,920-byte PCM) and produced zero words for PCM and Opus
  16/24/32.
- Counterevidence: the ASR baseline itself was weak on Indian names/numbers
  (PCM WER 0.478, 0/5 critical phrases) and overlap (PCM WER 0.769, 1/6).
  Matching that result establishes no observed codec regression on this small
  corpus, not general ASR quality.
- Both 16 and 32 kbps regressed quiet/noisy US WER from 0.531 to 0.594 and lost
  one critical phrase; 24 kbps did not. This supports the provisional 24 kbps
  default only for the tested corpus.
- Matrix `encode_micros` is encoder plus durable-spool wall time including
  fsync. Only the paced harness's thread clock isolates producer CPU.
- Exact 24 kbps PacketStream sizes after grouping were 39,021 (US), 34,763
  (quiet), 40,266 (noisy), 27,562 (India), 30,247 (GB), 17,883 (overlap), and
  7,243 bytes (silence). These grouping-specific results replace prior
  candidate artifact hashes.

The selected Linux model is Parakeet TDT 0.6b **v2**, not v3: pinned export
`smcleod/parakeet-tdt-0.6b-v2-int8` revision
`d64884b484b919e9656d0b70cb95dfdc98852bef`. Its encoder is 652,282,300
bytes, decoder/joint 8,998,557 bytes, and vocabulary 9,384 bytes: 661,290,241
bytes total (661.3 MB decimal / 630.7 MiB), plus the scoped official ONNX
Runtime 1.24.4. The existing Mac logical-v2 CoreML bundle is 464,413,250 bytes
(442.9 MiB), dominated by a 445,187,200-byte 6-bit-palettized mixed-precision
encoder weight. The apparent size difference is platform graph/quantization
packaging, not a smaller-language model on Mac or an unreported Linux download.

Opus is lossy, so integrity is not asserted against source PCM bytes. The
transport check hashes immutable encoded PacketStream bytes and validates a
deterministic stateful decode with declared pre-skip/source trimming. The real
encoder lookahead is 104 input samples (312 units on the 48 kHz Opus clock).
Original 48 kHz device bytes are not claimed to survive the approved one-time
anti-aliased conversion to 16 kHz.

## Historical `9353e00f` paced SSH result

Environment and production example:

```sh
export MARGINS_REMOTE=ssh://bs-server
export MARGINS_WORKSPACE=mac-e2e
export MARGINS_SSH_REMOTE_BINARY=/workspace/projects/margins-cargo-target/debug/margins-private
export MARGINS_SSH_REMOTE_DATA_DIR=/tmp/margins-opus-candidate-9353e00fa/data
export MARGINS_TRANSFER_DIR=/tmp/margins-opus-mac-9353e00fa-run1/transfers

MARGINS_FIXTURE_MIC_S16LE=/tmp/margins-mac-verify-thr_n2qdmhncai/fixtures/spoken-fixture-16k-mono-s16.pcm \
MARGINS_FIXTURE_SYSTEM_S16LE=/tmp/margins-mac-verify-thr_n2qdmhncai/fixtures/system-silence-16k-mono-s16.pcm \
MARGINS_FIXTURE_PACED=1 \
scripts/cargo-lane disposable -- cargo run -p margins-workflows \
  --example remote_opus_transport --quiet
```

Session/transfer `opus-fixture-1789501628472` used the same ID on client and
server. It produced exactly 62 commands per lane, sequences 0–61; the final
block starts at 6,100 ms and carries 23 ms. The manifest has 124 durable audio
ACK files, zero pending frames, one close, one memo ACK, and `completed=true`.
Close and finalize both use 6,123 ms.

The server independently recorded 124 audio ACK events, one segment finalize,
one session finalize, 127 distinct receipts, and no duplicate event. Its first
audio ACK was 19:47:11.124Z, last audio ACK 19:47:17.216Z, segment finalize
19:47:17.227Z, and authority session finalize 19:47:17.579Z. Delayed
decode/projection and ASR did not alter the pinned media duration.

Canonical encoded artifacts are deterministic across this Mac SSH run, the
lost-ACK recovery run, and the Linux same-host baseline:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| microphone PacketStream | 19,927 | `1b7c114d69750e38f4f514e45028392ccd554c27abcaa9cd3279229f23cb4886` |
| system PacketStream | 5,055 | `1adc54c1e3e20755d3b672b61d904761d96e930ec534cf152782cf2cd376198e` |
| transcript | 622 | retained/readable; body not recorded in this report |

ASR job `transcribe:opus-fixture-1789501628472` completed on attempt 1 at
progress 1.0 with no failure. Readback over the actual SSH adapter recognized
all expected semantic tokens and preserved the timed memo. The authority
finalize event precedes artifact projection by about 3.66 seconds and job
completion by about 6.75 seconds, demonstrating that ASR is asynchronous rather
than a device-release or finalize-response prerequisite.
The same adapter returned memo revision `v1-d5ef6b404db6641e` with its 0.25 s
timing and a literal `null` note association (exit 0), rather than confusing a
valid absent value with a missing response field.

The exact Linux same-host baseline for the same fixtures was 124 commands, 16
HTTP batches, 24,982 encoded bytes, 76,338 attempted batch-body bytes, maximum
backlog 14 commands / 2,999 bytes / 676 ms, 12 pending at Stop, last ACK +355
ms, drain 930 ms, aggregate request time 4,055 ms, producer thread CPU 2,184 ms,
and encode-plus-spool wall 2,606 ms.

The independently captured real SSH run with a durable post-memo stop reported:

| Metric | Actual SSH value |
| --- | ---: |
| capture wall / media boundary | 6,241 / 6,123 ms |
| durable commands / HTTP batches | 124 / 17 |
| encoded payload / attempted full bodies | 24,982 / 76,346 bytes |
| producer CPU / encode-plus-spool wall | 1,958 / 4,597 ms |
| maximum pending | 51 commands / 9,241 bytes / 3,208 ms oldest |
| pending at Stop | 49 commands / 9,241 bytes |
| last input ACK after Stop / complete drain | 3,164 / 3,976 ms |
| aggregate request time | 6,816 ms |

The two-lane source PCM is 391,856 bytes, so the canonical Opus payload is
6.38% overall (mic speech 10.17%, system silence 2.58%). This is a large byte
and request reduction versus the earlier PCM path, whose two 100 ms lanes made
about 20 requests/s and required roughly 17 seconds of post-device-release
drain in one 7.432-second user run.

There is still material counterevidence. The actual SSH average was about 401
ms/batch. At the advertised maximum of eight commands, capacity is about 19.95
commands/s, essentially equal to the two-lane 100 ms producer rate of 20
commands/s. The 6.1-second run therefore accumulated 3.2 seconds of pending age
and 4.0 seconds of Stop drain. Bytes are bounded and recoverable, but this run
does not establish steady-state bounded age for a long session; batch size 8
has no observed throughput margin on this path.

### `ddeb6f2d` sustained SSH counterevidence

The implementation raised the advertised/accepted catch-up batch from 8 to 16
and the partial-batch age from 320 to 500 ms without changing the 100 ms durable
command, journal, or ACK identities. The exact Mac source suite then passed
18/18. Its paced case reported capture 3,413 ms, maximum pending 12 commands /
984 bytes / 792 ms, nine commands pending at Stop, 508 ms drain, six requests
for 40 commands, and 1,228 ms aggregate request time. The canonical native
composition test also passed 1/1.

That short local-delay regression did not predict the actual SSH path. The
verifier repeated the speech/silence pair ten times without changing its PCM
content, producing separate immutable lanes of 979,640 frames (61.2275 s) and
1,959,280 bytes each:

| Lane | PCM SHA-256 |
| --- | --- |
| mic speech repeated 10 times | `da473d4f02b89407c1ad1cfae87560aee709ba94ee1ed3e1b561cbd06f5c3d86` |
| aligned system silence | `7a084babe457c2293ae967601fb604972cbb4f1367c84f3fc69b86b4c417b909` |

Session/transfer `opus-fixture-1789503580665` completed through the production
SSH adapter and retained the same ID on both sides. The actual result was:

| Metric | Exact `ddeb6f2d` SSH value |
| --- | ---: |
| media boundary / capture wall | 61,228 / 75,600 ms |
| durable commands / receipts / HTTP batches | 1,226 / 1,226 / 83 |
| canonical encoded / source PCM bytes | 249,425 / 3,918,560 (6.36%) |
| attempted encoded / full batch bodies | 249,425 / 759,046 bytes |
| maximum pending | 195 commands / 38,059 bytes / 17,428 ms oldest |
| pending at Stop | 187 commands / 28,585 bytes |
| last input ACK after Stop / complete drain | 10,947 / 16,243 ms |
| aggregate request time | 62,861 ms |
| producer thread CPU / encode-plus-spool wall | 20,807 / 50,355 ms |

The duration fix held: close/finalize remained pinned to 61,228 ms and did not
include the 16.243-second final drain. ASR attempt 1 completed at progress 1.0
with no failure. Canonical mic (199,188 bytes), system (50,237 bytes), and
transcript (3,708 bytes) artifacts were readable over SSH. Without recording
the transcript body, structured readback found the expected words and numeric
tokens (`margins`, `verification`, `alpha`, `lane`, `437`, `system`, `913`,
`stop`, `now`) ten times each, apart from `margins`/`lane` appearing in
additional structural or recognition context.

The backlog has a direct rate explanation. Requests averaged 757.4 ms and
batches averaged only 14.77 commands. The sequential uploader sleeps another
50 ms after every successful delivery loop, giving about 18.3 commands/s of
observed capacity against a fixed two-lane producer rate of 20 commands/s. Even
a perfectly full 16-command batch would provide only about 19.8 commands/s with
that sleep. Normal `persist_blocks` also scanned and decoded the entire pending
frame directory, then scanned the entire growing ACK directory, for every new
command before appending it. That quadratic hot path explains why measured
encode-plus-spool wall rose from 2.606 seconds for 124 commands on the short
same-host baseline to 50.355 seconds for 1,226 commands here. This is a
production pacing failure, not merely a test threshold:
the run completed durably, but capture-time backlog age did not stay bounded.
Evidence is preserved owner-only at
`/tmp/margins-opus-mac-ddeb-sustained.vPWXuj`; the example JSON SHA-256 is
`ba7d0a3d0f24d0faf2ec30a37bad8f9c1dc98d25808e88f1b9eb123223fc78ad`.

### `49a6d9a2` repair and like-for-like sustained pass

The repair keeps the 100 ms fsynced PCM recovery checkpoint but aggregates five
checkpoints (25 Opus packets) into each independently validated 500 ms network
command. It also removes historical pending-frame/ACK directory scans from the
normal append path, builds one indexed identity view on crash recovery, retains
legacy missing-field five-packet spool interpretation, and avoids a 50 ms sleep
when a full catch-up batch remains.

The exact Mac native composition passed 1/1 and the expanded transport suite
passed 19/19, including the new normal-append no-rescan regression. Its paced
case emitted 8 commands in 4 requests over a 2,560 ms capture, with maximum
pending 4 commands / 1,130 bytes / 735 ms, four pending at Stop, and 477 ms
drain.

The verifier then repeated the exact 61.2275-second fixtures over the real SSH
tunnel. Session/transfer `opus-fixture-1789505370245` completed with:

| Metric | Exact `49a6d9a2` SSH value | Prior `ddeb6f2d` |
| --- | ---: | ---: |
| media boundary / capture wall | 61,228 / 61,247 ms | 61,228 / 75,600 ms |
| durable commands / receipts | 246 / 246 | 1,226 / 1,226 |
| HTTP batches / aggregate request time | 64 / 20,504 ms | 83 / 62,861 ms |
| canonical encoded bytes | 218,065 | 249,425 |
| attempted full batch bodies | 320,522 bytes | 759,046 bytes |
| maximum pending | 6 commands / 5,321 bytes / 1,468 ms | 195 / 38,059 / 17,428 ms |
| pending at Stop | 6 commands / 4,221 bytes | 187 / 28,585 bytes |
| last input ACK after Stop / complete drain | 602 / 2,022 ms | 10,947 / 16,243 ms |
| producer thread CPU / encode-plus-spool wall | 22,549 / 37,913 ms | 20,807 / 50,355 ms |

Capture wall now differs from the source timeline by only 19 ms, pending age
stays bounded, and Stop has no capture-length-dependent drain. Close and
finalize both remain pinned to 61,228 ms. The client manifest has 246 ACKs,
zero pending frames, lane boundaries 123/123, `completed=true`, and no listed
recoverable transfer.

The server independently recorded 123 unique sequences per lane, 246 unique
audio events, one segment finalize, one session finalize, and 249 unique
create/audio/close/finalize receipts, with no duplicate identity. Each lane
spans and sums exactly 61,228 ms. Grouping-specific canonical artifacts are:

| Artifact | Bytes | SHA-256 |
| --- | ---: | --- |
| microphone PacketStream | 183,508 | `805800e20a023b86a9aa5625e3604614392132f64cc49149b112a6b904ac6d0e` |
| system PacketStream | 34,557 | `6e48a47483bcb427746a54064bea69956a41dd82e8bc44ed27db475fe5d1c00a` |
| transcript | 3,708 | retained/readable; body not recorded in this report |

ASR attempt 1 completed at progress 1.0 with no failure. Sanitized transcript
readback found each expected semantic word and numeric token ten times. The
local evidence directory is
`/tmp/margins-opus-mac-49a6-sustained.ODPlXU`; its result JSON SHA-256 is
`7ab5d79e8aec5ecdee7378be674477a16c2c9f95e4c93333a91050d2f4f457ba`.

## Lost finalize response plus scoped server restart

`MARGINS_FIXTURE_STOP_AFTER=memo` created retained transfer/session
`opus-fixture-1789501752481`. It had 124/124 audio ACKs, zero pending frames,
durable close and memo ACKs, a durable finalize intent, `completed=false`, and
the same 6,123 ms boundary. Only PID 2692838 was stopped; port 38871 was
confirmed closed before exact PID 2775389 restarted without provisioning.

The server audit clarified that finalize had already committed at
19:49:21.760Z before the 19:50:25Z restart. This is therefore a lost or
unobserved finalize-response state, not a first finalize after restart. The
fresh exact Mac CLI replayed it:

```sh
/usr/bin/time -p env \
  MARGINS_WORKSPACE=mac-e2e \
  MARGINS_TRANSFER_DIR=/tmp/margins-opus-mac-9353e00fa-restart/transfers \
  MARGINS_SSH_REMOTE_BINARY=/workspace/projects/margins-cargo-target/debug/margins-private \
  MARGINS_SSH_REMOTE_DATA_DIR=/tmp/margins-opus-candidate-9353e00fa/data \
  /Users/example/Hacks/gmk/margins-cargo-target/debug/margins-private \
  --remote ssh://bs-server transfers retry opus-fixture-1789501752481
```

Retry took 2.41 seconds, returned exit 0, changed the local recoverable list
from that exact ID to empty, and set `completed=true`. The server still had
exactly 62 chunks/lane, 124 audio events, 127 unique receipts, one segment
finalize, one session finalize, and the same two encoded hashes. Startup
rediscovered and completed the durable attempt-1 ASR job. This proves exact-ID
replay, no duplicate audio/finalize, no false saved state before ACK, and job
recovery across server restart.

## Portable recovery, legacy PCM, and duration coverage

The final `49a6d9a2` Mac `remote_workspace` run passed 19/19, including exact
direct-16 input, 44.1/48 kHz persistent
resampling and tail counts, anti-alias rejection, packet/tail/pre-skip checks,
silent-lane timing, low-space failure, interrupted open-segment recovery,
partial close cleanup, valid ACK-plus-frame crash state, stale-manifest and
reservation barriers, instance/producer fencing, lost ACK replay, and
pause/resume duration pinned to the last media close.

Backward-compatible 48 kHz PCM service replay passed independently:

```sh
scripts/cargo-lane disposable -- cargo test -p margins-workflows \
  --test workspace_service \
  composed_service_is_the_same_canonical_store_across_retry_and_restart \
  -- --exact --nocapture
```

That regression deliberately reserves the former raw mono s16 48 kHz format,
checks exact samples and duration, loses/replays ACKs, restarts the canonical
service, and replays finalize. It passed 1/1. The current candidate no longer
contains a `remote_pcm_transport` example, so a newly generated legacy PCM SSH
fixture could not be run from this exact source; its attempted invocation
failed before network or server mutation with “no example target named
`remote_pcm_transport`.” Existing old 16/48 kHz artifacts and spools were not
deleted or relabeled.

Source review confirms current remote Stop order: TUI sets the stop flag;
`RecorderHandle::stop_and_write` synchronously retires mic/system devices and
writes recovery audio; the spool worker joins; the segment closes; the last
durable media boundary is pinned; only then do uploader join, memo intent,
session seal, final delivery, and SSH teardown occur. ASR is admitted/scheduled
on the service side after finalize and is not awaited by device release.

The historical `9353e00f` synthetic SSH run proved 6,123 ms close/finalize
despite 3,976 ms final drain; the final `49a6d9a2` run proves 61,228 ms despite
2,022 ms final drain. The portable pause/resume regression also passed. It is
important not to label either as a native-device release measurement. The prior
`68e6a077` user-owned Terminal trace remains revision-scoped evidence: devices
and recovery WAV were complete 27 ms before close-intent creation, followed by
roughly 17 seconds of network drain. Candidate 9353 fixes that run's separate
5.75-second duration inflation by using the pre-join close boundary.

The default local path remains structurally separate. `create_native_session`,
`attach_native_session`, and `run_segment` construct the local recorder,
canonical SQLite session/memo, 16 kHz stereo archive writer, and optional local
live transcript. `RemoteConnection`, Opus resampler/encoder, transfer spool,
ACK worker, and SSH child are constructed only inside
`run_remote_native_capture`. Thus default local capture does not pay remote
transport/spool overhead. No new candidate-local native timing is claimed yet.

## Verifier failures, repairs, and open gates

The exact Mac command

```sh
scripts/cargo-lane disposable -- cargo test -p margins-workflows \
  --test remote_workspace -- --nocapture
```

returned 17/18 on `9353e00f`. Its paced test reported capture 3,201 ms against a strict
`<2,400 ms` assertion, maximum backlog 10 commands / 820 bytes / 591 ms,
pending-at-Stop 6, drain 394 ms, eight HTTP batches, 40 commands, 3,302 encoded
bytes, and 19,262 body bytes. An immediate exact-test-only repeat failed in its
fixture server instead: the accepted TCP stream hit its two-second read timeout
(`WouldBlock` at line 105), the uploader saw connection reset, and its join
panicked. The implementation owner repaired the harness timeout and replaced
host-specific batch/backlog thresholds with coherent run-relative bounds.
`ddeb6f2d` passes 18/18, so these two harness failures are closed. Its later
61-second SSH result exposed the separate production pacing failure; `49a6d9a2`
closes that failure with the like-for-like sustained pass above.

Native TCC belongs to the launching process identity. A prior user-owned
Terminal process successfully opened the devices, while the BB-launched binary
remained denied; a Terminal grant must not be inferred to apply to BB. After
the synthetic and SSH gates passed, the verifier prepared but did not launch
owner-only `/tmp/margins-terminal-opus-49a6d9a2/{home,transfers}` and supplied
one exact Terminal command for the remaining native test. Required controls are
Ctrl-S memo save, Ctrl-P pause/resume, and Ctrl-C immediate device stop followed
by durable drain. Until its result is inspected, this candidate has no native
new/attach/pause/resume/device-release timing claim.

The final disposition must therefore preserve these limitations:

1. inspect one user-owned exact-candidate native new plus attach/pause/resume,
   separating device retirement from network drain and asynchronous ASR;
2. keep real system-audio content fidelity unclaimed unless a scoped tone is
   actually played and observed; the earlier native system lane was correctly
   timed but silent.
