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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    mic_peak: Option<Arc<AtomicU32>>,
}

#[derive(Default)]
struct CaptureStatus {
    state: &'static str,
    session_id: Option<String>,
    transfer_id: Option<String>,
    error: Option<String>,
    opened_mic_name: Option<String>,
    local_audio_paths: Vec<String>,
    unsent_audio_path: Option<String>,
    completed: Counters,
    live: LiveCounters,
}

impl CaptureStatus {
    fn snapshot(&self, instance_id: &str, workspace_id: &str, selected_mic: Option<&str>) -> Value {
        let load = |counter: &Option<Arc<AtomicU64>>| {
            counter.as_ref().map_or(0, |v| v.load(Ordering::Relaxed))
        };
        let opened_mic = if matches!(self.state, "getting_ready" | "recording" | "paused") {
            self.opened_mic_name.clone()
        } else {
            None
        };
        json!({
            "state": self.visible_state(),
            "instanceId": instance_id,
            "workspaceId": workspace_id,
            "microphoneDeviceName": opened_mic
                .or_else(|| selected_mic.map(str::to_string))
                .or_else(crate::recorder::default_input_device_name),
            "microphoneDevicePinned": selected_mic.is_some(),
            "pid": std::process::id(),
            "sessionId": self.session_id,
            "transferId": self.transfer_id,
            "microphoneSamples": self.completed.mic + load(&self.live.mic),
            "systemSamples": self.completed.system + load(&self.live.system),
            "microphoneDroppedSamples": self.completed.mic_dropped + load(&self.live.mic_dropped),
            "systemDroppedSamples": self.completed.system_dropped + load(&self.live.system_dropped),
            "systemFrames": self.completed.frames + load(&self.live.frames),
            "systemSilentSamples": self.completed.silent + load(&self.live.silent),
            "micPeak": self.live.mic_peak.as_ref().map(|peak| f32::from_bits(peak.load(Ordering::Relaxed))).unwrap_or(0.0),
            "error": self.error,
            "localAudioPaths": self.local_audio_paths,
            "unsentAudioPath": self.unsent_audio_path,
        })
    }

    /// The state users act on. Start opens the devices before the remote
    /// session exists and buffers that audio; "recording" is reported once
    /// the microphone has actually delivered samples, so nobody speaks into a
    /// recorder that is still opening.
    fn visible_state(&self) -> &'static str {
        let mic = self.completed.mic
            + self
                .live
                .mic
                .as_ref()
                .map_or(0, |v| v.load(Ordering::Relaxed));
        match self.state {
            "" => "ready",
            "getting_ready" if mic > 0 => "recording",
            state => state,
        }
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

impl LiveCounters {
    fn for_segment(
        sink: &crate::recorder::LiveAudioSink,
        recorder: &crate::recorder::RecorderHandle,
    ) -> Self {
        LiveCounters {
            mic: Some(sink.mic_accepted_samples.clone()),
            system: Some(sink.system_accepted_samples.clone()),
            mic_dropped: Some(sink.mic_dropped_samples.clone()),
            system_dropped: Some(sink.system_dropped_samples.clone()),
            frames: Some(recorder.spk_frames()),
            silent: Some(recorder.spk_silence()),
            mic_peak: Some(recorder.mic_peak()),
        }
    }
}

impl CaptureController {
    /// The devices are open and audio is buffering while the remote session
    /// is reserved. Status turns to "recording" with the first mic samples.
    pub(super) fn capturing(
        &self,
        sink: &crate::recorder::LiveAudioSink,
        recorder: &crate::recorder::RecorderHandle,
    ) {
        let mut state = self.status.lock().unwrap();
        state.opened_mic_name = Some(recorder.mic_name().to_owned());
        state.live = LiveCounters::for_segment(sink, recorder);
    }

    pub(super) fn recording(
        &self,
        session: &str,
        transfer: &str,
        sink: &crate::recorder::LiveAudioSink,
        recorder: &crate::recorder::RecorderHandle,
    ) {
        recorder.mic_peak().store(0, Ordering::Relaxed);
        let mut state = self.status.lock().unwrap();
        state.state = "recording";
        state.session_id = Some(session.into());
        state.transfer_id = Some(transfer.into());
        state.opened_mic_name = Some(recorder.mic_name().to_owned());
        state.live = LiveCounters::for_segment(sink, recorder);
    }

