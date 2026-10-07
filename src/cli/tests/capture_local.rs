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

    /// A temp `MARGINS_HOME` holding one Workspace for `notes`.
    fn workspace_home(
        root: &Path,
        notes: &Path,
    ) -> (EnvRestore, std::path::PathBuf, std::path::PathBuf) {
        let restore = EnvRestore::capture(&["MARGINS_HOME", "MARGINS_WORKSPACE"]);
        let margins_home = root.join("state");
        std::env::set_var("MARGINS_HOME", &margins_home);
        std::env::remove_var("MARGINS_WORKSPACE");
        std::fs::create_dir_all(notes).unwrap();
        let captures = margins_workflows::workspace::create_workspace(
            &margins_home,
            "practice",
            None,
            notes,
        )
        .unwrap()
        .capture_store_dir()
        .unwrap();
        (restore, margins_home, captures)
    }

    #[test]
    fn production_new_from_macos_launcher_temp_records_into_the_default_workspace() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let (_restore, margins_home, captures) =
            workspace_home(stable_parent.path(), &stable_parent.path().join("notes"));
        margins_workflows::workspace::set_default_workspace(&margins_home, "practice").unwrap();
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
        assert_capture_artifacts(&captures, true);
        assert_capture_artifacts(&active, false);
        assert_capture_artifacts(launcher.path(), false);
    }

    #[test]
    fn production_new_from_a_notes_subfolder_records_into_the_covering_workspace() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("obsidian");
        let inbox = vault.join("inbox");
        std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        let (_restore, _home, captures) = workspace_home(temp.path(), &vault);
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&inbox).unwrap();
        let code = main_entry_with(["margins", "new"], &ArtifactWritingInteractive);
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&captures, true);
        assert!(!vault.join(".margins").exists());
        assert!(!inbox.join(".margins").exists());
    }

    #[test]
    fn production_new_without_a_workspace_refuses_and_creates_nothing() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let _restore = EnvRestore::capture(&["MARGINS_HOME", "MARGINS_WORKSPACE"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("state"));
        std::env::remove_var("MARGINS_WORKSPACE");
        let spy = SpyInteractive(AtomicUsize::new(0));
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&folder).unwrap();
        let codes = [
            main_entry_with(["margins", "new"], &spy),
            main_entry_with(["margins"], &spy),
            main_entry_with(["margins", "attach"], &spy),
        ];
        std::env::set_current_dir(&old_cwd).unwrap();

        assert!(codes.iter().all(|code| *code != 0), "{codes:?}");
        assert_eq!(spy.0.load(Ordering::SeqCst), 0);
        assert!(!folder.join(".margins").exists());
        assert!(!temp.path().join("state/workspaces").exists());
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
    fn production_new_never_records_into_an_explicit_per_folder_store() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let (_restore, _home, captures) =
            workspace_home(stable_parent.path(), &stable_parent.path().join("notes"));
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

        assert_ne!(code, 0);
        assert_capture_artifacts(launcher.path(), false);
        assert_capture_artifacts(&active, false);
        assert_capture_artifacts(&captures, false);
    }

    #[test]
    fn production_new_is_routed_through_private_interactive_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let (_restore, _home, _captures) = workspace_home(temp.path(), temp.path());
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
        let (_restore, _home, captures) = workspace_home(temp.path(), &vault);
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(&vault).unwrap();

        let without_current = SpyInteractive(AtomicUsize::new(0));
        let first_code = main_entry_with(["margins"], &without_current);

        let margins_dir = captures.join(".margins");
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
