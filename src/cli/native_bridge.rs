//! Paired, loopback-only controller for the native remote recorder.
//!
//! The authority and Workspace are fixed when this process starts. Browser
//! requests can operate the recorder but cannot redirect its audio.

use super::*;
use serde_json::{json, Value};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::time::Duration;

const DEFAULT_PORT: u16 = 18765;
const MAX_BODY: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureAction {
    Pause,
    Resume,
    Stop,
}

#[derive(Default)]
struct Counters {
    mic: u64,
    system: u64,
    mic_dropped: u64,
    system_dropped: u64,
    frames: u64,
    silent: u64,
}

#[derive(Default)]
struct LiveCounters {
    mic: Option<Arc<AtomicU64>>,
    system: Option<Arc<AtomicU64>>,
    mic_dropped: Option<Arc<AtomicU64>>,
    system_dropped: Option<Arc<AtomicU64>>,
    frames: Option<Arc<AtomicU64>>,
    silent: Option<Arc<AtomicU64>>,
}

#[derive(Default)]
struct CaptureStatus {
    state: &'static str,
    session_id: Option<String>,
    transfer_id: Option<String>,
    error: Option<String>,
    completed: Counters,
    live: LiveCounters,
}

impl CaptureStatus {
    fn snapshot(&self, instance_id: &str, workspace_id: &str) -> Value {
        let load = |counter: &Option<Arc<AtomicU64>>| {
            counter.as_ref().map_or(0, |v| v.load(Ordering::Relaxed))
        };
        json!({
            "state": if self.state.is_empty() { "ready" } else { self.state },
            "instanceId": instance_id,
            "workspaceId": workspace_id,
            "sessionId": self.session_id,
            "transferId": self.transfer_id,
            "microphoneSamples": self.completed.mic + load(&self.live.mic),
            "systemSamples": self.completed.system + load(&self.live.system),
            "microphoneDroppedSamples": self.completed.mic_dropped + load(&self.live.mic_dropped),
            "systemDroppedSamples": self.completed.system_dropped + load(&self.live.system_dropped),
            "systemFrames": self.completed.frames + load(&self.live.frames),
            "systemSilentSamples": self.completed.silent + load(&self.live.silent),
            "error": self.error,
        })
    }

    fn fold_live(&mut self) {
        let load = |counter: &Option<Arc<AtomicU64>>| {
            counter.as_ref().map_or(0, |v| v.load(Ordering::Relaxed))
        };
        self.completed.mic += load(&self.live.mic);
        self.completed.system += load(&self.live.system);
        self.completed.mic_dropped += load(&self.live.mic_dropped);
        self.completed.system_dropped += load(&self.live.system_dropped);
        self.completed.frames += load(&self.live.frames);
        self.completed.silent += load(&self.live.silent);
        self.live = LiveCounters::default();
    }
}

pub(super) struct CaptureController {
    receiver: mpsc::Receiver<CaptureAction>,
    status: Arc<Mutex<CaptureStatus>>,
}

impl CaptureController {
    pub(super) fn recording(
        &self,
        session: &str,
        transfer: &str,
        sink: &crate::recorder::LiveAudioSink,
        recorder: &crate::recorder::RecorderHandle,
    ) {
        let mut state = self.status.lock().unwrap();
        state.state = "recording";
        state.session_id = Some(session.into());
        state.transfer_id = Some(transfer.into());
        state.live = LiveCounters {
            mic: Some(sink.mic_accepted_samples.clone()),
            system: Some(sink.system_accepted_samples.clone()),
            mic_dropped: Some(sink.mic_dropped_samples.clone()),
            system_dropped: Some(sink.system_dropped_samples.clone()),
            frames: Some(recorder.spk_frames()),
            silent: Some(recorder.spk_silence()),
        };
    }

    pub(super) fn wait_action(&self, stop: &Arc<AtomicBool>) -> Result<crate::tui::TuiAction> {
        loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(crate::tui::TuiAction::Quit);
            }
            match self.receiver.recv_timeout(Duration::from_millis(100)) {
                Ok(CaptureAction::Pause) => {
                    stop.store(true, Ordering::SeqCst);
                    return Ok(crate::tui::TuiAction::Pause);
                }
                Ok(CaptureAction::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    stop.store(true, Ordering::SeqCst);
                    return Ok(crate::tui::TuiAction::Quit);
                }
                Ok(CaptureAction::Resume) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }

