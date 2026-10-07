    #[test]
    fn release_smoke_is_exact_and_does_not_invoke_interactive_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore =
            EnvRestore::capture(&["MARGINS_HOME", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("margins-home"));
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY");
        let spy = SpyInteractive(AtomicUsize::new(0));
        assert_eq!(main_entry_with(["margins", RELEASE_SMOKE_COMMAND], &spy), 0);
        assert_eq!(spy.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn version_flags_do_not_invoke_interactive_composition() {
        let spy = SpyInteractive(AtomicUsize::new(0));
        for flag in ["--version", "-V"] {
            assert_eq!(main_entry_with(["margins", flag], &spy), 0, "{flag}");
        }
        assert_eq!(spy.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn official_version_line_matches_release_smoke_build_and_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore =
            EnvRestore::capture(&["MARGINS_HOME", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("margins-home"));
        let smoke = official_capabilities_json();
        let line = official_version_line();
        assert!(line.starts_with(&format!("margins {} (", env!("CARGO_PKG_VERSION"))));
        assert!(line.contains(smoke["build"]["short"].as_str().unwrap()));
        assert!(line.ends_with(&format!(
            ", {})",
            smoke["composition"].as_str().unwrap()
        )));
    }

    #[test]
    fn official_capabilities_report_recall_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore =
            EnvRestore::capture(&["MARGINS_HOME", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("margins-home"));
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY");
        let value = official_capabilities_json();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["product"], "margins");
        assert_eq!(value["composition"], "official");
        assert_eq!(value["official"], true);
        assert_eq!(
            value["oauth_client"],
            crate::google_oauth_client::status()
        );
        assert_eq!(
            value["build"]["commit"],
            margins_cli::build_info::get().commit
        );
        assert_eq!(value["recall"]["indexing"], cfg!(feature = "recall"));
        assert_eq!(value["recall"]["lookup"], cfg!(feature = "recall"));
        assert!(value["recall"].get("scan").is_none());
        assert_eq!(value["workspace"]["preset"], cfg!(feature = "recall"));
        assert_eq!(value["workspace"]["program"], true);
        assert_eq!(
            value["recall"]["local_model"],
            cfg!(feature = "recall-local-model")
        );
        assert_eq!(value["distillation"]["workflow"], "connected_note");
        assert!(value["distillation"]["inputs"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("transcript")));
        assert!(value["catalyst"]["mode"].is_string());
        assert!(value["catalyst"]["usable"].is_boolean());
        assert!(value["catalyst"]["reason"].is_string());
        assert!(value["catalyst"]["expiry"]["state"].is_string());
        assert!(value["catalyst"]["expiry"]["bucket"].is_string());
        assert!(value["catalyst"].get("path").is_none());
        assert!(value["catalyst"].get("api_key").is_none());
        assert!(value["catalyst"].get("bootstrap_id").is_none());
        assert!(value["catalyst"].get("profile").is_none());
        assert!(value["catalyst"].get("expires_at").is_none());
    }

    #[test]
    fn transcribe_resolves_the_workspace_before_offering_the_speech_download() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let _restore = EnvRestore::capture(&["MARGINS_HOME", "MARGINS_WORKSPACE"]);
        let margins_home = temp.path().join("state");
        std::env::set_var("MARGINS_HOME", &margins_home);
        std::env::remove_var("MARGINS_WORKSPACE");
        let offers = AtomicUsize::new(0);
        let offer = || {
            offers.fetch_add(1, Ordering::SeqCst);
            Ok(())
        };

        // Remote transcription never offers this host's model.
        transcribe_preflight(true, Some("missing"), None, &folder, offer).unwrap();
        assert_eq!(offers.load(Ordering::SeqCst), 0);
        // A named Workspace that does not exist is refused first.
        assert!(transcribe_preflight(false, Some("missing"), None, &folder, offer).is_err());
        assert_eq!(offers.load(Ordering::SeqCst), 0);

        let error = transcribe_preflight(false, None, None, &folder, offer).unwrap_err();
        assert_eq!(error.code(), "workspace_required");
        assert_eq!(offers.load(Ordering::SeqCst), 0, "no download offer without a Workspace");

        margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &folder)
            .unwrap();
        transcribe_preflight(false, None, None, &folder, offer).unwrap();
        assert_eq!(offers.load(Ordering::SeqCst), 1);
    }
