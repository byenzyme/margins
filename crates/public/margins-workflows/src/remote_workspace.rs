//! Opt-in remote transport selection and crash-recoverable delivery spool.
//!
//! This module is never constructed by the default local capture path.

use anyhow::{bail, Context, Result};
use margins_meeting_protocol::{AudioChunkV1, ClientMessageBodyV1, ClientMessageV1};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
                let output = std::process::Command::new("ssh")
                    .args(ssh_discovery_args(&alias)?)
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
        envelope
            .result
            .context("successful response omitted its result")
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
    validate_ssh_alias(alias)?;
    Ok(vec![
        "-T".into(),
        "--".into(),
        alias.into(),
        "margins".into(),
        "service".into(),
        "discover".into(),
        "--json".into(),
    ])
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransferManifest {
    pub schema: String,
    pub transfer_id: String,
    pub instance_id: String,
    pub remote_url: String,
    pub workspace_id: String,
    pub session_id: String,
    pub producer_token: String,
    pub close_command: Option<ClientMessageV1>,
    pub finalize_command: Option<ClientMessageV1>,
}

#[derive(Deserialize)]
struct ClientEnvelope<T> {
    ok: bool,
    result: Option<T>,
    error: Option<margins_meeting_protocol::WorkspaceErrorV1>,
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
        set_directory_owner_only(&transfer_root)?;
        set_directory_owner_only(&transfer_root.join("chunks"))?;
        let manifest = TransferManifest {
            schema: "margins.remote-transfer.v1".into(),
            transfer_id: transfer_id.into(),
            instance_id: instance_id.into(),
            remote_url: remote_url.into(),
            workspace_id: workspace_id.into(),
            session_id: session_id.into(),
            producer_token: producer_token.into(),
            close_command: None,
            finalize_command: None,
        };
        atomic_json(&transfer_root.join("manifest.json"), &manifest)?;
        set_owner_only(&transfer_root.join("manifest.json"))?;
        Ok(Self {
            root: transfer_root,
            manifest,
            reserve_bytes,
        })
    }

    pub fn open(root: &Path, transfer_id: &str, reserve_bytes: u64) -> Result<Self> {
        validate_component(transfer_id)?;
        let root = root.join(transfer_id);
        let manifest = serde_json::from_slice(&std::fs::read(root.join("manifest.json"))?)?;
        Ok(Self {
            root,
            manifest,
            reserve_bytes,
        })
    }

    pub fn manifest(&self) -> &TransferManifest {
        &self.manifest
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
    }

    pub fn pending_chunks(&self) -> Result<Vec<SpoolChunk>> {
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
    }

    pub fn set_close(&mut self, command: ClientMessageV1) -> Result<()> {
        if !matches!(command.body, ClientMessageBodyV1::CloseSegment(_)) {
            bail!("close intent requires close_segment");
        }
        self.manifest.close_command = Some(command);
        self.persist_manifest()
    }

    pub fn set_finalize(&mut self, command: ClientMessageV1) -> Result<()> {
        if !matches!(command.body, ClientMessageBodyV1::FinalizeSession(_)) {
            bail!("finalize intent requires finalize_session");
        }
        self.manifest.finalize_command = Some(command);
        self.persist_manifest()
    }

    pub fn ready_to_finalize(&self) -> Result<bool> {
        Ok(self.pending_chunks()?.is_empty()
            && self.manifest.close_command.is_some()
            && self.manifest.finalize_command.is_some())
    }

    fn persist_manifest(&self) -> Result<()> {
        atomic_json(&self.root.join("manifest.json"), &self.manifest)
    }
}

pub fn list_transfers(root: &Path) -> Result<Vec<String>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut values = std::fs::read_dir(root)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
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