    pub(super) fn paused(&self) {
        let mut state = self.status.lock().unwrap();
        state.fold_live();
        state.state = "paused";
    }

    pub(super) fn wait_paused_action(&self) -> Result<crate::tui::TuiAction> {
        loop {
            match self.receiver.recv() {
                Ok(CaptureAction::Resume) => return Ok(crate::tui::TuiAction::Resume),
                Ok(CaptureAction::Stop) | Err(_) => return Ok(crate::tui::TuiAction::Quit),
                Ok(CaptureAction::Pause) => {}
            }
        }
    }

    pub(super) fn saving(&self) {
        let mut state = self.status.lock().unwrap();
        state.fold_live();
        state.state = "saving";
    }
}

struct Bridge {
    port: u16,
    remote: String,
    workspace: String,
    instance: String,
    origin: String,
    pair_code: Option<String>,
    token: Option<String>,
    sender: Option<mpsc::Sender<CaptureAction>>,
    permission_request: mpsc::Sender<mpsc::Sender<Result<(), String>>>,
    status: Arc<Mutex<CaptureStatus>>,
}

pub(super) fn maybe_main(args: &[OsString]) -> Option<i32> {
    if args.get(1).and_then(|v| v.to_str()) != Some("native-bridge") {
        return None;
    }
    Some(match run_bridge(args) {
        Ok(()) => 0,
        Err(error) => super::report_error(&format!("{error:#}")),
    })
}

fn run_bridge(args: &[OsString]) -> Result<()> {
    let mut remote = None;
    let mut workspace = None;
    let mut origin = None;
    let mut port = DEFAULT_PORT;
    let mut iter = args[2..].iter();
    while let Some(flag) = iter.next() {
        let value = iter
            .next()
            .and_then(|v| v.to_str())
            .context("native-bridge option needs a value")?;
        match flag.to_str() {
            Some("--remote") => remote = Some(value.to_string()),
            Some("--workspace") => workspace = Some(value.to_string()),
            Some("--origin") => origin = Some(value.to_string()),
            Some("--port") => port = parse_port(value)?,
            _ => bail!("unknown native-bridge option"),
        }
    }
    let remote = remote.context("native-bridge requires --remote")?;
    let workspace = workspace.context("native-bridge requires --workspace")?;
    let origin = origin.context("native-bridge requires --origin")?;
    if !valid_origin(&origin) {
        bail!("--origin must be one exact HTTPS origin (HTTP localhost is allowed for local development)");
    }
    let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
    let connection = margins_workflows::remote_workspace::RemoteConnection::connect(
        &remote,
        &workspace,
        token.as_deref(),
    )?;
    let instance = connection.capabilities.instance_id.as_ref().to_string();
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
        .with_context(|| format!("native bridge port {port} is already in use"))?;
    let pair_code = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    eprintln!("Margins native bridge on http://127.0.0.1:{port} for {instance} / {workspace}");
    eprintln!("Pairing code (paste into your BB recording panel): {pair_code}");
    let (permission_request, permission_receiver) = mpsc::channel();
    let mut bridge = Bridge {
        port,
        remote,
        workspace,
        instance,
        origin,
        pair_code: Some(pair_code),
        token: None,
        sender: None,
        permission_request,
        status: Arc::new(Mutex::new(CaptureStatus::default())),
    };
    std::thread::Builder::new()
        .name("margins-native-bridge-http".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        if let Err(error) = handle_stream(stream, &mut bridge) {
                            eprintln!("native bridge request failed: {error:#}");
                        }
                    }
                    Err(error) => eprintln!("native bridge accept failed: {error}"),
                }
            }
        })?;
    // AVFoundation presents the permission alert for this process. Keep that
    // request on the executable's main thread, while HTTP status and Stop stay
    // responsive on the server thread during the system-owned dialog.
    for reply in permission_receiver {
        let result = super::ensure_capture_permissions(&super::NativeCapturePermissionSource)
            .map_err(|error| format!("{error:#}"));
        let _ = reply.send(result);
    }
    Ok(())
}

fn parse_port(value: &str) -> Result<u16> {
    let port = value
        .parse::<u16>()
        .context("--port must be an integer from 1 to 65535")?;
    if port == 0 {
        bail!("--port must be an integer from 1 to 65535");
    }
    Ok(port)
}

