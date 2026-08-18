use chrono::{Local, TimeZone};
use clap::Parser;
use margins_cli::args::Args;
use margins_cli::run;
use margins_cli::services::{CliServices, Clock, ProjectService};
use margins_core::{
    AsrBackend, AsrRequest, AsrResult, AudioLane, CaptureCapabilities, CaptureCommand,
    CaptureCommandResult, CaptureCommandStatus, CaptureDevice, CaptureError, CaptureHandle,
    CaptureLaneSnapshot, CaptureLaneState, CaptureObserver, CaptureProvider, CaptureRequest,
    CaptureSnapshot, CaptureState, PermissionState, TranscriptError,
};
use margins_store::legacy;
use margins_workflows::project::{ProjectSource, ResolvedProject};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct FixedProject(PathBuf);

impl FixedProject {
    fn resolved(&self) -> ResolvedProject {
        ResolvedProject {
            project: ProjectSource {
                id: "test".into(),
                name: "Test".into(),
                path: self.0.to_string_lossy().into_owned(),
                inbox_folder: "meetings".into(),
                people_folder: "people".into(),
                readiness: "ready".into(),
            },
            root_dir: self.0.clone(),
            work_dir: self.0.clone(),
        }
    }
}

impl ProjectService for FixedProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        Ok(vec![self.resolved()])
    }
    fn resolve(&self, _selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn set_active(&self, _selector: &str) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn add(
        &self,
        _path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
}

struct FixedClock;

impl Clock for FixedClock {
    fn now(&self) -> chrono::DateTime<Local> {
        Local.with_ymd_and_hms(2026, 8, 10, 12, 0, 0).unwrap()
    }
}

fn services(root: &Path) -> CliServices {
    let mut services = CliServices::default();
    services.projects = Arc::new(FixedProject(root.to_path_buf()));
    services.clock = Arc::new(FixedClock);
    services
}

fn invoke(
    services: &CliServices,
    invocation_dir: &Path,
    args: &[&str],
) -> (Result<(), margins_cli::CliError>, String, String) {
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = run(
        services,
        invocation_dir,
        args.iter().copied(),
        &mut stdout,
        &mut stderr,
    );
    (
        result,
        String::from_utf8(stdout).unwrap(),
        String::from_utf8(stderr).unwrap(),
    )
}

#[test]
fn project_preprocessing_accepts_both_historical_spellings_anywhere() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    for args in [
        vec!["margins", "--project", "test", "recent"],
        vec!["margins", "recent", "--project=test"],
    ] {
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{stderr}");
        assert_eq!(stdout, "<margins_recent />\n");
    }
}

#[test]
fn clap_help_preserves_the_prior_argument_contract() {
    let error = Args::try_parse_from(["margins", "transcribe", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Audio file to decode in Rust, such as WAV, M4A, MP3, FLAC, or AAC"));
    assert!(help.contains("Session name. Defaults to a slug derived"));
    assert!(help.contains("Speaker count for diarizing the downmixed mono audio"));

    let error = Args::try_parse_from(["margins", "process", "--help"]).unwrap_err();
    let help = error.to_string();
    assert!(help.contains("Stable session id from `margins recent`, or `current`/`latest`"));
    assert!(help.contains("Rebuild alignment from the existing transcript without running ASR"));
}

#[test]
fn parser_accepts_workspace_setup_guide_command() {
    let parsed = Args::try_parse_from(["margins", "guide", "workspace-setup"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Guide {
            command: margins_cli::args::GuideCommand::WorkspaceSetup
        })
    ));
}

#[test]
fn parser_accepts_read_only_and_write_config_scan_commands() {
    let parsed = Args::try_parse_from(["margins", "scan"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan {
            write_config: false
        })
    ));

    let parsed = Args::try_parse_from(["margins", "scan", "--write-config"]).unwrap();
    assert!(matches!(
        parsed.command,
        Some(margins_cli::args::Command::Scan { write_config: true })
    ));
}

