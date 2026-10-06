//! Opt-in remote transport selection and crash-recoverable delivery spool.
//!
//! This module is never constructed by the default local capture path.

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use fs4::fs_std::FileExt;
use margins_meeting_protocol::{
    decode_opus_packet_blocks_v1, validate_opus_packet_stream_v1, AudioChunkBatchV1, AudioChunkV1,
    AudioCodecV1, AudioContainerV1, AudioFormatV1, CaptureLaneV1, CaptureModeV1,
    CaptureProvenanceHopV1, CaptureProvenanceV1, CaptureSourceKindV1, CaptureSourceV1,
    ClientMessageBodyV1, ClientMessageV1, CloseSegmentV1, ContentDigestV1, CreateSessionV1,
    DigestAlgorithmV1, DurationMillis, FinalizeSessionV1, LaneBoundaryV1, OpusPacketBlockV1,
    SegmentCloseReasonV1, SegmentCloseReferenceV1, SessionFinalizeReasonV1, SessionId,
    SessionMillis, WorkspaceAttachV1, WorkspaceMemoReplaceV1, WorkspaceMemoUpdateV1,
    WorkspaceNoteAssociationUpdateV1, WorkspaceRenameV1, AUDIO_CHUNK_BATCH_CONTENT_TYPE_V1,
    OPUS_PACKET_FRAME_SAMPLES_V1, OPUS_PACKET_MAX_BYTES_V1,
    OPUS_PACKET_STREAM_MAX_PACKETS_PER_BLOCK_V1, OPUS_PACKET_STREAM_SAMPLE_RATE_HZ_V1,
};
use rand::{distributions::Alphanumeric, Rng};
use ropus::{Application, Bitrate, Channels, DecodeMode, Decoder, Encoder, Signal};
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
use std::sync::{
    atomic::{AtomicU32, AtomicU64, Ordering},
    Arc, Mutex,
};

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
    relay_url: Option<url::Url>,
    token: String,
    workspace_id: String,
    expected_instance_id: Option<String>,
    client: reqwest::blocking::Client,
    max_chunk_bytes: Arc<AtomicU64>,
    max_batch_commands: Arc<AtomicU32>,
    observed_instance_id: Arc<Mutex<Option<String>>>,
    delivery_metrics: Arc<DeliveryMetrics>,
}

#[derive(Debug, Default)]
struct DeliveryMetrics {
    http_batch_requests: AtomicU64,
    durable_audio_commands: AtomicU64,
    durable_audio_receipts: AtomicU64,
    encoded_payload_bytes: AtomicU64,
    batch_body_bytes: AtomicU64,
    batch_request_micros: AtomicU64,
    last_batch_ack_unix_ms: AtomicU64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct DeliveryMetricsSnapshot {
    pub http_batch_requests: u64,
    pub durable_audio_commands: u64,
    pub durable_audio_receipts: u64,
    pub encoded_payload_bytes: u64,
    pub batch_body_bytes: u64,
    pub batch_request_micros: u64,
    pub last_batch_ack_unix_ms: u64,
}

pub struct RemoteConnection {
    pub client: WorkspaceHttpClient,
    pub capabilities: margins_meeting_protocol::WorkspaceCapabilitiesV1,
    tunnel: Option<std::process::Child>,
}

impl RemoteConnection {
    /// Reuse an already verified HTTP destination without repeating discovery
    /// when a paired recorder starts. SSH connections own a tunnel and must be
    /// established separately for each capture.
    pub fn reusable_http(&self) -> Option<Self> {
        self.tunnel.is_none().then(|| Self {
            client: self.client.clone(),
            capabilities: self.capabilities.clone(),
            tunnel: None,
        })
    }

