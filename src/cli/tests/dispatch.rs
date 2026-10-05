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
        assert_eq!(value["recall"]["scan"], cfg!(feature = "recall"));
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
