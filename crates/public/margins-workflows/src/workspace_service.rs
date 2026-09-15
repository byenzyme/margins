//! Shared in-process Workspace authority semantics.
//!
//! Local callers use this module directly. HTTP, SSH, BB, and Shortcut
//! adapters authenticate/frame around the same methods; no local call is
//! required to serialize itself or start a service.

use crate::workspace::{ResolvedWorkspace, WorkspaceBinding};
use anyhow::{bail, Context, Result};
use fs4::fs_std::FileExt;
use margins_core::SessionRepository;
use margins_meeting_protocol::{
    ArtifactId, AudioCodecV1, AudioContainerV1, AudioFormatV1, BeginCaptureGenerationV1,
    ClientMessageBodyV1, ClientMessageV1, DurationMillis, InstanceId, ProtocolVersionV1,
    SequenceRangeV1, ServerMessageBodyV1, SessionId, WorkspaceArtifactV1, WorkspaceAttachV1,
    WorkspaceCapabilitiesV1, WorkspaceId, WorkspaceLimitsV1, WorkspaceMemoLineV1,
    WorkspaceMemoReplaceV1, WorkspaceMemoUpdateV1, WorkspaceMemoV1,
    WorkspaceNoteAssociationUpdateV1, WorkspaceNoteAssociationV1, WorkspaceProcessingJobV1,
    WorkspaceRenameV1, WorkspaceSessionPageV1, WorkspaceSessionSummaryV1, WorkspaceSummaryV1,
    WorkspaceTranscriptV1,
};
use margins_meeting_runtime::{MeetingRuntime, MeetingRuntimeStorage, RuntimeResponseV1};
use margins_store::{
    canonical, ImportReceipt, SqliteMeetingRuntimeStorage, SqliteSessionRepository,
    SqliteWorkspaceAuthorityStorage,
};
use rand::{distributions::Alphanumeric, Rng};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

pub const DEFAULT_MAX_CHUNK_BYTES: u64 = 1_048_576;
pub const DEFAULT_MAX_IN_FLIGHT_CHUNKS: u32 = 8;
pub const DEFAULT_MAX_EVENT_PAGE: u32 = 256;
pub const DEFAULT_MAX_IMPORT_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const DEFAULT_SPOOL_RESERVE_BYTES: u64 = 512 * 1024 * 1024;

pub const OP_WORKSPACE_READ: &str = "workspace.read";
pub const OP_SESSION_READ: &str = "session.read";
pub const OP_SESSION_CREATE: &str = "session.create";
pub const OP_SESSION_WRITE: &str = "session.write";
pub const OP_CAPTURE_WRITE: &str = "capture.write";
pub const OP_MEMO_WRITE: &str = "memo.write";
pub const OP_NOTE_ASSOCIATE: &str = "note.associate";
pub const OP_JOB_READ: &str = "job.read";
pub const OP_IMPORT_WRITE: &str = "import.write";
pub const OP_IMPORT_RECEIPT: &str = "import.receipt";
pub const OP_RECALL_QUERY: &str = "recall.query";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServicePrincipal {
    pub id: String,
    pub workspace_ids: BTreeSet<String>,
    pub operations: BTreeSet<String>,
}

