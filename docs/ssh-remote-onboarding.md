# SSH-native remote onboarding and Workspace setup

Status: product and implementation specification; every `margins remote ...`
command shown here is proposed unless explicitly labeled current.

Date: 2026-09-15.

This document specializes the onboarding and lifecycle parts of
[Remote workspace authority and local capture](remote-workspace-implementation.md).
It does not replace that document's session, transport, storage, or application
contracts. It also does not replace the canonical
[Margins Workspace Setup skill](../crates/public/margins-workflows/resources/skills/margins-workspace-setup/SKILL.md).
Remote onboarding must run that setup on the selected remote authority rather
than inventing a second setup protocol.

## 1. Product decision

SSH is the native bootstrap, trust, discovery, and reconnect path for a
user-controlled remote Margins authority. BB is not required for onboarding.
A BB integration may later consume an established Margins binding, but BB host
enrollment, Connect credentials, project Sources, environments, and port shares
are not part of this handshake.

The ordinary client-first entry point is one command:

```sh
margins remote connect bs-server
```

`bs-server` is an OpenSSH config host. Margins uses the system `ssh` executable
and therefore preserves the user's existing host-key policy, agent, Tailscale
address, `ProxyJump`, identity selection, and reauthentication behavior. It does
not implement another SSH credential store.

The server-first entry point is directory-native:

```sh
ssh bs-server
cd /workspace/obsidian
margins remote enable
```

Then, on the capture machine:

```sh
margins remote connect bs-server
```

These are two entrances to the same persisted authority and Workspace binding.
Neither makes process cwd the enduring identity. Neither asks an ordinary user
for `MARGINS_HOME`, data/work directories, a port, instance ID, Workspace ID,
provision flags, model paths, or bearer credentials.

Onboarding is complete only when the remote Workspace has been set up and
proved, not when SSH connects or a process starts. Client-first onboarding also
proves the capture-to-transcript path when the local device supports it.

## 2. User-facing promise

From the user's point of view, the command answers four questions:

1. Which computer will remember and process this work?
2. Which existing notes belong to that practice?
3. What will Margins install or write?
4. Can this computer actually record into that remote practice and retrieve it?

A fresh client-first run should read approximately like this:

```text
Connecting to bs-server using SSH…

Server: beelink-ser5 · Linux
Margins is not installed yet.

Where are your notes on this server?
> /workspace/obsidian

Margins will:
  • install a background service for your user
  • process recordings on this server
  • use the existing notes in /workspace/obsidian
  • keep Margins state separate from those notes
  • install the speech model that matches this Mac
  • leave all existing files in place

Download: <exact size from the selected platform artifact manifest>
Writes: Margins application state and a systemd user service

Continue? [y/N]
```

Versions, artifact digests, concrete state paths, service-unit paths, model
backend/quantization, protocol numbers, IDs, and exact plan bodies belong in
`--details`, `--json`, and the durable receipt. The ordinary preview names their
consequences.

The notes folder is the only unavoidable fresh-server question. SSH cannot
safely infer a user's practice by searching an entire remote filesystem. The
question disappears when exactly one existing remote Workspace already declares
the folder, or when a server-first invocation establishes it from cwd. Optional
BB metadata may offer a path as a suggestion, but it never authorizes or proves
the Margins Source.

## 3. One journey, several truths

The product must preserve these states separately:

| State | What has been proved | What has not |
| --- | --- | --- |
| SSH reachable | OpenSSH authenticated the remote OS user under normal host-key policy | Margins installation, service, Workspace, recall, or capture |
| Changes approved | The user saw and accepted the proposed remote installs and writes | That any change succeeded |
| Authority installed | Exact binaries/models are verified and the user service can restart | Workspace understanding or recall |
| Workspace declared | A stable Workspace ID and exact Source boundary exist on the authority | That the practice was recognized or indexed |
| Workspace recognized | The user recognized or corrected the scan-grounded account | That init/sync/recall succeeded |
| Recall ready | Init/sync and Source-backed recall proof succeeded | Local microphone/system-audio permission or ASR delivery |
| Capture ready | Actual local speech became durable remotely, an ASR job finished, and its transcript was readable | Connected-note distillation, which is not onboarding |

