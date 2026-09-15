//! Opt-in remote transport selection and crash-recoverable delivery spool.
//!
//! This module is never constructed by the default local capture path.

use anyhow::{bail, Context, Result};
use fs4::fs_std::FileExt;
use margins_meeting_protocol::{
    AudioChunkV1, AudioCodecV1, AudioContainerV1, AudioFormatV1, CaptureLaneV1, CaptureModeV1,
    CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1, CaptureSourceV1,
    ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1, ContentDigestV1, CreateSessionV1,
    DigestAlgorithmV1, DurationMillis, FinalizeSessionV1, LaneBoundaryV1, SegmentCloseReasonV1,
    SegmentCloseReferenceV1, SessionFinalizeReasonV1, SessionId, SessionMillis, WorkspaceAttachV1,
    WorkspaceMemoReplaceV1, WorkspaceMemoUpdateV1, WorkspaceNoteAssociationUpdateV1,
    WorkspaceRenameV1,
};
use rand::{distributions::Alphanumeric, Rng};
use rubato::{
    Resampler, SincFixedIn, SincInterpolationParameters, SincInterpolationType, WindowFunction,
};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const DEFAULT_SERVICE_PORT: u16 = 8787;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceDiscoveryV1 {
    pub protocol_version: u16,
    pub instance_id: String,
    pub workspace_ids: Vec<String>,
    pub loopback_port: u16,
    pub credential: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceStateV1 {
    pub schema: String,
    pub protocol_version: u16,
    pub instance_id: String,
    pub workspace_ids: Vec<String>,
    pub loopback_port: u16,
}

#[derive(Debug, Clone)]
pub struct WorkspaceHttpClient {
    base_url: url::Url,
    token: String,
    workspace_id: String,
    expected_instance_id: Option<String>,
    client: reqwest::blocking::Client,
}

pub struct RemoteConnection {
    pub client: WorkspaceHttpClient,
    tunnel: Option<std::process::Child>,
}

impl RemoteConnection {
    pub fn connect(remote: &str, workspace_id: &str, https_token: Option<&str>) -> Result<Self> {
        match RemoteEndpoint::parse(remote)? {
            RemoteEndpoint::Https(url) | RemoteEndpoint::LoopbackHttp(url) => {
                let token = https_token.context("MARGINS_REMOTE_TOKEN is required for HTTPS")?;
                let client = WorkspaceHttpClient::new(url, token, workspace_id, None)?;
                client.capabilities()?;
                Ok(Self {
                    client,
                    tunnel: None,
                })
            }
            RemoteEndpoint::SshAlias(alias) => {
                let remote_binary = std::env::var("MARGINS_SSH_REMOTE_BINARY").ok();
                let remote_data_dir = std::env::var("MARGINS_SSH_REMOTE_DATA_DIR").ok();
                let output = std::process::Command::new("ssh")
                    .args(ssh_discovery_args_for_service(
                        &alias,
                        remote_binary.as_deref().unwrap_or("margins"),
                        remote_data_dir.as_deref(),
                    )?)
                    .output()
                    .context("failed to start system ssh for Margins discovery")?;
                if !output.status.success() {
                    bail!("Margins SSH discovery failed");
                }
                let discovery: ServiceDiscoveryV1 = serde_json::from_slice(&output.stdout)
                    .context("Margins SSH discovery returned invalid JSON")?;
                if discovery.protocol_version != 1 {
                    bail!("unsupported remote Workspace protocol version");
                }
                if !discovery.workspace_ids.iter().any(|id| id == workspace_id) {
                    bail!("remote principal is not authorized for this Workspace");
                }
                let listener = std::net::TcpListener::bind(("127.0.0.1", 0))?;
                let local_port = listener.local_addr()?.port();
                drop(listener);
                let mut tunnel = std::process::Command::new("ssh")
                    .args(ssh_tunnel_args(
                        &alias,
                        local_port,
                        discovery.loopback_port,
                    )?)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn()
                    .context("failed to start system ssh tunnel")?;
                let base = url::Url::parse(&format!("http://127.0.0.1:{local_port}/"))?;
                let client = WorkspaceHttpClient::new(
                    base,
                    discovery.credential,
                    workspace_id,
                    Some(discovery.instance_id),
                )?;
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
                loop {
                    if let Some(status) = tunnel.try_wait()? {
                        bail!("SSH tunnel exited before readiness ({status})");
                    }
                    match client.capabilities() {
                        Ok(_) => break,
                        Err(_) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(error) => {
                            let _ = tunnel.kill();
                            bail!("SSH tunnel did not become ready: {error}");
                        }
                    }
                }
                Ok(Self {
                    client,
                    tunnel: Some(tunnel),
                })
            }
        }
    }
}

impl Drop for RemoteConnection {
    fn drop(&mut self) {
        if let Some(tunnel) = self.tunnel.as_mut() {
            let _ = tunnel.kill();
            let _ = tunnel.wait();
        }
    }
}

impl WorkspaceHttpClient {
    pub fn new(
        base_url: url::Url,
        token: impl Into<String>,
        workspace_id: impl Into<String>,
        expected_instance_id: Option<String>,
    ) -> Result<Self> {
        match base_url.scheme() {
            "https" => {}
            "http" if matches!(base_url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => {}
            _ => bail!("Workspace HTTP client requires HTTPS or loopback HTTP"),
        }
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self {
            base_url,
            token: token.into(),
            workspace_id: workspace_id.into(),
            expected_instance_id,
            client,
        })
    }

    pub fn capabilities(&self) -> Result<margins_meeting_protocol::WorkspaceCapabilitiesV1> {
        let value: margins_meeting_protocol::WorkspaceCapabilitiesV1 =
            self.get("v1/capabilities")?;
        if value.workspace_id.as_ref() != self.workspace_id {
            bail!("server capability Workspace identity changed");
        }
        if self
            .expected_instance_id
            .as_deref()
            .is_some_and(|expected| expected != value.instance_id.as_ref())
        {
            bail!("server instance identity changed");
        }
        Ok(value)
    }

    pub fn sessions(
        &self,
        after: Option<&str>,
        limit: usize,
    ) -> Result<margins_meeting_protocol::WorkspaceSessionPageV1> {
        let mut path = format!(
            "v1/workspaces/{}/sessions?limit={}",
            self.workspace_id, limit
        );
        if let Some(after) = after {
            path.push_str("&after=");
            path.push_str(&urlencoding(after));
        }
        self.get(&path)
    }

    pub fn current(&self) -> Result<Option<margins_meeting_protocol::SessionId>> {
        self.get(&format!("v1/workspaces/{}/current", self.workspace_id))
    }

    pub fn latest_session(&self) -> Result<Option<SessionId>> {
        Ok(self
            .sessions(None, 1)?
            .sessions
            .into_iter()
            .next()
            .map(|value| value.session_id))
    }

    pub fn session_summary(
        &self,
        session_id: &str,
    ) -> Result<Option<margins_meeting_protocol::WorkspaceSessionSummaryV1>> {
        let mut after = None::<String>;
        loop {
            let page = self.sessions(after.as_deref(), 100)?;
            if let Some(found) = page
                .sessions
                .into_iter()
                .find(|session| session.session_id.as_ref() == session_id)
            {
                return Ok(Some(found));
            }
            let Some(next) = page.next_cursor else {
                return Ok(None);
            };
            after = Some(next);
        }
    }

    pub fn rename(
        &self,
        session: &str,
        request: &WorkspaceRenameV1,
    ) -> Result<margins_meeting_protocol::WorkspaceSessionSummaryV1> {
        self.request_json(
            self.client
                .put(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/title",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .json(request),
        )
    }

    pub fn memo(&self, session: &str) -> Result<margins_meeting_protocol::WorkspaceMemoV1> {
        self.get(&format!(
            "v1/workspaces/{}/sessions/{}/memo",
            self.workspace_id,
            urlencoding(session)
        ))
    }

    pub fn update_memo(
        &self,
        session: &str,
        request: &WorkspaceMemoUpdateV1,
    ) -> Result<margins_meeting_protocol::WorkspaceMemoV1> {
        self.request_json(
            self.client
                .put(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/memo",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .json(request),
        )
    }

    pub fn replace_memo(
        &self,
        session: &str,
        request: &WorkspaceMemoReplaceV1,
    ) -> Result<margins_meeting_protocol::WorkspaceMemoV1> {
        self.request_json(
            self.client
                .post(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/memo/replace",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .json(request),
        )
    }

    pub fn note_association(
        &self,
        session: &str,
    ) -> Result<Option<margins_meeting_protocol::WorkspaceNoteAssociationV1>> {
        self.get(&format!(
            "v1/workspaces/{}/sessions/{}/note-association",
            self.workspace_id,
            urlencoding(session)
        ))
    }

    pub fn link_note(
        &self,
        session: &str,
        request: &WorkspaceNoteAssociationUpdateV1,
    ) -> Result<margins_meeting_protocol::WorkspaceNoteAssociationV1> {
        self.request_json(
            self.client
                .put(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/note-association",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .json(request),
        )
    }

    pub fn unlink_note(
        &self,
        session: &str,
        request_id: &str,
        expected_revision: u64,
    ) -> Result<()> {
        let _: serde_json::Value = self.request_json(self.client.delete(self.url(&format!(
            "v1/workspaces/{}/sessions/{}/note-association?expected_revision={}&request_id={}",
            self.workspace_id,
            urlencoding(session),
            expected_revision,
            urlencoding(request_id)
        ))?))?;
        Ok(())
    }

    pub fn latest_job(
        &self,
        session: &str,
    ) -> Result<Option<margins_meeting_protocol::WorkspaceProcessingJobV1>> {
        self.get(&format!(
            "v1/workspaces/{}/sessions/{}/jobs/latest",
            self.workspace_id,
            urlencoding(session)
        ))
    }

    pub fn transcript(
        &self,
        session: &str,
    ) -> Result<margins_meeting_protocol::WorkspaceTranscriptV1> {
        self.get(&format!(
            "v1/workspaces/{}/sessions/{}/transcript",
            self.workspace_id,
            urlencoding(session)
        ))
    }

    pub fn artifacts(
        &self,
        session: &str,
    ) -> Result<Vec<margins_meeting_protocol::WorkspaceArtifactV1>> {
        self.get(&format!(
            "v1/workspaces/{}/sessions/{}/artifacts",
            self.workspace_id,
            urlencoding(session)
        ))
    }

    pub fn recall(
        &self,
        query: &str,
        source: Option<&str>,
    ) -> Result<crate::local_recall::LocalRecallOutput> {
        self.request_json(
            self.client
                .post(self.url(&format!("v1/workspaces/{}/recall", self.workspace_id))?)
                .json(&serde_json::json!({"query": query, "source": source})),
        )
    }

    pub fn execute(
        &self,
        session: &str,
        producer_token: &str,
        command: &ClientMessageV1,
    ) -> Result<margins_meeting_runtime::RuntimeResponseV1> {
        self.request_json(
            self.client
                .post(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/commands",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .header("X-Margins-Producer-Token", producer_token)
                .json(command),
        )
    }

    pub fn reserve(
        &self,
        command: &ClientMessageV1,
    ) -> Result<crate::workspace_service::SessionReservation> {
        self.request_json(
            self.client
                .post(self.url(&format!("v1/workspaces/{}/sessions", self.workspace_id))?)
                .json(command),
        )
    }

    pub fn attach(
        &self,
        session: &str,
        request: &margins_meeting_protocol::WorkspaceAttachV1,
    ) -> Result<crate::workspace_service::SessionReservation> {
        self.request_json(
            self.client
                .post(self.url(&format!(
                    "v1/workspaces/{}/sessions/{}/attach",
                    self.workspace_id,
                    urlencoding(session)
                ))?)
                .json(request),
        )
    }

    pub fn upload_chunk(
        &self,
        producer_token: &str,
        command: &ClientMessageV1,
    ) -> Result<margins_meeting_runtime::RuntimeResponseV1> {
        let ClientMessageBodyV1::AudioChunk(chunk) = &command.body else {
            bail!("upload_chunk requires audio_chunk");
        };
        let path = format!(
            "v1/workspaces/{}/sessions/{}/segments/{}/lanes/{}/chunks/{}",
            self.workspace_id,
            urlencoding(command.session_id.as_ref()),
            urlencoding(chunk.segment_id.as_ref()),
            urlencoding(chunk.lane_id.as_ref()),
            chunk.sequence
        );
        self.request_json(
            self.client
                .put(self.url(&path)?)
                .header("X-Margins-Producer-Token", producer_token)
                .header("X-Margins-Message-Id", command.message_id.as_ref())
                .header("X-Margins-Starts-At-Ms", chunk.starts_at_ms.0)
                .header("X-Margins-Duration-Ms", chunk.duration_ms.0)
                .header("X-Margins-Sha256", &chunk.payload_digest.hex)
                .header("X-Margins-Sent-At-Unix-Ms", command.sent_at_unix_ms.0)
                .body(chunk.payload.clone()),
        )
    }

    pub fn import(
        &self,
        upload_id: &str,
        session_id: &str,
        filename: &str,
        title: Option<&str>,
        bytes: Vec<u8>,
    ) -> Result<margins_store::ImportReceipt> {
        let mut request = self
            .client
            .put(self.url(&format!(
                "v1/workspaces/{}/imports/{}",
                self.workspace_id,
                urlencoding(upload_id)
            ))?)
            .header("X-Margins-Session-Id", session_id)
            .header("X-Margins-Filename", filename)
            .body(bytes);
        if let Some(title) = title {
            request = request.header("X-Margins-Title", title);
        }
        self.request_json(request)
    }

    fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request_json(self.client.get(self.url(path)?))
    }

    fn url(&self, path: &str) -> Result<url::Url> {
        self.base_url
            .join(path)
            .context("invalid Workspace API path")
    }

    fn request_json<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<T> {
        let response = request.bearer_auth(&self.token).send()?;
        let status = response.status();
        let body = response.bytes()?;
        let envelope: ClientEnvelope<T> = serde_json::from_slice(&body)
            .with_context(|| format!("server returned a non-contract response ({status})"))?;
        if !status.is_success() || !envelope.ok {
            let error = envelope
                .error
                .context("server error omitted its envelope")?;
            bail!("{}: {}", error.code, error.message);
        }
        match envelope.result {
            EnvelopeResult::Present(result) => Ok(result),
            EnvelopeResult::Missing => bail!("successful response omitted its result"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteEndpoint {
    Https(url::Url),
    LoopbackHttp(url::Url),
    SshAlias(String),
}

impl RemoteEndpoint {
    pub fn parse(value: &str) -> Result<Self> {
        let url = url::Url::parse(value).context("remote must be an ssh:// or https:// URL")?;
        match url.scheme() {
            "https" if url.host_str().is_some() => Ok(Self::Https(url)),
            "http" if matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1")) => {
                Ok(Self::LoopbackHttp(url))
            }
            "http" => bail!("plain HTTP is allowed only on loopback"),
            "ssh" => {
                if !url.username().is_empty()
                    || url.password().is_some()
                    || url.port().is_some()
                    || url.path() != ""
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    bail!("ssh remotes must name one SSH config alias only");
                }
                let alias = url.host_str().context("ssh remote has no alias")?;
                validate_ssh_alias(alias)?;
                Ok(Self::SshAlias(alias.to_string()))
            }
            _ => bail!("remote must use ssh://, https://, or loopback http://"),
        }
    }
}

pub fn validate_ssh_alias(alias: &str) -> Result<()> {
    if alias.is_empty()
        || alias.starts_with('-')
        || alias.len() > 253
        || !alias
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("unsafe SSH config alias");
    }
    Ok(())
}

pub fn ssh_discovery_args(alias: &str) -> Result<Vec<String>> {
    ssh_discovery_args_with_binary(alias, "margins")
}

pub fn ssh_discovery_args_with_binary(alias: &str, binary: &str) -> Result<Vec<String>> {
    ssh_discovery_args_for_service(alias, binary, None)
}

pub fn ssh_discovery_args_for_service(
    alias: &str,
    binary: &str,
    data_dir: Option<&str>,
) -> Result<Vec<String>> {
    validate_ssh_alias(alias)?;
    if binary != "margins"
        && (!binary.starts_with('/')
            || binary.len() > 1024
            || !binary.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            }))
    {
        bail!("MARGINS_SSH_REMOTE_BINARY must be an absolute shell-safe path");
    }
    if data_dir.is_some_and(|value| {
        !value.starts_with('/')
            || value.len() > 1024
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            })
    }) {
        bail!("MARGINS_SSH_REMOTE_DATA_DIR must be an absolute shell-safe path");
    }
    let mut args = vec![
        "-T".into(),
        "--".into(),
        alias.into(),
        binary.into(),
        "service".into(),
        "discover".into(),
        "--json".into(),
    ];
    if let Some(data_dir) = data_dir {
        args.push("--data-dir".into());
        args.push(data_dir.into());
    }
    Ok(args)
}

pub fn ssh_tunnel_args(alias: &str, local_port: u16, remote_port: u16) -> Result<Vec<String>> {
    validate_ssh_alias(alias)?;
    if local_port == 0 || remote_port == 0 {
        bail!("tunnel ports must be nonzero");
    }
    Ok(vec![
        "-N".into(),
        "-T".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-L".into(),
        format!("127.0.0.1:{local_port}:127.0.0.1:{remote_port}"),
        "--".into(),
        alias.into(),
    ])
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransferManifest {
    pub schema: String,
    pub transfer_id: String,
    pub instance_id: String,
    pub remote_url: String,
    pub workspace_id: String,
    pub session_id: String,
    /// Legacy manifests stored this capability inline. New manifests leave it
    /// empty and keep the owner-only secret in `producer-token`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub producer_token: String,
    #[serde(default = "producer_token_reference")]
    pub producer_token_ref: String,
    #[serde(default)]
    pub close_commands: Vec<ClientMessageV1>,
    /// V1 migration field. It is folded into `close_commands` on open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub close_command: Option<ClientMessageV1>,
    pub finalize_command: Option<ClientMessageV1>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memo_intent: Option<WorkspaceMemoReplaceV1>,
    #[serde(default)]
    pub completed: bool,
}

fn producer_token_reference() -> String {
    "producer-token".to_string()
}

#[derive(Deserialize)]
#[serde(bound(deserialize = "T: Deserialize<'de>"))]
struct ClientEnvelope<T> {
    ok: bool,
    #[serde(default)]
    result: EnvelopeResult<T>,
    error: Option<margins_meeting_protocol::WorkspaceErrorV1>,
}

enum EnvelopeResult<T> {
    Missing,
    Present(T),
}

impl<T> Default for EnvelopeResult<T> {
    fn default() -> Self {
        Self::Missing
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for EnvelopeResult<T> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        T::deserialize(deserializer).map(Self::Present)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpoolChunk {
    pub command: ClientMessageV1,
    pub payload_digest: String,
    pub path: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Clone)]
pub struct DurableTransferSpool {
    root: PathBuf,
    manifest: TransferManifest,
    reserve_bytes: u64,
}

impl DurableTransferSpool {
    pub fn create(
        root: &Path,
        transfer_id: &str,
        instance_id: &str,
        remote_url: &str,
        workspace_id: &str,
        session_id: &str,
        producer_token: &str,
        reserve_bytes: u64,
    ) -> Result<Self> {
        validate_component(transfer_id)?;
        let transfer_root = root.join(transfer_id);
        std::fs::create_dir_all(transfer_root.join("chunks"))?;
        std::fs::create_dir_all(transfer_root.join("acks"))?;
        std::fs::create_dir_all(transfer_root.join("control-acks"))?;
        std::fs::create_dir_all(transfer_root.join("recovery"))?;
        set_directory_owner_only(&transfer_root)?;
        set_directory_owner_only(&transfer_root.join("chunks"))?;
        set_directory_owner_only(&transfer_root.join("acks"))?;
        set_directory_owner_only(&transfer_root.join("control-acks"))?;
        set_directory_owner_only(&transfer_root.join("recovery"))?;
        let manifest = TransferManifest {
            schema: "margins.remote-transfer.v1".into(),
            transfer_id: transfer_id.into(),
            instance_id: instance_id.into(),
            remote_url: remote_url.into(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            producer_token: String::new(),
            producer_token_ref: producer_token_reference(),
            close_commands: Vec::new(),
            close_command: None,
            finalize_command: None,
            memo_intent: None,
            completed: false,
        };
        atomic_bytes(
            &transfer_root.join("producer-token"),
            producer_token.as_bytes(),
        )?;
        set_owner_only(&transfer_root.join("producer-token"))?;
        atomic_json(&transfer_root.join("manifest.json"), &manifest)?;
        set_owner_only(&transfer_root.join("manifest.json"))?;
        let lock = transfer_root.join("transfer.lock");
        File::options().create(true).append(true).open(&lock)?;
        set_owner_only(&lock)?;
        Ok(Self {
            root: transfer_root,
            manifest,
            reserve_bytes,
        })
    }

    pub fn open(root: &Path, transfer_id: &str, reserve_bytes: u64) -> Result<Self> {
        validate_component(transfer_id)?;
        let root = root.join(transfer_id);
        for directory in ["chunks", "acks", "control-acks", "recovery"] {
            std::fs::create_dir_all(root.join(directory))?;
            set_directory_owner_only(&root.join(directory))?;
        }
        let manifest = with_transfer_lock(&root, || {
            let mut manifest: TransferManifest =
                serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
            let mut changed = false;
            if let Some(close) = manifest.close_command.take() {
                if !manifest
                    .close_commands
                    .iter()
                    .any(|value| value.message_id == close.message_id)
                {
                    manifest.close_commands.push(close);
                }
                changed = true;
            }
            if !manifest.producer_token.is_empty() {
                atomic_bytes(
                    &root.join(&manifest.producer_token_ref),
                    manifest.producer_token.as_bytes(),
                )?;
                set_owner_only(&root.join(&manifest.producer_token_ref))?;
                manifest.producer_token.clear();
                changed = true;
            }
            if changed {
                atomic_json(&root.join("manifest.json"), &manifest)?;
            }
            Ok(manifest)
        })?;
        Ok(Self {
            root,
            manifest,
            reserve_bytes,
        })
    }

    pub fn manifest(&self) -> &TransferManifest {
        &self.manifest
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn recovery_path(&self, segment_id: &str) -> Result<PathBuf> {
        validate_component(segment_id)?;
        Ok(self.root.join("recovery").join(format!("{segment_id}.wav")))
    }

    pub fn producer_token(&self) -> Result<String> {
        let path = self.root.join(&self.manifest.producer_token_ref);
        let token = std::fs::read_to_string(path)?;
        if token.is_empty() {
            bail!("remote transfer producer credential is empty");
        }
        Ok(token)
    }

    /// Publish a complete frame before it can be scheduled for upload. A
    /// crash after rename but before return leaves a self-describing frame
    /// that `pending_chunks` discovers on restart.
    pub fn append_chunk(&self, command: &ClientMessageV1) -> Result<SpoolChunk> {
        let ClientMessageBodyV1::AudioChunk(chunk) = &command.body else {
            bail!("spool accepts audio chunk commands only");
        };
        if command.session_id.as_ref() != self.manifest.session_id {
            bail!("chunk session does not match transfer");
        }
        with_transfer_lock(&self.root, || {
            let available = fs4::available_space(&self.root)?;
            if available.saturating_sub(chunk.payload.len() as u64) < self.reserve_bytes {
                bail!("spool disk reserve would be breached");
            }
            let digest = format!("{:x}", Sha256::digest(&chunk.payload));
            if digest != chunk.payload_digest.hex {
                bail!("chunk payload digest does not match bytes");
            }
            let name = chunk_name(command, chunk);
            let path = self.root.join("chunks").join(name);
            let bytes = encode_spool_frame(command)?;
            atomic_bytes_noclobber(&path, &bytes)?;
            Ok(SpoolChunk {
                command: command.clone(),
                payload_digest: digest,
                path,
                size_bytes: chunk.payload.len() as u64,
            })
        })
    }

    pub fn pending_chunks(&self) -> Result<Vec<SpoolChunk>> {
        with_transfer_lock(&self.root, || self.pending_chunks_unlocked())
    }

    fn pending_chunks_unlocked(&self) -> Result<Vec<SpoolChunk>> {
        let mut chunks = Vec::new();
        for entry in std::fs::read_dir(self.root.join("chunks"))? {
            let entry = entry?;
            if entry.file_type()?.is_file()
                && entry.path().extension().is_some_and(|v| v == "frame")
            {
                let command = decode_spool_frame(&entry.path())?;
                let ClientMessageBodyV1::AudioChunk(chunk) = &command.body else {
                    bail!("spool frame does not contain audio");
                };
                let digest = format!("{:x}", Sha256::digest(&chunk.payload));
                if digest != chunk.payload_digest.hex {
                    bail!("spool frame digest is corrupt");
                }
                let size_bytes = chunk.payload.len() as u64;
                chunks.push(SpoolChunk {
                    command,
                    payload_digest: digest,
                    path: entry.path(),
                    size_bytes,
                });
            }
        }
        chunks.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(chunks)
    }

    /// Persist the ACK receipt before reclaiming bytes. A crash before unlink
    /// can repeat the exact command; a crash after unlink has the receipt.
    pub fn acknowledge(&self, chunk: &SpoolChunk) -> Result<()> {
        with_transfer_lock(&self.root, || {
            let name = chunk.path.file_name().context("chunk has no file name")?;
            let receipt = self.root.join("acks").join(name).with_extension("ack");
            std::fs::create_dir_all(receipt.parent().unwrap())?;
            set_directory_owner_only(receipt.parent().unwrap())?;
            atomic_bytes(&receipt, chunk.payload_digest.as_bytes())?;
            match std::fs::remove_file(&chunk.path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            sync_dir(chunk.path.parent().unwrap())?;
            Ok(())
        })
    }

    pub fn set_close(&mut self, command: ClientMessageV1) -> Result<()> {
        if !matches!(command.body, ClientMessageBodyV1::CloseSegment(_)) {
            bail!("close intent requires close_segment");
        }
        self.mutate_manifest(|manifest| {
            if let Some(existing) = manifest
                .close_commands
                .iter()
                .find(|value| value.message_id == command.message_id)
            {
                if existing != &command {
                    bail!("close message identity conflicts with different content");
                }
                return Ok(());
            }
            manifest.close_commands.push(command);
            Ok(())
        })
    }

    pub fn pending_closes(&self) -> Vec<ClientMessageV1> {
        self.manifest
            .close_commands
            .iter()
            .filter(|command| !self.control_ack_matches(command).unwrap_or(false))
            .cloned()
            .collect()
    }

    fn control_ack_matches(&self, command: &ClientMessageV1) -> Result<bool> {
        let receipt = self
            .root
            .join("control-acks")
            .join(format!("{}.ack", command.message_id.as_ref()));
        if !receipt.is_file() {
            return Ok(false);
        }
        let encoded = serde_json::to_vec(command)?;
        Ok(std::fs::read_to_string(receipt)? == format!("{:x}", Sha256::digest(encoded)))
    }

    /// Persist a control receipt locally before it may disappear from the
    /// capture-time retry set. The original command stays in the manifest so
    /// finalization can name its stable identity after a restart.
    pub fn acknowledge_control(&self, command: &ClientMessageV1) -> Result<()> {
        let receipt = self
            .root
            .join("control-acks")
            .join(format!("{}.ack", command.message_id.as_ref()));
        let encoded = serde_json::to_vec(command)?;
        let fingerprint = format!("{:x}", Sha256::digest(encoded));
        with_transfer_lock(&self.root, || {
            atomic_bytes(&receipt, fingerprint.as_bytes())
        })
    }

    pub fn set_memo_intent(&mut self, request: WorkspaceMemoReplaceV1) -> Result<()> {
        self.mutate_manifest(|manifest| {
            if let Some(existing) = &manifest.memo_intent {
                if existing != &request {
                    bail!("memo intent was already sealed with different content");
                }
                return Ok(());
            }
            manifest.memo_intent = Some(request);
            Ok(())
        })
    }

    pub fn memo_pending(&self) -> Result<bool> {
        let Some(request) = &self.manifest.memo_intent else {
            return Ok(false);
        };
        let receipt = self.root.join("control-acks/memo.ack");
        if !receipt.is_file() {
            return Ok(true);
        }
        let encoded = serde_json::to_vec(request)?;
        Ok(std::fs::read_to_string(receipt)? != format!("{:x}", Sha256::digest(encoded)))
    }

    pub fn acknowledge_memo(&self) -> Result<()> {
        let request = self
            .manifest
            .memo_intent
            .as_ref()
            .context("cannot acknowledge a missing memo intent")?;
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(request)?));
        with_transfer_lock(&self.root, || {
            atomic_bytes(
                &self.root.join("control-acks/memo.ack"),
                fingerprint.as_bytes(),
            )
        })
    }

    pub fn set_finalize(&mut self, command: ClientMessageV1) -> Result<()> {
        if !matches!(command.body, ClientMessageBodyV1::FinalizeSession(_)) {
            bail!("finalize intent requires finalize_session");
        }
        self.mutate_manifest(|manifest| {
            if let Some(existing) = &manifest.finalize_command {
                if existing != &command {
                    bail!("finalize intent was already sealed with different content");
                }
                return Ok(());
            }
            manifest.finalize_command = Some(command);
            Ok(())
        })
    }

    pub fn ready_to_finalize(&self) -> Result<bool> {
        Ok(self.pending_chunks()?.is_empty()
            && self.pending_closes().is_empty()
            && !self.memo_pending()?
            && self.manifest.finalize_command.is_some())
    }

    /// Mark delivery complete only after the server has durably acknowledged
    /// chunks, every segment close, memo state, and finalization. The producer
    /// capability is then removed; evidence and ACK receipts remain inspectable.
    pub fn mark_completed(&mut self) -> Result<()> {
        self.complete_with(|_, _| Ok(()))
    }

    fn complete_with(
        &mut self,
        finalize: impl FnOnce(&TransferManifest, &str) -> Result<()>,
    ) -> Result<()> {
        let updated = with_transfer_lock(&self.root, || {
            let mut manifest: TransferManifest =
                serde_json::from_slice(&std::fs::read(self.root.join("manifest.json"))?)?;
            let view = Self {
                root: self.root.clone(),
                manifest: manifest.clone(),
                reserve_bytes: self.reserve_bytes,
            };
            if !view.pending_chunks_unlocked()?.is_empty()
                || !view.pending_closes().is_empty()
                || view.memo_pending()?
                || manifest.finalize_command.is_none()
            {
                bail!("remote transfer still has unacknowledged audio, close, or memo input, or is missing finalization");
            }
            let producer_token =
                std::fs::read_to_string(self.root.join(&manifest.producer_token_ref))?;
            finalize(&manifest, &producer_token)?;
            manifest.completed = true;
            atomic_json(&self.root.join("manifest.json"), &manifest)?;
            match std::fs::remove_file(self.root.join(&manifest.producer_token_ref)) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
            // Recorder recovery WAVs are a second durable representation owned by
            // this transfer. Keep them through every failed/lost ACK, then reclaim
            // them only after chunks, closes, memo, and finalization are durable.
            let recovery = self.root.join("recovery");
            if recovery.is_dir() {
                std::fs::remove_dir_all(&recovery)?;
            }
            sync_dir(&self.root)?;
            Ok(manifest)
        })?;
        self.manifest = updated;
        Ok(())
    }

    fn mutate_manifest(
        &mut self,
        action: impl FnOnce(&mut TransferManifest) -> Result<()>,
    ) -> Result<()> {
        let updated = with_transfer_lock(&self.root, || {
            let mut current: TransferManifest =
                serde_json::from_slice(&std::fs::read(self.root.join("manifest.json"))?)?;
            action(&mut current)?;
            atomic_json(&self.root.join("manifest.json"), &current)?;
            Ok(current)
        })?;
        self.manifest = updated;
        Ok(())
    }
}

fn with_transfer_lock<T>(root: &Path, action: impl FnOnce() -> Result<T>) -> Result<T> {
    let path = root.join("transfer.lock");
    let lock = File::options().create(true).append(true).open(&path)?;
    set_owner_only(&path)?;
    lock.lock_exclusive()?;
    let result = action();
    FileExt::unlock(&lock)?;
    result
}

pub fn transfer_root() -> Result<PathBuf> {
    if let Some(value) = std::env::var_os("MARGINS_TRANSFER_DIR") {
        return Ok(PathBuf::from(value));
    }
    Ok(dirs::home_dir()
        .context("could not determine home directory")?
        .join(".margins/transfers"))
}

/// Non-secret, durable identity for a session reservation whose HTTP/SSH
/// response may be lost. The server's reservation endpoints are exact-replay
/// safe, so reopening this intent recovers the same producer capability rather
/// than creating a second session or capture generation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureReservationRequestV1 {
    Create { command: ClientMessageV1 },
    Attach { request: WorkspaceAttachV1 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureReservationIntentV1 {
    pub schema: String,
    pub transfer_id: String,
    pub instance_id: String,
    pub remote_url: String,
    pub workspace_id: String,
    pub session_id: String,
    pub request: CaptureReservationRequestV1,
}

impl CaptureReservationIntentV1 {
    pub fn persist(self, transfer_root: &Path) -> Result<Self> {
        validate_component(&self.transfer_id)?;
        let directory = transfer_root.join("reservations");
        std::fs::create_dir_all(&directory)?;
        set_directory_owner_only(&directory)?;
        with_reservation_lock(transfer_root, &self.transfer_id, || {
            let path = directory.join(format!("{}.json", self.transfer_id));
            atomic_json(&path, &self)?;
            set_owner_only(&path)
        })?;
        Ok(self)
    }

    pub fn remove(&self, transfer_root: &Path) -> Result<()> {
        with_reservation_lock(transfer_root, &self.transfer_id, || {
            let path = reservation_path(transfer_root, &self.transfer_id)?;
            match std::fs::remove_file(path) {
                Ok(()) => sync_dir(&transfer_root.join("reservations")),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        })
    }
}

pub fn pending_capture_reservations(
    transfer_root: &Path,
) -> Result<Vec<CaptureReservationIntentV1>> {
    let directory = transfer_root.join("reservations");
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for entry in std::fs::read_dir(&directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() || entry.path().extension().is_none_or(|v| v != "json") {
            continue;
        }
        let path = entry.path();
        let bytes =
            with_reservation_lock(transfer_root, &id_from_reservation_path(&path)?, || {
                match std::fs::read(&path) {
                    Ok(bytes) => Ok(Some(bytes)),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                    Err(error) => Err(error.into()),
                }
            })?;
        let Some(bytes) = bytes else { continue };
        let value: CaptureReservationIntentV1 = serde_json::from_slice(&bytes)?;
        validate_component(&value.transfer_id)?;
        if entry.path() != reservation_path(transfer_root, &value.transfer_id)? {
            bail!("reservation file identity does not match its durable content");
        }
        values.push(value);
    }
    values.sort_by(|left, right| left.transfer_id.cmp(&right.transfer_id));
    Ok(values)
}

fn id_from_reservation_path(path: &Path) -> Result<String> {
    let id = path
        .file_stem()
        .and_then(|value| value.to_str())
        .context("reservation path has no UTF-8 identity")?
        .to_string();
    validate_component(&id)?;
    Ok(id)
}

fn reservation_path(root: &Path, transfer_id: &str) -> Result<PathBuf> {
    validate_component(transfer_id)?;
    Ok(root
        .join("reservations")
        .join(format!("{transfer_id}.json")))
}

fn with_reservation_lock<T>(
    root: &Path,
    transfer_id: &str,
    action: impl FnOnce() -> Result<T>,
) -> Result<T> {
    validate_component(transfer_id)?;
    let directory = root.join("reservations");
    std::fs::create_dir_all(&directory)?;
    set_directory_owner_only(&directory)?;
    let path = directory.join(format!("{transfer_id}.lock"));
    let lock = File::options().create(true).append(true).open(&path)?;
    set_owner_only(&path)?;
    lock.lock_exclusive()?;
    let result = action();
    FileExt::unlock(&lock)?;
    result
}

pub fn promote_capture_reservation(
    intent: &CaptureReservationIntentV1,
    transfer_root: &Path,
    reserve_bytes: u64,
    obtain_producer_token: impl FnOnce() -> Result<String>,
) -> Result<DurableTransferSpool> {
    with_reservation_lock(transfer_root, &intent.transfer_id, || {
        let manifest = transfer_root
            .join(&intent.transfer_id)
            .join("manifest.json");
        let spool = if manifest.is_file() {
            DurableTransferSpool::open(transfer_root, &intent.transfer_id, reserve_bytes)?
        } else {
            let token = obtain_producer_token()?;
            DurableTransferSpool::create(
                transfer_root,
                &intent.transfer_id,
                &intent.instance_id,
                &intent.remote_url,
                &intent.workspace_id,
                &intent.session_id,
                &token,
                reserve_bytes,
            )?
        };
        let manifest = spool.manifest();
        if manifest.transfer_id != intent.transfer_id
            || manifest.instance_id != intent.instance_id
            || manifest.remote_url != intent.remote_url
            || manifest.workspace_id != intent.workspace_id
            || manifest.session_id != intent.session_id
        {
            bail!("promoted transfer identity conflicts with its reservation intent");
        }
        Ok(spool)
    })
}

pub struct TransferCaptureLease {
    _lock: File,
}

impl DurableTransferSpool {
    /// Hold for the entire recorder lifetime. Manifest operations use their own
    /// short lock; this lease prevents two retrying CLI processes from driving
    /// the same producer generation concurrently.
    pub fn acquire_capture_lease(&self) -> Result<TransferCaptureLease> {
        let path = self.root.join("capture.lock");
        let lock = File::options().create(true).append(true).open(&path)?;
        set_owner_only(&path)?;
        if !lock.try_lock_exclusive()? {
            bail!("this remote transfer is already being captured by another process");
        }
        Ok(TransferCaptureLease { _lock: lock })
    }
}

/// New native remote captures negotiate the same 16 kHz representation used by
/// the local durable archive and the offline ASR backend. Existing spools keep
/// their command-declared rate and are replayed without reinterpretation.
pub const NATIVE_PCM_RATE_HZ: u32 = 16_000;
pub const NATIVE_PCM_CHUNK_FRAMES: usize = 1_600;
const NATIVE_RESAMPLE_INPUT_FRAMES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativePcmLane {
    Microphone,
    System,
}

impl NativePcmLane {
    pub fn id(self) -> &'static str {
        match self {
            Self::Microphone => "mic",
            Self::System => "system",
        }
    }
}

pub fn native_create_session_command(
    session_id: &str,
    idempotency_key: &str,
    title: Option<String>,
    producer: &str,
) -> ClientMessageV1 {
    let sources = [
        ("mic", CaptureSourceKindV1::Microphone),
        ("system", CaptureSourceKindV1::SystemAudio),
    ]
    .into_iter()
    .map(|(id, kind)| CaptureSourceV1 {
        source_id: id.into(),
        kind,
        label: None,
        external_id: None,
    })
    .collect();
    let lanes = ["mic", "system"]
        .into_iter()
        .map(|id| CaptureLaneV1 {
            lane_id: id.into(),
            source_ids: vec![id.into()],
            label: None,
            format: AudioFormatV1 {
                codec: AudioCodecV1::PcmS16Le,
                container: AudioContainerV1::Raw,
                sample_rate_hz: NATIVE_PCM_RATE_HZ,
                channel_count: 1,
            },
        })
        .collect();
    let now = unix_ms();
    ClientMessageV1 {
        protocol_version: Default::default(),
        message_id: format!("create-{idempotency_key}").into(),
        session_id: session_id.into(),
        sent_at_unix_ms: margins_meeting_protocol::UnixMillis(now),
        body: ClientMessageBodyV1::CreateSession(CreateSessionV1 {
            idempotency_key: idempotency_key.to_string(),
            started_at_unix_ms: margins_meeting_protocol::UnixMillis(now),
            title,
            sources,
            lanes,
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: producer.to_string(),
                    producer_version: option_env!("CARGO_PKG_VERSION").map(str::to_string),
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: margins_meeting_protocol::UnixMillis(now),
                    attributes: BTreeMap::new(),
                }],
            },
        }),
    }
}

struct NativePcmSegment {
    id: String,
    starts_at_ms: u64,
    lanes: BTreeMap<NativePcmLane, NativePcmLaneStream>,
}

struct NativePcmLaneStream {
    input_rate_hz: u32,
    input_frames: u64,
    emitted_frames: u64,
    next_sequence: u64,
    input_pending: Vec<f32>,
    output_pending: Vec<f32>,
    pcm_pending: Vec<u8>,
    resampler: Option<SincFixedIn<f32>>,
    delay_to_trim: usize,
}

impl NativePcmLaneStream {
    fn new(input_rate_hz: u32) -> Result<Self> {
        if input_rate_hz == 0 {
            bail!("native input sample rate must be nonzero");
        }
        let (resampler, delay_to_trim) = if input_rate_hz == NATIVE_PCM_RATE_HZ {
            (None, 0)
        } else {
            let parameters = SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                oversampling_factor: 128,
                interpolation: SincInterpolationType::Cubic,
                window: WindowFunction::BlackmanHarris2,
            };
            let resampler = SincFixedIn::<f32>::new(
                f64::from(NATIVE_PCM_RATE_HZ) / f64::from(input_rate_hz),
                1.0,
                parameters,
                NATIVE_RESAMPLE_INPUT_FRAMES,
                1,
            )
            .context("failed to construct native anti-aliasing resampler")?;
            let delay = resampler.output_delay();
            (Some(resampler), delay)
        };
        Ok(Self {
            input_rate_hz,
            input_frames: 0,
            emitted_frames: 0,
            next_sequence: 0,
            input_pending: Vec::with_capacity(NATIVE_RESAMPLE_INPUT_FRAMES * 2),
            output_pending: Vec::new(),
            pcm_pending: Vec::with_capacity(NATIVE_PCM_CHUNK_FRAMES * 4),
            resampler,
            delay_to_trim,
        })
    }

    fn ensure_rate(&self, input_rate_hz: u32) -> Result<()> {
        if self.input_rate_hz != input_rate_hz {
            bail!(
                "native lane sample rate changed within a segment ({} -> {})",
                self.input_rate_hz,
                input_rate_hz
            );
        }
        Ok(())
    }

    fn append_f32(&mut self, samples: &[f32]) -> Result<usize> {
        let before = self.available_pcm_frames();
        self.input_frames = self.input_frames.saturating_add(samples.len() as u64);
        if self.resampler.is_none() {
            self.pcm_pending
                .extend_from_slice(&encode_pcm_s16le(samples));
            return Ok(self.available_pcm_frames().saturating_sub(before));
        }
        self.input_pending.extend_from_slice(samples);
        while self.input_pending.len() >= NATIVE_RESAMPLE_INPUT_FRAMES {
            let block = self
                .input_pending
                .drain(..NATIVE_RESAMPLE_INPUT_FRAMES)
                .collect::<Vec<_>>();
            self.process_resampler_block(Some(&block))?;
        }
        self.publish_resampled(false);
        Ok(self.available_pcm_frames().saturating_sub(before))
    }

    fn append_s16le(&mut self, bytes: &[u8]) -> Result<usize> {
        if bytes.len() % 2 != 0 {
            bail!("s16 PCM fixture has a partial sample");
        }
        if self.input_rate_hz == NATIVE_PCM_RATE_HZ {
            self.input_frames = self.input_frames.saturating_add((bytes.len() / 2) as u64);
            self.pcm_pending.extend_from_slice(bytes);
            return Ok(bytes.len() / 2);
        }
        let samples = bytes
            .chunks_exact(2)
            .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0)
            .collect::<Vec<_>>();
        self.append_f32(&samples)
    }

    fn finish(&mut self) -> Result<()> {
        if self.resampler.is_none() {
            return Ok(());
        }
        if !self.input_pending.is_empty() {
            let tail = std::mem::take(&mut self.input_pending);
            self.process_resampler_block(Some(&tail))?;
        }
        let target = self.target_output_frames(true);
        while self.emitted_frames
            + self.available_pcm_frames() as u64
            + (self.output_pending.len() as u64)
            < target
        {
            self.process_resampler_block(None)?;
        }
        self.publish_resampled(true);
        Ok(())
    }

    fn process_resampler_block(&mut self, input: Option<&[f32]>) -> Result<()> {
        let resampler = self
            .resampler
            .as_mut()
            .context("resampler block requested for a pass-through lane")?;
        let output = if let Some(input) = input {
            if input.len() == NATIVE_RESAMPLE_INPUT_FRAMES {
                resampler.process(&[input], None)
            } else {
                resampler.process_partial(Some(&[input]), None)
            }
        } else {
            resampler.process_partial::<&[f32]>(None, None)
        }
        .context("native anti-aliasing resampler failed")?;
        let mut output = output.into_iter().next().unwrap_or_default();
        let trim = self.delay_to_trim.min(output.len());
        if trim > 0 {
            output.drain(..trim);
            self.delay_to_trim -= trim;
        }
        self.output_pending.extend(output);
        Ok(())
    }

    fn publish_resampled(&mut self, finishing: bool) {
        let target = self.target_output_frames(finishing);
        let represented = self
            .emitted_frames
            .saturating_add(self.available_pcm_frames() as u64);
        let allowed = target
            .saturating_sub(represented)
            .min(self.output_pending.len() as u64) as usize;
        if allowed == 0 {
            return;
        }
        let samples = self.output_pending.drain(..allowed).collect::<Vec<_>>();
        self.pcm_pending
            .extend_from_slice(&encode_pcm_s16le(&samples));
    }

    fn target_output_frames(&self, finishing: bool) -> u64 {
        let numerator = u128::from(self.input_frames) * u128::from(NATIVE_PCM_RATE_HZ);
        let denominator = u128::from(self.input_rate_hz);
        if finishing {
            numerator.div_ceil(denominator) as u64
        } else {
            (numerator / denominator) as u64
        }
    }

    fn available_pcm_frames(&self) -> usize {
        self.pcm_pending.len() / 2
    }

    fn take_pcm_chunks(&mut self, include_partial: bool) -> Vec<(u64, u64, Vec<u8>)> {
        let mut chunks = Vec::new();
        while self.pcm_pending.len() >= NATIVE_PCM_CHUNK_FRAMES * 2
            || (include_partial && !self.pcm_pending.is_empty())
        {
            let byte_count = self.pcm_pending.len().min(NATIVE_PCM_CHUNK_FRAMES * 2);
            let payload = self.pcm_pending.drain(..byte_count).collect::<Vec<_>>();
            let frames = (payload.len() / 2) as u64;
            let frame_start = self.emitted_frames;
            let sequence = self.next_sequence;
            self.emitted_frames = self.emitted_frames.saturating_add(frames);
            self.next_sequence = self.next_sequence.saturating_add(1);
            chunks.push((sequence, frame_start, payload));
        }
        chunks
    }
}

/// Converts the recorder's bounded mono f32 lane feed into durable, bounded
/// PCM commands. This is the only adapter added to native capture; the normal
/// local recorder path does not construct it or hash/encode transport frames.
pub struct NativePcmTransfer {
    spool: DurableTransferSpool,
    segment: Option<NativePcmSegment>,
    closes: Vec<SegmentCloseReferenceV1>,
}

impl NativePcmTransfer {
    pub fn new(spool: DurableTransferSpool) -> Self {
        let closes = spool
            .manifest()
            .close_commands
            .iter()
            .filter_map(|command| match &command.body {
                ClientMessageBodyV1::CloseSegment(close) => Some(SegmentCloseReferenceV1 {
                    segment_id: close.segment_id.clone(),
                    close_message_id: command.message_id.clone(),
                }),
                _ => None,
            })
            .collect();
        Self {
            spool,
            segment: None,
            closes,
        }
    }

    pub fn begin_segment(&mut self, segment_id: String, starts_at_ms: u64) -> Result<()> {
        validate_component(&segment_id)?;
        if self.segment.is_some() {
            bail!("the previous native segment must be closed before resume");
        }
        self.segment = Some(NativePcmSegment {
            id: segment_id,
            starts_at_ms,
            lanes: BTreeMap::new(),
        });
        Ok(())
    }

    pub fn append_f32(
        &mut self,
        lane: NativePcmLane,
        input_rate_hz: u32,
        samples: &[f32],
    ) -> Result<usize> {
        let written = {
            let segment = self
                .segment
                .as_mut()
                .context("native PCM arrived without an open segment")?;
            if !segment.lanes.contains_key(&lane) {
                segment
                    .lanes
                    .insert(lane, NativePcmLaneStream::new(input_rate_hz)?);
            }
            let stream = segment.lanes.get_mut(&lane).expect("lane was inserted");
            stream.ensure_rate(input_rate_hz)?;
            stream.append_f32(samples)?
        };
        self.flush_lane(lane, false)?;
        Ok(written)
    }

    fn flush_lane(&mut self, lane: NativePcmLane, include_partial: bool) -> Result<()> {
        let (segment_id, starts_at_ms, chunks) = {
            let segment = self
                .segment
                .as_mut()
                .context("native PCM arrived without an open segment")?;
            let stream = segment
                .lanes
                .get_mut(&lane)
                .context("native lane has no stream state")?;
            (
                segment.id.clone(),
                segment.starts_at_ms,
                stream.take_pcm_chunks(include_partial),
            )
        };
        for (sequence, frame_start, payload) in chunks {
            let frame_count = payload.len() / 2;
            let command = ClientMessageV1 {
                protocol_version: Default::default(),
                message_id: next_message_id("pcm").into(),
                session_id: self.spool.manifest.session_id.clone().into(),
                sent_at_unix_ms: margins_meeting_protocol::UnixMillis(unix_ms()),
                body: ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                    segment_id: segment_id.clone().into(),
                    lane_id: lane.id().into(),
                    sequence,
                    starts_at_ms: SessionMillis(
                        starts_at_ms
                            + frame_start.saturating_mul(1_000) / u64::from(NATIVE_PCM_RATE_HZ),
                    ),
                    duration_ms: DurationMillis(
                        (frame_count as u64).saturating_mul(1_000) / u64::from(NATIVE_PCM_RATE_HZ),
                    ),
                    payload_digest: ContentDigestV1 {
                        algorithm: DigestAlgorithmV1::Sha256,
                        hex: format!("{:x}", Sha256::digest(&payload)),
                    },
                    payload,
                }),
            };
            self.spool.append_chunk(&command)?;
        }
        Ok(())
    }

    pub fn append_s16le(
        &mut self,
        lane: NativePcmLane,
        input_rate_hz: u32,
        bytes: &[u8],
    ) -> Result<usize> {
        let written = {
            let segment = self
                .segment
                .as_mut()
                .context("native PCM arrived without an open segment")?;
            if !segment.lanes.contains_key(&lane) {
                segment
                    .lanes
                    .insert(lane, NativePcmLaneStream::new(input_rate_hz)?);
            }
            let stream = segment.lanes.get_mut(&lane).expect("lane was inserted");
            stream.ensure_rate(input_rate_hz)?;
            stream.append_s16le(bytes)?
        };
        self.flush_lane(lane, false)?;
        Ok(written)
    }

    pub fn close_segment(&mut self, reason: SegmentCloseReasonV1) -> Result<ClientMessageV1> {
        let lanes = self
            .segment
            .as_ref()
            .context("no native segment is open")?
            .lanes
            .keys()
            .copied()
            .collect::<Vec<_>>();
        for lane in lanes {
            self.segment
                .as_mut()
                .expect("checked above")
                .lanes
                .get_mut(&lane)
                .expect("lane key came from map")
                .finish()?;
            self.flush_lane(lane, true)?;
        }
        let segment = self.segment.take().expect("checked above");
        let ended_at_ms = segment.starts_at_ms
            + segment
                .lanes
                .values()
                .map(|lane| lane.emitted_frames)
                .max()
                .unwrap_or(0)
                .saturating_mul(1_000)
                / u64::from(NATIVE_PCM_RATE_HZ);
        let message_id = next_message_id("close");
        let command = ClientMessageV1 {
            protocol_version: Default::default(),
            message_id: message_id.clone().into(),
            session_id: self.spool.manifest.session_id.clone().into(),
            sent_at_unix_ms: margins_meeting_protocol::UnixMillis(unix_ms()),
            body: ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                segment_id: segment.id.clone().into(),
                ended_at_ms: SessionMillis(ended_at_ms),
                lane_boundaries: [NativePcmLane::Microphone, NativePcmLane::System]
                    .into_iter()
                    .map(|lane| LaneBoundaryV1 {
                        lane_id: lane.id().into(),
                        next_sequence: segment
                            .lanes
                            .get(&lane)
                            .map_or(0, |lane| lane.next_sequence),
                    })
                    .collect(),
                reason,
            }),
        };
        self.spool.set_close(command.clone())?;
        self.closes.push(SegmentCloseReferenceV1 {
            segment_id: segment.id.into(),
            close_message_id: message_id.into(),
        });
        Ok(command)
    }

    pub fn seal_session(
        &mut self,
        ended_at_ms: u64,
        reason: SessionFinalizeReasonV1,
    ) -> Result<ClientMessageV1> {
        if self.segment.is_some() {
            bail!("native segment must be closed before session finalization");
        }
        let command = ClientMessageV1 {
            protocol_version: Default::default(),
            message_id: next_message_id("finalize").into(),
            session_id: self.spool.manifest.session_id.clone().into(),
            sent_at_unix_ms: margins_meeting_protocol::UnixMillis(unix_ms()),
            body: ClientMessageBodyV1::FinalizeSession(FinalizeSessionV1 {
                ended_at_ms: SessionMillis(ended_at_ms),
                segment_closes: self.closes.clone(),
                reason,
            }),
        };
        self.spool.set_finalize(command.clone())?;
        Ok(command)
    }

    pub fn spool(&self) -> &DurableTransferSpool {
        &self.spool
    }

    pub fn spool_mut(&mut self) -> &mut DurableTransferSpool {
        &mut self.spool
    }

    pub fn into_spool(self) -> DurableTransferSpool {
        self.spool
    }
}

