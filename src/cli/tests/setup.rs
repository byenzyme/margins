    #[test]
    fn cli_error_boundary_rewords_transport_and_engine_internals() {
        let output =
            margins_user_message("enzyme login --use-env-llm api.enzyme.garden OpenRouter");
        let lower = output.to_ascii_lowercase();
        for forbidden in [
            "enzyme login",
            "--use-env-llm",
            "api.enzyme.garden",
            "openrouter",
        ] {
            assert!(!lower.contains(forbidden), "leaked {forbidden}: {output}");
        }
        assert!(output.contains("margins setup"));
        assert!(output.contains("hosted catalyst service"));
        assert!(output.contains("hosted catalyst provider"));
    }

    struct ScopedTestSettings {
        _override: margins_workflows::project::TestSettingsPathOverride,
        _dir: tempfile::TempDir,
    }

    impl ScopedTestSettings {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path_override = margins_workflows::project::override_settings_path_for_test(
                dir.path().join("settings.json"),
            );
            Self {
                _override: path_override,
                _dir: dir,
            }
        }
    }

    struct SpyInteractive(AtomicUsize);
    impl InteractiveSession for SpyInteractive {
        fn create(&self, _work_dir: &Path, _title: Option<&str>) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn attach(&self, _work_dir: &Path, _selected: Option<&str>) -> Result<()> {
            self.0.fetch_add(10, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ScriptedPermissionSource {
        mic: margins_core::PermissionState,
        requested_mic: margins_core::PermissionState,
        system: margins_core::PermissionState,
        request_count: AtomicUsize,
    }

    impl CapturePermissionSource for ScriptedPermissionSource {
        fn permission(
            &self,
            lane: margins_core::AudioLane,
        ) -> Result<margins_core::PermissionState> {
            match lane {
                margins_core::AudioLane::Microphone => Ok(self.mic),
                margins_core::AudioLane::System => Ok(self.system),
                _ => Ok(margins_core::PermissionState::Unavailable),
            }
        }

        fn request_permission(
            &self,
            lane: margins_core::AudioLane,
        ) -> Result<margins_core::PermissionState> {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(lane, margins_core::AudioLane::Microphone);
            Ok(self.requested_mic)
        }
    }

    #[test]
    fn native_permission_preflight_requests_mic_only_from_explicit_capture() {
        let source = ScriptedPermissionSource {
            mic: margins_core::PermissionState::NotDetermined,
            requested_mic: margins_core::PermissionState::Granted,
            system: margins_core::PermissionState::Unknown,
            request_count: AtomicUsize::new(0),
        };

        ensure_capture_permissions(&source).unwrap();

        assert_eq!(source.request_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn native_permission_preflight_blocks_denied_mic_without_requesting() {
        let source = ScriptedPermissionSource {
            mic: margins_core::PermissionState::Denied,
            requested_mic: margins_core::PermissionState::Granted,
            system: margins_core::PermissionState::Unknown,
            request_count: AtomicUsize::new(0),
        };

        let error = ensure_capture_permissions(&source).unwrap_err().to_string();

        assert!(error.contains("Microphone permission"));
        assert_eq!(source.request_count.load(Ordering::SeqCst), 0);
    }

    struct SpySetupProvisioner {
        hosted_calls: AtomicUsize,
        speech_calls: AtomicUsize,
        local_calls: AtomicUsize,
        hosted: HostedCatalystSetup,
        speech_model: Option<std::path::PathBuf>,
        speech_error: Option<&'static str>,
        local_model: Option<std::path::PathBuf>,
    }

    impl SetupMachineProvisioner for SpySetupProvisioner {
        fn provision_hosted_catalyst(&self, _home: &Path) -> Result<HostedCatalystSetup> {
            self.hosted_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.hosted.clone())
        }

        fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
            self.speech_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.speech_error {
                bail!(error);
            }
            Ok(self.speech_model.clone())
        }

        fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
            self.local_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.local_model.clone())
        }
    }

    struct FailingSpeechNativeProvisioner {
        speech_marker: std::path::PathBuf,
        local_calls: AtomicUsize,
    }

    impl SetupMachineProvisioner for FailingSpeechNativeProvisioner {
        fn provision_hosted_catalyst(&self, home: &Path) -> Result<HostedCatalystSetup> {
            NativeSetupMachineProvisioner.provision_hosted_catalyst(home)
        }

        fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
            std::fs::write(&self.speech_marker, "speech attempted")?;
            bail!("fixture speech setup failed")
        }

        fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
            self.local_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[cfg(feature = "recall")]
    fn start_fake_setup_broker() -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 2048];
            loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"api_key":"sk-or-v1-secret-shaped-setup-fixture","base_url":"https://fixture.invalid/v1","model":"fixture-catalyst-model","expires_at":4102444800}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/llm/free-config"), server)
    }

    struct ArtifactWritingInteractive;

    impl InteractiveSession for ArtifactWritingInteractive {
        fn create(&self, work_dir: &Path, _title: Option<&str>) -> Result<()> {
            let margins_dir = work_dir.join(".margins");
            std::fs::create_dir_all(&margins_dir)?;
            let name = "capture-root-regression";
            let memo = margins_dir.join(format!("{name}.md"));
            let audio = margins_dir.join(format!("{name}_seg0.wav"));
            let checkpoint = margins_dir.join(format!("{name}_seg0.live-transcript.json"));
            std::fs::write(&memo, "test memo")?;
            std::fs::write(&audio, b"test wav fixture")?;
            std::fs::write(&checkpoint, br#"{"terminal":false,"transcripts":[]}"#)?;
            margins_store::canonical::create_session(
                &margins_dir,
                name,
                &chrono::Local::now(),
                &format!(".margins/{name}.md"),
            )?;
            margins_store::canonical::add_segment(
                &margins_dir,
                name,
                0,
                &format!(".margins/{name}_seg0.wav"),
                0,
                None,
            )?;
            Ok(())
        }

        fn attach(&self, _work_dir: &Path, _selected: Option<&str>) -> Result<()> {
            unreachable!("capture-root regression only invokes margins new")
        }
    }

    fn seed_capture_root_settings(active: &Path, launcher: &Path) {
        let settings = margins_workflows::project::settings_path();
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let value = serde_json::json!({
            "vault_path": active.to_string_lossy(),
            "active_project_id": "active-vault",
            "projects": [
                {
                    "id": "active-vault",
                    "name": "Obsidian",
                    "path": active.to_string_lossy(),
                    "readiness": "ready"
                },
                {
                    "id": "launcher-vault",
                    "name": "Explicit launcher fixture",
                    "path": launcher.to_string_lossy(),
                    "readiness": "ready"
                }
            ]
        });
        std::fs::write(&settings, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn assert_capture_artifacts(root: &Path, expected: bool) {
        let margins_dir = root.join(".margins");
        for path in [
            margins_dir.join("capture-root-regression.md"),
            margins_dir.join("capture-root-regression_seg0.wav"),
            margins_dir.join("capture-root-regression_seg0.live-transcript.json"),
            margins_store::canonical::database_path(&margins_dir),
        ] {
            assert_eq!(
                path.is_file(),
                expected,
                "unexpected state for {}",
                path.display()
            );
        }
    }

    #[test]
    fn setup_materializes_every_embedded_skill_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        let expected = [
            (
                "margins/SKILL.md",
                MARGINS_SKILL.get_file("SKILL.md").unwrap().contents(),
            ),
            (
                "margins/distillation-core.md",
                MARGINS_SKILL
                    .get_file("distillation-core.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/hosts/desktop.md",
                MARGINS_SKILL
                    .get_file("hosts/desktop.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/1on1-idea-exchange.md",
                MARGINS_SKILL
                    .get_file("templates/1on1-idea-exchange.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/design-scoping-session.md",
                MARGINS_SKILL
                    .get_file("templates/design-scoping-session.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/discovery-call.md",
                MARGINS_SKILL
                    .get_file("templates/discovery-call.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/group-conversation.md",
                MARGINS_SKILL
                    .get_file("templates/group-conversation.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/talk-reflection.md",
                MARGINS_SKILL
                    .get_file("templates/talk-reflection.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "watermark/SKILL.md",
                WATERMARK_SKILL.get_file("SKILL.md").unwrap().contents(),
            ),
        ];

        for (relative, contents) in expected {
            assert_eq!(
                std::fs::read(temp.path().join(".margins/skills").join(relative)).unwrap(),
                contents,
                "materialized content differed for {relative}"
            );
        }
    }

    #[test]
    fn setup_skips_absent_agents_without_creating_their_home_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        for agent_home in [".claude", ".codex", ".cursor"] {
            assert!(!temp.path().join(agent_home).exists());
        }
        assert_eq!(
            String::from_utf8(report).unwrap(),
            "Claude Code skills: skipped (not installed)\nCodex skills: skipped (not installed)\nCursor skills: skipped (not installed)\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn setup_links_skills_for_every_detected_agent() {
        let temp = tempfile::tempdir().unwrap();
        for agent_home in [".claude", ".codex", ".cursor"] {
            std::fs::create_dir(temp.path().join(agent_home)).unwrap();
        }
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        for agent_home in [".claude", ".codex", ".cursor"] {
            for skill in ["margins", "watermark"] {
                let link = temp.path().join(agent_home).join("skills").join(skill);
                assert!(std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                assert_eq!(
                    std::fs::canonicalize(link).unwrap(),
                    std::fs::canonicalize(temp.path().join(".margins/skills").join(skill)).unwrap()
                );
            }
        }
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("Claude Code skills: linked margins, watermark"));
        assert!(report.contains("Codex skills: linked margins, watermark"));
        assert!(report.contains("Cursor skills: linked margins, watermark"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_preserves_preexisting_real_skill_directory() {
        let temp = tempfile::tempdir().unwrap();
        let user_skill = temp.path().join(".claude/skills/margins");
        std::fs::create_dir_all(&user_skill).unwrap();
        std::fs::write(user_skill.join("user-owned.txt"), "keep me").unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        assert_eq!(
            std::fs::read_to_string(user_skill.join("user-owned.txt")).unwrap(),
            "keep me"
        );
        assert!(!std::fs::symlink_metadata(&user_skill)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(
            std::fs::symlink_metadata(temp.path().join(".claude/skills/watermark"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(String::from_utf8(report)
            .unwrap()
            .contains("Claude Code skills: linked watermark; skipped margins (user dir present)"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_repoints_stale_link_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let codex_skills = temp.path().join(".codex/skills");
        let stale_target = temp.path().join("old-margins-skill");
        std::fs::create_dir_all(&codex_skills).unwrap();
        std::fs::create_dir(&stale_target).unwrap();
        std::os::unix::fs::symlink(&stale_target, codex_skills.join("margins")).unwrap();
        let mut first_report = Vec::new();

        install_embedded_skills(temp.path(), &mut first_report).unwrap();
        let link = codex_skills.join("margins");
        let first_target = std::fs::read_link(&link).unwrap();
        std::fs::write(
            temp.path().join(".margins/skills/margins/SKILL.md"),
            "stale canonical content",
        )
        .unwrap();
        std::fs::write(
            temp.path()
                .join(".margins/skills/margins/removed-from-binary.md"),
            "stale removed file",
        )
        .unwrap();

        let mut second_report = Vec::new();
        install_embedded_skills(temp.path(), &mut second_report).unwrap();

        assert_eq!(std::fs::read_link(&link).unwrap(), first_target);
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(temp.path().join(".margins/skills/margins")).unwrap()
        );
        assert_eq!(
            std::fs::read(temp.path().join(".margins/skills/margins/SKILL.md")).unwrap(),
            MARGINS_SKILL.get_file("SKILL.md").unwrap().contents()
        );
        assert!(!temp
            .path()
            .join(".margins/skills/margins/removed-from-binary.md")
            .exists());
        assert_eq!(first_report, second_report);
    }

    #[test]
    fn setup_runs_independent_steps_and_names_only_the_selected_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_model_dir = temp.path().join("speech-cache");
        std::fs::create_dir_all(&speech_model_dir).unwrap();
        std::fs::write(speech_model_dir.join("fixture.bin"), [0u8; 2048]).unwrap();
        crate::hosted_credentials::install_bundle(
            &margins_home,
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Hosted {
                model: "fixture-model".into(),
            },
            speech_model: Some(speech_model_dir.clone()),
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(&[], None, SetupLocalModelPolicyArg::Fallback),
            Some(&margins_home),
            None,
            Some(&skill_home),
            Some("practice"),
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(!failed);
        assert_eq!(provisioner.hosted_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provisioner.speech_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nHelp me set up Margins workspace practice so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: ok"));
        assert!(stderr.contains("ready with fixture-model"));
        // The only Margins-home path setup prints is the Workspace program.
        let home_text = margins_home.to_string_lossy().to_string();
        assert!(
            stderr
                .lines()
                .filter(|line| line.contains(&home_text))
                .all(|line| line.contains(&format!("{home_text}/configs/"))),
            "{stderr}"
        );
        assert!(stderr.contains("setup local catalyst: ok — not installed under fallback policy"));
        assert!(stderr.contains("setup skills: ok"));
        assert!(stderr.contains(&format!(
            "setup speech: ok — model cache {}",
            speech_model_dir.display()
        )));
        assert!(stderr.contains("2.0 KB"));
        assert!(stderr.contains("catalyst mode: hosted — hosted_bundle_ready"));
    }

    #[test]
    fn setup_without_workspace_never_uses_the_process_directory_for_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Offline {
                reason: "unused".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Skills],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            Some(&home),
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(
            !failed,
            "an unconfigured catalyst is a note, not a failure, under --only skills"
        );
        assert_eq!(provisioner.hosted_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nGo to my notes folder, then help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Claude Code skills: skipped (not installed)"));
        assert!(stderr.contains("Codex skills: skipped (not installed)"));
        assert!(stderr.contains("Cursor skills: skipped (not installed)"));
        assert!(stderr.contains("setup skills: ok"));
        assert!(stderr.contains(
            "catalyst mode: none — setup_required (not part of this setup; run margins setup --only catalyst)"
        ));
        assert!(!temp.path().join(".margins").exists());
    }

    fn speech_only_provisioner(speech_error: Option<&'static str>) -> SpySetupProvisioner {
        SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Offline {
                reason: "unused".into(),
            },
            speech_model: None,
            speech_error,
            local_model: None,
        }
    }

    fn run_speech_only(provisioner: &SpySetupProvisioner, margins_home: &Path) -> (bool, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let failed = run_setup_with(
            provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Speech],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(margins_home),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        (failed, String::from_utf8(stderr).unwrap())
    }

    #[test]
    fn setup_only_speech_succeeds_without_a_configured_catalyst() {
        let temp = tempfile::tempdir().unwrap();
        let provisioner = speech_only_provisioner(None);

        let (failed, stderr) = run_speech_only(&provisioner, temp.path());

        assert!(!failed, "{stderr}");
        assert_eq!(provisioner.speech_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provisioner.hosted_calls.load(Ordering::SeqCst), 0);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert!(stderr.contains("setup speech: ok"));
        assert!(stderr.contains(
            "catalyst mode: none — setup_required (not part of this setup; run margins setup --only catalyst)"
        ));
    }

    #[test]
    fn setup_only_speech_fails_when_speech_fails_even_with_a_usable_catalyst() {
        let temp = tempfile::tempdir().unwrap();
        crate::hosted_credentials::install_bundle(
            temp.path(),
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let provisioner = speech_only_provisioner(Some("fixture speech setup failed"));

        let (failed, stderr) = run_speech_only(&provisioner, temp.path());

        assert!(failed, "{stderr}");
        assert!(stderr.contains("setup speech: failed — fixture speech setup failed"));
        assert!(stderr.contains("catalyst mode: hosted — hosted_bundle_ready"));
    }

    #[test]
    fn setup_only_skills_fails_when_skills_cannot_be_installed() {
        let temp = tempfile::tempdir().unwrap();
        let provisioner = speech_only_provisioner(None);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Skills],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup skills: failed"));
    }

    #[test]
    fn setup_with_catalyst_selected_still_fails_without_a_usable_generator() {
        let temp = tempfile::tempdir().unwrap();
        let provisioner = speech_only_provisioner(None);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Speech, SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup speech: ok"));
        assert!(stderr.contains("catalyst mode: none — setup_required\n"));
    }

    #[cfg(all(feature = "recall", unix))]
    #[test]
    fn setup_fake_broker_provisions_credentials_and_exits_zero_despite_failing_speech() {
        use std::os::unix::fs::PermissionsExt;

        let _global_env_lock = crate::test_process_env_lock().lock().unwrap();
        let _env_lock = PROCESS_ENV_LOCK.lock().unwrap();
        let names = [
            "ENZYME_FREE_CONFIG_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        ];
        let _restore = EnvRestore::capture(&names);
        for name in &names[1..] {
            std::env::remove_var(name);
        }
        let (broker_url, broker) = start_fake_setup_broker();
        std::env::set_var("ENZYME_FREE_CONFIG_URL", broker_url);
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_marker = temp.path().join("speech-attempted");
        let provisioner = FailingSpeechNativeProvisioner {
            speech_marker: speech_marker.clone(),
            local_calls: AtomicUsize::new(0),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(&[], None, SetupLocalModelPolicyArg::Fallback),
            Some(&margins_home),
            None,
            Some(&skill_home),
            Some("fixture-workspace"),
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        let request = broker.join().unwrap();

        assert!(
            !failed,
            "a usable hosted generator makes setup successful despite speech failure"
        );
        assert!(speech_marker.is_file());
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert!(request.starts_with("GET /llm/free-config HTTP/1.1\r\n"));
        assert!(request.lines().any(|line| line
            .to_ascii_lowercase()
            .starts_with("x-enzyme-bootstrap-id:")));
        for name in [
            crate::hosted_credentials::BOOTSTRAP_FILE,
            crate::hosted_credentials::BUNDLE_FILE,
        ] {
            let path = margins_home.join(name);
            assert!(path.is_file(), "missing {}", path.display());
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(std::fs::read_to_string(margins_home.join("configs/settings.enzyme"))
            .unwrap()
            .contains("generation hosted"));
        let stderr = String::from_utf8(stderr).unwrap();
        let hosted = stderr.find("setup hosted catalyst: ok").unwrap();
        let skills = stderr.find("setup skills: ok").unwrap();
        let speech = stderr
            .find("setup speech: failed — fixture speech setup failed")
            .unwrap();
        let local = stderr.find("setup local catalyst: ok").unwrap();
        assert!(
            hosted < skills && skills < speech && speech < local,
            "{stderr}"
        );
        // Exit 0 by the catalyst contract, but the failure is said plainly.
        assert!(
            stderr.contains(
                "Setup finished with failures: speech. Recall can still work without them. Retry with `margins setup --only speech`.\n"
            ),
            "{stderr}"
        );
        assert!(stderr.contains("catalyst mode: hosted — hosted_bundle_ready"));
        // The only Margins-home path setup prints is the Workspace program.
        let home_text = margins_home.to_string_lossy().to_string();
        assert!(
            stderr
                .lines()
                .filter(|line| line.contains(&home_text))
                .all(|line| line.contains(&format!("{home_text}/configs/"))),
            "{stderr}"
        );
        assert!(!stderr.contains(crate::hosted_credentials::BUNDLE_FILE));
        assert!(!stderr.contains("sk-or-v1-secret-shaped-setup-fixture"));
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nHelp me set up Margins workspace fixture-workspace so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn setup_only_catalyst_with_hosted_broker_touches_no_model_or_skill_paths() {
        let _global_env_lock = crate::test_process_env_lock().lock().unwrap();
        let _env_lock = PROCESS_ENV_LOCK.lock().unwrap();
        let names = [
            "ENZYME_FREE_CONFIG_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        ];
        let _restore = EnvRestore::capture(&names);
        for name in &names[1..] {
            std::env::remove_var(name);
        }
        let (broker_url, broker) = start_fake_setup_broker();
        std::env::set_var("ENZYME_FREE_CONFIG_URL", broker_url);
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_marker = temp.path().join("speech-attempted");
        let provisioner = FailingSpeechNativeProvisioner {
            speech_marker: speech_marker.clone(),
            local_calls: AtomicUsize::new(0),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(&margins_home),
            None,
            Some(&skill_home),
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        broker.join().unwrap();

        assert!(!failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert!(!speech_marker.exists());
        assert!(!margins_home.join("models").exists());
        assert!(!skill_home.join(".margins/skills").exists());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: ok"));
        assert!(stderr.contains("setup local catalyst: ok — not installed under fallback policy"));
        // The only Margins-home path setup prints is the Workspace program.
        let home_text = margins_home.to_string_lossy().to_string();
        assert!(
            stderr
                .lines()
                .filter(|line| line.contains(&home_text))
                .all(|line| line.contains(&format!("{home_text}/configs/"))),
            "{stderr}"
        );
        assert!(!stderr.contains(crate::hosted_credentials::BUNDLE_FILE));
        assert!(!stderr.contains("sk-or-v1-secret-shaped-setup-fixture"));
        assert!(!stderr.contains("setup skills:"));
        assert!(!stderr.contains("setup speech:"));
    }

    #[test]
    fn setup_exits_nonzero_when_no_generator_is_usable() {
        let temp = tempfile::tempdir().unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Offline {
                reason: "fixture broker offline".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 1);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: failed — fixture broker offline"));
        assert!(stderr.contains("catalyst mode: none — setup_required"));
    }

    #[test]
    fn setup_local_model_always_fails_plainly_when_this_build_cannot_install_one() {
        let temp = tempfile::tempdir().unwrap();
        crate::hosted_credentials::install_bundle(
            temp.path(),
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        margins_workflows::machine_config::set_generation(temp.path(), "hosted").unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Hosted {
                model: "fixture-model".into(),
            },
            speech_model: None,
            speech_error: None,
            // A build without the local-model installer.
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Always,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed, "`always` without a local model must not succeed");
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 1);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(
            stderr.contains(
                "setup local catalyst: failed — this Margins build cannot install a local catalyst model, so --local-model always cannot be honored"
            ),
            "{stderr}"
        );
        assert!(!stderr.contains("selected without a bundled local-model installer"));
        assert!(
            stderr.contains("Setup finished with failures: local catalyst (not available in this build)."),
            "{stderr}"
        );
        assert!(!stderr.contains("Retry with"), "{stderr}");
        // Generation is left as it was; it is not silently claimed as local.
        assert_eq!(
            margins_workflows::machine_config::engine_settings(temp.path())
                .unwrap()
                .generation
                .as_deref(),
            Some("hosted")
        );
    }

    #[test]
    fn setup_local_model_always_policy_installs_after_hosted_success() {
        let temp = tempfile::tempdir().unwrap();
        let local_model = temp.path().join("models/fixture.gguf");
        std::fs::create_dir_all(local_model.parent().unwrap()).unwrap();
        std::fs::write(&local_model, [0u8; 1024]).unwrap();
        crate::hosted_credentials::install_bundle(
            temp.path(),
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Hosted {
                model: "fixture-model".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: Some(local_model.clone()),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Always,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(!failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 1);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains(&format!(
            "setup local catalyst: ok — installed at {}",
            local_model.display()
        )));
        // `always` selects the local model even though hosted is ready.
        assert_eq!(
            margins_workflows::machine_config::engine_settings(temp.path())
                .unwrap()
                .generation
                .as_deref(),
            Some("local")
        );
        assert!(stderr.contains("catalyst mode: local"), "{stderr}");
    }

    #[test]
    fn first_record_states_the_speech_download_size_and_asks() {
        let mut input = "y\n".as_bytes();
        let mut output = Vec::new();
        confirm_speech_download(
            SpeechPurpose::Record,
            SPEECH_MODEL_APPROX_BYTES,
            true,
            &mut input,
            &mut output,
        )
        .unwrap();
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Recording needs the local speech model, a one-time download of about 464.0 MB. Download now? [Y/n] "
        );
    }

    #[test]
    fn declining_the_speech_download_downloads_nothing_and_names_the_command() {
        let mut input = "n\n".as_bytes();
        let mut output = Vec::new();
        let error = confirm_speech_download(
            SpeechPurpose::Transcribe,
            SPEECH_MODEL_APPROX_BYTES,
            true,
            &mut input,
            &mut output,
        )
        .unwrap_err()
        .to_string();
        assert!(String::from_utf8(output).unwrap().starts_with("Transcribing needs"));
        assert_eq!(
            error,
            "Nothing was downloaded. Run `margins setup --only speech` when ready (about 464.0 MB)."
        );
    }

    #[test]
    fn non_interactive_speech_download_refuses_with_command_and_size() {
        let mut input = "y\n".as_bytes();
        let mut output = Vec::new();
        let error = confirm_speech_download(
            SpeechPurpose::Record,
            SPEECH_MODEL_APPROX_BYTES + 30 * 1_048_576,
            false,
            &mut input,
            &mut output,
        )
        .unwrap_err()
        .to_string();
        assert!(output.is_empty(), "no prompt without a terminal");
        assert_eq!(
            error,
            "Recording needs the local speech model, a one-time download of about 494.0 MB. Run `margins setup --only speech` in a terminal to download it."
        );
    }

    #[test]
    fn speech_download_runs_only_after_consent() {
        let downloads = AtomicUsize::new(0);
        let stub = || {
            downloads.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };
        // Installed: no prompt, no download.
        let mut output = Vec::new();
        assert!(!offer_speech_download(
            true,
            SpeechPurpose::Record,
            SPEECH_MODEL_APPROX_BYTES,
            true,
            &mut "".as_bytes(),
            &mut output,
            stub,
        )
        .unwrap());
        assert!(output.is_empty());
        // Non-interactive and declined: refused before the download.
        for (interactive, answer) in [(false, "y\n"), (true, "n\n")] {
            let result = offer_speech_download(
                false,
                SpeechPurpose::Record,
                SPEECH_MODEL_APPROX_BYTES,
                interactive,
                &mut answer.as_bytes(),
                &mut Vec::new(),
                stub,
            );
            assert!(result.unwrap_err().to_string().contains("margins setup --only speech"));
        }
        assert_eq!(downloads.load(Ordering::SeqCst), 0);
        // Accepted: the stubbed download runs once.
        assert!(offer_speech_download(
            false,
            SpeechPurpose::Record,
            SPEECH_MODEL_APPROX_BYTES,
            true,
            &mut "\n".as_bytes(),
            &mut Vec::new(),
            stub,
        )
        .unwrap());
        assert_eq!(downloads.load(Ordering::SeqCst), 1);
    }