#[test]
fn setup_prints_minimal_handoff_for_absolute_invocation_dir_without_side_effects() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("note.md"), "real note").unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "setup"]);

    assert!(result.is_ok(), "{stderr}");
    assert_eq!(
        stdout,
        format!(
            "Paste into your agent:\nSet up Margins in {}. Run margins guide workspace-setup and follow it end to end.\n",
            temp.path().display()
        )
    );
    assert!(stderr.is_empty());
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn workspace_setup_guide_is_embedded_margins_native_and_read_only() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("note.md"), "real note").unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "guide", "workspace-setup"],
    );

    assert!(result.is_ok(), "{stderr}");
    assert_eq!(
        stdout,
        margins_workflows::resources::MARGINS_WORKSPACE_SETUP_GUIDE
    );
    assert!(stdout.contains("margins init"));
    assert!(stdout.contains("margins scan"));
    assert!(stdout.contains("top_tags"));
    assert!(stdout.contains("top_links"));
    assert!(stdout.contains("log:journal"));
    assert!(stdout.contains("margins recall"));
    assert!(stdout.contains(".margins/recall/index.db"));
    assert!(stdout.contains("~/.margins/config.toml"));
    assert!(stdout.contains("excluded_folders"));
    assert!(stdout.contains("entities = ["));
    assert!(stdout.contains("ask before deleting"));
    assert!(stdout.contains("Never edits note bodies"));
    assert!(!stdout.to_lowercase().contains("enzyme"));
    assert!(!stdout.contains("enzyme doctor"));
    assert!(!stdout.contains("enzyme scan"));
    assert!(!stdout.contains("enzyme init"));
    assert!(!stdout.contains(".enzyme/enzyme.db"));
    assert!(!stdout.contains(".enzyme/"));
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn public_capabilities_report_no_recall_composition() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "capabilities"]);

    assert!(result.is_ok(), "{stderr}");
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["schema"], 1);
    assert_eq!(value["product"], "margins");
    assert_eq!(value["composition"], "public-open-core");
    assert_eq!(value["official"], false);
    assert_eq!(value["recall"]["indexing"], false);
    assert_eq!(value["recall"]["lookup"], false);
    assert_eq!(value["recall"]["scan"], false);
}

#[test]
fn public_scan_is_not_the_product_workspace_discovery() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "scan"]);

    assert!(result.is_err());
    assert!(stdout.is_empty());
    assert!(stderr.contains("composition_unavailable"));
    assert!(stderr.contains("official Margins CLI"));
    assert!(!temp.path().join(".margins").exists());
}

#[test]
fn public_init_is_not_the_product_workspace_setup() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "init"]);

    assert!(result.is_err());
    assert!(stdout.is_empty());
    assert!(stderr.contains("composition_unavailable"));
    assert!(stderr.contains("official Margins CLI"));
    assert!(!temp.path().join(".margins").exists());
    assert!(!temp.path().join(".margins/recall/index.db").exists());
}

#[test]
fn init_xml_can_report_effective_config_path() {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("home/.margins/config.toml");
    let mut stdout = Vec::new();

    margins_cli::commands::projects::write_init(&mut stdout, temp.path(), "ok", Some(&config))
        .unwrap();

    assert_eq!(
        String::from_utf8(stdout).unwrap(),
        format!(
            "<margins_init path=\"{}\" status=\"ok\" config_path=\"{}\" />\n",
            temp.path().display(),
            config.display()
        )
    );
}

struct RecordingProject {
    root: PathBuf,
    added_paths: Mutex<Vec<String>>,
    resolved_selectors: Mutex<Vec<Option<String>>>,
}

struct MultiProject {
    active_id: String,
    vaults: Vec<ResolvedProject>,
    list_calls: Mutex<usize>,
}

impl ProjectService for MultiProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        *self.list_calls.lock().unwrap() += 1;
        Ok(self.vaults.clone())
    }

    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        let id = selector.unwrap_or(&self.active_id);
        self.vaults
            .iter()
            .find(|vault| vault.project.id == id || vault.project.path == id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown test vault {id}"))
    }

    fn set_active(&self, selector: &str) -> anyhow::Result<ResolvedProject> {
        self.resolve(Some(selector))
    }

    fn add(
        &self,
        path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        self.resolve(Some(path))
    }
}