fn encode_pcm_s16le(samples: &[f32]) -> Vec<u8> {
    samples
        .iter()
        .flat_map(|sample| {
            let value = if *sample <= -1.0 {
                i16::MIN
            } else {
                (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
            };
            value.to_le_bytes()
        })
        .collect()
}

/// Decode a finalized mono/raw/s16 remote lane for the fixed 16 kHz ASR
/// boundary. New transfers are a byte-preserving decode; legacy 48 kHz
/// transfers use the same anti-aliasing converter as capture-time delivery.
pub fn remote_pcm_s16le_for_asr(bytes: &[u8], sample_rate_hz: u32) -> Result<Vec<f32>> {
    if bytes.len() % 2 != 0 {
        bail!("remote PCM artifact has a partial s16 sample");
    }
    if sample_rate_hz == 0 {
        bail!("remote PCM artifact has a zero sample rate");
    }
    let samples = bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0)
        .collect::<Vec<_>>();
    if sample_rate_hz == NATIVE_PCM_RATE_HZ {
        return Ok(samples);
    }
    let mut stream = NativePcmLaneStream::new(sample_rate_hz)?;
    stream.append_f32(&samples)?;
    stream.finish()?;
    let expected = stream.target_output_frames(true) as usize;
    let output = stream
        .pcm_pending
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32_768.0)
        .collect::<Vec<_>>();
    if output.len() != expected {
        bail!(
            "remote PCM resampler produced {} frames; expected {expected}",
            output.len()
        );
    }
    Ok(output)
}

