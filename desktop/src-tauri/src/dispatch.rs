// ---------------------------------------------------------------------------
// dispatch — headless command router (WP1)
//
// Routes JSON payloads to the appropriate `_impl` function so that the axum
// HTTP server (WP2) can call every Tauri command without the Tauri runtime.
//
// Naming convention: the HTTP path / JSON `cmd` field uses the same snake_case
// name that the frontend passes to `invoke(...)`.  Tauri v2 converts camelCase
// JS keys to snake_case Rust params; the HTTP path will send the same JSON the
// frontend passes to invoke, so each arg struct uses
// `#[serde(rename_all = "camelCase")]` with `#[serde(alias = "...")]` for the
// snake_case variant so both forms are accepted.
// ---------------------------------------------------------------------------

use crate::ctx::Ctx;
use crate::web_session;
use serde::Deserialize;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Per-command argument structs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSettingsArgs {
    settings: crate::settings::Settings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateAudioSettingsArgs {
    audio_input_ready: bool,
    system_audio_ready: bool,
    input_device_mode: crate::settings::InputDeviceMode,
    input_device_uid: Option<String>,
    input_device_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewAiResolutionArgs {
    settings: crate::settings::Settings,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegisterProjectArgs {
    #[serde(alias = "_settings")]
    _settings: crate::settings::Settings,
    project: crate::settings::ProjectSource,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProjectReadinessArgs {
    id: String,
    readiness: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ValidateVaultArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexVaultArgs {
    path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetCalendarEventSuggestionArgs {
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSessionPeopleArgs {
    name: String,
    people: Vec<String>,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSessionTitleArgs {
    name: String,
    title: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GetProjectFilesFingerprintArgs {
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SurveyGranolaImportArgs {
    paths: Vec<String>,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportGranolaFilesArgs {
    paths: Vec<String>,
    options: crate::granola_import::GranolaImportOptions,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RevokeGranolaImportAuthorizationArgs {
    #[serde(default)]
    account: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrepareSpeechModelsArgs {
    #[serde(alias = "parakeet_model_dir")]
    parakeet_model_dir: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenPrivacyPaneArgs {
    pane: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionsArgs {
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteSessionArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartRecordingArgs {
    name: String,
    #[serde(default)]
    device_uid: Option<String>,
    #[serde(default, alias = "device_index")]
    device_index: Option<usize>,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionRecordingArgs {
    #[serde(default, alias = "recording_id")]
    recording_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SwitchRecordingDeviceArgs {
    #[serde(default)]
    device_uid: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SetLiveTranscriptionModeArgs {
    mode: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncMemoArgs {
    lines: Vec<crate::MemoLine>,
    session_name: String,
    #[serde(default, alias = "recording_id")]
    recording_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateWebRecordingNotepadArgs {
    recording_id: String,
    owner_id: String,
    expected_revision: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CheckpointMemoLineArgs {
    lines: Vec<crate::MemoLine>,
    #[serde(alias = "committed_index")]
    committed_index: usize,
    session_name: String,
    #[serde(default, alias = "recording_id")]
    recording_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimWebRecordingArgs {
    #[serde(alias = "recording_id")]
    recording_id: String,
    owner_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestBackchannelForMemoArgs {
    lines: Vec<crate::MemoLine>,
    #[serde(alias = "committed_index")]
    committed_index: usize,
    session_name: String,
    #[serde(default, alias = "recording_id")]
    recording_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
    client_sent_unix_ms: Option<u128>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteerBackchannelForMemoArgs {
    #[serde(alias = "memo_index")]
    memo_index: usize,
    steering: String,
    #[serde(alias = "previous_suggestion")]
    previous_suggestion: Option<String>,
    session_name: String,
    #[serde(default, alias = "recording_id")]
    recording_id: Option<String>,
    #[serde(default)]
    owner_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProcessSessionArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
    #[serde(alias = "overwrite_existing_note")]
    overwrite_existing_note: Option<bool>,
    #[serde(alias = "max_speakers")]
    max_speakers: Option<usize>,
    #[serde(alias = "force_transcribe")]
    force_transcribe: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RetrySessionArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefineSessionArgs {
    name: String,
    message: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReprocessSessionWithPeopleArgs {
    name: String,
    people: Vec<String>,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveDraftNoteArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
    #[serde(alias = "overwrite_existing_note")]
    overwrite_existing_note: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscardNoteArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelProcessSessionArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearSessionNoteErrorArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportAudioFileArgs {
    path: String,
    #[serde(alias = "max_speakers")]
    max_speakers: Option<usize>,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionNameProjectArgs {
    name: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenNoteTargetInObsidianArgs {
    target: String,
    #[serde(alias = "project_id")]
    project_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallAgentHooksArgs {
    dir: String,
    #[serde(default)]
    agents: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct TestAudioInputArgs {
    #[serde(default)]
    device_uid: Option<String>,
    #[serde(default, alias = "device_index")]
    device_index: Option<usize>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HydratePrepSketchArgs {
    lines: Vec<crate::MemoLine>,
    #[serde(alias = "session_name")]
    session_name: String,
    people: Vec<String>,
    #[serde(alias = "event_title")]
    event_title: Option<String>,
    #[serde(alias = "block_ordinal", default)]
    block_ordinal: u32,
    #[serde(alias = "pulled_texts", default)]
    pulled_texts: Vec<String>,
    #[serde(alias = "meeting_so_far")]
    meeting_so_far: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SteerPrepHydrationArgs {
    #[serde(alias = "session_name")]
    session_name: String,
    #[serde(alias = "block_ordinal", default)]
    block_ordinal: u32,
    instruction: String,
}

// ---------------------------------------------------------------------------
// dispatch
// ---------------------------------------------------------------------------

/// Route a command name and JSON payload to the appropriate `_impl` function.
///
/// This is the entry point for the axum HTTP server (WP2). The `command`
/// string is the snake_case name of the Tauri command. `args` is the JSON
/// object that the frontend would pass to `invoke()`.
pub(crate) async fn dispatch(ctx: &Ctx, command: &str, args: Value) -> Result<Value, String> {
    macro_rules! de {
        ($T:ty) => {
            serde_json::from_value::<$T>(args.clone())
                .map_err(|e| format!("bad args for {command}: {e}"))?
        };
    }
    macro_rules! ok {
        ($expr:expr) => {
            serde_json::to_value($expr).map_err(|e| format!("serialize error: {e}"))
        };
    }

    match command {
        // ---- Settings ----
        "get_settings" => ok!(crate::settings::hosted_settings(
            &ctx.state.settings.lock().unwrap()
        )),
        "update_settings" => {
            let a = de!(UpdateSettingsArgs);
            ok!(crate::update_hosted_settings_impl(ctx, a.settings).await?)
        }
        "update_audio_settings" => {
            let a = de!(UpdateAudioSettingsArgs);
            ok!(crate::update_audio_settings_impl(
                ctx,
                a.audio_input_ready,
                a.system_audio_ready,
                a.input_device_mode,
                a.input_device_uid,
                a.input_device_name,
            )
            .await?)
        }
        "register_project" => {
            let a = de!(RegisterProjectArgs);
            ok!(crate::register_project_impl(ctx, a._settings, a.project).await?)
        }
        "update_project_readiness" => {
            let a = de!(UpdateProjectReadinessArgs);
            ok!(crate::update_project_readiness_impl(ctx, a.id, a.readiness).await?)
        }
        // ---- AI / Auth ----
        "get_ai_status" => ok!(crate::get_ai_status_impl().await?),
        "get_included_ai_status" => ok!(crate::get_included_ai_status_impl()?),
        "prepare_included_ai" => ok!(crate::prepare_included_ai_impl().await?),
        "sign_in_chatgpt" => ok!(crate::sign_in_chatgpt_impl().await?),
        "get_ai_readiness" => ok!(crate::get_ai_readiness_impl().await?),
        "preview_ai_resolution" => {
            let a = de!(PreviewAiResolutionArgs);
            ok!(crate::preview_ai_resolution_impl(a.settings)?)
        }
        // ---- Vault ----
        "validate_vault" => {
            let a = de!(ValidateVaultArgs);
            ok!(crate::validate_vault_impl(a.path)?)
        }
        "index_vault" => {
            let a = de!(IndexVaultArgs);
            ok!(crate::index_vault_impl(a.path).await?)
        }
        // ---- Calendar ----
        "get_calendar_event_suggestion" => {
            let a = de!(GetCalendarEventSuggestionArgs);
            ok!(crate::get_calendar_event_suggestion_impl(ctx, a.project_id).await?)
        }
        // ---- Session metadata ----
        "update_session_people" => {
            let a = de!(UpdateSessionPeopleArgs);
            ok!(crate::update_session_people_impl(
                ctx,
                a.name,
                a.people,
                a.project_id
            )?)
        }
        "update_session_title" => {
            let a = de!(UpdateSessionTitleArgs);
            ok!(crate::update_session_title_impl(
                ctx,
                a.name,
                a.title,
                a.project_id
            )?)
        }
        "get_project_files_fingerprint" => {
            let a = de!(GetProjectFilesFingerprintArgs);
            ok!(crate::get_project_files_fingerprint_impl(ctx, a.project_id))
        }
        // ---- Granola ----
        "survey_granola_import" => {
            let a = de!(SurveyGranolaImportArgs);
            ok!(crate::survey_granola_import_impl(
                ctx,
                a.paths,
                a.project_id
            )?)
        }
        "import_granola_files" => {
            let a = de!(ImportGranolaFilesArgs);
            ok!(crate::import_granola_files_impl(
                ctx,
                a.paths,
                a.options,
                a.project_id
            )?)
        }
        "get_granola_import_status" => {
            ok!(crate::get_granola_import_status_impl(ctx))
        }
        "authorize_granola_import" => {
            ok!(crate::authorize_granola_import_impl(ctx).await?)
        }
        "revoke_granola_import_authorization" => {
            let a = de!(RevokeGranolaImportAuthorizationArgs);
            ok!(crate::revoke_granola_import_authorization_impl(
                ctx,
                a.account.as_deref()
            )?)
        }
        "import_granola_mcp" => {
            // import_granola_mcp emits Tauri events via AppHandle during import;
            // it cannot run headlessly in server mode.
            Err("import_granola_mcp is not available in headless server mode".to_string())
        }
        // ---- Speech models ----
        "prepare_speech_models" => {
            let a = de!(PrepareSpeechModelsArgs);
            ok!(crate::prepare_speech_models_impl(ctx, a.parakeet_model_dir).await?)
        }
        "probe_speech_models" => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct ProbeSpeechModelsArgs {
                #[serde(alias = "custom_path")]
                custom_path: Option<String>,
            }
            let a = de!(ProbeSpeechModelsArgs);
            ok!(crate::probe_speech_models_headless_impl(ctx, a.custom_path).await?)
        }
        "cancel_speech_model_download" => {
            ok!(crate::cancel_speech_model_download_impl(ctx)?)
        }
        "clear_speech_models" => {
            ok!(crate::clear_speech_models_impl(ctx).await?)
        }
        // ---- Devices ----
        "list_devices" => {
            if ctx.sink.is_web() {
                // No local audio devices in headless mode
                ok!(Vec::<crate::device_registry::DeviceInfo>::new())
            } else {
                ok!(crate::list_devices_impl(ctx))
            }
        }
        "test_audio_input" => {
            if ctx.sink.is_web() {
                Err("test_audio_input is not available in headless server mode".to_string())
            } else {
                let a = de!(TestAudioInputArgs);
                let _ = a.device_index;
                ok!(crate::test_audio_input_impl(ctx, a.device_uid)?)
            }
        }
        "test_system_audio_tap" => {
            if ctx.sink.is_web() {
                Err("test_system_audio_tap is not available in headless server mode".to_string())
            } else {
                ok!(crate::test_system_audio_tap_impl()?)
            }
        }
        "open_privacy_pane" => {
            if ctx.sink.is_web() {
                Err("open_privacy_pane is not available in headless server mode".to_string())
            } else {
                let a = de!(OpenPrivacyPaneArgs);
                ok!(crate::open_privacy_pane_impl(a.pane)?)
            }
        }
        // ---- App control (Tauri-only; stub in server mode) ----
        "restart_app" => Err("restart_app is not available in headless server mode".to_string()),
        "install_agent_hooks" => {
            let a = de!(InstallAgentHooksArgs);
            ok!(crate::agent_hooks::install_agent_hooks_impl(
                a.dir, a.agents
            )?)
        }
        "install_cli_tool" => {
            if ctx.sink.is_web() {
                Err("install_cli_tool is not available in headless server mode".to_string())
            } else {
                ok!(crate::install_cli_tool_impl()?)
            }
        }
        "ensure_cli_tools" => {
            // The impl can block on a release download; keep it off the async runtime.
            ok!(
                crate::async_runtime::spawn_blocking(crate::ensure_cli_tools_impl)
                    .await
                    .map_err(|e| format!("ensure_cli_tools task failed: {e}"))??
            )
        }
        // ---- Sessions ----
        "list_sessions" => {
            let a = de!(ListSessionsArgs);
            ok!(crate::list_sessions_impl(ctx, a.project_id)?)
        }
        "delete_session" => {
            let a = de!(DeleteSessionArgs);
            ok!(crate::delete_session_impl(ctx, a.name, a.project_id)?)
        }
        "reconcile_project_notes" => {
            let a = de!(ListSessionsArgs);
            ok!(crate::reconcile_project_notes_impl(ctx, a.project_id)?)
        }
        // ---- Recording ----
        "start_recording" => {
            let a = de!(StartRecordingArgs);
            let _ = a.device_index;
            if ctx.sink.is_web() {
                // Web mode: create a WebRecordingState (no local audio hardware).
                // Guard: block if a desktop capture is also in progress.
                {
                    let guard = ctx.state.recording.lock().unwrap();
                    if guard.is_some() {
                        return Err("A desktop recording is already active".into());
                    }
                }
                crate::validate_session_name(&a.name)?;
                // Hosted capture is already scoped by this margins-server
                // process. Its AppState root is authoritative; machine-global
                // desktop project preferences must never redirect browser
                // audio into a different vault.
                let work_dir = ctx.state.work_dir.lock().unwrap().clone();
                let margins_dir = work_dir.join(".margins");
                std::fs::create_dir_all(&margins_dir).map_err(|e| e.to_string())?;
                let name = crate::unique_session_name(&work_dir, &margins_dir, &a.name);
                let owner_id = a
                    .owner_id
                    .filter(|value| !value.trim().is_empty())
                    .ok_or("Hosted start_recording requires ownerId")?;
                ok!(web_session::start_web_recording(
                    &ctx.state, work_dir, name, owner_id
                )?)
            } else {
                ok!(crate::start_recording_impl(
                    ctx,
                    a.name,
                    a.device_uid,
                    a.project_id
                )?)
            }
        }
        "cancel_recording_startup" => {
            if ctx.sink.is_web() {
                ok!(false)
            } else {
                ok!(crate::cancel_recording_startup_impl(&ctx.state))
            }
        }
        "stop_recording" => {
            if ctx.sink.is_web() {
                let a = de!(SessionRecordingArgs);
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted stop_recording requires recordingId")?;
                ok!(web_session::stop_web_recording(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default()
                )?)
            } else {
                ok!(crate::stop_recording_impl(ctx).await?)
            }
        }
        "discard_recording" => {
            if ctx.sink.is_web() {
                let a = de!(SessionRecordingArgs);
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted discard_recording requires recordingId")?;
                ok!(web_session::discard_web_recording(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default()
                )?)
            } else {
                ok!(crate::discard_recording_impl(ctx).await?)
            }
        }
        "switch_recording_device" => {
            if ctx.sink.is_web() {
                Err("switch_recording_device is not available in headless server mode".to_string())
            } else {
                let a = de!(SwitchRecordingDeviceArgs);
                ok!(crate::switch_recording_device_impl(ctx, a.device_uid)?)
            }
        }
        "restart_system_audio_capture" => {
            if ctx.sink.is_web() {
                Err(
                    "restart_system_audio_capture is not available in headless server mode"
                        .to_string(),
                )
            } else {
                ok!(crate::restart_system_audio_capture_impl(ctx)?)
            }
        }
        "pause_recording" => {
            if ctx.sink.is_web() {
                let a = de!(SessionRecordingArgs);
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted pause_recording requires recordingId")?;
                ok!(web_session::set_web_recording_paused(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default(),
                    true
                )?)
            } else {
                ok!(crate::pause_recording_impl(ctx)?)
            }
        }
        "resume_recording" => {
            if ctx.sink.is_web() {
                let a = de!(SessionRecordingArgs);
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted resume_recording requires recordingId")?;
                ok!(web_session::set_web_recording_paused(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default(),
                    false
                )?)
            } else {
                ok!(crate::resume_recording_impl(ctx)?)
            }
        }
        "set_live_transcription_mode" => {
            if ctx.sink.is_web() {
                Err(
                    "set_live_transcription_mode is not available in headless server mode"
                        .to_string(),
                )
            } else {
                let a = de!(SetLiveTranscriptionModeArgs);
                ok!(crate::set_live_transcription_mode_impl(ctx, a.mode).await?)
            }
        }
        "get_recording_status" => {
            if ctx.sink.is_web() {
                if let Some(status) = web_session::active_web_recording_status(&ctx.state) {
                    ok!(status)
                } else {
                    // No active web session; report idle
                    ok!(crate::recording::idle_recording_status())
                }
            } else {
                ok!(crate::get_recording_status_impl(ctx))
            }
        }
        "get_hosted_capture_protocol" => {
            if !ctx.sink.is_web() {
                return Err("get_hosted_capture_protocol is only available in hosted mode".into());
            }
            ok!(serde_json::json!({
                "version": web_session::HOSTED_CAPTURE_PROTOCOL_VERSION,
                "minimumClientVersion": web_session::HOSTED_CAPTURE_PROTOCOL_VERSION,
            }))
        }
        "list_web_recording_recoveries" => {
            if !ctx.sink.is_web() {
                return Err(
                    "list_web_recording_recoveries is only available in hosted mode".into(),
                );
            }
            ok!(web_session::web_recording_recoveries(&ctx.state))
        }
        "get_web_recording_status" => {
            if !ctx.sink.is_web() {
                return Err("get_web_recording_status is only available in hosted mode".into());
            }
            let a = de!(SessionRecordingArgs);
            let recording_id = a
                .recording_id
                .as_deref()
                .ok_or("get_web_recording_status requires recordingId")?;
            ok!(
                web_session::web_recording_status_by_id(&ctx.state, recording_id)
                    .ok_or_else(|| format!("No hosted recording for ID '{recording_id}'"))?
            )
        }
        // ---- Memo / backchannel ----
        "sync_memo" => {
            let a = de!(SyncMemoArgs);
            if ctx.sink.is_web() {
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted sync_memo requires recordingId")?;
                web_session::sync_web_recording_memo(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default(),
                    a.lines,
                )?;
                ok!(())
            } else {
                ok!(crate::sync_memo_impl(ctx, a.lines, &a.session_name)?)
            }
        }
        "checkpoint_memo_line" => {
            let a = de!(CheckpointMemoLineArgs);
            if ctx.sink.is_web() {
                let recording_id = a
                    .recording_id
                    .as_deref()
                    .ok_or("Hosted checkpoint_memo_line requires recordingId")?;
                web_session::sync_web_recording_memo(
                    &ctx.state,
                    recording_id,
                    a.owner_id.as_deref().unwrap_or_default(),
                    a.lines.clone(),
                )?;
            }
            ok!(crate::checkpoint_memo_line_impl(
                ctx,
                a.lines,
                a.committed_index,
                &a.session_name,
                a.recording_id.as_deref()
            )?)
        }
        "heartbeat_web_recording" => {
            let a = de!(SessionRecordingArgs);
            if !ctx.sink.is_web() {
                return Err("heartbeat_web_recording is only available in hosted mode".into());
            }
            web_session::heartbeat_web_recording(
                &ctx.state,
                a.recording_id
                    .as_deref()
                    .ok_or("heartbeat_web_recording requires recordingId")?,
                a.owner_id.as_deref().unwrap_or_default(),
            )?;
            ok!(())
        }
        "claim_web_recording_recovery" => {
            let a = de!(ClaimWebRecordingArgs);
            if !ctx.sink.is_web() {
                return Err("claim_web_recording_recovery is only available in hosted mode".into());
            }
            web_session::claim_web_recording_recovery(&ctx.state, &a.recording_id, a.owner_id)?;
            ok!(())
        }
        "hydrate_web_recording_memo" => {
            let a = de!(SessionRecordingArgs);
            if !ctx.sink.is_web() {
                return Err("hydrate_web_recording_memo is only available in hosted mode".into());
            }
            ok!(web_session::hydrate_web_recording_memo(
                &ctx.state,
                a.recording_id
                    .as_deref()
                    .ok_or("hydrate_web_recording_memo requires recordingId")?,
                a.owner_id.as_deref(),
            )?)
        }
        "update_web_recording_notepad" => {
            if !ctx.sink.is_web() {
                return Err("update_web_recording_notepad is only available in hosted mode".into());
            }
            let a = de!(UpdateWebRecordingNotepadArgs);
            ok!(web_session::update_web_recording_notepad(
                &ctx.state,
                &a.recording_id,
                &a.owner_id,
                &a.expected_revision,
                &a.text,
            )?)
        }
        "get_web_recording_notepad" => {
            if !ctx.sink.is_web() {
                return Err("get_web_recording_notepad is only available in hosted mode".into());
            }
            let a = de!(SessionRecordingArgs);
            ok!(web_session::get_web_recording_notepad(
                &ctx.state,
                a.recording_id
                    .as_deref()
                    .ok_or("get_web_recording_notepad requires recordingId")?,
                a.owner_id.as_deref().unwrap_or_default(),
            )?)
        }
        "request_backchannel_for_memo" => {
            let a = de!(RequestBackchannelForMemoArgs);
            ok!(crate::request_backchannel_for_memo_impl(
                ctx,
                a.lines,
                a.committed_index,
                a.session_name,
                a.recording_id
                    .as_deref()
                    .ok_or("Hosted backchannel requires recordingId")?,
                a.owner_id.as_deref().unwrap_or_default(),
                a.client_sent_unix_ms
            )?)
        }
        "steer_backchannel_for_memo" => {
            let a = de!(SteerBackchannelForMemoArgs);
            ok!(crate::steer_backchannel_for_memo_impl(
                ctx,
                a.memo_index,
                a.steering,
                a.previous_suggestion,
                a.session_name,
                a.recording_id
                    .as_deref()
                    .ok_or("Hosted backchannel steering requires recordingId")?,
                a.owner_id.as_deref().unwrap_or_default()
            )?)
        }
        "hydrate_prep_sketch" => {
            let a = de!(HydratePrepSketchArgs);
            ok!(crate::hydrate_prep_sketch_impl(
                ctx,
                a.lines,
                a.session_name,
                a.people,
                a.event_title,
                a.block_ordinal,
                a.pulled_texts,
                a.meeting_so_far
            )?)
        }
        "steer_prep_hydration" => {
            let a = de!(SteerPrepHydrationArgs);
            ok!(crate::steer_prep_hydration_impl(
                ctx,
                a.session_name,
                a.block_ordinal,
                a.instruction
            )?)
        }
        // ---- Processing pipeline ----
        "process_session" => {
            let a = de!(ProcessSessionArgs);
            ok!(crate::process_session_impl(
                ctx,
                a.name,
                a.project_id,
                a.overwrite_existing_note,
                a.max_speakers,
                a.force_transcribe
            )
            .await?)
        }
        "retry_session" => {
            let a = de!(RetrySessionArgs);
            ok!(crate::retry_session_impl(ctx, a.name, a.project_id).await?)
        }
        "refine_session" => {
            let a = de!(RefineSessionArgs);
            ok!(crate::refine_session_impl(ctx, a.name, a.message, a.project_id).await?)
        }
        "reprocess_session_with_people" => {
            let a = de!(ReprocessSessionWithPeopleArgs);
            ok!(
                crate::reprocess_session_with_people_impl(ctx, a.name, a.people, a.project_id)
                    .await?
            )
        }
        "save_draft_note" => {
            let a = de!(SaveDraftNoteArgs);
            ok!(crate::save_draft_note_impl(
                ctx,
                a.name,
                a.project_id,
                a.overwrite_existing_note
            )?)
        }
        "discard_note" => {
            let a = de!(DiscardNoteArgs);
            ok!(crate::discard_note_impl(ctx, a.name, a.project_id)?)
        }
        "cancel_process_session" => {
            let a = de!(CancelProcessSessionArgs);
            ok!(crate::cancel_process_session_impl(
                ctx,
                a.name,
                a.project_id
            )?)
        }
        "clear_session_note_error" => {
            let a = de!(ClearSessionNoteErrorArgs);
            ok!(crate::clear_session_note_error_impl(
                ctx,
                a.name,
                a.project_id
            )?)
        }
        "import_transcript" => {
            let a = de!(crate::ImportTranscriptArgs);
            ok!(crate::import_transcript_impl(ctx, a)?)
        }
        "import_audio_file" => {
            let a = de!(ImportAudioFileArgs);
            ok!(crate::import_audio_file_impl(ctx, a.path, a.max_speakers, a.project_id).await?)
        }
        // ---- Read/view ----
        "get_aligned_content" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::get_aligned_content_impl(ctx, a.name, a.project_id)?)
        }
        "get_session_memo" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::get_session_memo_impl(ctx, a.name, a.project_id)?)
        }
        "get_vault_note" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::get_vault_note_impl(ctx, a.name, a.project_id)?)
        }
        "get_session_grounding" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::get_session_grounding_impl(
                ctx,
                a.name,
                a.project_id
            )?)
        }
        "get_distill_trace" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::get_distill_trace_impl(ctx, a.name, a.project_id)?)
        }
        // ---- Open / Obsidian (Tauri-only; stub in server mode) ----
        "open_note" => {
            if ctx.sink.is_web() {
                Err("open_note is not available in headless server mode".to_string())
            } else {
                let a = de!(SessionNameProjectArgs);
                ok!(crate::open_note_impl(ctx, a.name, a.project_id)?)
            }
        }
        "open_note_in_obsidian" => {
            let a = de!(SessionNameProjectArgs);
            ok!(crate::open_note_in_obsidian_impl(
                ctx,
                a.name,
                a.project_id
            )?)
        }
        "open_note_target_in_obsidian" => {
            let a = de!(OpenNoteTargetInObsidianArgs);
            ok!(crate::open_note_target_in_obsidian_impl(
                ctx,
                a.target,
                a.project_id
            )?)
        }
        // ---- Capabilities ----
        "get_capabilities" => ok!(crate::get_capabilities_impl(ctx)),
        _ => Err(format!("Unknown command: {command}")),
    }
}