fn valid_origin(value: &str) -> bool {
    let scheme_ok = value.starts_with("https://")
        || value.starts_with("http://localhost:")
        || value.starts_with("http://127.0.0.1:");
    let authority = value
        .split_once("://")
        .map(|(_, authority)| authority)
        .unwrap_or("");
    scheme_ok && !authority.is_empty() && !authority.contains(['/', '?', '#', '@'])
}

fn handle_stream(mut stream: TcpStream, bridge: &mut Bridge) -> Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(_) => {
            write_response(&mut stream, 400, &json!({"error":"invalid_request"}), None)?;
            return Ok(());
        }
    };
    if request
        .headers
        .iter()
        .filter(|(name, _)| name == "host")
        .count()
        != 1
        || request
            .headers
            .iter()
            .filter(|(name, _)| name == "origin")
            .count()
            != 1
    {
        write_response(&mut stream, 400, &json!({"error":"invalid_headers"}), None)?;
        return Ok(());
    }
    let host_ok = request.header("host") == Some(format!("127.0.0.1:{}", bridge.port).as_str());
    let origin_ok = request.header("origin") == Some(bridge.origin.as_str());
    if !host_ok || !origin_ok {
        write_response(
            &mut stream,
            403,
            &json!({"error":"origin_or_host_rejected"}),
            None,
        )?;
        return Ok(());
    }
    let allowed_origin = bridge.origin.clone();
    let cors = Some(allowed_origin.as_str());
    if request.method == "OPTIONS" {
        write_response(&mut stream, 204, &json!({}), cors)?;
        return Ok(());
    }
    if request.method == "POST" && request.path == "/v1/pair" {
        let value: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
        let supplied = value.get("code").and_then(Value::as_str).unwrap_or("");
        if !bridge
            .pair_code
            .as_deref()
            .is_some_and(|code| code == supplied)
        {
            write_response(
                &mut stream,
                403,
                &json!({"error":"invalid_pairing_code"}),
                cors,
            )?;
            return Ok(());
        }
        bridge.pair_code = None;
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        bridge.token = Some(token.clone());
        let status = bridge
            .status
            .lock()
            .unwrap()
            .snapshot(&bridge.instance, &bridge.workspace);
        write_response(
            &mut stream,
            200,
            &json!({"token":token,"status":status,"instanceId":bridge.instance,"workspaceId":bridge.workspace}),
            cors,
        )?;
        return Ok(());
    }
    let bearer = bridge.token.as_ref().map(|token| format!("Bearer {token}"));
    if bearer.as_deref() != request.header("authorization") || bearer.is_none() {
        write_response(&mut stream, 401, &json!({"error":"pairing_required"}), cors)?;
        return Ok(());
    }
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/v1/status") => (
            200,
            bridge
                .status
                .lock()
                .unwrap()
                .snapshot(&bridge.instance, &bridge.workspace),
        ),
        ("POST", "/v1/start") => {
            let value: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let title = value
                .get("title")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty() && t.len() <= 200)
                .map(str::to_string);
            let mut status = bridge.status.lock().unwrap();
            if !matches!(status.state, "" | "ready" | "saved" | "needs_attention") {
                (409, json!({"error":"capture_already_active"}))
            } else {
                *status = CaptureStatus {
                    state: "getting_ready",
                    ..Default::default()
                };
                drop(status);
                let (sender, receiver) = mpsc::channel();
                bridge.sender = Some(sender);
                let controller = CaptureController {
                    receiver,
                    status: bridge.status.clone(),
                };
                let (permission_reply, permission_result) = mpsc::channel();
                bridge.permission_request.send(permission_reply)?;
                let remote = bridge.remote.clone();
                let workspace = bridge.workspace.clone();
                let bridge_status = bridge.status.clone();
                std::thread::Builder::new()
                    .name("margins-native-bridge-capture".into())
                    .spawn(move || {
                        let result = permission_result
                            .recv()
                            .context("microphone permission request did not complete")
                            .and_then(|result| result.map_err(anyhow::Error::msg))
                            .and_then(|()| {
                                super::run_remote_native_capture(
                                    &remote,
                                    &workspace,
                                    &Some(Command::New { title }),
                                    Some(controller),
                                )
                            });
                        let mut state = bridge_status.lock().unwrap();
                        match result {
                            Ok(()) => state.state = "saved",
                            Err(error) => {
                                state.state = "needs_attention";
                                state.error = Some(format!("{error:#}"));
                            }
                        }
                    })?;
                (202, json!({"state":"getting_ready"}))
            }
        }
        ("POST", "/v1/pause") => control(bridge, "recording", CaptureAction::Pause),
        ("POST", "/v1/resume") => control(bridge, "paused", CaptureAction::Resume),
        ("POST", "/v1/stop") => {
            let state = bridge.status.lock().unwrap().state;
            if !matches!(state, "recording" | "paused" | "getting_ready") {
                (409, json!({"error":"capture_not_active"}))
            } else {
                control_unchecked(bridge, CaptureAction::Stop)
            }
        }
        _ => (404, json!({"error":"not_found"})),
    };
    write_response(&mut stream, response.0, &response.1, cors)
}