fn next_message_id(prefix: &str) -> String {
    let suffix: String = rand::thread_rng()
        .sample_iter(&Alphanumeric)
        .take(20)
        .map(char::from)
        .collect();
    format!("{prefix}-{suffix}")
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Deliver one sealed transfer in durability order. Every operation is
/// identity-stable, so a lost response or process restart repeats the exact
/// command. Local bytes are reclaimed only after the corresponding durable
/// server ACK; the transfer remains listed until finalization is acknowledged.
pub fn deliver_transfer(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_chunks_unchecked(spool, client)?;
    deliver_closes_unchecked(spool, client)?;
    deliver_memo_unchecked(spool, client)?;
    deliver_finalize_unchecked(spool, client)
}

/// Drain the capture-time portion of a transfer without finalizing it. This is
/// safe to call concurrently with a producer that atomically publishes new
/// frames and close intents. Errors leave the exact bytes/IDs pending.
pub fn deliver_available(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_chunks_unchecked(spool, client)?;
    deliver_closes_unchecked(spool, client)
}

pub fn deliver_chunks(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_chunks_unchecked(spool, client)
}

fn deliver_chunks_unchecked(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    let producer_token = spool.producer_token()?;
    for chunk in spool.pending_chunks()? {
        client.upload_chunk(&producer_token, &chunk.command)?;
        spool.acknowledge(&chunk)?;
    }
    Ok(())
}

pub fn deliver_closes(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_closes_unchecked(spool, client)
}

fn deliver_closes_unchecked(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    let producer_token = spool.producer_token()?;
    for close in spool.pending_closes() {
        client.execute(&spool.manifest.session_id, &producer_token, &close)?;
        spool.acknowledge_control(&close)?;
    }
    Ok(())
}

pub fn deliver_memo(spool: &mut DurableTransferSpool, client: &WorkspaceHttpClient) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_memo_unchecked(spool, client)
}