impl ServicePrincipal {
    pub fn scoped(
        id: impl Into<String>,
        workspace_ids: impl IntoIterator<Item = String>,
        operations: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            id: id.into(),
            workspace_ids: workspace_ids.into_iter().collect(),
            operations: operations.into_iter().collect(),
        }
    }
    pub fn full(id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            workspace_ids: [workspace_id.into()].into_iter().collect(),
            operations: [
                OP_WORKSPACE_READ,
                OP_SESSION_READ,
                OP_SESSION_CREATE,
                OP_SESSION_WRITE,
                OP_CAPTURE_WRITE,
                OP_MEMO_WRITE,
                OP_NOTE_ASSOCIATE,
                OP_JOB_READ,
                OP_IMPORT_WRITE,
                OP_IMPORT_RECEIPT,
                OP_RECALL_QUERY,
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }

    pub fn upload_only(id: impl Into<String>, workspace_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            workspace_ids: [workspace_id.into()].into_iter().collect(),
            operations: [OP_IMPORT_WRITE, OP_IMPORT_RECEIPT]
                .into_iter()
                .map(str::to_string)
                .collect(),
        }
    }

    pub fn require(&self, workspace_id: &str, operation: &str) -> Result<()> {
        if !self.workspace_ids.contains(workspace_id) {
            bail!("principal is not authorized for this Workspace");
        }
        if !self.operations.contains(operation) {
            bail!("principal is not authorized for operation {operation}");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CredentialFile {
    schema: String,
    records: Vec<CredentialRecord>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CredentialRecord {
    principal_id: String,
    token_hash: String,
    workspace_ids: Vec<String>,
    operations: Vec<String>,
    expires_at_unix_ms: Option<u64>,
    revoked: bool,
}

#[derive(Debug, Clone)]
pub struct ScopedCredentialStore {
    path: PathBuf,
}

impl ScopedCredentialStore {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let store = Self { path: path.into() };
        if let Some(parent) = store.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(store)
    }

    pub fn issue(
        &self,
        principal_id: &str,
        workspace_ids: Vec<String>,
        operations: Vec<String>,
        ttl: Option<std::time::Duration>,
    ) -> Result<String> {
        if principal_id.trim().is_empty() || workspace_ids.is_empty() || operations.is_empty() {
            bail!("credential requires principal, Workspace, and operation scopes");
        }
        let token = random_secret(64);
        self.register(principal_id, &token, workspace_ids, operations, ttl)?;
        Ok(token)
    }

    pub fn register(
        &self,
        principal_id: &str,
        token: &str,
        workspace_ids: Vec<String>,
        operations: Vec<String>,
        ttl: Option<std::time::Duration>,
    ) -> Result<()> {
        if token.is_empty()
            || principal_id.trim().is_empty()
            || workspace_ids.is_empty()
            || operations.is_empty()
        {
            bail!("credential requires token, principal, Workspace, and operation scopes");
        }
        let expires_at_unix_ms = ttl.map(|ttl| unix_ms().saturating_add(ttl.as_millis() as u64));
        self.mutate(|file| {
            let token_hash = hash_secret(token);
            if let Some(record) = file
                .records
                .iter()
                .find(|record| record.token_hash == token_hash)
            {
                if !record.revoked
                    && record.principal_id == principal_id
                    && record.workspace_ids == workspace_ids
                    && record.operations == operations
                    && record.expires_at_unix_ms == expires_at_unix_ms
                {
                    return Ok(());
                }
                bail!("credential token is already registered with different scope");
            }
            file.records.push(CredentialRecord {
                principal_id: principal_id.to_string(),
                token_hash,
                workspace_ids,
                operations,
                expires_at_unix_ms,
                revoked: false,
            });
            Ok(())
        })?;
        Ok(())
    }

    pub fn authorize(&self, token: &str, workspace_id: &str) -> Result<ServicePrincipal> {
        let file = self.read()?;
        let hash = hash_secret(token);
        let record = file
            .records
            .iter()
            .find(|record| record.token_hash == hash)
            .context("credential is invalid")?;
        if record.revoked
            || record
                .expires_at_unix_ms
                .is_some_and(|value| value <= unix_ms())
        {
            bail!("credential is revoked or expired");
        }
        if !record
            .workspace_ids
            .iter()
            .any(|value| value == workspace_id)
        {
            bail!("credential is not authorized for this Workspace");
        }
        Ok(ServicePrincipal::scoped(
            record.principal_id.clone(),
            record.workspace_ids.clone(),
            record.operations.clone(),
        ))
    }

    pub fn revoke(&self, principal_id: &str) -> Result<usize> {
        let mut count = 0;
        self.mutate(|file| {
            for record in &mut file.records {
                if record.principal_id == principal_id && !record.revoked {
                    record.revoked = true;
                    count += 1;
                }
            }
            Ok(())
        })?;
        Ok(count)
    }

    fn read(&self) -> Result<CredentialFile> {
        match std::fs::read(&self.path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(CredentialFile {
                schema: "margins.credentials.v1".to_string(),
                records: Vec::new(),
            }),
            Err(error) => Err(error.into()),
        }
    }

    fn mutate(&self, action: impl FnOnce(&mut CredentialFile) -> Result<()>) -> Result<()> {
        let lock_path = self.path.with_extension("lock");
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        lock.lock_exclusive()?;
        let mut file = self.read()?;
        action(&mut file)?;
        let bytes = serde_json::to_vec_pretty(&file)?;
        let parent = self
            .path
            .parent()
            .context("credential path has no parent")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        use std::io::Write as _;
        temporary.write_all(&bytes)?;
        temporary.as_file().sync_all()?;
        temporary.persist(&self.path).map_err(|error| error.error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionReservation {
    pub response: RuntimeResponseV1,
    pub producer_token: String,
}

#[derive(Debug, Clone)]
pub struct WorkspaceService {
    instance_id: InstanceId,
    workspace: ResolvedWorkspace,
    capture_root: PathBuf,
    margins_dir: PathBuf,
    runtime: Arc<MeetingRuntime<SqliteMeetingRuntimeStorage>>,
    authority: SqliteWorkspaceAuthorityStorage,
    asr_available: bool,
    recall_available: bool,
}

impl WorkspaceService {
    pub fn open(instance_id: impl Into<String>, workspace: ResolvedWorkspace) -> Result<Self> {
        Self::open_with_capabilities(instance_id, workspace, false, false)
    }

    pub fn open_with_capabilities(
        instance_id: impl Into<String>,
        workspace: ResolvedWorkspace,
        asr_available: bool,
        recall_available: bool,
    ) -> Result<Self> {
        let capture_root = workspace.capture_store_dir()?;
        std::fs::create_dir_all(&capture_root)?;
        let margins_dir = capture_root.join(".margins");
        std::fs::create_dir_all(&margins_dir)?;
        let runtime_storage = SqliteMeetingRuntimeStorage::open(&margins_dir)?;
        let authority = SqliteWorkspaceAuthorityStorage::open(&margins_dir)?;
        Ok(Self {
            instance_id: InstanceId(instance_id.into()),
            workspace,
            capture_root,
            margins_dir,
            runtime: Arc::new(MeetingRuntime::new(runtime_storage)),
            authority,
            asr_available,
            recall_available,
        })
    }

    pub fn workspace_id(&self) -> &str {
        &self.workspace.config.id
    }

    pub fn capture_root(&self) -> &Path {
        &self.capture_root
    }

    pub fn margins_dir(&self) -> &Path {
        &self.margins_dir
    }

    pub fn asr_available(&self) -> bool {
        self.asr_available
    }

    pub fn capabilities(&self, principal: &ServicePrincipal) -> Result<WorkspaceCapabilitiesV1> {
        principal.require(self.workspace_id(), OP_WORKSPACE_READ)?;
        Ok(WorkspaceCapabilitiesV1 {
            protocol_version: ProtocolVersionV1,
            instance_id: self.instance_id.clone(),
            workspace_id: WorkspaceId(self.workspace_id().to_string()),
            limits: WorkspaceLimitsV1 {
                max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
                max_in_flight_chunks: DEFAULT_MAX_IN_FLIGHT_CHUNKS,
                max_event_page: DEFAULT_MAX_EVENT_PAGE,
                max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
                spool_reserve_bytes: DEFAULT_SPOOL_RESERVE_BYTES,
            },
            capture_formats: vec![AudioFormatV1 {
                codec: AudioCodecV1::PcmS16Le,
                container: AudioContainerV1::Raw,
                sample_rate_hz: crate::remote_workspace::NATIVE_PCM_RATE_HZ,
                channel_count: 1,
            }],
            operations: principal
                .operations
                .iter()
                .filter(|operation| self.recall_available || operation.as_str() != OP_RECALL_QUERY)
                .cloned()
                .collect(),
            asr_available: self.asr_available,
            recall_available: self.recall_available,
        })
    }

    pub fn summary(&self, principal: &ServicePrincipal) -> Result<WorkspaceSummaryV1> {
        principal.require(self.workspace_id(), OP_WORKSPACE_READ)?;
        Ok(WorkspaceSummaryV1 {
            instance_id: self.instance_id.clone(),
            workspace_id: WorkspaceId(self.workspace_id().to_string()),
            display_name: self
                .workspace
                .config
                .name
                .clone()
                .unwrap_or_else(|| self.workspace_id().to_string()),
            source_ids: self.workspace.config.bindings.keys().cloned().collect(),
            source_freshness: "explicit_sync_required".to_string(),
        })
    }

    pub fn recall(
        &self,
        principal: &ServicePrincipal,
        query: &str,
        source: Option<&str>,
    ) -> Result<crate::local_recall::LocalRecallOutput> {
        principal.require(self.workspace_id(), OP_RECALL_QUERY)?;
        if !self.recall_available {
            bail!("recall capability is unavailable on this instance");
        }
        crate::local_recall::search(&self.workspace, query, source)
    }

    pub fn reserve_session(
        &self,
        principal: &ServicePrincipal,
        command: ClientMessageV1,
    ) -> Result<SessionReservation> {
        principal.require(self.workspace_id(), OP_SESSION_CREATE)?;
        if !matches!(command.body, ClientMessageBodyV1::CreateSession(_)) {
            bail!("session reservation requires create_session");
        }
        if let ClientMessageBodyV1::CreateSession(create) = &command.body {
            for lane in &create.lanes {
                let format = &lane.format;
                if format.codec != AudioCodecV1::PcmS16Le
                    || format.container != AudioContainerV1::Raw
                    || format.channel_count != 1
                    || !matches!(format.sample_rate_hz, 16_000 | 48_000)
                {
                    bail!(
                        "unsupported capture format for lane {}; use mono raw PCM s16le at 16 kHz (48 kHz is accepted only for durable legacy recovery)",
                        lane.lane_id.as_ref()
                    );
                }
            }
        }
        let session_id = command.session_id.clone();
        let response = self
            .runtime
            .handle(command)
            .map_err(|error| anyhow::anyhow!(error))?;
        let producer_token = random_secret(48);
        self.authority
            .reserve_producer(session_id.as_ref(), &principal.id, &producer_token)?;
        self.authority
            .set_current(&principal.id, self.workspace_id(), session_id.as_ref())?;
        let memo_path = self.margins_dir.join(format!("{}.md", session_id.as_ref()));
        if !memo_path.exists() {
            atomic_empty_file(&memo_path)?;
        }
        Ok(SessionReservation {
            response,
            producer_token,
        })
    }

    pub fn attach_session(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request: &WorkspaceAttachV1,
    ) -> Result<SessionReservation> {
        principal.require(self.workspace_id(), OP_SESSION_CREATE)?;
        if request.request_id.len() < 20 || request.request_id.chars().any(char::is_whitespace) {
            bail!("attach request identity must contain at least 20 non-space characters");
        }
        let command = ClientMessageV1 {
            protocol_version: ProtocolVersionV1,
            message_id: format!("attach-{}", request.request_id).into(),
            session_id: session_id.clone(),
            sent_at_unix_ms: request.requested_at_unix_ms,
            body: ClientMessageBodyV1::BeginCaptureGeneration(BeginCaptureGenerationV1 {
                prior_finalize_message_id: request.prior_finalize_message_id.clone(),
                started_at_ms: request.started_at_ms,
            }),
        };
        let response = self
            .runtime
            .handle(command)
            .map_err(|error| anyhow::anyhow!(error))?;
        ensure_runtime_accepted(&response)?;
        // The high-entropy request identity makes this deterministic across a
        // lost response without storing a replayable producer secret in the
        // application database.
        let producer_token = format!(
            "{:x}",
            Sha256::digest(
                format!(
                    "margins.attach.v1\0{}\0{}\0{}\0{}\0{}",
                    self.instance_id.as_ref(),
                    self.workspace_id(),
                    principal.id,
                    session_id.as_ref(),
                    request.request_id
                )
                .as_bytes()
            )
        );
        self.authority
            .reserve_producer(session_id.as_ref(), &principal.id, &producer_token)?;
        self.authority
            .set_current(&principal.id, self.workspace_id(), session_id.as_ref())?;
        Ok(SessionReservation {
            response,
            producer_token,
        })
    }

    pub fn execute_capture(
        &self,
        principal: &ServicePrincipal,
        producer_token: &str,
        command: ClientMessageV1,
    ) -> Result<RuntimeResponseV1> {
        principal.require(self.workspace_id(), OP_CAPTURE_WRITE)?;
        if matches!(command.body, ClientMessageBodyV1::CreateSession(_)) {
            bail!("create_session must use reservation");
        }
        self.authority.authorize_producer(
            command.session_id.as_ref(),
            &principal.id,
            producer_token,
        )?;
        if let ClientMessageBodyV1::AudioChunk(chunk) = &command.body {
            if chunk.payload.len() as u64 > DEFAULT_MAX_CHUNK_BYTES {
                bail!("audio chunk exceeds advertised maximum");
            }
        }
        let finalized = matches!(command.body, ClientMessageBodyV1::FinalizeSession(_));
        let session_id = command.session_id.clone();
        let response = self
            .runtime
            .handle(command)
            .map_err(|error| anyhow::anyhow!(error))?;
        ensure_runtime_accepted(&response)?;
        if finalized
            && response
                .messages
                .iter()
                .any(|message| matches!(message.body, ServerMessageBodyV1::SessionFinalized(_)))
        {
            if self.asr_available {
                // Admission is durable and precedes producer release. A lost
                // finalize response can therefore be retried without ever
                // leaving finalized audio in an unobservable processing gap.
                self.admit_transcription_job(&session_id)?;
            }
            self.authority
                .release_producer(session_id.as_ref(), &principal.id, producer_token)?;
        }
        Ok(response)
    }

    pub fn current(&self, principal: &ServicePrincipal) -> Result<Option<SessionId>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        Ok(self
            .authority
            .current(&principal.id, self.workspace_id())?
            .map(SessionId))
    }

    pub fn sessions(
        &self,
        principal: &ServicePrincipal,
        after: Option<&str>,
        limit: usize,
    ) -> Result<WorkspaceSessionPageV1> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let limit = limit.clamp(1, 100);
        // Empty reservations are retained for audit/retry but are not ordinary
        // meetings. A segment or registered artifact makes the session visible.
        let all = canonical::list_sessions(&self.margins_dir)?
            .into_iter()
            .filter(|session| {
                session.segment_count > 0
                    || canonical::list_session_artifacts(&self.margins_dir, &session.name)
                        .is_ok_and(|artifacts| !artifacts.is_empty())
            })
            .collect::<Vec<_>>();
        let start = after
            .and_then(|cursor| all.iter().position(|session| session.name == cursor))
            .map_or(0, |position| position + 1);
        let page = all.iter().skip(start).take(limit).collect::<Vec<_>>();
        let next_cursor = (start + page.len() < all.len())
            .then(|| page.last().map(|session| session.name.clone()))
            .flatten();
        let sessions = page
            .into_iter()
            .map(|session| self.session_summary(&session.name))
            .collect::<Result<Vec<_>>>()?;
        Ok(WorkspaceSessionPageV1 {
            sessions,
            next_cursor,
        })
    }

    fn session_summary(&self, session_id: &str) -> Result<WorkspaceSessionSummaryV1> {
        let session = canonical::list_sessions(&self.margins_dir)?
            .into_iter()
            .find(|value| value.name == session_id)
            .context("session not found")?;
        let meta = canonical::get_session_meta(&self.margins_dir, session_id)?;
        let runtime = self
            .runtime
            .storage()
            .load_session(&SessionId(session_id.to_string()))?;
        let input_finalized = runtime
            .as_ref()
            .is_some_and(|value| value.input_finalized())
            || (!meta.segments.is_empty()
                && meta
                    .segments
                    .iter()
                    .all(|segment| segment.duration_secs.is_some()))
            || canonical::list_session_artifacts(&self.margins_dir, session_id)?
                .iter()
                .any(|artifact| artifact.kind == "original_audio");
        let finalized_input = runtime.as_ref().and_then(|value| value.finalized_input());
        let capture_duration_ms = finalized_input.map(|(_, ended)| DurationMillis(ended.0));
        let capture_finalize_message_id = finalized_input.map(|(message, _)| message.clone());
        Ok(WorkspaceSessionSummaryV1 {
            session_id: SessionId(session_id.to_string()),
            title: meta.title,
            started_at: session.start_time,
            segment_count: session.segment_count.max(0) as u64,
            input_finalized,
            capture_duration_ms,
            capture_finalize_message_id,
            processing_state: meta.processing_state.unwrap_or_else(|| "none".to_string()),
        })
    }

    pub fn events(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        after: Option<u64>,
        limit: usize,
    ) -> Result<Vec<margins_meeting_protocol::ServerMessageV1>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let stored = self
            .runtime
            .storage()
            .load_session(session_id)?
            .context("session has no capture event stream")?;
        let start = after.map_or(0, |value| value.saturating_add(1));
        self.runtime.storage().load_events(
            session_id,
            SequenceRangeV1 {
                start,
                end_exclusive: stored.next_event_sequence(),
            },
            limit.min(DEFAULT_MAX_EVENT_PAGE as usize),
        )
    }

    pub fn transcript(
        &self,
        principal: &ServicePrincipal,
        requested: &str,
    ) -> Result<WorkspaceTranscriptV1> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let view = crate::transcript_view::load_transcript_view(
            &self.capture_root,
            &self.margins_dir,
            requested,
        )?;
        Ok(WorkspaceTranscriptV1 {
            session_id: SessionId(view.session_name),
            body: view.body,
            view: view.view.to_string(),
            decoded_until_ms: view.decoded_until_ms,
            committed_until_ms: view.committed_until_ms,
            updated_at_unix_ms: view.updated_at_unix_ms,
            live: view.live,
            terminal: view.terminal,
            source_artifact: view.source_path.to_string_lossy().into_owned(),
        })
    }

    pub fn artifacts(
        &self,
        principal: &ServicePrincipal,
        session_id: &str,
    ) -> Result<Vec<WorkspaceArtifactV1>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let session_id = resolve_session_id(&self.margins_dir, session_id)?;
        canonical::list_session_artifacts(&self.margins_dir, &session_id)?
            .into_iter()
            .map(|artifact| {
                let path = crate::artifacts::confined_session_artifact_access_disk_path(
                    &self.margins_dir,
                    &session_id,
                    &artifact.path,
                );
                Ok(WorkspaceArtifactV1 {
                    artifact_id: ArtifactId(format!(
                        "{}:{}:{}",
                        session_id, artifact.kind, artifact.ordinal
                    )),
                    session_id: SessionId(session_id.clone()),
                    kind: artifact.kind,
                    ordinal: artifact.ordinal,
                    size_bytes: path.and_then(|path| path.metadata().ok().map(|value| value.len())),
                    retention_class: artifact.retention_class,
                    created_at: artifact.created_at,
                })
            })
            .collect()
    }

    pub fn artifact_content(
        &self,
        principal: &ServicePrincipal,
        artifact_id: &str,
    ) -> Result<Vec<u8>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let mut parts = artifact_id.rsplitn(3, ':');
        let ordinal: i64 = parts.next().context("invalid artifact id")?.parse()?;
        let kind = parts.next().context("invalid artifact id")?;
        let session = parts.next().context("invalid artifact id")?;
        let artifact = canonical::list_session_artifacts(&self.margins_dir, session)?
            .into_iter()
            .find(|value| value.kind == kind && value.ordinal == ordinal)
            .context("artifact not found")?;
        let path = crate::artifacts::confined_session_artifact_access_disk_path(
            &self.margins_dir,
            session,
            &artifact.path,
        )
        .context("artifact path is outside its session scope")?;
        Ok(std::fs::read(path)?)
    }

    pub fn memo(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
    ) -> Result<WorkspaceMemoV1> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        ensure_session(&self.margins_dir, session_id.as_ref())?;
        let memo = self.authority.memo(session_id.as_ref())?;
        Ok(WorkspaceMemoV1 {
            session_id: session_id.clone(),
            revision: memo.revision,
            lines: memo.lines.into_iter().map(memo_line).collect(),
        })
    }

    pub fn update_memo(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request: &WorkspaceMemoUpdateV1,
    ) -> Result<WorkspaceMemoV1> {
        principal.require(self.workspace_id(), OP_MEMO_WRITE)?;
        ensure_session(&self.margins_dir, session_id.as_ref())?;
        let memo = self.authority.update_memo(
            session_id.as_ref(),
            &principal.id,
            &request.request_id,
            &request.expected_revision,
            request.observed_at_ms.0,
            request.paused,
            &request.text,
        )?;
        Ok(WorkspaceMemoV1 {
            session_id: session_id.clone(),
            revision: memo.revision,
            lines: memo.lines.into_iter().map(memo_line).collect(),
        })
    }

    pub fn replace_memo(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request: &WorkspaceMemoReplaceV1,
    ) -> Result<WorkspaceMemoV1> {
        principal.require(self.workspace_id(), OP_MEMO_WRITE)?;
        ensure_session(&self.margins_dir, session_id.as_ref())?;
        let lines = request
            .lines
            .iter()
            .cloned()
            .map(timed_memo_line)
            .collect::<Vec<_>>();
        let memo = self.authority.replace_memo_lines(
            session_id.as_ref(),
            &principal.id,
            &request.request_id,
            &request.expected_revision,
            &lines,
        )?;
        Ok(WorkspaceMemoV1 {
            session_id: session_id.clone(),
            revision: memo.revision,
            lines: memo.lines.into_iter().map(memo_line).collect(),
        })
    }

    pub fn rename_session(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request: &WorkspaceRenameV1,
    ) -> Result<WorkspaceSessionSummaryV1> {
        principal.require(self.workspace_id(), OP_SESSION_WRITE)?;
        ensure_session(&self.margins_dir, session_id.as_ref())?;
        self.authority.rename_session(
            session_id.as_ref(),
            &principal.id,
            &request.request_id,
            &request.title,
        )?;
        self.session_summary(session_id.as_ref())
    }

    pub fn link_note(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request: &WorkspaceNoteAssociationUpdateV1,
    ) -> Result<WorkspaceNoteAssociationV1> {
        principal.require(self.workspace_id(), OP_NOTE_ASSOCIATE)?;
        let source = self
            .workspace
            .config
            .bindings
            .get(&request.source_id)
            .context("note Source is not declared in this Workspace")?;
        if !matches!(source, WorkspaceBinding::NativeMarkdown { .. }) {
            bail!("note associations require a native notes Source");
        }
        validate_relative_path(&request.relative_path)?;
        self.authority
            .link_note(
                session_id.as_ref(),
                &principal.id,
                &request.request_id,
                &request.source_id,
                &request.relative_path,
                request.observed_content_hash.as_deref(),
                request.expected_revision,
            )
            .map(note_association)
    }

    pub fn note_association(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
    ) -> Result<Option<WorkspaceNoteAssociationV1>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        canonical::get_note_association(&self.margins_dir, session_id.as_ref())
            .map(|value| value.map(note_association))
    }

    pub fn unlink_note(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
        request_id: &str,
        expected_revision: u64,
    ) -> Result<()> {
        principal.require(self.workspace_id(), OP_NOTE_ASSOCIATE)?;
        self.authority.unlink_note(
            session_id.as_ref(),
            &principal.id,
            request_id,
            expected_revision,
        )
    }

    pub fn latest_job(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
    ) -> Result<Option<WorkspaceProcessingJobV1>> {
        principal.require(self.workspace_id(), OP_JOB_READ)?;
        canonical::latest_processing_job(&self.margins_dir, session_id.as_ref())
            .map(|value| value.map(processing_job))
    }

    pub fn capture_lane_format(
        &self,
        session_id: &SessionId,
        lane_id: &str,
    ) -> Result<AudioFormatV1> {
        let session = self
            .runtime
            .storage()
            .load_session(session_id)?
            .context("session has no capture authority state")?;
        session
            .create()
            .lanes
            .iter()
            .find(|lane| lane.lane_id.as_ref() == lane_id)
            .map(|lane| lane.format.clone())
            .context("capture lane was not declared")
    }

    /// Idempotently admit the ASR-only job for the current finalized capture
    /// revision. A completed job for the same revision is never reset.
    pub fn admit_transcription_job(
        &self,
        session_id: &SessionId,
    ) -> Result<WorkspaceProcessingJobV1> {
        if !self.asr_available {
            bail!("ASR capability is unavailable on this instance");
        }
        let stored = self
            .runtime
            .storage()
            .load_session(session_id)?
            .context("session has no capture authority state")?;
        if !stored.input_finalized() {
            bail!("transcription can begin only after durable input finalization");
        }
        let job_id = format!("transcribe:{}", session_id.as_ref());
        let input_revision = format!("meeting:{}", stored.revision());
        if let Some(current) = canonical::get_processing_job(&self.margins_dir, &job_id)? {
            if current.input_revision == input_revision && current.status == "complete" {
                return Ok(processing_job(current));
            }
        }
        canonical::begin_processing_job(
            &self.margins_dir,
            session_id.as_ref(),
            &job_id,
            "transcribe_session",
            &input_revision,
        )
        .map(processing_job)
    }

    pub fn update_transcription_job(
        &self,
        job_id: &str,
        attempt: u64,
        status: &str,
        progress: Option<f64>,
        result_ref: Option<&str>,
        failure: Option<&str>,
    ) -> Result<WorkspaceProcessingJobV1> {
        canonical::update_processing_job(
            &self.margins_dir,
            job_id,
            attempt,
            status,
            progress,
            result_ref,
            failure,
            failure.map(|_| "transcribe"),
        )
        .map(processing_job)
    }

    pub fn pending_transcription_jobs(&self) -> Result<Vec<WorkspaceProcessingJobV1>> {
        canonical::pending_processing_jobs(&self.margins_dir, "transcribe_session")
            .map(|jobs| jobs.into_iter().map(processing_job).collect())
    }

    pub fn import_finished_file(
        &self,
        principal: &ServicePrincipal,
        upload_id: &str,
        session_id: &str,
        original_filename: &str,
        title: Option<&str>,
        bytes: &[u8],
    ) -> Result<ImportReceipt> {
        principal.require(self.workspace_id(), OP_IMPORT_WRITE)?;
        if bytes.is_empty() || bytes.len() as u64 > DEFAULT_MAX_IMPORT_BYTES {
            bail!("import body is empty or exceeds the advertised maximum");
        }
        if !canonical::session_exists(&self.margins_dir, session_id)? {
            let now = chrono::Local::now();
            canonical::create_session(
                &self.margins_dir,
                session_id,
                &now,
                &format!(".margins/{session_id}.md"),
            )?;
            if let Some(title) = title {
                canonical::set_title(&self.margins_dir, session_id, Some(title.to_string()))?;
            }
        }
        let receipt = self.authority.record_import(
            upload_id,
            &principal.id,
            session_id,
            original_filename,
            bytes,
        )?;
        canonical::upsert_session_artifact(
            &self.margins_dir,
            session_id,
            "original_audio",
            0,
            &receipt.stored_path,
            "durable",
            None,
        )?;
        Ok(receipt)
    }

    pub fn import_receipt(
        &self,
        principal: &ServicePrincipal,
        upload_id: &str,
    ) -> Result<Option<ImportReceipt>> {
        principal.require(self.workspace_id(), OP_IMPORT_RECEIPT)?;
        Ok(self
            .authority
            .import_receipt(&principal.id, upload_id)?
            .filter(|_| principal.operations.contains(OP_IMPORT_RECEIPT)))
    }

    pub fn repository_record(
        &self,
        principal: &ServicePrincipal,
        session_id: &SessionId,
    ) -> Result<Option<margins_core::SessionRecord>> {
        principal.require(self.workspace_id(), OP_SESSION_READ)?;
        let repository_id = margins_core::SessionId(session_id.0.clone());
        SqliteSessionRepository::open(&self.margins_dir)?
            .get(&repository_id)
            .map_err(Into::into)
    }
}