fn resolved_vault(id: &str, root: &Path) -> ResolvedProject {
    ResolvedProject {
        project: ProjectSource {
            id: id.into(),
            name: id.into(),
            path: root.to_string_lossy().into_owned(),
            inbox_folder: "meetings".into(),
            people_folder: "people".into(),
            readiness: "ready".into(),
        },
        root_dir: root.to_path_buf(),
        work_dir: root.to_path_buf(),
    }
}

fn seed_inspectable_session(vault: &Path, meeting_id: &str, marker: &str) {
    let margins_dir = vault.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::write(margins_dir.join(format!("{meeting_id}.md")), marker).unwrap();
    std::fs::write(
        margins_dir.join(format!("{meeting_id}_aligned.md")),
        format!("# {marker}"),
    )
    .unwrap();
    legacy::create_session(
        &margins_dir,
        meeting_id,
        &Local::now(),
        &format!(".margins/{meeting_id}.md"),
    )
    .unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        meeting_id,
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        &format!(".margins/{meeting_id}_aligned.md"),
        "durable",
        None,
    )
    .unwrap();
}

fn multi_project_services(projects: Arc<MultiProject>) -> CliServices {
    let mut services = services(&projects.vaults[0].root_dir);
    services.projects = projects;
    services
}

impl RecordingProject {
    fn resolved(&self) -> ResolvedProject {
        FixedProject(self.root.clone()).resolved()
    }
}

impl ProjectService for RecordingProject {
    fn list(&self) -> anyhow::Result<Vec<ResolvedProject>> {
        Ok(vec![self.resolved()])
    }
    fn resolve(&self, selector: Option<&str>) -> anyhow::Result<ResolvedProject> {
        self.resolved_selectors
            .lock()
            .unwrap()
            .push(selector.map(str::to_string));
        Ok(self.resolved())
    }
    fn set_active(&self, _selector: &str) -> anyhow::Result<ResolvedProject> {
        Ok(self.resolved())
    }
    fn add(
        &self,
        path: &str,
        _name: Option<&str>,
        _inbox_folder: Option<&str>,
    ) -> anyhow::Result<ResolvedProject> {
        self.added_paths.lock().unwrap().push(path.to_string());
        Ok(self.resolved())
    }
}

#[test]
fn selected_project_is_forwarded_without_changing_process_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let project_root = temp.path().join("selected-project");
    std::fs::create_dir_all(&project_root).unwrap();
    let projects = Arc::new(RecordingProject {
        root: project_root,
        added_paths: Mutex::new(Vec::new()),
        resolved_selectors: Mutex::new(Vec::new()),
    });
    let mut services = services(&projects.root);
    services.projects = projects.clone();
    let cwd_before = std::env::current_dir().unwrap();

    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "recent", "--project=selected"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert_eq!(stdout, "<margins_recent />\n");
    assert_eq!(
        projects.resolved_selectors.lock().unwrap().as_slice(),
        &[Some("selected".into())]
    );
    assert_eq!(std::env::current_dir().unwrap(), cwd_before);
}

#[test]
fn concrete_meeting_commands_find_the_unique_owning_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let owner = temp.path().join("owner");
    std::fs::create_dir_all(active.join(".margins")).unwrap();
    seed_inspectable_session(&owner, "cross-vault", "owner transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("owner", &owner),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    for args in [
        vec!["margins", "artifacts", "cross-vault"],
        vec!["margins", "transcript", "cross-vault"],
    ] {
        let (result, stdout, stderr) = invoke(&services, &active, &args);
        assert!(result.is_ok(), "{args:?}: {stderr}");
        assert!(stdout.contains(&owner.to_string_lossy().to_string()));
        assert!(!stdout.contains("<exists>false</exists>"));
    }
}