Status may truthfully stop at any row and resume later. “Connected” must not be
used as a synonym for “ready.” A server-first run can finish at **recall ready**;
the local client reaches **capture ready** only after it connects and uses the
actual local capture process.

These are not redundant health checks:

- **SSH reachability is an access check.** It proves only that OpenSSH reached
  and authenticated the intended operating-system account.
- **Grounded recognition is a setup decision.** The complete remote scan lets
  Margins propose what the declared practice appears to be before deriving
  exclusions, attention, profile, or expansion policy. The user recognizes or
  corrects that account. Its value is not peculiar to remote operation, but the
  remote boundary makes it especially important not to configure a plausible
  local folder or the wrong same-named remote folder.
- **Recall proof has mechanical and semantic halves.** Exact-phrase recall is
  the Source/index smoke test: it proves that the selected authority can retrieve
  from the declared native remote Source. The one question promised by the
  grounded review is the semantic smoke test: it proves that retrieval supports
  the understanding Margins presented, rather than merely returning some text.
- **Capture readiness is an operational proof.** Actual local speech must become
  a durable remote session, complete an explicit ASR job, produce a transcript,
  and be retrievable. It cannot be inferred from SSH, service health, capability
  flags, or Workspace recall alone.

Grounded recognition is not part of every SSH connection. A successful setup
persists the reviewed config revision, declared Source identity, and proof status
on the authority. An ordinary reconnect validates the instance and Workspace
binding and reuses that result without showing the scan account again. Setup is
re-entered only for a new Workspace, an incomplete prior setup, an explicit user
request to review it, or a conflicting/materially changed Source or policy
binding. Ordinary note edits and service restarts do not by themselves trigger
recognition again.

## 4. SSH-native client-first flow

### 4.1 Read-only preflight

Before asking approval, the local CLI may use fixed, non-mutating OpenSSH
commands to determine:

- the SSH destination as OpenSSH resolves it, remote host name, OS,
  architecture, and remote user;
- whether a compatible `margins` command and authority manifest already exist;
- whether the proposed notes path exists, is a directory, and has bounded
  Markdown evidence;
- existing Workspace declarations whose native Source contains that canonical
  path;
- existing Margins state, service manager support, installation ownership,
  available disk space, and conflicts;
- the exact client/server and logical ASR model compatibility plan.

Preflight must not create a temporary remote helper, state directory, Workspace,
service, credential, model cache, or shell profile entry. If no remote Margins
binary exists, it uses only fixed operating-system probes and asks for the notes
path. It does not stream and execute an unreviewed shell installer in order to
produce the preview.

OpenSSH owns host-key prompts. A changed or unknown host key cannot be accepted
by Margins on the user's behalf. An SSH alias resolving to a different host later
must also fail the persisted Margins instance check.

### 4.2 One infrastructure preview and approval

The preview includes all remote application, model, service, and initial
Workspace-declaration writes. It distinguishes:

- new installation, compatible existing installation, repair, and explicit
  upgrade;
- new Workspace declaration, exact existing Workspace reuse, ambiguous match,
  and conflict;
- product state paths from note Source paths;
- estimated download size and whether download occurs on the client or server;
- service behavior after logout/reboot, including an explicit warning when a
  Linux user service cannot survive without a user session or configured linger;
- the logical ASR model version and language set shared with the client.

Approval authorizes only the displayed infrastructure/declaration plan. A stale
remote inventory, changed SSH identity, changed artifact, different Source path,
or changed existing Workspace invalidates it and returns to preview. Declining or
interrupting before approval leaves the remote unchanged.

### 4.3 Verified, resumable installation

After approval, the local client transfers an exact release through SSH into a
user-owned staging directory. The release and platform artifact are digest- or
signature-verified before and after transfer. Activation is atomic: a partial
download or crash cannot replace the last working binary. The installer creates
or repairs a launchd agent or systemd user service only as described in the
approved plan; it never requires `sudo`, edits shell startup files, or exposes a
public listening port.