fn control(bridge: &Bridge, expected: &str, action: CaptureAction) -> (u16, Value) {
    if bridge.status.lock().unwrap().state != expected {
        return (409, json!({"error":"invalid_capture_state"}));
    }
    control_unchecked(bridge, action)
}

fn control_unchecked(bridge: &Bridge, action: CaptureAction) -> (u16, Value) {
    match bridge
        .sender
        .as_ref()
        .and_then(|sender| sender.send(action).ok())
    {
        Some(()) => (202, json!({"accepted":true})),
        None => (503, json!({"error":"capture_worker_unavailable"})),
    }
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    use std::io::Read;
    let mut bytes = Vec::new();
    let header_end = loop {
        if bytes.len() > 8192 {
            bail!("headers too large");
        }
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            bail!("incomplete request");
        }
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(pos) = bytes.windows(4).position(|slice| slice == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let header_text = std::str::from_utf8(&bytes[..header_end])?.to_string();
    let mut lines = header_text.split("\r\n");
    let request_line = lines.next().context("missing request line")?;
    let fields = request_line.split_whitespace().collect::<Vec<_>>();
    if fields.len() != 3 || fields[2] != "HTTP/1.1" {
        bail!("invalid request line");
    }
    let mut headers = Vec::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (name, value) = line.split_once(':').context("invalid header")?;
        headers.push((name.to_ascii_lowercase(), value.trim().to_string()));
    }
    if headers.iter().any(|(name, _)| name == "transfer-encoding")
        || headers
            .iter()
            .filter(|(name, _)| name == "content-length")
            .count()
            > 1
    {
        bail!("ambiguous request body framing");
    }
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .map(|(_, value)| value.parse::<usize>())
        .transpose()?
        .unwrap_or(0);
    if length > MAX_BODY || bytes.len() - header_end > MAX_BODY {
        bail!("body too large");
    }
    while bytes.len() - header_end < length {
        let mut chunk = [0; 1024];
        let read = stream.read(&mut chunk)?;
        if read == 0 {
            bail!("incomplete body");
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    bytes.truncate(header_end + length);
    Ok(Request {
        method: fields[0].to_string(),
        path: fields[1].to_string(),
        headers,
        body: bytes[header_end..].to_vec(),
    })
}

fn write_response(
    stream: &mut TcpStream,
    status: u16,
    value: &Value,
    origin: Option<&str>,
) -> Result<()> {
    let body = if status == 204 {
        Vec::new()
    } else {
        serde_json::to_vec(value)?
    };
    let reason = match status {
        200 => "OK",
        202 => "Accepted",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        409 => "Conflict",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n", body.len())?;
    if let Some(origin) = origin {
        write!(stream, "Access-Control-Allow-Origin: {origin}\r\nVary: Origin\r\nAccess-Control-Allow-Methods: GET, POST, OPTIONS\r\nAccess-Control-Allow-Headers: authorization, content-type\r\nAccess-Control-Allow-Private-Network: true\r\n")?;
    }
    write!(stream, "\r\n")?;
    stream.write_all(&body)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn exchange(bridge: &mut Bridge, raw: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let raw = raw.to_string();
        std::thread::scope(|scope| {
            let client = scope.spawn(move || {
                let mut stream = TcpStream::connect(address).unwrap();
                stream.write_all(raw.as_bytes()).unwrap();
                stream.shutdown(std::net::Shutdown::Write).unwrap();
                let mut output = String::new();
                stream.read_to_string(&mut output).unwrap();
                output
            });
            let (stream, _) = listener.accept().unwrap();
            handle_stream(stream, bridge).unwrap();
            client.join().unwrap()
        })
    }

    fn bridge() -> Bridge {
        Bridge {
            port: DEFAULT_PORT,
            remote: "ssh://configured-server".into(),
            workspace: "journal".into(),
            instance: "linux-instance".into(),
            origin: "https://example.test".into(),
            pair_code: Some("secret-code".into()),
            token: None,
            sender: None,
            permission_request: mpsc::channel().0,
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        }
    }

    #[test]
    fn exact_origin_host_pairing_and_authority_identity() {
        let mut bridge = bridge();
        let body = r#"{"code":"secret-code"}"#;
        let request = |host: &str, origin: &str| {
            format!(
            "POST /v1/pair HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        };
        assert!(
            exchange(&mut bridge, &request("evil.test", "https://example.test"))
                .starts_with("HTTP/1.1 403")
        );
        assert!(exchange(&mut bridge, &format!(
            "POST /v1/pair HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nHost: evil.test\r\nOrigin: https://example.test\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )).starts_with("HTTP/1.1 400"));
        assert!(exchange(
            &mut bridge,
            &request("127.0.0.1:18765", "https://evil.test")
        )
        .starts_with("HTTP/1.1 403"));
        let paired = exchange(
            &mut bridge,
            &request("127.0.0.1:18765", "https://example.test"),
        );
        assert!(paired.starts_with("HTTP/1.1 200"));
        assert!(paired.contains("\"instanceId\":\"linux-instance\""));
        assert!(paired.contains("\"workspaceId\":\"journal\""));
        assert!(exchange(
            &mut bridge,
            &request("127.0.0.1:18765", "https://example.test")
        )
        .starts_with("HTTP/1.1 403"));
        let token = bridge.token.clone().unwrap();
        let status = exchange(&mut bridge, &format!(
            "GET /v1/status HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nOrigin: https://example.test\r\nAuthorization: Bearer {}\r\n\r\n",
            token
        ));
        assert!(status.starts_with("HTTP/1.1 200"));
        assert!(status.contains("\"state\":\"ready\""));
    }

    #[test]
    fn selected_port_is_required_in_host_header() {
        let mut bridge = bridge();
        bridge.port = parse_port("19331").unwrap();
        let body = r#"{"code":"secret-code"}"#;
        let request = |port: u16| {
            format!(
            "POST /v1/pair HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://example.test\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        )
        };
        assert!(exchange(&mut bridge, &request(DEFAULT_PORT)).starts_with("HTTP/1.1 403"));
        assert!(exchange(&mut bridge, &request(19331)).starts_with("HTTP/1.1 200"));
        assert!(parse_port("0").is_err());
        assert!(parse_port("65536").is_err());
        assert_eq!(parse_port("65535").unwrap(), 65535);
    }

    #[test]
    fn start_keeps_status_responsive_while_main_thread_requests_permission() {
        let mut bridge = bridge();
        bridge.token = Some("paired-token".into());
        let (permission_request, permission_receiver) = mpsc::channel();
        bridge.permission_request = permission_request;
        let request = |method: &str, path: &str, body: &str| {
            format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nOrigin: https://example.test\r\nAuthorization: Bearer paired-token\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        assert!(
            exchange(&mut bridge, &request("POST", "/v1/start", "{}")).starts_with("HTTP/1.1 202")
        );
        let reply = permission_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        let pending = exchange(&mut bridge, &request("GET", "/v1/status", ""));
        assert!(pending.contains("\"state\":\"getting_ready\""));
        reply.send(Err("microphone denied".into())).unwrap();
        for _ in 0..20 {
            let status = exchange(&mut bridge, &request("GET", "/v1/status", ""));
            if status.contains("\"state\":\"needs_attention\"") {
                assert!(status.contains("microphone denied"));
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("capture worker did not publish the permission error");
    }

    #[test]
    fn origin_requires_a_single_authority() {
        assert!(valid_origin("https://bb.example"));
        assert!(valid_origin("http://localhost:5173"));
        assert!(!valid_origin("http://bb.example"));
        assert!(!valid_origin("https://bb.example/path"));
        assert!(!valid_origin("https://bb.example@evil.test"));
    }
}