    pub fn connect(remote: &str, workspace_id: &str, https_token: Option<&str>) -> Result<Self> {
        match RemoteEndpoint::parse(remote)? {
            RemoteEndpoint::Https(url) | RemoteEndpoint::LoopbackHttp(url) => {
                let token = https_token.context("MARGINS_REMOTE_TOKEN is required for HTTPS")?;
                let client = WorkspaceHttpClient::new(url, token, workspace_id, None)?;
                let capabilities = client.capabilities()?;
                Ok(Self {
                    client,
                    capabilities,
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
                let capabilities = loop {
                    if let Some(status) = tunnel.try_wait()? {
                        bail!("SSH tunnel exited before readiness ({status})");
                    }
                    match client.capabilities() {
                        Ok(capabilities) => break capabilities,
                        Err(_) if std::time::Instant::now() < deadline => {
                            std::thread::sleep(std::time::Duration::from_millis(50));
                        }
                        Err(error) => {
                            let _ = tunnel.kill();
                            bail!("SSH tunnel did not become ready: {error}");
                        }
                    }
                };
                Ok(Self {
                    client,
                    capabilities,
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
        mut base_url: url::Url,
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
        let relay_url = base_url
            .path()
            .ends_with("/api/v1/plugins/margins/http/menu/relay")
            .then(|| base_url.clone());
        if relay_url.is_some() {
            base_url.set_path("/");
            base_url.set_query(None);
        }
        Ok(Self {
            base_url,
            relay_url,
            token: token.into(),
            workspace_id: workspace_id.into(),
            expected_instance_id,
            client,
            max_chunk_bytes: Arc::new(AtomicU64::new(1_048_576)),
            max_batch_commands: Arc::new(AtomicU32::new(1)),
            observed_instance_id: Arc::new(Mutex::new(None)),
            delivery_metrics: Arc::new(DeliveryMetrics::default()),
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
        self.max_chunk_bytes
            .store(value.limits.max_chunk_bytes, Ordering::Release);
        self.max_batch_commands
            .store(value.limits.max_in_flight_chunks.max(1), Ordering::Release);
        *self
            .observed_instance_id
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace client instance cache is poisoned"))? =
            Some(value.instance_id.0.clone());
        Ok(value)
    }

    pub fn delivery_metrics(&self) -> DeliveryMetricsSnapshot {
        DeliveryMetricsSnapshot {
            http_batch_requests: self
                .delivery_metrics
                .http_batch_requests
                .load(Ordering::Acquire),
            durable_audio_commands: self
                .delivery_metrics
                .durable_audio_commands
                .load(Ordering::Acquire),
            durable_audio_receipts: self
                .delivery_metrics
                .durable_audio_receipts
                .load(Ordering::Acquire),
            encoded_payload_bytes: self
                .delivery_metrics
                .encoded_payload_bytes
                .load(Ordering::Acquire),
            batch_body_bytes: self
                .delivery_metrics
                .batch_body_bytes
                .load(Ordering::Acquire),
            batch_request_micros: self
                .delivery_metrics
                .batch_request_micros
                .load(Ordering::Acquire),
            last_batch_ack_unix_ms: self
                .delivery_metrics
                .last_batch_ack_unix_ms
                .load(Ordering::Acquire),
        }
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
        let request_builder = self
            .client
            .post(self.url(&format!(
                "v1/workspaces/{}/sessions/{}/memo/replace",
                self.workspace_id,
                urlencoding(session)
            ))?)
            .json(request);
        self.request_json(self.capture_request(request_builder)?)
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

    /// Provisional on-device words are best-effort; durable audio delivery is
    /// independent of this request. Send the worker's exact v2 checkpoint.
    pub fn put_live_checkpoint(
        &self,
        session: &str,
        producer_token: &str,
        checkpoint: Vec<u8>,
    ) -> Result<()> {
        if checkpoint.len() > 256 * 1024 {
            bail!("live checkpoint exceeds 256 KiB");
        }
        let request = self
            .client
            .put(self.url(&format!(
                "v1/workspaces/{}/sessions/{}/live-checkpoint",
                self.workspace_id,
                urlencoding(session)
            ))?)
            .header("X-Margins-Producer-Token", producer_token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(3))
            .body(checkpoint);
        let _: serde_json::Value = self.request_json(self.capture_request(request)?)?;
        Ok(())
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
        let request = self
            .client
            .post(self.url(&format!(
                "v1/workspaces/{}/sessions/{}/commands",
                self.workspace_id,
                urlencoding(session)
            ))?)
            .header("X-Margins-Producer-Token", producer_token)
            .json(command);
        self.request_json(self.capture_request(request)?)
    }

    pub fn reserve(
        &self,
        command: &ClientMessageV1,
    ) -> Result<crate::workspace_service::SessionReservation> {
        let request = self
            .client
            .post(self.url(&format!("v1/workspaces/{}/sessions", self.workspace_id))?)
            .json(command);
        self.request_json(self.capture_request(request)?)
    }

    pub fn attach(
        &self,
        session: &str,
        request: &margins_meeting_protocol::WorkspaceAttachV1,
    ) -> Result<crate::workspace_service::SessionReservation> {
        let request_builder = self
            .client
            .post(self.url(&format!(
                "v1/workspaces/{}/sessions/{}/attach",
                self.workspace_id,
                urlencoding(session)
            ))?)
            .json(request);
        self.request_json(self.capture_request(request_builder)?)
    }

    pub fn upload_chunks(
        &self,
        producer_token: &str,
        commands: &[ClientMessageV1],
    ) -> Result<usize> {
        let session = commands
            .first()
            .context("upload_chunks requires at least one audio command")?
            .session_id
            .clone();
        if commands.iter().any(|command| command.session_id != session) {
            bail!("audio chunk batch crosses session identity");
        }
        let max_commands = self.max_batch_commands.load(Ordering::Acquire).max(1) as usize;
        let max_chunk_bytes = self.max_chunk_bytes.load(Ordering::Acquire);
        let body = AudioChunkBatchV1 {
            commands: commands.to_vec(),
        }
        .encode(max_commands, max_chunk_bytes)
        .map_err(anyhow::Error::msg)?;
        let body_bytes = body.len();
        let payload_bytes = commands
            .iter()
            .filter_map(|command| match &command.body {
                ClientMessageBodyV1::AudioChunk(chunk) => Some(chunk.payload.len() as u64),
                _ => None,
            })
            .sum::<u64>();
        self.delivery_metrics
            .http_batch_requests
            .fetch_add(1, Ordering::AcqRel);
        self.delivery_metrics
            .durable_audio_commands
            .fetch_add(commands.len() as u64, Ordering::AcqRel);
        self.delivery_metrics
            .encoded_payload_bytes
            .fetch_add(payload_bytes, Ordering::AcqRel);
        self.delivery_metrics
            .batch_body_bytes
            .fetch_add(body_bytes as u64, Ordering::AcqRel);
        let started = std::time::Instant::now();
        let request = self
            .client
            .post(self.url(&format!(
                "v1/workspaces/{}/sessions/{}/audio-chunks",
                self.workspace_id,
                urlencoding(session.as_ref())
            ))?)
            .header("X-Margins-Producer-Token", producer_token)
            .header(
                reqwest::header::CONTENT_TYPE,
                AUDIO_CHUNK_BATCH_CONTENT_TYPE_V1,
            )
            .body(body);
        let responses: Vec<margins_meeting_runtime::RuntimeResponseV1> =
            self.request_json(self.capture_request(request)?)?;
        self.delivery_metrics
            .batch_request_micros
            .fetch_add(started.elapsed().as_micros() as u64, Ordering::AcqRel);
        self.delivery_metrics
            .last_batch_ack_unix_ms
            .store(unix_ms(), Ordering::Release);
        if responses.len() != commands.len() {
            bail!("audio chunk batch response count does not match its commands");
        }
        self.delivery_metrics
            .durable_audio_receipts
            .fetch_add(responses.len() as u64, Ordering::AcqRel);
        Ok(body_bytes)
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

    fn capture_instance_id(&self) -> Result<String> {
        if let Some(instance_id) = self
            .observed_instance_id
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace client instance cache is poisoned"))?
            .clone()
        {
            return Ok(instance_id);
        }
        self.capabilities()?;
        self.observed_instance_id
            .lock()
            .map_err(|_| anyhow::anyhow!("Workspace client instance cache is poisoned"))?
            .clone()
            .context("Workspace capabilities omitted instance identity")
    }

    fn capture_request(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<reqwest::blocking::RequestBuilder> {
        Ok(request.header("X-Margins-Instance-Id", self.capture_instance_id()?))
    }

    fn request_json<T: DeserializeOwned>(
        &self,
        request: reqwest::blocking::RequestBuilder,
    ) -> Result<T> {
        let (status, body) = if let Some(relay_url) = &self.relay_url {
            let request = request.build()?;
            let body = request
                .body()
                .and_then(reqwest::blocking::Body::as_bytes)
                .unwrap_or(&[]);
            if body.len() > 1_500_000 {
                bail!("capture relay payload exceeds 1.5 MB");
            }
            let header = |name: &str| {
                request
                    .headers()
                    .get(name)
                    .and_then(|value| value.to_str().ok())
            };
            let mut payload = serde_json::json!({
                "method": request.method().as_str(),
                "path": format!("{}{}", request.url().path().trim_start_matches('/'),
                    request.url().query().map(|query| format!("?{query}")).unwrap_or_default()),
                "bodyBase64": base64::engine::general_purpose::STANDARD.encode(body),
            });
            for (field, name) in [
                ("contentType", "content-type"),
                ("producerToken", "x-margins-producer-token"),
                ("instanceId", "x-margins-instance-id"),
            ] {
                if let Some(value) = header(name) {
                    payload[field] = serde_json::Value::String(value.to_string());
                }
            }
            let response = self
                .client
                .post(relay_url.clone())
                .bearer_auth(&self.token)
                .json(&payload)
                .send()?;
            let relay_status = response.status();
            let relay: serde_json::Value = response.json()?;
            if !relay_status.is_success() {
                bail!("capture relay rejected request ({relay_status})");
            }
            let status = relay
                .get("status")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u16::try_from(value).ok())
                .and_then(|value| reqwest::StatusCode::from_u16(value).ok())
                .context("capture relay omitted upstream status")?;
            let body = relay
                .get("bodyBase64")
                .and_then(serde_json::Value::as_str)
                .context("capture relay omitted upstream body")?;
            (
                status,
                base64::engine::general_purpose::STANDARD.decode(body)?,
            )
        } else {
            let response = request.bearer_auth(&self.token).send()?;
            (response.status(), response.bytes()?.to_vec())
        };
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
                bail!(
                    "remote transfer still has unacknowledged audio, close, or memo input, or is missing finalization"
                );
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

/// New native remote captures negotiate the same 16 kHz decoded representation
/// used by the local archive and offline ASR. Opus is remote-only; normal local
/// capture never constructs this encoder or transfer spool.
pub const NATIVE_REMOTE_RATE_HZ: u32 = OPUS_PACKET_STREAM_SAMPLE_RATE_HZ_V1;
pub const NATIVE_OPUS_BITRATE_BPS: u32 = 24_000;
/// Recovery PCM and its durable source-length marker are fsynced every 100 ms.
/// Network command framing is deliberately independent: one command carries
/// 500 ms so server-side receipt/fsync work has ample headroom over two-lane
/// real-time ingress on a measured SSH tunnel.
pub const NATIVE_OPUS_CHECKPOINT_PACKETS: usize = 5;
pub const NATIVE_OPUS_CHECKPOINT_FRAMES: usize =
    NATIVE_OPUS_CHECKPOINT_PACKETS * OPUS_PACKET_FRAME_SAMPLES_V1 as usize;
pub const NATIVE_OPUS_NETWORK_PACKETS: usize = 25;
pub const NATIVE_OPUS_NETWORK_FRAMES: usize =
    NATIVE_OPUS_NETWORK_PACKETS * OPUS_PACKET_FRAME_SAMPLES_V1 as usize;
const NATIVE_OPUS_LEGACY_NETWORK_PACKETS: usize = 5;
const NATIVE_RESAMPLE_INPUT_FRAMES: usize = 1_024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum NativeRemoteLane {
    Microphone,
    System,
}

impl NativeRemoteLane {
    pub fn id(self) -> &'static str {
        match self {
            Self::Microphone => "mic",
            Self::System => "system",
        }
    }
}

/// Require the immutable lane graph used by the native remote adapter. Attach
/// never guesses from instance capabilities because a session can outlive a
/// server format migration.
pub fn validate_native_opus_capture_lanes(lanes: &[CaptureLaneV1]) -> Result<()> {
    for required in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
        let lane = lanes
            .iter()
            .find(|lane| lane.lane_id.as_ref() == required.id())
            .with_context(|| {
                format!("remote session does not declare the {} lane", required.id())
            })?;
        if lane.format.codec != AudioCodecV1::Opus
            || lane.format.container != AudioContainerV1::PacketStream
            || lane.format.sample_rate_hz != NATIVE_REMOTE_RATE_HZ
            || lane.format.channel_count != 1
        {
            bail!(
                "remote session lane {} is not native 16 kHz mono Opus packet-stream; retry its existing transfer instead of attaching with this recorder",
                required.id()
            );
        }
    }
    if lanes.len() != 2 {
        bail!("remote session capture graph is incompatible with the native two-lane recorder");
    }
    Ok(())
}

pub fn native_create_session_command(
    session_id: &str,
    idempotency_key: &str,
    title: Option<String>,
    producer: &str,
) -> ClientMessageV1 {
    native_create_session_command_at(session_id, idempotency_key, title, producer, unix_ms())
}

/// As [`native_create_session_command`], with the session clock anchored at
/// `started_at_unix_ms`: a recorder that captured before reserving dates the
/// session from when capture began, so memo edits and audio share one clock.
pub fn native_create_session_command_at(
    session_id: &str,
    idempotency_key: &str,
    title: Option<String>,
    producer: &str,
    started_at_unix_ms: u64,
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
                codec: AudioCodecV1::Opus,
                container: AudioContainerV1::PacketStream,
                sample_rate_hz: NATIVE_REMOTE_RATE_HZ,
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
            started_at_unix_ms: margins_meeting_protocol::UnixMillis(started_at_unix_ms),
            title,
            sources,
            lanes,
            provenance: CaptureProvenanceV1 {
                hops: vec![CaptureProvenanceHopV1 {
                    producer: producer.to_string(),
                    producer_version: option_env!("CARGO_PKG_VERSION").map(str::to_string),
                    mode: CaptureModeV1::Live,
                    observed_at_unix_ms: margins_meeting_protocol::UnixMillis(now),
                    attributes: BTreeMap::from([
                        (
                            "opus.target_bitrate_per_lane_bps".to_string(),
                            NATIVE_OPUS_BITRATE_BPS.to_string(),
                        ),
                        ("opus.frame_ms".to_string(), "20".to_string()),
                    ]),
                }],
            },
        }),
    }
}

struct NativeResampledPcmStream {
    input_rate_hz: u32,
    input_frames: u64,
    emitted_frames: u64,
    input_pending: Vec<f32>,
    output_pending: Vec<f32>,
    pcm_pending: Vec<u8>,
    resampler: Option<SincFixedIn<f32>>,
    delay_to_trim: usize,
}

impl NativeResampledPcmStream {
    fn new(input_rate_hz: u32) -> Result<Self> {
        if input_rate_hz == 0 {
            bail!("native input sample rate must be nonzero");
        }
        let (resampler, delay_to_trim) = if input_rate_hz == NATIVE_REMOTE_RATE_HZ {
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
                f64::from(NATIVE_REMOTE_RATE_HZ) / f64::from(input_rate_hz),
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
            input_pending: Vec::with_capacity(NATIVE_RESAMPLE_INPUT_FRAMES * 2),
            output_pending: Vec::new(),
            pcm_pending: Vec::with_capacity(NATIVE_OPUS_CHECKPOINT_FRAMES * 4),
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
        if self.input_rate_hz == NATIVE_REMOTE_RATE_HZ {
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
        let numerator = u128::from(self.input_frames) * u128::from(NATIVE_REMOTE_RATE_HZ);
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

    fn take_pcm_s16le(&mut self) -> Vec<u8> {
        let bytes = std::mem::take(&mut self.pcm_pending);
        self.emitted_frames = self.emitted_frames.saturating_add((bytes.len() / 2) as u64);
        bytes
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct NativeOpenSegmentV1 {
    schema: String,
    segment_id: String,
    starts_at_ms: u64,
    bitrate_bps: u32,
    /// Absent in the first PacketStream writer, which used five packets per
    /// command. Preserve that exact framing when recovering an old open spool
    /// so already-persisted payload identities remain replayable.
    #[serde(default)]
    network_packets: Option<usize>,
}

struct NativeRemoteSegment {
    id: String,
    starts_at_ms: u64,
    network_packets: usize,
    replay_index: Option<BTreeMap<(NativeRemoteLane, u64), NativeReplayIdentity>>,
    lanes: BTreeMap<NativeRemoteLane, NativeOpusLaneStream>,
}

#[derive(Clone, Default)]
struct NativeReplayIdentity {
    pending: Option<SpoolChunk>,
    ack_digest: Option<String>,
}

struct PendingOpusPacket {
    bytes: Vec<u8>,
    source_frames: u32,
}

struct NativeOpusLaneStream {
    pcm: NativeResampledPcmStream,
    encoder: Encoder,
    pre_skip_input_frames: u32,
    source_frames: u64,
    persisted_source_frames: u64,
    encoded_packet_count: u64,
    next_sequence: u64,
    pcm_frame_pending: Vec<i16>,
    packet_pending: Vec<PendingOpusPacket>,
    recovery: File,
    recovery_path: PathBuf,
    durable_length_path: PathBuf,
    durable_recovery_bytes: u64,
    reserve_bytes: u64,
    network_packets: usize,
}

impl NativeOpusLaneStream {
    fn new(
        input_rate_hz: u32,
        bitrate_bps: u32,
        recovery_path: PathBuf,
        replay: bool,
        reserve_bytes: u64,
        network_packets: usize,
    ) -> Result<Self> {
        if network_packets == 0 || network_packets > OPUS_PACKET_STREAM_MAX_PACKETS_PER_BLOCK_V1 {
            bail!("native Opus network packet aggregation is out of bounds");
        }
        let mut encoder =
            Encoder::builder(NATIVE_REMOTE_RATE_HZ, Channels::Mono, Application::Voip)
                .bitrate(Bitrate::try_bits(bitrate_bps).context("invalid native Opus bitrate")?)
                .complexity(10)
                .signal(Signal::Voice)
                .vbr(true)
                .vbr_constraint(true)
                .dtx(false)
                .build()
                .context("failed to construct native Opus encoder")?;
        // ropus 0.12.18's high-level rustdoc incorrectly calls this 48 kHz
        // units. The implementation returns input-rate samples: 104 at 16 kHz.
        let pre_skip_input_frames = encoder.lookahead();
        if pre_skip_input_frames == 0
            || pre_skip_input_frames
                .checked_mul(48_000 / NATIVE_REMOTE_RATE_HZ)
                .is_none()
        {
            bail!("native Opus encoder returned an invalid lookahead");
        }
        // Exercise the configured encoder before publishing any durable state.
        encoder
            .set_bitrate(Bitrate::Bits(bitrate_bps))
            .context("failed to apply native Opus bitrate")?;
        let durable_length_path = recovery_path.with_extension("durable");
        if let Some(parent) = recovery_path.parent() {
            std::fs::create_dir_all(parent)?;
            set_directory_owner_only(parent)?;
        }
        let recovery = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&recovery_path)?;
        set_owner_only(&recovery_path)?;
        if !replay {
            recovery.set_len(0)?;
            atomic_bytes(&durable_length_path, &0u64.to_le_bytes())?;
        }
        let durable_recovery_bytes = if replay {
            recovery.metadata()?.len()
        } else {
            0
        };
        Ok(Self {
            pcm: NativeResampledPcmStream::new(input_rate_hz)?,
            encoder,
            pre_skip_input_frames,
            source_frames: 0,
            persisted_source_frames: 0,
            encoded_packet_count: 0,
            next_sequence: 0,
            pcm_frame_pending: Vec::with_capacity(OPUS_PACKET_FRAME_SAMPLES_V1 as usize * 2),
            packet_pending: Vec::with_capacity(network_packets + 2),
            recovery,
            recovery_path,
            durable_length_path,
            durable_recovery_bytes,
            reserve_bytes,
            network_packets,
        })
    }

    fn ensure_rate(&self, input_rate_hz: u32) -> Result<()> {
        self.pcm.ensure_rate(input_rate_hz)
    }

    fn append_f32(&mut self, samples: &[f32]) -> Result<(usize, Vec<OpusPacketBlockV1>)> {
        let written = self.pcm.append_f32(samples)?;
        let bytes = self.pcm.take_pcm_s16le();
        self.consume_resampled_pcm(&bytes, false)
            .map(|blocks| (written, blocks))
    }

    fn append_s16le(&mut self, bytes: &[u8]) -> Result<(usize, Vec<OpusPacketBlockV1>)> {
        let written = self.pcm.append_s16le(bytes)?;
        let bytes = self.pcm.take_pcm_s16le();
        self.consume_resampled_pcm(&bytes, false)
            .map(|blocks| (written, blocks))
    }

    fn replay_s16le(&mut self, bytes: &[u8]) -> Result<Vec<OpusPacketBlockV1>> {
        if bytes.len() % 2 != 0 {
            bail!("native Opus recovery PCM has a partial sample");
        }
        self.consume_pcm_without_journal(bytes, false)
    }

    fn consume_resampled_pcm(
        &mut self,
        bytes: &[u8],
        finishing: bool,
    ) -> Result<Vec<OpusPacketBlockV1>> {
        self.consume_resampled_pcm_observing(bytes, finishing, |_| Ok(()))
    }

    fn consume_resampled_pcm_observing(
        &mut self,
        bytes: &[u8],
        finishing: bool,
        mut checkpointed: impl FnMut(u64) -> Result<()>,
    ) -> Result<Vec<OpusPacketBlockV1>> {
        const CHECKPOINT_OVERHEAD_BYTES: u64 = 8 * 1024;
        const CHECKPOINT_PCM_BYTES: usize = (NATIVE_REMOTE_RATE_HZ as usize / 10) * 2;
        let mut blocks = Vec::new();

        // An adapter caller is allowed to hand us a large callback or fixture.
        // Split internally so that crash durability never depends on that caller's
        // chunk size. Small live callbacks still accumulate and fsync only when
        // their combined journal growth reaches 100 ms.
        for (index, slice) in bytes.chunks(CHECKPOINT_PCM_BYTES).enumerate() {
            let required = (slice.len() as u64).saturating_add(CHECKPOINT_OVERHEAD_BYTES);
            if fs4::available_space(&self.recovery_path)?.saturating_sub(required)
                < self.reserve_bytes
            {
                bail!("native recovery journal would breach the spool disk reserve");
            }
            self.recovery.write_all(slice)?;
            let last = (index + 1) * CHECKPOINT_PCM_BYTES >= bytes.len();
            let produced = self.consume_pcm_without_journal(slice, finishing && last)?;
            let journal_bytes = self.recovery.metadata()?.len();
            let checkpoint_due = journal_bytes.saturating_sub(self.durable_recovery_bytes)
                >= CHECKPOINT_PCM_BYTES as u64;
            if checkpoint_due || !produced.is_empty() || (finishing && last) {
                self.checkpoint_recovery()?;
                checkpointed(self.durable_recovery_bytes)?;
            }
            blocks.extend(produced);
        }

        if bytes.is_empty() && finishing {
            blocks.extend(self.consume_pcm_without_journal(&[], true)?);
            self.checkpoint_recovery()?;
            checkpointed(self.durable_recovery_bytes)?;
        }
        Ok(blocks)
    }

    fn consume_pcm_without_journal(
        &mut self,
        bytes: &[u8],
        finishing: bool,
    ) -> Result<Vec<OpusPacketBlockV1>> {
        self.pcm_frame_pending.extend(
            bytes
                .chunks_exact(2)
                .map(|sample| i16::from_le_bytes([sample[0], sample[1]])),
        );
        self.source_frames = self.source_frames.saturating_add((bytes.len() / 2) as u64);
        while self.pcm_frame_pending.len() >= OPUS_PACKET_FRAME_SAMPLES_V1 as usize {
            let frame = self
                .pcm_frame_pending
                .drain(..OPUS_PACKET_FRAME_SAMPLES_V1 as usize)
                .collect::<Vec<_>>();
            self.encode_packet(&frame, OPUS_PACKET_FRAME_SAMPLES_V1 as u32)?;
        }
        if finishing && !self.pcm_frame_pending.is_empty() {
            let source_frames = self.pcm_frame_pending.len() as u32;
            self.pcm_frame_pending
                .resize(OPUS_PACKET_FRAME_SAMPLES_V1 as usize, 0);
            let frame = std::mem::take(&mut self.pcm_frame_pending);
            self.encode_packet(&frame, source_frames)?;
        }
        let mut blocks = self.take_complete_blocks(false)?;
        if finishing && self.source_frames != 0 {
            let required_capacity = self
                .source_frames
                .checked_add(u64::from(self.pre_skip_input_frames))
                .context("native Opus source length overflowed")?;
            while self
                .encoded_packet_count
                .saturating_mul(u64::from(OPUS_PACKET_FRAME_SAMPLES_V1))
                < required_capacity
            {
                self.encode_packet(&[0; OPUS_PACKET_FRAME_SAMPLES_V1 as usize], 0)?;
            }
            blocks.extend(self.take_complete_blocks(true)?);
        }
        Ok(blocks)
    }

    fn finish(&mut self) -> Result<Vec<OpusPacketBlockV1>> {
        self.pcm.finish()?;
        let bytes = self.pcm.take_pcm_s16le();
        self.consume_resampled_pcm(&bytes, true)
    }

    fn encode_packet(&mut self, frame: &[i16], source_frames: u32) -> Result<()> {
        let mut packet = [0u8; OPUS_PACKET_MAX_BYTES_V1];
        let encoded = self
            .encoder
            .encode(frame, &mut packet)
            .context("native Opus encode failed")?;
        self.packet_pending.push(PendingOpusPacket {
            bytes: packet[..encoded].to_vec(),
            source_frames,
        });
        self.encoded_packet_count = self.encoded_packet_count.saturating_add(1);
        Ok(())
    }

    fn take_complete_blocks(&mut self, finishing: bool) -> Result<Vec<OpusPacketBlockV1>> {
        let mut blocks = Vec::new();
        if finishing {
            if !self.packet_pending.is_empty() {
                let count = self.packet_pending.len();
                blocks.push(self.take_block(count, true)?);
            }
            return Ok(blocks);
        }
        while self.packet_pending.len() > self.network_packets {
            blocks.push(self.take_block(self.network_packets, false)?);
        }
        Ok(blocks)
    }

    fn take_block(&mut self, count: usize, stream_end: bool) -> Result<OpusPacketBlockV1> {
        let packets = self.packet_pending.drain(..count).collect::<Vec<_>>();
        let source_frame_count = packets.iter().try_fold(0u32, |total, packet| {
            total
                .checked_add(packet.source_frames)
                .context("native Opus block source length overflowed")
        })?;
        if source_frame_count == 0 {
            bail!("native Opus terminal block has no source frames");
        }
        let stream_start = self.next_sequence == 0;
        let pre_skip_48k = if stream_start {
            u16::try_from(self.pre_skip_input_frames.saturating_mul(3))
                .context("native Opus pre-skip exceeds packet framing")?
        } else {
            0
        };
        let block = OpusPacketBlockV1 {
            stream_start,
            stream_end,
            pre_skip_48k,
            sample_rate_hz: NATIVE_REMOTE_RATE_HZ,
            source_start_frame: self.persisted_source_frames,
            source_frame_count,
            frame_samples: OPUS_PACKET_FRAME_SAMPLES_V1,
            packets: packets.into_iter().map(|packet| packet.bytes).collect(),
        };
        self.persisted_source_frames = self
            .persisted_source_frames
            .saturating_add(u64::from(source_frame_count));
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok(block)
    }

    fn checkpoint_recovery(&mut self) -> Result<()> {
        self.recovery.sync_all()?;
        self.durable_recovery_bytes = self.recovery.metadata()?.len();
        atomic_bytes(
            &self.durable_length_path,
            &self.durable_recovery_bytes.to_le_bytes(),
        )
    }

    fn remove_recovery(self) -> Result<()> {
        drop(self.recovery);
        for path in [self.recovery_path, self.durable_length_path] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }
}

/// Remote-only native capture adapter. Resampling, recovery journaling, Opus
/// encoding, hashing, and spool I/O all run on the bounded worker, never the
/// device callback. The default local recorder path does not construct it.
pub struct NativeRemoteTransfer {
    spool: DurableTransferSpool,
    segment: Option<NativeRemoteSegment>,
    closes: Vec<SegmentCloseReferenceV1>,
    bitrate_bps: u32,
}

impl NativeRemoteTransfer {
    pub fn new(spool: DurableTransferSpool) -> Self {
        Self::new_with_bitrate(spool, NATIVE_OPUS_BITRATE_BPS)
    }

    pub fn new_with_bitrate(spool: DurableTransferSpool, bitrate_bps: u32) -> Self {
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
            bitrate_bps,
        }
    }

    pub fn begin_segment(&mut self, segment_id: String, starts_at_ms: u64) -> Result<()> {
        validate_component(&segment_id)?;
        if self.segment.is_some() || self.open_segment_path().is_file() {
            bail!("the previous native segment must be recovered or closed before resume");
        }
        let open = NativeOpenSegmentV1 {
            schema: "margins.native-opus-segment.v1".to_string(),
            segment_id: segment_id.clone(),
            starts_at_ms,
            bitrate_bps: self.bitrate_bps,
            network_packets: Some(NATIVE_OPUS_NETWORK_PACKETS),
        };
        atomic_json(&self.open_segment_path(), &open)?;
        self.segment = Some(NativeRemoteSegment {
            id: segment_id,
            starts_at_ms,
            network_packets: NATIVE_OPUS_NETWORK_PACKETS,
            replay_index: None,
            lanes: BTreeMap::new(),
        });
        Ok(())
    }

    /// Re-encode an interrupted segment from its fsynced 16 kHz recovery PCM,
    /// validating every already-published block digest, then close it before a
    /// new capture generation begins. At most the worker's sub-100 ms
    /// uncheckpointed tail is absent after a process/power loss.
    pub fn recover_interrupted_segment(
        &mut self,
        reason: SegmentCloseReasonV1,
    ) -> Result<Option<ClientMessageV1>> {
        if self.segment.is_some() {
            bail!("cannot recover while a native segment is active");
        }
        let path = self.open_segment_path();
        if !path.is_file() {
            return Ok(None);
        }
        let open: NativeOpenSegmentV1 = serde_json::from_slice(&std::fs::read(&path)?)?;
        if open.schema != "margins.native-opus-segment.v1" || open.bitrate_bps != self.bitrate_bps {
            bail!("native recovery segment codec identity changed");
        }
        validate_component(&open.segment_id)?;
        let network_packets = open
            .network_packets
            .unwrap_or(NATIVE_OPUS_LEGACY_NETWORK_PACKETS);
        if network_packets == 0 || network_packets > OPUS_PACKET_STREAM_MAX_PACKETS_PER_BLOCK_V1 {
            bail!("native recovery segment has invalid network packet aggregation");
        }
        if let Some(close) = self
            .spool
            .manifest
            .close_commands
            .iter()
            .find(|command| {
                matches!(&command.body, ClientMessageBodyV1::CloseSegment(close)
                    if close.segment_id.as_ref() == open.segment_id)
            })
            .cloned()
        {
            // The close manifest is committed before cleanup begins. A crash
            // while removing per-lane PCM journals must therefore finish that
            // cleanup, not try to synthesize a different one-lane close from
            // whichever journal happened to survive.
            self.validate_durable_closed_segment(&open.segment_id, &close)?;
            self.clear_open_segment_files(&open.segment_id)?;
            return Ok(Some(close));
        }
        let replay_index = self.build_native_replay_index(&open.segment_id)?;
        self.segment = Some(NativeRemoteSegment {
            id: open.segment_id.clone(),
            starts_at_ms: open.starts_at_ms,
            network_packets,
            replay_index: Some(replay_index),
            lanes: BTreeMap::new(),
        });
        let mut recovered_any = false;
        for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
            let recovery_path = self.lane_recovery_path(&open.segment_id, lane)?;
            let durable_path = recovery_path.with_extension("durable");
            if !recovery_path.is_file() || !durable_path.is_file() {
                continue;
            }
            let durable = std::fs::read(&durable_path)?;
            if durable.len() != 8 {
                bail!("native recovery checkpoint length is corrupt");
            }
            let length = u64::from_le_bytes(durable.try_into().unwrap());
            if length == 0 {
                continue;
            }
            let mut bytes = std::fs::read(&recovery_path)?;
            if length > bytes.len() as u64 || length % 2 != 0 {
                bail!("native recovery checkpoint exceeds its PCM journal");
            }
            bytes.truncate(length as usize);
            std::fs::OpenOptions::new()
                .write(true)
                .open(&recovery_path)?
                .set_len(length)?;
            let mut stream = NativeOpusLaneStream::new(
                NATIVE_REMOTE_RATE_HZ,
                self.bitrate_bps,
                recovery_path,
                true,
                self.spool.reserve_bytes,
                network_packets,
            )?;
            let blocks = stream.replay_s16le(&bytes)?;
            self.segment
                .as_mut()
                .expect("recovery segment was installed")
                .lanes
                .insert(lane, stream);
            self.persist_blocks(lane, blocks)?;
            recovered_any = true;
        }
        if !recovered_any {
            self.segment = None;
            self.clear_open_segment_files(&open.segment_id)?;
            return Ok(None);
        }
        self.close_segment(reason).map(Some)
    }

    fn validate_durable_closed_segment(
        &self,
        segment_id: &str,
        command: &ClientMessageV1,
    ) -> Result<()> {
        let ClientMessageBodyV1::CloseSegment(close) = &command.body else {
            bail!("native recovery close intent has the wrong operation");
        };
        if close.segment_id.as_ref() != segment_id {
            bail!("native recovery close intent names a different segment");
        }
        let replay_index = self.build_native_replay_index(segment_id)?;
        let mut lane_boundaries = BTreeMap::new();
        for boundary in &close.lane_boundaries {
            if lane_boundaries
                .insert(boundary.lane_id.as_ref(), boundary.next_sequence)
                .is_some()
            {
                bail!("native recovery close repeats a lane boundary");
            }
        }
        for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
            let next_sequence = lane_boundaries
                .get(lane.id())
                .copied()
                .context("native recovery close is missing a declared lane")?;
            if replay_index.keys().any(|(candidate_lane, sequence)| {
                *candidate_lane == lane && *sequence >= next_sequence
            }) {
                bail!("native recovery has audio beyond its durable close boundary");
            }
            let mut all_payloads_local = true;
            let mut local_stream = Vec::new();
            for sequence in 0..next_sequence {
                let identity = replay_index
                    .get(&(lane, sequence))
                    .cloned()
                    .unwrap_or_default();
                let chunk =
                    identity
                        .pending
                        .as_ref()
                        .and_then(|pending| match &pending.command.body {
                            ClientMessageBodyV1::AudioChunk(chunk) => Some(chunk),
                            _ => None,
                        });
                let ack = identity.ack_digest;
                let local_chunk = match (chunk, ack) {
                    (Some(chunk), Some(ack)) => {
                        // ACK is fsynced before the frame is unlinked. A crash
                        // between those operations leaves both, and is valid
                        // only when both name the identical immutable payload.
                        if ack != chunk.payload_digest.hex {
                            bail!("native recovery pending frame conflicts with its durable ACK");
                        }
                        Some(chunk)
                    }
                    (None, None) => bail!("native recovery close has a missing durable sequence"),
                    (Some(chunk), None) => Some(chunk),
                    (None, Some(_)) => {
                        all_payloads_local = false;
                        None
                    }
                };
                if let Some(chunk) = local_chunk {
                    let blocks =
                        decode_opus_packet_blocks_v1(&chunk.payload).map_err(anyhow::Error::msg)?;
                    if blocks.len() != 1 {
                        bail!("native recovery command contains multiple Opus blocks");
                    }
                    let block = &blocks[0];
                    if block.stream_start != (sequence == 0)
                        || block.stream_end != (sequence + 1 == next_sequence)
                    {
                        bail!("native recovery Opus stream markers conflict with its close");
                    }
                    local_stream.extend_from_slice(&chunk.payload);
                }
            }
            if next_sequence > 0 && all_payloads_local {
                validate_opus_packet_stream_v1(&local_stream).map_err(anyhow::Error::msg)?;
            }
        }
        Ok(())
    }

    /// Build the only replay identity view with one bounded directory pass.
    /// Normal monotonic capture never scans prior frames or ACKs; a crash/open
    /// recovery pays this cost once, then performs O(log n) sequence lookups.
    fn build_native_replay_index(
        &self,
        segment_id: &str,
    ) -> Result<BTreeMap<(NativeRemoteLane, u64), NativeReplayIdentity>> {
        let mut index = BTreeMap::<(NativeRemoteLane, u64), NativeReplayIdentity>::new();
        for pending in self.spool.pending_chunks()? {
            let ClientMessageBodyV1::AudioChunk(chunk) = &pending.command.body else {
                continue;
            };
            if chunk.segment_id.as_ref() != segment_id {
                continue;
            }
            let lane = match chunk.lane_id.as_ref() {
                "mic" => NativeRemoteLane::Microphone,
                "system" => NativeRemoteLane::System,
                _ => bail!("native recovery frame names an unknown lane"),
            };
            let identity = index.entry((lane, chunk.sequence)).or_default();
            if identity.pending.replace(pending).is_some() {
                bail!("native recovery sequence has multiple pending identities");
            }
        }
        for entry in std::fs::read_dir(self.spool.root.join("acks"))? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some((lane, remainder)) = [
                (NativeRemoteLane::Microphone, "mic"),
                (NativeRemoteLane::System, "system"),
            ]
            .into_iter()
            .find_map(|(lane, lane_id)| {
                let prefix = format!("{}-{lane_id}-", safe_name(segment_id));
                name.strip_prefix(&prefix)
                    .map(|rest| (lane, rest.to_string()))
            }) else {
                continue;
            };
            if !remainder.ends_with(".ack") || remainder.len() < 22 {
                bail!("native recovery ACK filename is corrupt");
            }
            let (sequence, message_suffix) = remainder
                .split_once('-')
                .context("native recovery ACK filename has no message identity")?;
            if sequence.len() != 20
                || !sequence.bytes().all(|byte| byte.is_ascii_digit())
                || message_suffix == ".ack"
            {
                bail!("native recovery ACK filename is corrupt");
            }
            let sequence = sequence.parse::<u64>()?;
            let value = std::fs::read_to_string(entry.path())?;
            if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                bail!("native recovery ACK digest is corrupt");
            }
            let identity = index.entry((lane, sequence)).or_default();
            if identity.ack_digest.replace(value).is_some() {
                bail!("native recovery sequence has multiple durable ACK identities");
            }
        }
        Ok(index)
    }

    pub fn append_f32(
        &mut self,
        lane: NativeRemoteLane,
        input_rate_hz: u32,
        samples: &[f32],
    ) -> Result<usize> {
        self.ensure_lane(lane, input_rate_hz)?;
        let (written, blocks) = self
            .segment
            .as_mut()
            .unwrap()
            .lanes
            .get_mut(&lane)
            .unwrap()
            .append_f32(samples)?;
        self.persist_blocks(lane, blocks)?;
        Ok(written)
    }

    pub fn append_s16le(
        &mut self,
        lane: NativeRemoteLane,
        input_rate_hz: u32,
        bytes: &[u8],
    ) -> Result<usize> {
        self.ensure_lane(lane, input_rate_hz)?;
        let (written, blocks) = self
            .segment
            .as_mut()
            .unwrap()
            .lanes
            .get_mut(&lane)
            .unwrap()
            .append_s16le(bytes)?;
        self.persist_blocks(lane, blocks)?;
        Ok(written)
    }

    fn ensure_lane(&mut self, lane: NativeRemoteLane, input_rate_hz: u32) -> Result<()> {
        let (segment_id, path, network_packets) = {
            let segment = self
                .segment
                .as_ref()
                .context("native audio arrived without an open segment")?;
            (
                segment.id.clone(),
                self.lane_recovery_path(&segment.id, lane)?,
                segment.network_packets,
            )
        };
        let segment = self.segment.as_mut().unwrap();
        if !segment.lanes.contains_key(&lane) {
            segment.lanes.insert(
                lane,
                NativeOpusLaneStream::new(
                    input_rate_hz,
                    self.bitrate_bps,
                    path,
                    false,
                    self.spool.reserve_bytes,
                    network_packets,
                )?,
            );
        }
        segment
            .lanes
            .get(&lane)
            .expect("lane was inserted")
            .ensure_rate(input_rate_hz)
            .with_context(|| format!("native segment {segment_id} lane rate changed"))
    }

    fn persist_blocks(&self, lane: NativeRemoteLane, blocks: Vec<OpusPacketBlockV1>) -> Result<()> {
        let segment = self
            .segment
            .as_ref()
            .context("native Opus block has no segment")?;
        let stream = segment
            .lanes
            .get(&lane)
            .context("native Opus block has no lane")?;
        let first_sequence = stream.next_sequence.saturating_sub(blocks.len() as u64);
        for (offset, block) in blocks.into_iter().enumerate() {
            let sequence = first_sequence + offset as u64;
            let frame_start = block.source_start_frame;
            let frame_end = frame_start.saturating_add(u64::from(block.source_frame_count));
            let starts_at_ms = segment
                .starts_at_ms
                .saturating_add(frames_to_ms_ceil(frame_start));
            let ends_at_ms = segment
                .starts_at_ms
                .saturating_add(frames_to_ms_ceil(frame_end));
            let payload = block.encode().map_err(anyhow::Error::msg)?;
            let command = ClientMessageV1 {
                protocol_version: Default::default(),
                message_id: next_message_id("opus").into(),
                session_id: self.spool.manifest.session_id.clone().into(),
                sent_at_unix_ms: margins_meeting_protocol::UnixMillis(unix_ms()),
                body: ClientMessageBodyV1::AudioChunk(AudioChunkV1 {
                    segment_id: segment.id.clone().into(),
                    lane_id: lane.id().into(),
                    sequence,
                    starts_at_ms: SessionMillis(starts_at_ms),
                    duration_ms: DurationMillis(ends_at_ms.saturating_sub(starts_at_ms).max(1)),
                    payload_digest: ContentDigestV1 {
                        algorithm: DigestAlgorithmV1::Sha256,
                        hex: format!("{:x}", Sha256::digest(&payload)),
                    },
                    payload,
                }),
            };
            if let Some(replay_index) = &segment.replay_index {
                let identity = replay_index
                    .get(&(lane, sequence))
                    .cloned()
                    .unwrap_or_default();
                self.append_or_validate_replay(&command, identity)?;
            } else {
                // The capture lease and lane stream own a strictly monotonic
                // sequence. Do not rescan every historical frame and ACK on
                // this hot path; that made sustained capture quadratic.
                self.spool.append_chunk(&command)?;
            }
        }
        Ok(())
    }

    fn append_or_validate_replay(
        &self,
        command: &ClientMessageV1,
        identity: NativeReplayIdentity,
    ) -> Result<()> {
        let ClientMessageBodyV1::AudioChunk(expected) = &command.body else {
            unreachable!();
        };
        if let Some(existing) = identity.pending {
            let ClientMessageBodyV1::AudioChunk(actual) = existing.command.body else {
                unreachable!();
            };
            if actual != *expected {
                bail!("re-encoded native recovery block conflicts with its durable frame");
            }
            return Ok(());
        }
        if let Some(digest) = identity.ack_digest {
            if digest != expected.payload_digest.hex {
                bail!("re-encoded native recovery block conflicts with its durable ACK");
            }
            return Ok(());
        }
        self.spool.append_chunk(command)?;
        Ok(())
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
            let blocks = self
                .segment
                .as_mut()
                .unwrap()
                .lanes
                .get_mut(&lane)
                .unwrap()
                .finish()?;
            self.persist_blocks(lane, blocks)?;
        }
        let segment = self.segment.take().expect("checked above");
        let ended_at_ms = segment.starts_at_ms.saturating_add(frames_to_ms_ceil(
            segment
                .lanes
                .values()
                .map(|lane| lane.source_frames)
                .max()
                .unwrap_or(0),
        ));
        let message_id = next_message_id("close");
        let command = ClientMessageV1 {
            protocol_version: Default::default(),
            message_id: message_id.clone().into(),
            session_id: self.spool.manifest.session_id.clone().into(),
            sent_at_unix_ms: margins_meeting_protocol::UnixMillis(unix_ms()),
            body: ClientMessageBodyV1::CloseSegment(CloseSegmentV1 {
                segment_id: segment.id.clone().into(),
                ended_at_ms: SessionMillis(ended_at_ms),
                lane_boundaries: [NativeRemoteLane::Microphone, NativeRemoteLane::System]
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
        if let Some(existing) = self.spool.manifest.close_commands.iter().find(|existing| {
            matches!(&existing.body, ClientMessageBodyV1::CloseSegment(close)
                if close.segment_id.as_ref() == segment.id)
        }) {
            let ClientMessageBodyV1::CloseSegment(actual) = &existing.body else {
                unreachable!();
            };
            let ClientMessageBodyV1::CloseSegment(expected) = &command.body else {
                unreachable!();
            };
            if actual.ended_at_ms != expected.ended_at_ms
                || actual.lane_boundaries != expected.lane_boundaries
            {
                bail!("recovered native close conflicts with its durable intent");
            }
        } else {
            self.spool.set_close(command.clone())?;
            self.closes.push(SegmentCloseReferenceV1 {
                segment_id: segment.id.clone().into(),
                close_message_id: message_id.into(),
            });
        }
        for (_, lane) in segment.lanes {
            lane.remove_recovery()?;
        }
        self.clear_open_segment_files(&segment.id)?;
        Ok(command)
    }

    pub fn last_closed_ended_at_ms(&self) -> Option<u64> {
        self.spool
            .manifest
            .close_commands
            .iter()
            .filter_map(|command| match &command.body {
                ClientMessageBodyV1::CloseSegment(close) => Some(close.ended_at_ms.0),
                _ => None,
            })
            .max()
    }

    pub fn seal_session(
        &mut self,
        ended_at_ms: u64,
        reason: SessionFinalizeReasonV1,
    ) -> Result<ClientMessageV1> {
        if self.segment.is_some() || self.open_segment_path().is_file() {
            bail!("native segment must be closed before session finalization");
        }
        match self.last_closed_ended_at_ms() {
            Some(last_closed) if ended_at_ms != last_closed => {
                bail!("native finalization must use the last closed media boundary")
            }
            None if reason != SessionFinalizeReasonV1::Error => {
                bail!("native session has no closed media boundary")
            }
            _ => {}
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

    fn open_segment_path(&self) -> PathBuf {
        self.spool.root.join("open-native-segment.json")
    }

    fn lane_recovery_path(&self, segment: &str, lane: NativeRemoteLane) -> Result<PathBuf> {
        validate_component(segment)?;
        Ok(self
            .spool
            .root
            .join("recovery")
            .join(format!("{segment}-{}.s16le", lane.id())))
    }

    fn clear_open_segment_files(&self, segment: &str) -> Result<()> {
        match std::fs::remove_file(self.open_segment_path()) {
            Ok(()) => sync_dir(&self.spool.root),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }?;
        for lane in [NativeRemoteLane::Microphone, NativeRemoteLane::System] {
            let path = self.lane_recovery_path(segment, lane)?;
            for path in [path.clone(), path.with_extension("durable")] {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(())
    }
}

fn frames_to_ms_ceil(frames: u64) -> u64 {
    frames
        .saturating_mul(1_000)
        .div_ceil(u64::from(NATIVE_REMOTE_RATE_HZ))
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
    if sample_rate_hz == NATIVE_REMOTE_RATE_HZ {
        return Ok(samples);
    }
    let mut stream = NativeResampledPcmStream::new(sample_rate_hz)?;
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

/// Decode one complete Margins Opus packet stream to exactly the declared
/// 16 kHz source-frame count. Encoded padding and lookahead never reach ASR.
pub fn remote_opus_packet_stream_for_asr(bytes: &[u8]) -> Result<Vec<f32>> {
    let summary = validate_opus_packet_stream_v1(bytes).map_err(anyhow::Error::msg)?;
    let blocks = decode_opus_packet_blocks_v1(bytes).map_err(anyhow::Error::msg)?;
    let mut decoder = Decoder::new(NATIVE_REMOTE_RATE_HZ, Channels::Mono)
        .context("failed to construct native Opus decoder")?;
    let mut decoded = vec![0i16; OPUS_PACKET_FRAME_SAMPLES_V1 as usize];
    let mut output = Vec::with_capacity(summary.source_frame_count as usize);
    let mut pre_skip = usize::from(summary.pre_skip_48k / 3);
    for block in blocks {
        for packet in block.packets {
            let frames = decoder
                .decode(&packet, &mut decoded, DecodeMode::Normal)
                .context("native Opus packet decode failed")?;
            if frames != OPUS_PACKET_FRAME_SAMPLES_V1 as usize {
                bail!(
                    "native Opus packet decoded {frames} frames; expected {} (20 ms)",
                    OPUS_PACKET_FRAME_SAMPLES_V1
                );
            }
            let start = pre_skip.min(frames);
            pre_skip -= start;
            output.extend(
                decoded[start..frames]
                    .iter()
                    .map(|sample| f32::from(*sample) / 32_768.0),
            );
        }
    }
    if pre_skip != 0 || output.len() < summary.source_frame_count as usize {
        bail!("native Opus stream ended before its declared source timeline");
    }
    output.truncate(summary.source_frame_count as usize);
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
    deliver_chunks_unchecked(spool, client, true)?;
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
    let force_partial = !spool.pending_closes().is_empty();
    deliver_chunks_unchecked(spool, client, force_partial)?;
    deliver_closes_unchecked(spool, client)
}

pub fn deliver_chunks(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
) -> Result<()> {
    validate_transfer_instance(spool, client)?;
    deliver_chunks_unchecked(spool, client, true)
}

fn deliver_chunks_unchecked(
    spool: &mut DurableTransferSpool,
    client: &WorkspaceHttpClient,
    force_partial: bool,
) -> Result<()> {
    let producer_token = spool.producer_token()?;
    let chunks = spool.pending_chunks()?;
    let batch_limit = client.max_batch_commands.load(Ordering::Acquire).max(1) as usize;
    // The recoverable PCM/source marker remains durable at 100 ms while each
    // immutable network command carries 500 ms. This extra wait coalesces
    // commands from both lanes without widening the crash-recovery window.
    const MAX_CAPTURE_BATCH_AGE: std::time::Duration = std::time::Duration::from_millis(500);
    if !force_partial && chunks.len() < batch_limit {
        let old_enough = chunks.first().is_some_and(|chunk| {
            chunk
                .path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age >= MAX_CAPTURE_BATCH_AGE)
        });
        if !old_enough {
            return Ok(());
        }
    }
    for batch in chunks.chunks(batch_limit) {
        let commands = batch
            .iter()
            .map(|chunk| chunk.command.clone())
            .collect::<Vec<_>>();
        client.upload_chunks(&producer_token, &commands)?;
        for chunk in batch {
            spool.acknowledge(chunk)?;
        }
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
    if client.capture_instance_id()? != spool.manifest.instance_id {
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

#[cfg(test)]
mod native_opus_durability_tests {
    use super::*;

    #[test]
    fn legacy_five_packet_open_spool_reencodes_existing_payload_identity() {
        let temp = tempfile::tempdir().unwrap();
        let spool = DurableTransferSpool::create(
            temp.path(),
            "legacy-open",
            "instance-a",
            "https://example.test",
            "workspace-a",
            "session-a",
            "producer-secret",
            0,
        )
        .unwrap();
        let mut transfer = NativeRemoteTransfer::new(spool);
        transfer.begin_segment("legacy-segment".into(), 0).unwrap();
        transfer.segment.as_mut().unwrap().network_packets = NATIVE_OPUS_LEGACY_NETWORK_PACKETS;
        let open_path = transfer.open_segment_path();
        let mut open: NativeOpenSegmentV1 =
            serde_json::from_slice(&std::fs::read(&open_path).unwrap()).unwrap();
        open.network_packets = None;
        atomic_json(&open_path, &open).unwrap();
        transfer
            .append_s16le(NativeRemoteLane::Microphone, 16_000, &vec![3; 6_400])
            .unwrap();
        let before = transfer.spool().pending_chunks().unwrap();
        assert_eq!(before.len(), 1);
        let first_digest = before[0].payload_digest.clone();
        drop(transfer);

        let spool = DurableTransferSpool::open(temp.path(), "legacy-open", 0).unwrap();
        let mut recovered = NativeRemoteTransfer::new(spool);
        recovered
            .recover_interrupted_segment(SegmentCloseReasonV1::Error)
            .unwrap()
            .expect("legacy open stream should close its durable prefix");
        let mut after = recovered.spool().pending_chunks().unwrap();
        after.sort_by(|left, right| left.path.cmp(&right.path));
        assert_eq!(after.len(), 2);
        assert_eq!(after[0].payload_digest, first_digest);
    }

    #[test]
    fn one_large_append_advances_recovery_checkpoint_at_most_every_100_ms() {
        let temp = tempfile::tempdir().unwrap();
        let recovery = temp.path().join("large-append.s16le");
        let mut lane = NativeOpusLaneStream::new(
            NATIVE_REMOTE_RATE_HZ,
            NATIVE_OPUS_BITRATE_BPS,
            recovery.clone(),
            false,
            0,
            NATIVE_OPUS_NETWORK_PACKETS,
        )
        .unwrap();
        let bytes = vec![7u8; (NATIVE_REMOTE_RATE_HZ as usize * 2 * 350) / 1_000];
        let mut checkpoints = Vec::new();
        let error = lane
            .consume_resampled_pcm_observing(&bytes, false, |durable_bytes| {
                checkpoints.push(durable_bytes);
                if checkpoints.len() == 2 {
                    return Err(anyhow::anyhow!("injected process loss after checkpoint"));
                }
                Ok(())
            })
            .unwrap_err();
        assert!(error.to_string().contains("injected process loss"));
        assert_eq!(checkpoints, vec![3_200, 6_400]);

        let durable = std::fs::read(recovery.with_extension("durable")).unwrap();
        assert_eq!(u64::from_le_bytes(durable.try_into().unwrap()), 6_400);
        assert_eq!(std::fs::metadata(recovery).unwrap().len(), 6_400);
    }
}