The service binds remote loopback. It reads one product-owned authority manifest
containing stable instance ID, selected Workspace IDs, exact software/wire/model
identities, and product-resolved state paths. Ordinary operation does not depend
on inherited environment variables or process cwd.

Retrying the same approved plan reuses its request identity. It may recover or
discard only its own incomplete staging files. Existing Workspace state,
captures, indexes, notes, models owned by another installation, and user service
files with conflicting ownership are never deleted, reset, moved, or silently
adopted. Conflicts stop for a new preview or separate migration workflow.

### 4.4 Declare or reuse the Workspace

Infrastructure setup now executes the Workspace resolution rules in section 6.
Creating the minimal declaration is not the grounded review: it establishes only
the stable Workspace ID, one writable home Source, and product-owned capture
storage needed for the canonical scan/setup path.

### 4.5 Run canonical Workspace setup on the remote authority

Once the Source boundary and Workspace ID exist, the onboarding coordinator
first checks the persisted setup revision and proof status. If they remain valid,
it reports that the existing reviewed setup is being reused and proceeds without
another recognition turn. Otherwise it runs or resumes the installed
`margins guide workspace-setup` contract against that remote Workspace:

1. check redacted capabilities;
2. save and consume the complete read-only `scan.v2` result when available;
3. form a tentative, source-grounded account of the practice;
4. show that understanding before settings and ask the one recognition question;
5. reflect the user's correction, if any;
6. derive only the minimum settings implied by the recognized account;
7. compile a complete desired config with the existing Workspace plan operation;
8. show the consequences and apply the final reviewed plan unchanged;
9. prepare catalyst capability only when needed, immediately before init;
10. run `init`, then `sync`;
11. prove a declared Source with an exact phrase and test the question promised
    by the grounded review;
12. report the Workspace in the user's language and confirm that notes were not
    modified.

The exact phrase is the narrow transport/Source/index smoke test. The promised
question is a semantic check on the setup claim. Neither is a substitute for the
later real speech-to-session-to-ASR readiness calibration, and neither causes
connected-note distillation.

This reasoning remains agent-led. The SSH/CLI coordinator exposes structured,
redacted remote operations and transports their results; it does not replace the
skill with a deterministic scan renderer. If a bare terminal invocation has no
agent capable of completing the grounded review, it may finish infrastructure
and declaration but must stop at `workspace_setup_required`, print the exact
agent handoff, and withhold “ready” status.

The user's initial request to connect and set up the remote practice, followed by
recognition or correction of the grounded account, authorizes the minimum
ordinary Workspace settings derived from it. Do not add a second apply-confirmation
turn after recognition. Infrastructure installation has its own earlier approval
because it writes outside the Workspace and may download substantial artifacts.

### 4.6 Calibrate the real capture path

Workspace setup is now complete; it has not transcribed a meeting, processed a
session, drafted a note, or started distillation. Remote onboarding then offers a
separate capture-readiness calibration using the actual local Margins process:

```text
Your notes are ready on bs-server.
Now say the short phrase shown below to check recording and transcription.
```

Success requires the spoken signal to traverse the same production path as a
meeting:

1. the local process obtains its own microphone/system-audio permission;
2. distinct microphone and system lanes use the negotiated native 16 kHz signed
   16-bit PCM transport without exposing sample rate as a user choice;
3. acknowledged chunks and finalization create a durable remote session;
4. an explicit ASR job is queued, runs, and reaches a terminal success state;
5. the expected transcript is readable through the bound remote Workspace;
6. remote notes recall remains usable after capture.

No connected note is generated. A local permission denial leaves the remote
Workspace at **recall ready**, reports capture readiness as incomplete, and does
not create a successful empty session. A BB daemon or plugin cannot satisfy or
grant macOS TCC. Permission held by a user-launched TUI does not imply permission
for a BB host process, desktop app, browser, or another executable.

### 4.7 Persist and select the binding

