# Remote capture integration and onboarding handoff

This local integration combines the capture and remote-authority implementation
with its independent Mac evidence and the subsequent onboarding specification.
It is not a release or a claim of completed onboarding.

## Sources

- Capture foundation: `4228ef5396074413cf4349365a5529132b4b956d`.
- Final production transport: `49a6d9a225d65a1ad4188db63017157f49fd2821`.
- Implementation/report base: `d8ce52d7332616f5ae7599a90167f7842284b75d`;
  both production commits above are ancestors, not separate cherry-picks.
- [Mac report](mac-audio-transport-verification.md) copied from verifier
  `8c7a5cd962a02bf4b480d1a58d12977dd0a1d081`, branch
  `bb/mac-e2e-verify-opus-49a6d9a2`. Imported file SHA-256:
  `c9315de340105978ee5581270f0dd73f4192d942d017cc62367bc7875f2cb92a`.
- [SSH onboarding specification](ssh-remote-onboarding.md) from the primary
  checkout's previously ignored document, authored by `thr_wnwv3m7ish`.

## Implemented versus proposed

The production client captures locally, checkpoints its recovery journal every
100 ms, sends immutable 500 ms Opus blocks in bounded HTTP batches over an SSH
tunnel, and retries durable receipts. The server owns canonical Workspace session
state, audio artifacts, and asynchronous Parakeet-v2 transcription. Local capture
does not require a server. See [operations](remote-workspace-operations.md) and
[implementation coverage](remote-workspace-implementation-coverage.md).

`margins remote connect` and `margins remote enable` are proposed, not implemented.
SSH discovery/tunneling currently assumes an already provisioned running server.
The onboarding implementation must compose preview/approval, verified installation,
user-service lifecycle, persistent binding, canonical remote Workspace recognition
and recall proofs, and separate capture readiness. It must not invent another
Workspace setup protocol or treat transport health as completed onboarding.

## Evidence and remaining gates

Exact production-source automated evidence includes portable suites, Mac native
composition/remote suites, PCM recovery, the pinned-v2 quality matrix, and the
61-second SSH run. The latter had max six queued commands, 1.468 s maximum backlog
age, last ACK 602 ms after Stop, and 2.022 s final drain. This is scoped debug-build
evidence, not a universal latency guarantee.

Current native user-owned new/attach/pause/resume evidence remains pending.
Full BB-host UI, physical phone/Shortcut lifecycle, system-audio fidelity and device
latency distributions, signed installation, and provisioned-service release gates
also remain unverified. The native journey is not the sole product release gate.

The temporary test service and private fixtures are not a real notes deployment.
This integration does not stop it, delete user data, install software, or change the
primary checkout's unrelated uncommitted edits. No push or release is implied.

## Consolidation checks

On the Linux integration worktree, canonical origin and foundation ancestry were
verified; the imported Mac report hash matches the remote source exactly.
`git diff --check` passed. Production source is unchanged from the implementation
base; this integration adds documentation and allowlist entries only.

`scripts/cargo-lane disposable -- cargo test -p margins-workflows
--no-default-features --test remote_workspace --test workspace_service` passed
19 transport and 7 service tests. The disposable target was removed on completion;
no shared evidence binary was rebuilt. Broader and native results above are the
revision-scoped child evidence, not reruns by the integration coordinator.