fn ensure_runtime_accepted(response: &RuntimeResponseV1) -> Result<()> {
    if let Some(rejection) = response
        .messages
        .iter()
        .find_map(|message| match &message.body {
            ServerMessageBodyV1::CommandRejected(rejection) => Some(rejection),
            _ => None,
        })
    {
        bail!(
            "capture command rejected ({}): {}",
            rejection.code,
            rejection.message.as_deref().unwrap_or("no detail")
        );
    }
    Ok(())
}

fn ensure_session(directory: &Path, session_id: &str) -> Result<()> {
    if !canonical::session_exists(directory, session_id)? {
        bail!("session not found");
    }
    Ok(())
}

fn resolve_session_id(directory: &Path, requested: &str) -> Result<String> {
    match requested {
        "latest" => crate::transcript_view::resolve_session_name(directory, "latest"),
        "current" => bail!("current is principal-scoped; use the current operation"),
        value => {
            ensure_session(directory, value)?;
            Ok(value.to_string())
        }
    }
}

fn validate_relative_path(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("note path must be Source-relative and cannot traverse");
    }
    Ok(())
}

fn memo_line(line: margins_core::TimedMemoLine) -> WorkspaceMemoLineV1 {
    WorkspaceMemoLineV1 {
        text: line.text,
        created_secs: line.created_secs,
        edited_secs: line.edited_secs,
        draft_started_secs: line.draft_started_secs,
        audio_pending_at_mark: line.audio_pending_at_mark,
        block_ordinal: line.block_ordinal,
    }
}

