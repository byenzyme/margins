    #[test]
    fn post_capture_prompt_defaults_to_distill() {
        let mut input = "\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::Distill);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Turn this session into a note? [Y/n] "
        );
    }

    #[test]
    fn post_capture_prompt_accepts_saving_without_distilling() {
        let mut input = "no\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::SavedOnly);
    }

    #[test]
    fn post_capture_prompt_retries_an_invalid_answer() {
        let mut input = "later\ny\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::Distill);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Turn this session into a note? [Y/n] Please answer y or n.\n\
             Turn this session into a note? [Y/n] "
        );
    }

    #[test]
    fn post_capture_prompt_treats_eof_as_save_only() {
        let mut input = "".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::SavedOnly);
    }

    #[test]
    fn production_new_from_macos_launcher_temp_roots_all_capture_files_in_active_vault() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let launcher = tempfile::Builder::new()
            .prefix(".tmp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        seed_capture_root_settings(&active, launcher.path());
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(launcher.path()).unwrap();
        let code = main_entry_with(["margins", "new"], &ArtifactWritingInteractive);
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&active, true);
        assert_capture_artifacts(launcher.path(), false);
    }

    #[test]
    fn production_new_from_obsidian_inbox_uses_obsidian_root() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("obsidian");
        let inbox = vault.join("inbox");
        std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&inbox).unwrap();
        let code = main_entry_with(["margins", "new"], &ArtifactWritingInteractive);
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&vault, true);
        assert!(!inbox.join(".margins").exists());
    }

    #[test]
    fn production_new_with_workspace_ignores_cwd_and_legacy_project() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("state");
        let notes = temp.path().join("notes");
        let unrelated = temp.path().join("unrelated");
        std::fs::create_dir_all(&notes).unwrap();
        std::fs::create_dir_all(&unrelated).unwrap();
        let _restore = EnvRestore::capture(&["MARGINS_HOME", "MARGINS_WORKSPACE"]);
        std::env::set_var("MARGINS_HOME", &margins_home);
        std::env::remove_var("MARGINS_WORKSPACE");
        margins_workflows::workspace::create_workspace(&margins_home, "practice", None, &notes)
            .unwrap();
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&unrelated).unwrap();
        let code = main_entry_with(
            ["margins", "--workspace", "practice", "new"],
            &ArtifactWritingInteractive,
        );
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&margins_home.join("workspaces/practice/captures"), true);
        assert_capture_artifacts(&unrelated, false);
        assert_capture_artifacts(&notes, false);
    }

    #[test]
    fn production_new_allows_launcher_temp_only_when_explicitly_selected() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let launcher = tempfile::Builder::new()
            .prefix(".tmp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        seed_capture_root_settings(&active, launcher.path());
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(launcher.path()).unwrap();
        let code = main_entry_with(
            ["margins", "new", "--project=launcher-vault"],
            &ArtifactWritingInteractive,
        );
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(launcher.path(), true);
        assert_capture_artifacts(&active, false);
    }

    #[test]
    fn production_new_is_routed_through_private_interactive_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let spy = SpyInteractive(AtomicUsize::new(0));
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        let code = main_entry_with(["margins", "new"], &spy);
        std::env::set_current_dir(old).unwrap();
        assert_eq!(code, 0);
        assert_eq!(spy.0.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn bare_margins_creates_without_a_current_session_and_resumes_when_present() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&vault).unwrap();

        let without_current = SpyInteractive(AtomicUsize::new(0));
        let first_code = main_entry_with(["margins"], &without_current);

        let margins_dir = vault.join(".margins");
        std::fs::create_dir_all(&margins_dir).unwrap();
        margins_store::canonical::create_session(
            &margins_dir,
            "current-session",
            &chrono::Local::now(),
            ".margins/current-session.md",
        )
        .unwrap();
        std::fs::write(margins_dir.join("current"), "current-session\n").unwrap();

        let with_current = SpyInteractive(AtomicUsize::new(0));
        let second_code = main_entry_with(["margins"], &with_current);
        std::env::set_current_dir(old).unwrap();

        assert_eq!(first_code, 0);
        assert_eq!(without_current.0.load(Ordering::SeqCst), 1);
        assert_eq!(second_code, 0);
        assert_eq!(with_current.0.load(Ordering::SeqCst), 10);
    }