fn deliver_memo_unchecked(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    if spool.memo_pending()? {
        let memo = spool
            .manifest
            .memo_intent
            .as_ref()
            .context("pending memo has no durable intent")?;
        client.replace_memo(&spool.manifest.session_id, memo)?;
        spool.acknowledge_memo()?;
    }
    Ok(())
}

pub fn deliver_finalize(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_finalize_unchecked(spool, client)
}

fn deliver_finalize_unchecked(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    spool.complete_with(|manifest, producer_token| {
        let finalize = manifest
            .finalize_command
            .as_ref()
            .context("remote transfer has not been sealed for finalization")?;
        client.execute(&manifest.session_id, producer_token, finalize)?;
        Ok(())
    })
}

fn validate_transfer_instance(
    spool: &DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    let capabilities = client.capabilities()?;
    if capabilities.instance_id.as_ref() != spool.manifest.instance_id {
        bail!("remote instance identity changed; preserved transfer was not sent");
    }
    Ok(())
}

pub fn list_transfers(root: &Path) -> Result<Vec<String>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut values = std::fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let id = entry.file_name().into_string().ok()?;
            let manifest: TransferManifest =
                serde_json::from_slice(&std::fs::read(entry.path().join("manifest.json")).ok()?)
                    .ok()?;
            (!manifest.completed).then_some(id)
        })
        .collect::<Vec<_>>();
    values.sort();
    Ok(values)
}