    /// Pause or Stop sent while the session is still being reserved. Resume
    /// cannot arrive first: the bridge accepts it only when paused.
    pub(super) fn pre_session_control(
        &self,
        timeout: Duration,
    ) -> Option<super::PreSessionControl> {
        match self.receiver.recv_timeout(timeout) {
            Ok(CaptureAction::Pause) => Some(super::PreSessionControl::Pause),
            Ok(CaptureAction::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                Some(super::PreSessionControl::Stop)
            }
            Ok(CaptureAction::Resume) | Err(mpsc::RecvTimeoutError::Timeout) => None,
        }
    }

    /// The session exists for capture that was paused or stopped before it
    /// did; the state the user chose stands.
    pub(super) fn session_ready(&self, session: &str, transfer: &str) {
        let mut state = self.status.lock().unwrap();
        state.session_id = Some(session.into());
        state.transfer_id = Some(transfer.into());
    }

    pub(super) fn unsent_audio_saved(&self, path: &std::path::Path) {
        self.status.lock().unwrap().unsent_audio_path = Some(path.to_string_lossy().into_owned());
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
        state.opened_mic_name = None;
    }

    pub(super) fn local_audio_saved(&self, path: &std::path::Path) {
        self.status
            .lock()
            .unwrap()
            .local_audio_paths
            .push(path.to_string_lossy().into_owned());
    }