#[test]
fn concrete_meeting_lookup_deduplicates_the_active_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    seed_inspectable_session(&active, "active-meeting", "active transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        // The initial resolved vault is appended by the dispatcher; this list
        // entry must not make the active meeting look ambiguous.
        vaults: vec![resolved_vault("active", &active)],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    let (result, stdout, stderr) = invoke(
        &services,
        &active,
        &["margins", "transcript", "active-meeting"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("active transcript"));
}

#[test]
fn duplicate_concrete_meeting_ids_fail_closed_even_when_active_has_one() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "duplicate", "active transcript");
    seed_inspectable_session(&other, "duplicate", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects);

    let (result, stdout, stderr) =
        invoke(&services, &active, &["margins", "artifacts", "duplicate"]);
    let error = result.unwrap_err();
    assert_eq!(error.code(), "ambiguous_meeting");
    assert!(stdout.is_empty());
    assert!(stderr.contains("active"));
    assert!(stderr.contains("other"));
    assert!(stderr.contains("Pass --project"));
}

#[test]
fn explicit_project_remains_authoritative_for_duplicate_meeting_ids() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "duplicate", "active transcript");
    seed_inspectable_session(&other, "duplicate", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects.clone());

    let (result, stdout, stderr) = invoke(
        &services,
        &active,
        &["margins", "transcript", "duplicate", "--project=other"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("other transcript"));
    assert!(!stdout.contains("active transcript"));
    assert_eq!(*projects.list_calls.lock().unwrap(), 0);
}

#[test]
fn latest_remains_scoped_to_the_active_vault() {
    let temp = tempfile::tempdir().unwrap();
    let active = temp.path().join("active");
    let other = temp.path().join("other");
    seed_inspectable_session(&active, "active-latest", "active transcript");
    seed_inspectable_session(&other, "other-latest", "other transcript");
    let projects = Arc::new(MultiProject {
        active_id: "active".into(),
        vaults: vec![
            resolved_vault("active", &active),
            resolved_vault("other", &other),
        ],
        list_calls: Mutex::new(0),
    });
    let services = multi_project_services(projects.clone());

    let (result, stdout, stderr) = invoke(&services, &active, &["margins", "transcript", "latest"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("active transcript"));
    assert!(!stdout.contains("other transcript"));
    assert_eq!(*projects.list_calls.lock().unwrap(), 0);
}

struct EmptyAsr;

impl AsrBackend for EmptyAsr {
    fn backend_name(&self) -> &'static str {
        "empty-fixture"
    }

    fn transcribe(&self, _request: AsrRequest) -> Result<AsrResult, TranscriptError> {
        Ok(AsrResult {
            words: Vec::new(),
            detected_language: None,
        })
    }
}

#[test]
fn transcribe_resolves_audio_and_memo_relative_to_invocation() {
    let temp = tempfile::tempdir().unwrap();
    let invocation = temp.path().join("invocation");
    let project = temp.path().join("project");
    std::fs::create_dir_all(&invocation).unwrap();
    std::fs::create_dir_all(&project).unwrap();
    margins_media::audio::write_interleaved_wav(
        &invocation.join("input.wav"),
        &[0.0; 320],
        16_000,
        1,
    )
    .unwrap();
    std::fs::write(invocation.join("memo.md"), "[00:00] relative memo\n").unwrap();
    let mut services = services(&project);
    services.asr = Arc::new(EmptyAsr);

    let (result, stdout, stderr) = invoke(
        &services,
        &invocation,
        &[
            "margins",
            "transcribe",
            "input.wav",
            "--memo",
            "memo.md",
            "--name",
            "relative",
        ],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("meeting_id=\"relative\""));
    assert_eq!(
        std::fs::read_to_string(project.join(".margins/relative.md")).unwrap(),
        "[00:00] relative memo\n"
    );
    assert!(project.join(".margins/relative_seg0.wav").is_file());
    assert!(!invocation.join(".margins").exists());
}

#[test]
fn unavailable_capture_is_stable_and_precedes_all_mutation() {
    for args in [
        vec!["margins"],
        vec!["margins", "new", "--title", "Never written"],
        vec!["margins", "attach", "missing"],
    ] {
        let temp = tempfile::tempdir().unwrap();
        let services = services(temp.path());
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        let error = result.unwrap_err();
        assert_eq!(error.code(), "capture_unavailable");
        assert_eq!(error.exit_code(), 69);
        assert!(stdout.is_empty());
        assert_eq!(stderr, "<margins_error code=\"capture_unavailable\">capture is unavailable in this build</margins_error>\n");
        assert!(!temp.path().join(".margins").exists());
    }
}