fn chunk_name(command: &ClientMessageV1, chunk: &AudioChunkV1) -> String {
    format!(
        "{}-{}-{:020}-{}.frame",
        safe_name(chunk.segment_id.as_ref()),
        safe_name(chunk.lane_id.as_ref()),
        chunk.sequence,
        safe_name(command.message_id.as_ref())
    )
}

fn encode_spool_frame(command: &ClientMessageV1) -> Result<Vec<u8>> {
    let json = serde_json::to_vec(command)?;
    let length = u32::try_from(json.len()).context("spool header too large")?;
    let mut bytes = Vec::with_capacity(4 + json.len());
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

fn decode_spool_frame(path: &Path) -> Result<ClientMessageV1> {
    let mut file = File::open(path)?;
    let mut length = [0; 4];
    file.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length) as usize;
    if length > 4 * 1024 * 1024 {
        bail!("spool header exceeds bound");
    }
    let mut json = vec![0; length];
    file.read_exact(&mut json)?;
    if file.read(&mut [0])? != 0 {
        bail!("spool frame has trailing bytes");
    }
    Ok(serde_json::from_slice(&json)?)
}

fn safe_name(value: &str) -> String {
    value
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || matches!(value, '-' | '_') {
                value
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn validate_component(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || safe_name(value) != value {
        bail!("unsafe transfer id");
    }
    Ok(())
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec_pretty(value)?)
}

fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    sync_dir(parent)
}

fn atomic_bytes_noclobber(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        if std::fs::read(path)? == bytes {
            return Ok(());
        }
        bail!("spool identity conflicts with different bytes");
    }
    let parent = path.parent().context("path has no parent")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    sync_dir(parent)
}

fn sync_dir(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

fn set_directory_owner_only(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<()> {
    Ok(())
}