    pub(super) fn local_audio_failed(&self, error: &str) {
        self.status.lock().unwrap().error = Some(format!("Local audio copy failed: {error}"));
    }
}

struct Bridge {
    port: u16,
    remote: String,
    workspace: String,
    instance: String,
    origin: String,
    menu_origin: Option<String>,
    mic_device_name: Option<String>,
    local_audio_dir: Option<std::path::PathBuf>,
    prepared_connection: Option<margins_workflows::remote_workspace::RemoteConnection>,
    pair_code: Option<String>,
    pair_code_file: Option<std::path::PathBuf>,
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
    let mut menu_origin = None;
    let mut remote_token_file = None;
    let mut mic_device_name = None;
    let mut local_audio_dir = None;
    let mut pair_code_file = None;
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
            Some("--menu-origin") => menu_origin = Some(value.to_string()),
            Some("--remote-token-file") => {
                remote_token_file = Some(std::path::PathBuf::from(value))
            }
            Some("--port") => port = parse_port(value)?,
            Some("--mic-device") => mic_device_name = Some(value.to_string()),
            Some("--local-audio-dir") => local_audio_dir = Some(std::path::PathBuf::from(value)),
            Some("--pair-code-file") => pair_code_file = Some(std::path::PathBuf::from(value)),
            _ => bail!("unknown native-bridge option"),
        }
    }
    let remote = remote.context("native-bridge requires --remote")?;
    let workspace = workspace.context("native-bridge requires --workspace")?;
    let origin = origin.context("native-bridge requires --origin")?;
    if mic_device_name
        .as_ref()
        .is_some_and(|name: &String| name.trim().is_empty())
    {
        bail!("--mic-device must name an input device");
    }
    if local_audio_dir
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        bail!("--local-audio-dir must be an absolute path");
    }
    if pair_code_file
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        bail!("--pair-code-file must be an absolute path");
    }
    if !valid_origin(&origin) {
        bail!("--origin must be one exact HTTPS origin (HTTP localhost is allowed for local development)");
    }
    if menu_origin
        .as_deref()
        .is_some_and(|value| !valid_origin(value))
    {
        bail!("--menu-origin must be one exact local origin");
    }
    if let Some(path) = remote_token_file {
        if !path.is_absolute() || !std::fs::symlink_metadata(&path)?.is_file() {
            bail!("--remote-token-file must be an absolute plain file");
        }
        let token = std::fs::read_to_string(&path)?.trim().to_string();
        if token.len() < 32 {
            bail!("remote capture grant is invalid");
        }
        std::env::set_var("MARGINS_REMOTE_TOKEN", token);
        std::fs::remove_file(path)?;
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
    if let Some(path) = &pair_code_file {
        write_private_pair_code(path, &pair_code)?;
    }
    let (permission_request, permission_receiver) = mpsc::channel();
    let mut bridge = Bridge {
        port,
        remote,
        workspace,
        instance,
        origin,
        menu_origin,
        mic_device_name,
        local_audio_dir,
        prepared_connection: Some(connection),
        pair_code: Some(pair_code),
        pair_code_file,
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

fn write_private_pair_code(path: &std::path::Path, code: &str) -> Result<()> {
    use std::fs::OpenOptions;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    writeln!(file, "{code}")?;
    file.sync_all()?;
    Ok(())
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
    let origin = request.header("origin").unwrap_or("");
    let origin_ok = origin == bridge.origin || bridge.menu_origin.as_deref() == Some(origin);
    if !host_ok || !origin_ok {
        write_response(
            &mut stream,
            403,
            &json!({"error":"origin_or_host_rejected"}),
            None,
        )?;
        return Ok(());
    }
    let allowed_origin = origin.to_string();
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
        if let Some(path) = bridge.pair_code_file.take() {
            let _ = std::fs::remove_file(path);
        }
        let token = format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        );
        bridge.token = Some(token.clone());
        let status = bridge.status.lock().unwrap().snapshot(
            &bridge.instance,
            &bridge.workspace,
            bridge.mic_device_name.as_deref(),
        );
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
            bridge.status.lock().unwrap().snapshot(
                &bridge.instance,
                &bridge.workspace,
                bridge.mic_device_name.as_deref(),
            ),
        ),
        ("GET", "/v1/microphones") => (200, microphones(bridge)),
        ("GET", "/v1/microphone-permission") => (200, microphone_permission()),
        ("POST", "/v1/microphone-permission") => {
            let idle = matches!(
                bridge.status.lock().unwrap().state,
                "" | "ready" | "saved" | "needs_attention"
            );
            if !idle {
                (
                    409,
                    json!({"error":"Finish the meeting before changing microphone access"}),
                )
            } else {
                let (reply, result) = mpsc::channel();
                if bridge.permission_request.send(reply).is_err() {
                    (503, json!({"error":"permission_worker_unavailable"}))
                } else {
                    match result.recv_timeout(Duration::from_secs(95)) {
                        Ok(Ok(())) => (200, microphone_permission()),
                        Ok(Err(error)) => (
                            403,
                            json!({"error":error,"permission":microphone_permission()}),
                        ),
                        Err(_) => (
                            503,
                            json!({"error":"microphone_permission_request_timed_out"}),
                        ),
                    }
                }
            }
        }
        ("POST", "/v1/microphone") => {
            let value: Value = serde_json::from_slice(&request.body).unwrap_or(Value::Null);
            let requested = value.get("deviceName");
            let name = requested
                .and_then(Value::as_str)
                .filter(|name| !name.is_empty());
            let idle = matches!(
                bridge.status.lock().unwrap().state,
                "" | "ready" | "saved" | "needs_attention"
            );
            if !idle {
                (
                    409,
                    json!({"error":"Finish the meeting before changing microphones"}),
                )
            } else if !matches!(requested, Some(Value::Null) | Some(Value::String(_)))
                || requested.is_some_and(|value| value.as_str() == Some(""))
            {
                (
                    400,
                    json!({"error":"Select a microphone or System default"}),
                )
            } else if name.is_some_and(|name| {
                name.len() > 200
                    || !crate::recorder::list_input_devices()
                        .iter()
                        .any(|(available, _)| available == name)
            }) {
                (
                    400,
                    json!({"error":"The selected microphone is unavailable"}),
                )
            } else {
                bridge.mic_device_name = name.map(str::to_string);
                bridge.status.lock().unwrap().opened_mic_name = None;
                (200, microphones(bridge))
            }
        }
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
                let mic_device_name = bridge.mic_device_name.clone();
                let local_audio_dir = bridge.local_audio_dir.clone();
                let prepared_connection = bridge
                    .prepared_connection
                    .as_ref()
                    .and_then(|connection| connection.reusable_http());
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
                                    local_audio_dir.as_deref(),
                                    mic_device_name.as_deref(),
                                    prepared_connection,
                                    true,
                                )
                            });
                        let mut state = bridge_status.lock().unwrap();
                        state.opened_mic_name = None;
                        match result {
                            Ok(()) => state.state = "saved",
                            Err(error) => {
                                state.state = "needs_attention";
                                state.error = Some(match &state.unsent_audio_path {
                                    Some(path) => format!(
                                        "{error:#}. Recording saved locally: {path}. Import it with `margins transcribe {path:?}`."
                                    ),
                                    None => format!("{error:#}"),
                                });
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

fn microphones(bridge: &Bridge) -> Value {
    let mut devices: Vec<String> = crate::recorder::list_input_devices()
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    devices.sort();
    devices.dedup();
    json!({
        "devices": devices,
        "defaultDeviceName": crate::recorder::default_input_device_name(),
        "selectedDeviceName": bridge.mic_device_name,
    })
}

fn microphone_permission() -> Value {
    let status = match crate::recorder::microphone_authorization() {
        Ok(crate::recorder::MicrophoneAuthorization::Authorized) => "authorized",
        Ok(crate::recorder::MicrophoneAuthorization::NotDetermined) => "not_determined",
        Ok(crate::recorder::MicrophoneAuthorization::Denied) => "denied",
        Ok(crate::recorder::MicrophoneAuthorization::Restricted) => "restricted",
        Err(_) => "unknown",
    };
    json!({"status":status})
}

fn control(bridge: &Bridge, expected: &str, action: CaptureAction) -> (u16, Value) {
    // A Pause sent while the session is still being reserved is queued and
    // applied as soon as the first segment begins.
    if bridge.status.lock().unwrap().visible_state() != expected {
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
            menu_origin: Some("http://127.0.0.1:18766".into()),
            mic_device_name: None,
            local_audio_dir: None,
            prepared_connection: None,
            pair_code: Some("secret-code".into()),
            pair_code_file: None,
            token: None,
            sender: None,
            permission_request: mpsc::channel().0,
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        }
    }

    #[test]
    fn unpinned_bridge_reports_the_input_it_actually_opened() {
        let status = CaptureStatus {
            state: "recording",
            opened_mic_name: Some("Yeti Stereo Microphone".into()),
            ..Default::default()
        };
        let snapshot = status.snapshot("instance", "workspace", None);
        assert_eq!(snapshot["microphoneDeviceName"], "Yeti Stereo Microphone");
        assert_eq!(snapshot["microphoneDevicePinned"], false);
        let finished = CaptureStatus {
            state: "saved",
            ..status
        };
        let snapshot = finished.snapshot("instance", "workspace", Some("USB Digital Audio"));
        assert_eq!(snapshot["microphoneDeviceName"], "USB Digital Audio");
        assert_eq!(snapshot["microphoneDevicePinned"], true);
    }

    #[test]
    fn status_reports_recording_only_once_the_microphone_delivers_audio() {
        let mic = Arc::new(AtomicU64::new(0));
        let status = Arc::new(Mutex::new(CaptureStatus {
            state: "getting_ready",
            live: LiveCounters {
                mic: Some(mic.clone()),
                ..Default::default()
            },
            ..Default::default()
        }));
        let snapshot = status
            .lock()
            .unwrap()
            .snapshot("instance", "workspace", None);
        assert_eq!(snapshot["state"], "getting_ready");
        assert!(snapshot["sessionId"].is_null());

        // Devices are open and buffering before the session exists.
        mic.store(480, Ordering::Relaxed);
        let snapshot = status
            .lock()
            .unwrap()
            .snapshot("instance", "workspace", None);
        assert_eq!(snapshot["state"], "recording");
        assert_eq!(snapshot["microphoneSamples"], 480);

        // Pause is accepted and queued until the first segment begins.
        let (sender, receiver) = mpsc::channel();
        let mut bridge = bridge();
        bridge.status = status;
        bridge.sender = Some(sender);
        assert_eq!(control(&bridge, "recording", CaptureAction::Pause).0, 202);
        assert!(receiver.try_recv() == Ok(CaptureAction::Pause));
    }

    #[test]
    fn pause_and_stop_act_before_the_session_exists() {
        let (sender, receiver) = mpsc::channel();
        let controller = CaptureController {
            receiver,
            status: Arc::new(Mutex::new(CaptureStatus::default())),
        };
        let wait = Duration::from_millis(1);
        assert_eq!(controller.pre_session_control(wait), None);
        sender.send(CaptureAction::Pause).unwrap();
        assert_eq!(
            controller.pre_session_control(wait),
            Some(super::super::PreSessionControl::Pause)
        );
        sender.send(CaptureAction::Stop).unwrap();
        assert_eq!(
            controller.pre_session_control(wait),
            Some(super::super::PreSessionControl::Stop)
        );
        controller.paused();
        controller.session_ready("session", "transfer");
        controller.unsent_audio_saved(std::path::Path::new("/tmp/unsent.wav"));
        let snapshot = controller.status.lock().unwrap().snapshot("i", "w", None);
        assert_eq!(
            snapshot["state"], "paused",
            "the user's Pause stands once the session exists"
        );
        assert_eq!(snapshot["sessionId"], "session");
        assert_eq!(snapshot["unsentAudioPath"], "/tmp/unsent.wav");
        drop(sender);
        assert_eq!(
            controller.pre_session_control(wait),
            Some(super::super::PreSessionControl::Stop),
            "a vanished bridge stops capture"
        );
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
    fn private_pair_file_is_removed_after_pairing() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pair-code");
        write_private_pair_code(&path, "secret-code").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret-code\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let mut bridge = bridge();
        bridge.pair_code_file = Some(path.clone());
        let body = r#"{"code":"secret-code"}"#;
        let raw = format!(
            "POST /v1/pair HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nOrigin: https://example.test\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        assert!(exchange(&mut bridge, &raw).starts_with("HTTP/1.1 200"));
        assert!(!path.exists());
    }

    #[test]
    fn microphone_choice_requires_pairing_and_idle_capture() {
        let mut bridge = bridge();
        let request = |method: &str, path: &str, body: &str| {
            format!(
                "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nOrigin: https://example.test\r\nAuthorization: Bearer paired-token\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            )
        };
        assert!(
            exchange(&mut bridge, &request("GET", "/v1/microphones", ""))
                .starts_with("HTTP/1.1 401")
        );
        bridge.token = Some("paired-token".into());
        assert!(
            exchange(&mut bridge, &request("GET", "/v1/microphones", ""))
                .contains("\"selectedDeviceName\":null")
        );
        assert!(exchange(
            &mut bridge,
            &request(
                "POST",
                "/v1/microphone",
                "{\"deviceName\":\"missing-device\"}"
            )
        )
        .starts_with("HTTP/1.1 400"));
        bridge.status.lock().unwrap().state = "recording";
        assert!(exchange(
            &mut bridge,
            &request("POST", "/v1/microphone", "{\"deviceName\":null}")
        )
        .starts_with("HTTP/1.1 409"));
        bridge.status.lock().unwrap().state = "ready";
        bridge.status.lock().unwrap().opened_mic_name = Some("previous microphone".into());
        assert!(exchange(
            &mut bridge,
            &request("POST", "/v1/microphone", "{\"deviceName\":null}")
        )
        .starts_with("HTTP/1.1 200"));
        assert!(bridge.status.lock().unwrap().opened_mic_name.is_none());
    }

    #[test]
    fn microphone_permission_request_is_paired_and_does_not_start_capture() {
        let mut bridge = bridge();
        let request = |method: &str| {
            format!(
                "{method} /v1/microphone-permission HTTP/1.1\r\nHost: 127.0.0.1:18765\r\nOrigin: https://example.test\r\nAuthorization: Bearer paired-token\r\nContent-Length: 2\r\n\r\n{{}}"
            )
        };
        assert!(exchange(&mut bridge, &request("POST")).starts_with("HTTP/1.1 401"));
        bridge.token = Some("paired-token".into());
        bridge.status.lock().unwrap().state = "recording";
        assert!(exchange(&mut bridge, &request("POST")).starts_with("HTTP/1.1 409"));
        bridge.status.lock().unwrap().state = "ready";
        let (permission_request, permission_receiver) = mpsc::channel();
        bridge.permission_request = permission_request;
        let worker = std::thread::spawn(move || {
            permission_receiver.recv().unwrap().send(Ok(())).unwrap();
        });
        let response = exchange(&mut bridge, &request("POST"));
        worker.join().unwrap();
        assert!(response.starts_with("HTTP/1.1 200"));
        assert_eq!(bridge.status.lock().unwrap().state, "ready");
        assert!(bridge.status.lock().unwrap().session_id.is_none());
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