#[derive(Default)]
struct FakeCapture;

impl CaptureProvider for FakeCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            available: true,
            supported_lanes: vec![AudioLane::Microphone],
            supports_device_selection: false,
            supports_live_pcm: false,
            unavailable_reason: None,
        }
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn start(
        &self,
        request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Ok(Box::new(FakeHandle {
            snapshot: CaptureSnapshot {
                session_id: request.session_id,
                segment_id: request.segment_id,
                state: CaptureState::Capturing,
                lanes: vec![CaptureLaneSnapshot {
                    lane: AudioLane::Microphone,
                    state: CaptureLaneState::Active,
                    generation: 0,
                    delivered_frames: 0,
                    durable_frames: 0,
                    observed_signal: false,
                    dropped_live_frames: 0,
                    dropped_durable_frames: 0,
                    last_error_code: None,
                }],
                timeline_reusable: true,
            },
        }))
    }
}

struct FailingStartCapture;

impl CaptureProvider for FailingStartCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        FakeCapture.capabilities()
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Granted)
    }
    fn start(
        &self,
        _request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        Err(CaptureError::unavailable("provider disappeared"))
    }
}

struct DeniedCapture;

impl CaptureProvider for DeniedCapture {
    fn capabilities(&self) -> CaptureCapabilities {
        FakeCapture.capabilities()
    }
    fn devices(&self) -> Result<Vec<CaptureDevice>, CaptureError> {
        Ok(Vec::new())
    }
    fn permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Denied)
    }
    fn request_permission(&self, _lane: AudioLane) -> Result<PermissionState, CaptureError> {
        Ok(PermissionState::Denied)
    }
    fn start(
        &self,
        _request: CaptureRequest,
        _observer: Arc<dyn CaptureObserver>,
    ) -> Result<Box<dyn CaptureHandle>, CaptureError> {
        unreachable!("permission denial must preflight before start")
    }
}

struct FakeHandle {
    snapshot: CaptureSnapshot,
}

impl CaptureHandle for FakeHandle {
    fn snapshot(&self) -> Result<CaptureSnapshot, CaptureError> {
        Ok(self.snapshot.clone())
    }
    fn command(&self, command: CaptureCommand) -> Result<CaptureCommandResult, CaptureError> {
        let mut snapshot = self.snapshot.clone();
        snapshot.state = CaptureState::Finished;
        Ok(CaptureCommandResult {
            operation_id: command.operation_id,
            status: CaptureCommandStatus::Applied,
            snapshot,
            completed_artifacts: Vec::new(),
        })
    }
}