On success, persist a client-local, non-secret remote binding under the normal
Margins state root. It records:

- the human SSH selector (`bs-server`);
- the stable remote Margins instance and Workspace IDs;
- client installation/principal ID;
- approved software, wire, and logical model identities;
- last successful Source/recall and capture-readiness receipts;
- no bearer credential, private key, audio, transcript, or note content.

On a fresh client with no selected Margins authority, `remote connect` selects
this binding for ordinary `margins new`, retrieval, and recall commands. If the
client already has a local or remote default, the preview states that the default
would change and asks as part of the infrastructure approval. Explicit `--local`
or an explicit remote selector still wins. An unavailable selected remote never
falls back to local.

`margins remote forget bs-server` removes only the client binding and cached
non-secret receipts. It does not revoke SSH, disable the authority, or delete
remote data. Revocation and authority disablement are separate, explicit remote
administrative actions.

## 5. Server-first `remote enable`

`margins remote enable` is compatible with the Workspace ID contract only when
cwd is treated as evidence for a Source boundary, never as the enduring
Workspace identity or service configuration.

From `/workspace/obsidian`, it performs the same read-only inspection and shows:

```text
Use this folder as the home of Workspace “obsidian”
  Notes: /workspace/obsidian
  Margins state: stored separately
  Remote access: through your existing SSH account only

Enable remote use and set up this Workspace? [y/N]
```

After approval it installs/repairs the user service, resolves or declares the
Workspace, and enters the canonical grounded setup. It emits a non-secret client
hint:

```text
On the computer where you record:
  margins remote connect bs-server
```

It never prints a bearer credential. The hint may not know the client's SSH alias;
when it cannot, it says to use the alias the user already uses for this server.

Changing directory after enablement has no effect. Restarting the service opens
the persisted Workspace ID and declared absolute Source. Running `remote enable`
again from the same canonical Source is idempotent. Running it from a different
directory does not retarget the existing authority: it produces a conflict or an
explicit proposal for another Workspace.

## 6. Workspace identity and directory resolution

### 6.1 Invariants

- A Workspace ID is the stable lowercase/digit/internal-hyphen identifier already
  used by Margins state at `workspaces/<id>`.
- The ID identifies one knowledge practice. It is not a filesystem path, SSH
  alias, host name, BB project ID, server instance ID, or display name.
- The home Source is an independently declared canonical absolute path on the
  authority. Exactly one notes Source has role `home`.
- Margins state and capture storage remain under the Workspace's product-owned
  state directory. No `.margins` directory is created inside the notes folder.
- Once persisted, service start and client reconnect select the literal Workspace
  ID. They never derive it again from cwd.
- Moving or replacing a Source requires the existing reviewed Workspace
  plan/apply workflow. A changed SSH alias or BB default Source cannot retarget it.

### 6.2 Resolution algorithm

Canonicalize the proposed notes directory, without following symlinks outside the
declared boundary, and compare it with existing Workspace Sources.

1. **Exactly one existing Workspace contains the directory:** reuse that
   Workspace's literal ID. Show the existing home boundary; do not create or
   rename anything.
2. **Multiple existing Workspaces contain it:** stop and ask which practice the
   user intends. This is a real boundary ambiguity, so one plain-language question
   is appropriate.
3. **No Workspace contains it:** propose an ID by slugging the directory basename
   under the existing ID grammar. `/workspace/obsidian` proposes `obsidian`.
4. **The proposed ID is free:** include creation of that ID and the exact home
   Source in the approved onboarding preview.
5. **The proposed ID exists with the same canonical home:** reuse it after
   validating the complete declaration.
6. **The proposed ID exists with a different home, malformed state, or incomplete
   transaction:** stop. Show the conflict and let the user explicitly choose the
   existing Workspace or a new human name. Never silently create `obsidian-2`,
   delete the old directory, or repair by resetting it.

Remote enablement must therefore split the current implicit helper's useful
behavior from its unsafe onboarding behavior: reuse cwd containment and the
isolation deny-list, but do not call a mutation that silently chooses the next
available numeric suffix.