fn timed_memo_line(line: WorkspaceMemoLineV1) -> margins_core::TimedMemoLine {
    margins_core::TimedMemoLine {
        text: line.text,
        created_secs: line.created_secs,
        edited_secs: line.edited_secs,
        draft_started_secs: line.draft_started_secs,
        audio_pending_at_mark: line.audio_pending_at_mark,
        block_ordinal: line.block_ordinal,
    }
}

fn note_association(value: canonical::NoteAssociation) -> WorkspaceNoteAssociationV1 {
    WorkspaceNoteAssociationV1 {
        session_id: SessionId(value.session_name),
        source_id: value.source_id,
        relative_path: value.relative_path,
        observed_content_hash: value.observed_content_hash,
        revision: value.revision,
    }
}

fn processing_job(value: canonical::ProcessingJob) -> WorkspaceProcessingJobV1 {
    WorkspaceProcessingJobV1 {
        job_id: value.job_id,
        session_id: SessionId(value.session_name),
        operation: value.operation,
        input_revision: value.input_revision,
        attempt: value.attempt,
        status: value.status,
        progress: value.progress,
        result_ref: value.result_ref,
        failure: value.failure,
        failed_stage: value.failed_stage,
    }
}

fn random_secret(length: usize) -> String {
    rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(length)
        .map(char::from)
        .collect()
}

fn hash_secret(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn atomic_empty_file(path: &Path) -> Result<()> {
    let parent = path.parent().context("memo path has no parent")?;
    let temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    Ok(())
}