#[test]
fn injected_capture_keeps_one_stable_id_across_attaches() {
    let temp = tempfile::tempdir().unwrap();
    let mut services = services(temp.path());
    services.capture = Arc::new(FakeCapture);
    for args in [
        vec!["margins", "new", "--title", "Stable"],
        vec!["margins", "attach"],
        vec!["margins", "attach"],
    ] {
        let (result, _, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{stderr}");
    }
    let margins_dir = temp.path().join(".margins");
    let current = std::fs::read_to_string(margins_dir.join("current")).unwrap();
    let id = current.trim();
    assert_eq!(id, "2026-08-10-12-00-00");
    let meta = legacy::get_session_meta(&margins_dir, id).unwrap();
    assert_eq!(meta.title.as_deref(), Some("Stable"));
    assert_eq!(
        meta.segments
            .iter()
            .map(|segment| segment.segment_index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(legacy::list_sessions(&margins_dir).unwrap().len(), 1);
}

#[test]
fn permission_and_failed_start_leave_no_capture_mutation() {
    for (provider, expected) in [
        (
            Arc::new(DeniedCapture) as Arc<dyn CaptureProvider>,
            "capture_permission_denied",
        ),
        (
            Arc::new(FailingStartCapture) as Arc<dyn CaptureProvider>,
            "capture_unavailable",
        ),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let mut services = services(temp.path());
        services.capture = provider;
        let (result, stdout, _) = invoke(&services, temp.path(), &["margins", "new"]);
        assert_eq!(result.unwrap_err().code(), expected);
        assert!(stdout.is_empty());
        assert!(!temp.path().join(".margins").exists());
    }
}

#[test]
fn no_feature_asr_fails_before_transcribe_writes() {
    let temp = tempfile::tempdir().unwrap();
    let audio = temp.path().join("relative.wav");
    margins_media::audio::write_interleaved_wav(&audio, &[0.0; 320], 16_000, 1).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let services = services(&project);
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcribe", "relative.wav", "--name", "blocked"],
    );
    assert_eq!(result.unwrap_err().code(), "asr_unavailable");
    assert!(stdout.is_empty());
    assert_eq!(stderr, "<margins_error code=\"asr_unavailable\">ASR is unavailable in this build</margins_error>\n");
    assert!(!project.join(".margins").exists());
}

#[test]
fn read_only_input_errors_precede_backend_unavailability() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let invocation = temp.path().join("invocation");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::create_dir_all(&invocation).unwrap();
    let services = services(&project);

    let (result, stdout, stderr) = invoke(
        &services,
        &invocation,
        &["margins", "transcribe", "missing.wav"],
    );
    assert_eq!(result.unwrap_err().code(), "audio_not_found");
    assert!(stdout.is_empty());
    assert!(stderr.contains("Audio file not found:"));
    assert!(!project.join(".margins").exists());

    let (result, stdout, stderr) =
        invoke(&services, &invocation, &["margins", "process", "missing"]);
    assert_eq!(result.unwrap_err().code(), "store_not_found");
    assert!(stdout.is_empty());
    assert_eq!(
        stderr,
        "<margins_error code=\"store_not_found\">No .margins/ directory found.</margins_error>\n"
    );
    assert!(!project.join(".margins").exists());
}

#[test]
fn unavailable_process_backends_do_not_replace_existing_outputs() {
    let temp = tempfile::tempdir().unwrap();
    let project = temp.path().join("project");
    let margins_dir = project.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    let started = Local.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        None,
    )
    .unwrap();
    let transcript = margins_dir.join("meeting_transcript.json");
    std::fs::write(&transcript, "preserve-me").unwrap();
    let services = services(&project);

    for args in [
        vec!["margins", "process", "meeting"],
        vec!["margins", "process", "meeting", "--speakers", "2"],
    ] {
        let (result, stdout, _) = invoke(&services, temp.path(), &args);
        let error = result.unwrap_err();
        assert!(matches!(
            error.code(),
            "asr_unavailable" | "diarization_unavailable"
        ));
        assert!(stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&transcript).unwrap(), "preserve-me");
        assert!(!margins_dir.join("meeting_aligned.md").exists());
        assert!(!margins_dir.join("artifacts").exists());
    }
}

#[test]
fn xml_and_json_presenters_escape_user_controlled_values() {
    let temp = tempfile::tempdir().unwrap();
    let services = services(temp.path());
    let margins_dir = temp.path().join(".margins");
    let started = Local.with_ymd_and_hms(2026, 8, 10, 10, 0, 0).unwrap();
    legacy::create_session(&margins_dir, "meeting", &started, ".margins/meeting.md").unwrap();
    legacy::set_title(
        &margins_dir,
        "meeting",
        Some("A <title> & \"quote\"".into()),
    )
    .unwrap();
    std::fs::write(margins_dir.join("current"), "meeting\n").unwrap();

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "current"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<title>A &lt;title&gt; &amp; \"quote\"</title>"));
    assert!(!stdout.contains(''));
}

#[test]
fn no_feature_diarization_fails_before_transcribe_writes() {
    let temp = tempfile::tempdir().unwrap();
    let audio = temp.path().join("relative.wav");
    margins_media::audio::write_interleaved_wav(&audio, &[0.0; 320], 16_000, 1).unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let services = services(&project);
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "transcribe", "relative.wav", "--speakers", "2"],
    );
    assert_eq!(result.unwrap_err().code(), "diarization_unavailable");
    assert!(stdout.is_empty());
    assert_eq!(stderr, "<margins_error code=\"diarization_unavailable\">diarization is unavailable in this build</margins_error>\n");
    assert!(!project.join(".margins").exists());
}