The existing implicit-home refusals remain applicable. Do not implicitly declare
the filesystem root, user home, Margins/Enzyme state, a temporary directory, a
directory above another Workspace home, or a folder with no Markdown evidence.
Reference Sources are added only when the user already mentioned them or the
existing declaration leaves a real boundary ambiguity; scan never adds them.

### 6.3 Creation and later configuration

Initial declaration uses the existing Workspace creation contract: literal ID,
one canonical writable home Source, and product-owned capture storage. It must be
shown in the infrastructure/declaration preview because it is a state change.

After scan and recognition, any Source, exclusion, attention, profile, expansion,
name, or retention change uses the existing complete desired config -> Workspace
plan -> unchanged Workspace apply path. The onboarding layer neither hand-edits
plan JSON nor adds another anchor/setup schema. A stale or altered plan fails. If
the user corrects the grounded account, regenerate the desired config and plan.

An incomplete initial declaration or failed init/sync remains visible and
resumable. Onboarding does not treat it as an unused directory to remove. A retry
must distinguish “declaration exists,” “review incomplete,” “plan applied,”
“init incomplete,” and “recall proof incomplete.”

## 7. SSH session and application security

- Use the system OpenSSH client; do not accept host keys, import private keys, or
  rewrite SSH configuration.
- The SSH-authenticated remote OS user is the bootstrap administrator. Margins
  still uses a stable client installation ID so two clients sharing that OS user
  can be scoped and revoked independently.
- Discovery runs a fixed remote Margins command and returns redacted instance,
  Workspace, protocol, loopback-port, and short-lived scoped authorization.
- The client opens an SSH local forward to the remote loopback service. Margins
  never binds the service directly to a LAN or public interface as a side effect
  of onboarding.
- Short-lived credentials stay in process memory. Long capture does not depend on
  a five-minute credential remaining valid: discovery can renew authorization,
  while the unguessable producer credential remains scoped to its session and
  recovery contract.
- Reconnect repeats SSH discovery, checks the pinned instance and Workspace, and
  resumes only unacknowledged durable spool entries. Instance mismatch fails
  closed and requires an explicit rebind preview.
- Logs and receipts may contain host alias, stable IDs, versions, sizes, stages,
  retries, and error codes. They exclude SSH material, bearer/producer tokens,
  audio, transcript text, note text, and retrieved recall content.
- Disabling remote use does not delete data. Credential revocation, service
  disablement, client forgetting, migration, and retention/deletion are separate
  operations with separate scopes.

## 8. Version and model negotiation

The preview and authority manifest pin:

- Margins application version and artifact digest;
- remote service protocol and supported client range;
- stable instance schema version;
- one logical ASR model ID, upstream revision, and language set;
- acoustic/tokenizer/vocabulary compatibility identities;
- platform backend and artifact digest;
- packaging and quantization.

CoreML on Mac and ONNX on Linux may use different packaging or quantization only
when they are verified representations of the same logical model and language
set. A folder name or generic “TDT” capability is insufficient. Setup refuses a
silent v2-to-v3 switch, model downgrade, vocabulary mismatch, or unrecognized
artifact. Model changes require an explicit preview; existing sessions and data
remain readable when an upgrade is declined.

The speech model is machine-level capability, not Workspace policy. Its install
is covered by the infrastructure approval and reported separately from Workspace
settings. Sample rate is an internal negotiated transport detail and never an
ordinary onboarding choice.

## 9. Failure and resume behavior

| Failure | Required outcome |
| --- | --- |
| SSH unavailable before approval | No remote change; show the OpenSSH-level next action |
| SSH reauthentication during capture | Continue bounded local spool; request reauthentication; no local fallback |
| Host key or instance changes | Fail closed; preserve spool; require explicit rebind preview |
| Existing incompatible binary/service | Inventory and stop or propose an explicit upgrade/repair; never overwrite unowned files |
| Partial binary/model download | Preserve prior active version; resume verified staging or remove only owned staging after approval |
| Existing Workspace path conflict | Stop before mutation; ask which practice/path is intended |
| Existing incomplete Workspace state | Preserve and diagnose; no automatic delete/reset/suffix |
| Scan or recognition interrupted | Persist declaration and scan evidence; resume at recognition without init |
| Plan becomes stale | Re-scan/rebuild consequences as required; never apply altered plan |
| Init/sync fails | Preserve declaration and applied policy; report recall not ready |
| Exact-phrase or grounded recall fails | Report proof incomplete; do not substitute a nearby result |
| Service started but ASR/model unavailable | Report service reachable and capture not ready |
| Local TCC denied | Keep remote recall ready; no successful empty session |
| Upload/ACK interrupted | Preserve stable IDs and unacknowledged lane chunks in the local spool |
| Source sync differs across machines | Remote Source proof governs the authority; report mismatch; do not add a notes proxy |

## 10. E2E and rollout specification

The existing [setup E2E lanes](setup-e2e-lanes.md) remain authoritative for
Workspace behavior. Remote onboarding adds SSH-native variants; it does not
weaken, replace, or fold those lanes into transport-only tests.

### 10.1 Deterministic public SSH lane

A credential-free test uses the exact public export, a disposable client home,
a disposable remote authority reachable through a real system OpenSSH client,
and a notes fixture that exists only on the remote side. It proves:

- read-only preflight causes no remote filesystem delta;
- the preview contains the intended install, service, Workspace ID/home, and
  model consequences but no secret values;
- decline causes no remote write;
- apply is idempotent and preserves pre-existing unrelated state;
- `/workspace/obsidian` proposes `obsidian`, an exact existing declaration is
  reused, and an ID/path collision never becomes `obsidian-2` silently;
- Workspace state is outside the notes fixture and Markdown hashes do not change;
- `workspace new`, scan when supported, init, sync, exact-phrase recall, status,
  and Source reporting execute on the remote authority;
- the client has no notes folder, index, or accidental local Workspace;
- reconnect selects the same instance/Workspace and wrong-instance replay fails;
- partial transfer/service/declaration crashes resume without deletion or duplicate
  Workspace/session identities.

As in the existing public lane, temporary homes, authority state, SSH material,
and fixtures are isolated and removed only after restoration and evidence capture.
The test must not depend on ambient provider credentials.

### 10.2 Official hosted grounded-review lane

The hosted lane runs the same remote authority through the complete canonical
grounded review. It proves that:

- complete remote `scan.v2` evidence, not a deterministic summary, grounds the
  interpretation;
- the user sees and recognizes/corrects the account before policy derivation;
- the final reviewed desired configuration is compiled and its exact plan is
  accepted unchanged by revision-guarded apply;
- stale apply fails;
- catalyst preparation occurs only when required and immediately before init;
- remote init/sync use the official composition;
- exact-phrase and grounded-question recall retain native Markdown provenance;
- no credential, note-body, or raw model response is copied into ordinary logs.

This lane may spend model resources and follows the existing explicit credential
broker and secret-isolation requirements.

### 10.3 Ecological remote rollout review

Add a remote companion to
[Workspace setup rollout review](workspace-setup-rollout-review.md). Give the
agent one ordinary prompt, without implementation hints or a transition checklist:

> Help me connect Margins to bs-server and set it up with the notes in /workspace/obsidian so I can record on this computer and remember them there.

The client begins with no notes or Margins binding. The authority begins in the
chosen fresh/existing scenario. Preserve the complete agent transcript and have
an independent reviewer judge the experience as a whole: whether it was
SSH-native, intelligible, appropriately cautious, grounded, minimal, and honest
about readiness.

The harness backs up and restores remote Workspace state, service manifests, and
the fixture's note hashes without printing or copying private contents into the
review report. It also snapshots the clean client state to prove there was no
local Workspace or notes proxy. Existing remote data is restored byte-for-byte;
an interrupted review has an explicit restore operation.

Universal rollout blockers include the existing setup blockers plus:

- any remote write before infrastructure approval;
- a silently invented/suffixed Workspace ID or retargeted Source;
- an exposed SSH/app credential or public service port;
- a “ready” claim based only on SSH, process, health, or capability flags;
- local fallback after explicit remote selection;
- a claim that remote handshake granted local device permission;
- a model identity or language-set mismatch;
- any deletion, move, reset, or unreviewed migration of existing data.

### 10.4 Real readiness lanes

Deterministic setup proof does not replace production capture evidence.

- A Linux authority fixture injects known spoken audio through the exact native
  16 kHz s16 mic/system lane protocol and proves durable session, ACK/retry,
  explicit ASR job, transcript, and post-capture recall.
- A provisioned Mac client run uses the actual user-launched Margins process and
  real TCC prompt. It verifies that local mic/system capture reaches the Linux
  authority and that denying permission produces no successful empty session.
- A separate test keeps the BB host/plugin chain denied while a user-launched TUI
  is allowed, proving those permission identities are not conflated.
- Restart and network-fault cases preserve the same remote instance, Workspace,
  session, job, and lane identities and resume only unacknowledged data.

## 11. Acceptance matrix

Before describing SSH-native remote onboarding as shipped, verify at least:

1. fresh Linux authority, fresh client, and notes only on the authority;
2. server-first enable from the notes root, a nested Source directory, an unsafe
   directory, and a directory above an existing Workspace;
3. client-first connect with a new Workspace, exact existing Workspace, multiple
   containing Workspaces, ID collision, different-path collision, and incomplete
   Workspace transaction;
4. compatible existing service, stopped service, service without linger, unowned
   service file, older compatible version, newer incompatible version, and
   interrupted upgrade;
5. first download, partial download, digest mismatch, insufficient disk, retry,
   restart, and no-network remote with client-side transfer;
6. SSH agent success, interactive reauthentication, stale control connection,
   changed host key, changed Margins instance, revoked SSH access, and revoked
   Margins client principal;
7. canonical scan/review/plan/apply/init/sync/proof completion and resume from each
   intermediate state;
8. source freshness/sync mismatch and exact-phrase failure without notes proxy or
   substituted evidence;
9. logical model parity across CoreML/ONNX packaging, explicit version/language
   drift, quantization mismatch, and refused silent v2/v3 switch;
10. actual speech -> two durable lanes -> one session -> ASR job -> transcript ->
    remote notes recall;
11. local TCC allowed/denied independently for desktop, user-launched TUI, BB
    host/plugin, and browser producers;
12. client forget, authority disable, credential revocation, Workspace migration,
    and retention each affect only their documented scope and never imply deletion.

## 12. Implementation seams and deletions

Keep this as a narrow composition over existing contracts:

1. Split Workspace **inspection** from the current implicit create path so remote
   preview can resolve cwd, collisions, and unsafe roots without writes. Reuse the
   existing ID grammar and containment rules, but remove silent numeric suffixing
   from remote onboarding.
2. Add one SSH onboarding coordinator and durable authority/binding manifests.
   Reuse system OpenSSH, existing remote discovery/tunnel, scoped Workspace service,
   and durable capture spool. Do not add a notes transport or SSH library.
3. Add exact-version user-service install/repair as an explicit approved operation.
   Remove user-facing dependence on service environment variables and discovery
   binary/data-dir overrides after the manifest path is complete.
4. Make the coordinator call the existing Workspace creation and setup operations.
   The setup skill owns interpretation and decision policy; the SSH layer owns
   address, transport, installation preview, and remote invocation only.
5. Add structured model identity and staged readiness to the existing protocol.
   Capability booleans remain supporting evidence, never final readiness.
6. Let BB consume the resulting remote binding or shared service later. Delete
   plugin-only implicit Workspace provisioning, installer policy, and health-only
   readiness when the shared lifecycle is available; do not put them into the SSH
   handshake.

The intended result is one user story—“use that server for Margins with those
notes”—implemented by a resumable sequence of existing Workspace and capture
contracts. SSH makes the authority reachable. The Workspace setup makes the
practice legible. Real capture calibration proves the whole system. None of those
facts substitutes for another.