#[test]
fn align_only_does_not_consult_unavailable_asr() {
    let temp = tempfile::tempdir().unwrap();
    let margins_dir = temp.path().join(".margins");
    let memo = temp.path().join("memo.md");
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        &memo.to_string_lossy(),
    )
    .unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        Some(1.0),
    )
    .unwrap();
    std::fs::write(
        margins_dir.join("meeting_transcript.json"),
        r#"{"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":"hello"}]}]}"#,
    )
    .unwrap();
    let services = services(temp.path());
    let (result, stdout, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "process", "meeting", "--align-only"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("<margins_process meeting_id=\"meeting\" status=\"ok\">"));
    assert!(margins_dir.join("meeting_aligned.md").exists());
}

#[test]
fn archive_commands_route_new_aligned_output_to_visible_default_folder() {
    let temp = tempfile::tempdir().unwrap();
    let margins_dir = temp.path().join(".margins");
    let memo = margins_dir.join("meeting.md");
    std::fs::create_dir_all(&margins_dir).unwrap();
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    legacy::add_segment(
        &margins_dir,
        "meeting",
        0,
        ".margins/meeting_seg0.wav",
        0,
        Some(1.0),
    )
    .unwrap();
    std::fs::write(
        margins_dir.join("meeting_transcript.json"),
        r#"{"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":"hello"}]}]}"#,
    )
    .unwrap();
    let services = services(temp.path());

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "archive", "on"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"true\""));
    assert!(stdout.contains("path=\"") && stdout.contains("_margins"));

    let (result, _, stderr) = invoke(
        &services,
        temp.path(),
        &["margins", "process", "meeting", "--align-only"],
    );
    assert!(result.is_ok(), "{stderr}");
    assert!(temp.path().join("_margins/meeting_aligned.md").is_file());
    assert!(!margins_dir.join("meeting_aligned.md").exists());
    assert_eq!(
        legacy::list_session_artifacts(&margins_dir, "meeting").unwrap()[0].path,
        "_margins/meeting_aligned.md"
    );

    let (result, stdout, stderr) =
        invoke(&services, temp.path(), &["margins", "archive", "status"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"true\""));
    assert!(stdout.contains("transcripts=\"1\""));

    let (result, stdout, stderr) = invoke(&services, temp.path(), &["margins", "archive", "off"]);
    assert!(result.is_ok(), "{stderr}");
    assert!(stdout.contains("enabled=\"false\""));
    assert!(margins_dir.join("meeting_aligned.md").is_file());
    assert!(!temp.path().join("_margins").exists());
}

#[test]
fn session_catalog_and_artifact_commands_emit_vault_anchored_paths() {
    let temp = tempfile::tempdir().unwrap();
    let vault = temp.path().join("vault");
    let margins_dir = vault.join(".margins");
    std::fs::create_dir_all(&margins_dir).unwrap();
    let memo = margins_dir.join("meeting.md");
    let transcript = margins_dir.join("meeting_seg0.live-transcript.json");
    std::fs::write(&memo, "[00:01] checkpoint").unwrap();
    std::fs::write(
        &transcript,
        r#"{"terminal":true,"transcripts":[{"words":[{"channel":0,"start_ms":500,"end_ms":900,"text":" hello"}]}]}"#,
    )
    .unwrap();
    legacy::create_session(
        &margins_dir,
        "meeting",
        &Local::now(),
        ".margins/meeting.md",
    )
    .unwrap();
    legacy::upsert_session_artifact(
        &margins_dir,
        "meeting",
        legacy::SESSION_ARTIFACT_KIND_TRANSCRIPT,
        0,
        ".margins/meeting_seg0.live-transcript.json",
        "durable",
        None,
    )
    .unwrap();
    let services = services(&vault);

    for args in [
        vec!["margins", "recent"],
        vec!["margins", "artifacts", "meeting"],
        vec!["margins", "transcript", "meeting"],
    ] {
        let (result, stdout, stderr) = invoke(&services, temp.path(), &args);
        assert!(result.is_ok(), "{args:?}: {stderr}");
        assert!(
            stdout.contains(&vault.to_string_lossy().to_string()),
            "{args:?} returned a cwd-relative artifact path: {stdout}"
        );
    }
}
