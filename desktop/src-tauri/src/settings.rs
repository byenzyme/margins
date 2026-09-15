use serde::{Deserialize, Serialize};
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Mutex, OnceLock};

static KEYCHAIN_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Persisted microphone-selection policy. `Missing` is an internal serde
/// migration sentinel and is normalized immediately after settings load.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InputDeviceMode {
    FollowDefault,
    Pinned,
    #[serde(skip)]
    #[default]
    Missing,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProjectSource {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(default = "default_inbox_folder")]
    pub(crate) inbox_folder: String,
    #[serde(default = "default_people_folder")]
    pub(crate) people_folder: String,
    #[serde(default = "default_project_readiness")]
    pub(crate) readiness: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Settings {
    pub(crate) vault_path: Option<String>,
    #[serde(default)]
    pub(crate) projects: Vec<ProjectSource>,
    #[serde(default)]
    pub(crate) active_project_id: Option<String>,
    #[serde(default)]
    pub(crate) ai_mode: Option<String>,
    #[serde(default)]
    pub(crate) ai_base_url: Option<String>,
    #[serde(default)]
    pub(crate) ai_model: Option<String>,
    #[serde(default)]
    pub(crate) chatgpt_model: Option<String>,
    #[serde(default)]
    pub(crate) ai_models_namespaced: bool,
    pub(crate) api_key: Option<String>,
    #[serde(default)]
    pub(crate) backchannel_base_url: Option<String>,
    #[serde(default)]
    pub(crate) backchannel_model: Option<String>,
    #[serde(default)]
    pub(crate) backchannel_api_key: Option<String>,
    /// When `Some(true)` or absent, live cues reuse the distillation model.
    /// `Some(false)` opts into a separate, faster cue model via the backchannel
    /// fields. Absent (legacy settings) defaults to "same as distillation".
    #[serde(default)]
    pub(crate) backchannel_same_as_distill: Option<bool>,
    pub(crate) cleanup_policy: String,
    #[serde(default)]
    pub(crate) input_device_uid: Option<String>,
    pub(crate) input_device_name: Option<String>,
    #[serde(default)]
    pub(crate) input_device_mode: InputDeviceMode,
    #[serde(default)]
    pub(crate) audio_input_ready: bool,
    #[serde(default)]
    pub(crate) system_audio_ready: bool,
    #[serde(default)]
    pub(crate) editor_command: Option<String>,
    pub(crate) parakeet_model_dir: Option<String>,
    #[serde(default)]
    pub(crate) rust_diarization_enabled: bool,
    #[serde(default)]
    pub(crate) import_speaker_count: u8,
    #[serde(default = "default_inbox_folder")]
    pub(crate) inbox_folder: String,
    #[serde(default = "default_people_folder")]
    pub(crate) people_folder: String,
    #[serde(default = "default_created_date_format")]
    pub(crate) created_date_format: String,
    #[serde(default = "default_sidebar_date_format")]
    pub(crate) sidebar_date_format: String,
    #[serde(default = "default_note_filename_template")]
    pub(crate) note_filename_template: String,
    #[serde(default = "default_person_note_template")]
    pub(crate) person_note_template: String,
    #[serde(default = "default_distill_instructions")]
    pub(crate) distill_instructions: String,
    /// When true (default), auto-start countdown fires at the calendar event time.
    /// When false, the user must start manually.
    #[serde(default = "default_true")]
    pub(crate) auto_start_from_calendar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            vault_path: Some(default_vault_path_string()),
            projects: vec![default_project_source(
                &default_vault_path_string(),
                &default_inbox_folder(),
                &default_people_folder(),
            )],
            active_project_id: Some(project_id_from_path(&default_vault_path_string())),
            ai_mode: Some("included".to_string()),
            ai_base_url: None,
            ai_model: None,
            chatgpt_model: None,
            ai_models_namespaced: true,
            api_key: None,
            backchannel_base_url: None,
            backchannel_model: None,
            backchannel_api_key: None,
            backchannel_same_as_distill: Some(true),
            cleanup_policy: "immediate".to_string(),
            input_device_uid: None,
            input_device_name: None,
            input_device_mode: InputDeviceMode::FollowDefault,
            audio_input_ready: false,
            system_audio_ready: false,
            editor_command: None,
            parakeet_model_dir: None,
            rust_diarization_enabled: false,
            import_speaker_count: 1,
            inbox_folder: default_inbox_folder(),
            people_folder: default_people_folder(),
            created_date_format: default_created_date_format(),
            sidebar_date_format: default_sidebar_date_format(),
            note_filename_template: default_note_filename_template(),
            person_note_template: default_person_note_template(),
            distill_instructions: default_distill_instructions(),
            auto_start_from_calendar: true,
        }
    }
}

pub(crate) fn migrate_input_device_uid(
    settings: &mut Settings,
    devices: &[crate::device_registry::DeviceInfo],
) -> bool {
    if settings.input_device_uid.is_some() {
        return false;
    }
    let Some(name) = settings.input_device_name.as_deref() else {
        return false;
    };
    let Some(uid) = crate::device_registry::legacy_uid_for_name(devices, name) else {
        return false;
    };
    settings.input_device_uid = Some(uid);
    settings.input_device_mode = InputDeviceMode::Pinned;
    true
}

pub(crate) fn profile_name() -> String {
    std::env::var("MARGINS_PROFILE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "default")
        .or_else(first_run_bundle_profile)
        .unwrap_or_else(|| "default".to_string())
}

fn first_run_bundle_profile() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.to_string_lossy();
    path.contains("/Margins First Run.app/")
        .then(|| "first-run-test".to_string())
}

fn profile_slug() -> String {
    let profile = profile_name();
    if profile == "default" {
        return "margins".to_string();
    }
    let slug = profile
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    format!(
        "margins-{}",
        if slug.is_empty() { "profile" } else { &slug }
    )
}

pub(crate) fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(profile_slug())
        .join("settings.json")
}

pub(crate) fn app_data_dir() -> PathBuf {
    dirs::data_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(profile_slug())
}

pub(crate) fn keychain_service(scope: &str) -> String {
    if profile_name() == "default" {
        format!("margins.{scope}")
    } else {
        format!("{}.{scope}", profile_slug())
    }
}

fn default_pi_agent_dir() -> PathBuf {
    app_data_dir().join("pi-agent")
}

pub(crate) fn pi_agent_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MARGINS_PI_AGENT_DIR") {
        return PathBuf::from(dir);
    }
    if std::env::var_os("MARGINS_USE_PI_CODING_AGENT_DIR").is_some() {
        if let Some(dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
            return PathBuf::from(dir);
        }
    }
    default_pi_agent_dir()
}

pub(crate) fn pi_auth_path() -> PathBuf {
    pi_agent_dir().join("auth.json")
}

pub(crate) fn configure_pi_agent_dir() {
    let dir = pi_agent_dir();
    let _ = std::fs::create_dir_all(&dir);
    // Margins owns its embedded Pi runtime auth store. Do not inherit a caller's
    // global PI_CODING_AGENT_DIR unless explicitly opted in above; Rust Pi auth
    // uses a different file-lock/schema than the global JS Pi CLI.
    std::env::set_var("PI_CODING_AGENT_DIR", &dir);
}

pub(crate) fn load_settings() -> Settings {
    let mut settings: Settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default();
    normalize_settings(&mut settings);
    apply_ai_env_defaults(&mut settings);
    settings
}

/// Tri-state result for the settings watcher: distinguishes a missing file
/// (normal on first run) from a transient parse failure (truncated mid-write).
pub(crate) enum LoadSettingsResult {
    /// File was read and parsed successfully.
    Ok(Settings),
    /// File does not exist yet — treat like a fresh install.
    Missing,
    /// File exists but could not be parsed (e.g. truncated during a write).
    /// The watcher should skip overwriting in-memory AppState in this case.
    ParseError,
}

/// Like `load_settings` but returns a `LoadSettingsResult` so the caller can
/// distinguish a transient parse failure from a legitimately missing file.
/// Used by the settings-file watcher to avoid clobbering live AppState with
/// `Settings::default()` when the file is momentarily unreadable.
pub(crate) fn try_load_settings() -> LoadSettingsResult {
    let path = settings_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadSettingsResult::Missing,
        Err(_) => return LoadSettingsResult::ParseError,
    };
    match serde_json::from_str::<Settings>(&text) {
        Ok(mut settings) => {
            normalize_settings(&mut settings);
            apply_ai_env_defaults(&mut settings);
            LoadSettingsResult::Ok(settings)
        }
        Err(_) => LoadSettingsResult::ParseError,
    }
}

/// Load and normalize settings from an arbitrary path. Used in tests and by
/// the settings-file watcher path to re-read after an external write.
#[cfg(test)]
pub(crate) fn load_settings_from(path: &std::path::Path) -> Settings {
    let mut settings: Settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default();
    normalize_settings(&mut settings);
    settings
}

fn apply_ai_env_defaults(settings: &mut Settings) {
    let env = crate::ai_config::load_openai_env_config();
    if clean_optional(settings.api_key.as_deref()).is_none() {
        settings.api_key = keychain_get_service(&keychain_service("note-ai"), "api-key")
            .or_else(|| env.api_key.clone());
    }
    if clean_optional(settings.ai_base_url.as_deref()).is_none() {
        settings.ai_base_url = env.base_url.clone();
    }
    let chatgpt_mode = clean_optional(settings.ai_mode.as_deref())
        .is_some_and(|mode| matches!(mode.to_ascii_lowercase().as_str(), "chatgpt" | "codex"));
    if !chatgpt_mode && clean_optional(settings.ai_model.as_deref()).is_none() {
        settings.ai_model = env.model.clone();
    }
    if clean_optional(settings.backchannel_api_key.as_deref()).is_none() {
        settings.backchannel_api_key =
            keychain_get_service(&keychain_service("backchannel"), "api-key").or(env.api_key);
    }
    if clean_optional(settings.backchannel_base_url.as_deref()).is_none() {
        settings.backchannel_base_url = env.base_url;
    }
    if clean_optional(settings.backchannel_model.as_deref()).is_none() {
        settings.backchannel_model = env.model;
    }
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

pub(crate) fn normalize_settings(settings: &mut Settings) {
    if settings.input_device_mode == InputDeviceMode::Missing {
        settings.input_device_mode = if settings.input_device_uid.is_some() {
            InputDeviceMode::Pinned
        } else {
            InputDeviceMode::FollowDefault
        };
    }
    migrate_ai_model_namespaces(settings);
    if settings
        .vault_path
        .as_deref()
        .map(str::trim)
        .unwrap_or_default()
        .is_empty()
    {
        settings.vault_path = Some(default_vault_path_string());
    }
    if settings.projects.is_empty() {
        let root = settings
            .vault_path
            .clone()
            .unwrap_or_else(default_vault_path_string);
        settings.projects.push(default_project_source(
            &root,
            &settings.inbox_folder,
            &settings.people_folder,
        ));
    }
    for project in &mut settings.projects {
        if project.path.trim().is_empty() {
            project.path = default_vault_path_string();
        }
        if project.id.trim().is_empty() {
            project.id = project_id_from_path(&project.path);
        }
        if project.name.trim().is_empty() {
            project.name = project_name_from_path(&project.path);
        }
        if project.people_folder.trim().is_empty() {
            project.people_folder = default_people_folder();
        }
        if project.readiness.trim().is_empty() {
            project.readiness = default_project_readiness();
        }
    }
    let active_exists = settings
        .active_project_id
        .as_deref()
        .map(|id| settings.projects.iter().any(|project| project.id == id))
        .unwrap_or(false);
    if !active_exists {
        settings.active_project_id = settings.projects.first().map(|project| project.id.clone());
    }
    if let Some((path, inbox_folder, people_folder)) = active_project(settings).map(|project| {
        (
            project.path.clone(),
            project.inbox_folder.clone(),
            project.people_folder.clone(),
        )
    }) {
        settings.vault_path = Some(path);
        settings.inbox_folder = inbox_folder;
        settings.people_folder = people_folder;
    }
    if settings.people_folder.trim().is_empty() {
        settings.people_folder = default_people_folder();
    }
    if settings.created_date_format.trim().is_empty() {
        settings.created_date_format = default_created_date_format();
    }
    if settings.note_filename_template.trim().is_empty() {
        settings.note_filename_template = default_note_filename_template();
    }
    if settings.person_note_template.trim().is_empty() {
        settings.person_note_template = default_person_note_template();
    }
    if settings.distill_instructions.trim().is_empty() {
        settings.distill_instructions = default_distill_instructions();
    }
}

fn migrate_ai_model_namespaces(settings: &mut Settings) {
    let chatgpt_mode = clean_optional(settings.ai_mode.as_deref())
        .map(|mode| matches!(mode.to_ascii_lowercase().as_str(), "chatgpt" | "codex"))
        .unwrap_or(false);
    if !settings.ai_models_namespaced && chatgpt_mode {
        if clean_optional(settings.chatgpt_model.as_deref()).is_none() {
            settings.chatgpt_model = sanitize_chatgpt_model(settings.ai_model.as_deref());
        }
        settings.ai_model = None;
    }
    if chatgpt_mode {
        settings.chatgpt_model = sanitize_chatgpt_model(settings.chatgpt_model.as_deref());
    }
    settings.ai_models_namespaced = true;
}

fn sanitize_chatgpt_model(model: Option<&str>) -> Option<String> {
    clean_optional(model).filter(|model| {
        !model.contains('/')
            && model
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || ".-_".contains(character))
    })
}

pub(crate) fn active_project(settings: &Settings) -> Option<&ProjectSource> {
    settings
        .active_project_id
        .as_deref()
        .and_then(|id| settings.projects.iter().find(|project| project.id == id))
        .or_else(|| settings.projects.first())
}

pub(crate) fn save_settings(settings: &Settings) -> Result<(), String> {
    persist_note_ai_secrets(settings)?;
    persist_backchannel_secrets(settings)?;
    save_settings_document(settings)
}

/// Persist the non-secret settings document without touching Keychain.
///
/// Narrow runtime updates (for example, recording a completed audio readiness
/// check) use this path so an unrelated preference cannot rewrite every saved
/// credential or fail because Keychain is temporarily unavailable.
pub(crate) fn save_settings_document(settings: &Settings) -> Result<(), String> {
    let mut persisted = settings.clone();
    strip_ephemeral_env_defaults(&mut persisted);
    persisted.api_key = None;
    persisted.backchannel_api_key = None;
    let path = settings_path();
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    // Atomic tmp + rename so the settings watcher never observes a truncated file
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&persisted).unwrap())
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn strip_ephemeral_env_defaults(persisted: &mut Settings) {
    let existing: Settings = std::fs::read_to_string(settings_path())
        .ok()
        .and_then(|d| serde_json::from_str(&d).ok())
        .unwrap_or_default();
    let env = crate::ai_config::load_openai_env_config();
    // A separate cue route is active when the user opted out of "same as
    // note-making" and supplied either a dedicated cue key OR a model-only
    // override (reusing the note-making key with a different model). In both
    // cases keep the saved base URL/model instead of stripping them as env
    // defaults.
    let preserve_backchannel_routing = persisted.backchannel_same_as_distill == Some(false)
        && (clean_optional(persisted.backchannel_api_key.as_deref()).is_some()
            || clean_optional(persisted.backchannel_model.as_deref()).is_some());

    if preserve_backchannel_routing {
        return;
    }

    if should_strip_env_default(
        &existing.backchannel_base_url,
        &persisted.backchannel_base_url,
        &env.base_url,
    ) {
        persisted.backchannel_base_url = None;
    }
    if should_strip_env_default(
        &existing.backchannel_model,
        &persisted.backchannel_model,
        &env.model,
    ) {
        persisted.backchannel_model = None;
    }
}

fn persist_note_ai_secrets(settings: &Settings) -> Result<(), String> {
    persist_api_key_secret("note-ai", settings.api_key.as_deref())
}

fn persist_backchannel_secrets(settings: &Settings) -> Result<(), String> {
    persist_api_key_secret("backchannel", settings.backchannel_api_key.as_deref())
}

fn persist_api_key_secret(scope: &str, value: Option<&str>) -> Result<(), String> {
    let service = keychain_service(scope);
    if let Some(key) = clean_optional(value) {
        // If the field was auto-populated from process env or packaged .env,
        // clicking Save promotes it into macOS Keychain so dev and installed
        // builds have a stable shared secret store without re-packaging .env.
        keychain_set_service(&service, "api-key", &key)?;
    } else {
        keychain_delete_service(&service, "api-key")?;
    }
    Ok(())
}

fn should_strip_env_default(
    existing: &Option<String>,
    incoming: &Option<String>,
    env: &Option<String>,
) -> bool {
    clean_optional(existing.as_deref()).is_none()
        && clean_optional(incoming.as_deref()) == clean_optional(env.as_deref())
}

pub(crate) fn keychain_set_service(
    service: &str,
    account: &str,
    value: &str,
) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let _guard = KEYCHAIN_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "macOS Keychain write lock was poisoned".to_string())?;
    // `security add-generic-password -U` can still attempt a duplicate-item
    // update even when the value is unchanged. Besides being unnecessary, a
    // burst of those updates from settings saves can trip Keychain's unique
    // index handling. Compare under the same process-wide lock and skip it.
    if keychain_get_service(service, account).as_deref() == Some(value) {
        return Ok(());
    }
    let status = keychain_command_status(
        Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                service,
                "-a",
                account,
                "-w",
                value,
            ])
            .status(),
        "write",
        account,
    )?;
    let Some(status) = status else {
        return Ok(());
    };
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "security add-generic-password failed for {account}"
        ))
    }
}

pub(crate) fn keychain_get_service(service: &str, account: &str) -> Option<String> {
    if keychain_disabled() {
        return None;
    }
    let output = Command::new("security")
        .args(["find-generic-password", "-s", service, "-a", account, "-w"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string()).filter(|s| !s.is_empty())
}

pub(crate) fn keychain_delete_service(service: &str, account: &str) -> Result<(), String> {
    if keychain_disabled() {
        return Ok(());
    }
    let _guard = KEYCHAIN_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "macOS Keychain write lock was poisoned".to_string())?;
    let status = keychain_command_status(
        Command::new("security")
            .args(["delete-generic-password", "-s", service, "-a", account])
            .status(),
        "delete",
        account,
    )?;
    // Treat missing items as success. The `security` tool does not distinguish
    // that case cleanly via exit status, and delete should be idempotent here.
    let _ = status;
    Ok(())
}

fn keychain_command_status(
    result: io::Result<std::process::ExitStatus>,
    operation: &str,
    account: &str,
) -> Result<Option<std::process::ExitStatus>, String> {
    match result {
        Ok(status) => Ok(Some(status)),
        // `security` is a macOS-only tool. A hosted/non-macOS process has no
        // Keychain to update, so a missing executable is the same unavailable
        // capability already represented by `keychain_disabled()`.
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "failed to {operation} macOS Keychain item {account}: {error}"
        )),
    }
}

fn keychain_disabled() -> bool {
    keychain_disabled_for_platform(
        cfg!(target_os = "macos"),
        std::env::var("MARGINS_DISABLE_KEYCHAIN").ok().as_deref(),
    )
}

fn keychain_disabled_for_platform(is_macos: bool, configured: Option<&str>) -> bool {
    !is_macos
        || configured.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

pub(crate) fn default_work_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("margins-sessions")
}

pub(crate) fn default_vault_path_string() -> String {
    "~/Documents/margins".to_string()
}

fn default_project_readiness() -> String {
    "needs_setup".to_string()
}

fn project_id_from_path(path: &str) -> String {
    let mut out = String::new();
    for ch in path.trim_start_matches("~/").chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 42 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "project".to_string()
    } else {
        out
    }
}

fn project_name_from_path(path: &str) -> String {
    let clean = path.trim().trim_end_matches('/');
    let leaf = clean
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("Project");
    leaf.chars()
        .map(|ch| if ch == '-' || ch == '_' { ' ' } else { ch })
        .collect::<String>()
        .split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn default_project_source(path: &str, inbox_folder: &str, people_folder: &str) -> ProjectSource {
    ProjectSource {
        id: project_id_from_path(path),
        name: project_name_from_path(path),
        path: path.to_string(),
        inbox_folder: inbox_folder.to_string(),
        people_folder: if people_folder.trim().is_empty() {
            default_people_folder()
        } else {
            people_folder.to_string()
        },
        readiness: default_project_readiness(),
    }
}

fn default_inbox_folder() -> String {
    "meetings".to_string()
}
fn default_people_folder() -> String {
    "people".to_string()
}
fn default_created_date_format() -> String {
    "[[%Y-%m-%d]]".to_string()
}
fn default_sidebar_date_format() -> String {
    "compact".to_string()
}
fn default_note_filename_template() -> String {
    "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}".to_string()
}
fn default_person_note_template() -> String {
    "# {{name}}\n".to_string()
}
fn default_true() -> bool {
    true
}

fn default_distill_instructions() -> String {
    r#"Final Markdown note instructions:
- Save meeting notes in the configured meeting notes folder.
- Use the timestamped filename template; include the calendar event title when available.
- Preserve Markdown frontmatter. Always include `created` with the configured date format.
- Put attendees / people pills in frontmatter as wikilinks under `people:`.
- Create a people note for each attendee if one does not already exist.
- Match the frontmatter key set of existing vault notes; only fall back to created/tags/people when no vault note exists.
- Collect all action items into one `### Action items` checklist at the end."#
        .to_string()
}

pub(crate) fn redacted_settings(settings: &Settings) -> Settings {
    settings.clone()
}

/// Hosted settings are transported to an untrusted browser runtime. Secrets
/// are supplied to the server process through its deployment environment and
/// must never be returned to, or accepted from, this document endpoint.
pub(crate) fn hosted_settings(settings: &Settings) -> Settings {
    let mut out = redacted_settings(settings);
    out.api_key = None;
    out.backchannel_api_key = None;
    out
}

pub(crate) fn reject_hosted_settings_secrets(settings: &Settings) -> Result<(), String> {
    let supplied = [
        ("api_key", settings.api_key.as_deref()),
        (
            "backchannel_api_key",
            settings.backchannel_api_key.as_deref(),
        ),
    ]
    .into_iter()
    .find_map(|(name, value)| clean_optional(value).map(|_| name));

    match supplied {
        Some(name) => Err(format!(
            "Hosted credentials are managed by the server environment; remove {name} from this settings request and configure it on the server."
        )),
        None => Ok(()),
    }
}

impl Settings {
    pub(crate) fn import_speaker_count_opt(&self) -> Option<usize> {
        Some(self.import_speaker_count.max(1) as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_is_always_disabled_off_macos() {
        assert!(keychain_disabled_for_platform(false, None));
        assert!(keychain_disabled_for_platform(false, Some("false")));
    }

    #[test]
    fn macos_keychain_can_be_explicitly_disabled() {
        assert!(!keychain_disabled_for_platform(true, None));
        assert!(!keychain_disabled_for_platform(true, Some("false")));
        assert!(keychain_disabled_for_platform(true, Some("true")));
        assert!(keychain_disabled_for_platform(true, Some("1")));
    }

    #[test]
    fn missing_security_tool_is_an_unavailable_keychain() {
        let result = keychain_command_status(
            Err(io::Error::from(io::ErrorKind::NotFound)),
            "delete",
            "api-key",
        );

        assert!(matches!(result, Ok(None)));
    }

    #[test]
    fn other_keychain_spawn_failures_remain_errors() {
        let result = keychain_command_status(
            Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            "write",
            "api-key",
        );

        assert!(result
            .unwrap_err()
            .contains("failed to write macOS Keychain"));
    }

    #[test]
    fn normalizes_required_note_settings() {
        let mut settings = Settings::default();
        settings.vault_path = Some("  ".to_string());
        settings.inbox_folder.clear();
        settings.people_folder.clear();
        settings.created_date_format.clear();
        settings.note_filename_template.clear();
        settings.person_note_template.clear();
        settings.distill_instructions.clear();

        normalize_settings(&mut settings);

        assert_eq!(settings.vault_path.as_deref(), Some("~/Documents/margins"));
        assert_eq!(settings.inbox_folder, "meetings");
        assert_eq!(settings.people_folder, "people");
        assert!(!settings.created_date_format.is_empty());
        assert!(!settings.note_filename_template.is_empty());
        assert!(!settings.person_note_template.is_empty());
        assert!(!settings.distill_instructions.is_empty());
    }

    #[test]
    fn migrates_legacy_chatgpt_model_out_of_api_namespace() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        let legacy = legacy.as_object_mut().unwrap();
        legacy.remove("ai_models_namespaced");
        legacy.insert("ai_mode".to_string(), serde_json::json!("chatgpt"));
        legacy.insert("ai_model".to_string(), serde_json::json!("gpt-5.1-codex"));
        let mut settings: Settings =
            serde_json::from_value(serde_json::Value::Object(legacy.clone())).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.chatgpt_model.as_deref(), Some("gpt-5.1-codex"));
        assert_eq!(settings.ai_model, None);
        assert!(settings.ai_models_namespaced);
    }

    #[test]
    fn legacy_chatgpt_migration_rejects_foreign_provider_slug() {
        let mut legacy = serde_json::to_value(Settings::default()).unwrap();
        let legacy = legacy.as_object_mut().unwrap();
        legacy.remove("ai_models_namespaced");
        legacy.insert("ai_mode".to_string(), serde_json::json!("codex"));
        legacy.insert(
            "ai_model".to_string(),
            serde_json::json!("anthropic/claude-sonnet-4.6"),
        );
        let mut settings: Settings =
            serde_json::from_value(serde_json::Value::Object(legacy.clone())).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.chatgpt_model, None);
        assert_eq!(settings.ai_model, None);
        assert!(settings.ai_models_namespaced);
    }

    #[test]
    fn namespaced_models_survive_mode_switches_independently() {
        let mut settings = Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.ai_model = Some("anthropic/claude-sonnet-4.6".to_string());
        settings.chatgpt_model = Some("gpt-5.1-codex".to_string());

        normalize_settings(&mut settings);

        assert_eq!(
            settings.ai_model.as_deref(),
            Some("anthropic/claude-sonnet-4.6")
        );
        assert_eq!(settings.chatgpt_model.as_deref(), Some("gpt-5.1-codex"));
    }

    #[test]
    fn defaults_new_projects_to_meetings_folder() {
        let settings = Settings::default();

        assert_eq!(settings.inbox_folder, "meetings");
        assert_eq!(settings.projects[0].inbox_folder, "meetings");
    }

    #[test]
    fn migrates_legacy_microphone_name_when_registry_contains_it() {
        let mut settings = Settings::default();
        settings.input_device_name = Some("Yeti".to_string());
        let devices = vec![crate::device_registry::DeviceInfo {
            uid: "coreaudio-yeti".to_string(),
            name: "Yeti".to_string(),
            is_default: false,
            sample_rate: Some(48_000),
        }];

        assert!(migrate_input_device_uid(&mut settings, &devices));
        assert_eq!(settings.input_device_uid.as_deref(), Some("coreaudio-yeti"));
        assert_eq!(settings.input_device_name.as_deref(), Some("Yeti"));
        assert_eq!(settings.input_device_mode, InputDeviceMode::Pinned);
        assert!(!migrate_input_device_uid(&mut settings, &devices));
    }

    #[test]
    fn leaves_legacy_microphone_name_until_device_returns() {
        let mut settings = Settings::default();
        settings.input_device_name = Some("Travel Mic".to_string());

        assert!(!migrate_input_device_uid(&mut settings, &[]));
        assert_eq!(settings.input_device_uid, None);
        assert_eq!(settings.input_device_name.as_deref(), Some("Travel Mic"));
    }

    #[test]
    fn missing_input_device_mode_migrates_uid_to_pinned() {
        let raw = r#"{
            "cleanup_policy": "immediate",
            "input_device_uid": "coreaudio-yeti",
            "input_device_name": "Yeti"
        }"#;
        let mut settings: Settings = serde_json::from_str(raw).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.input_device_mode, InputDeviceMode::Pinned);
        assert_eq!(
            serde_json::to_value(&settings).unwrap()["input_device_mode"],
            "pinned"
        );
    }

    #[test]
    fn missing_input_device_mode_without_uid_migrates_to_follow_default() {
        let raw = r#"{"cleanup_policy":"immediate"}"#;
        let mut settings: Settings = serde_json::from_str(raw).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.input_device_mode, InputDeviceMode::FollowDefault);
    }

    #[test]
    fn explicit_input_device_mode_survives_load_migration() {
        let raw = r#"{
            "cleanup_policy": "immediate",
            "input_device_uid": "coreaudio-yeti",
            "input_device_mode": "follow_default"
        }"#;
        let mut settings: Settings = serde_json::from_str(raw).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.input_device_mode, InputDeviceMode::FollowDefault);
    }

    #[test]
    fn deserializes_missing_project_folder_to_meetings() {
        let raw = r#"{
            "vault_path": "/vault",
            "projects": [{
                "id": "vault",
                "name": "Vault",
                "path": "/vault"
            }],
            "active_project_id": "vault",
            "cleanup_policy": "immediate"
        }"#;
        let mut settings: Settings = serde_json::from_str(raw).unwrap();

        normalize_settings(&mut settings);

        assert_eq!(settings.inbox_folder, "meetings");
        assert_eq!(settings.projects[0].inbox_folder, "meetings");
    }

    #[test]
    fn hosted_settings_round_trip_redacts_and_rejects_secrets() {
        let mut settings = Settings::default();
        settings.api_key = Some("note-secret".to_string());
        settings.backchannel_api_key = Some("cue-secret".to_string());

        let hosted = hosted_settings(&settings);
        let round_trip: Settings =
            serde_json::from_value(serde_json::to_value(hosted).unwrap()).unwrap();
        assert_eq!(round_trip.api_key, None);
        assert_eq!(round_trip.backchannel_api_key, None);

        let error = reject_hosted_settings_secrets(&settings).unwrap_err();
        assert!(error.contains("server environment"));
        assert!(error.contains("api_key"));
        assert!(reject_hosted_settings_secrets(&round_trip).is_ok());
    }

    /// Verify that settings_path() is accessible (pub(crate)) and returns a
    /// non-empty path under the system config dir.
    #[test]
    fn settings_path_is_reachable_and_non_empty() {
        let path = settings_path();
        assert!(
            !path.as_os_str().is_empty(),
            "settings_path() should return a non-empty path"
        );
        // Should end with settings.json
        assert_eq!(
            path.file_name().and_then(|n| n.to_str()),
            Some("settings.json")
        );
    }

    /// Verify that writing a settings.json with an extra project and re-loading
    /// from that file picks up the new project (simulates what the backend
    /// settings-file watcher does after `margins projects add`).
    #[test]
    fn load_settings_from_picks_up_externally_written_project() {
        let tmp = std::env::temp_dir().join(format!(
            "margins-test-settings-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0)
        ));
        let raw = r#"{
            "vault_path": "/Users/test/existing-vault",
            "projects": [
                {
                    "id": "existing-vault",
                    "name": "Existing Vault",
                    "path": "/Users/test/existing-vault",
                    "readiness": "ready"
                },
                {
                    "id": "new-project-added-by-cli",
                    "name": "New Project",
                    "path": "/Users/test/real-notes",
                    "readiness": "needs_setup"
                }
            ],
            "active_project_id": "new-project-added-by-cli",
            "cleanup_policy": "immediate"
        }"#;
        std::fs::write(&tmp, raw).expect("could not write temp settings file");

        let settings = load_settings_from(&tmp);
        let _ = std::fs::remove_file(&tmp);

        assert_eq!(settings.projects.len(), 2, "both projects should be loaded");
        assert!(
            settings
                .projects
                .iter()
                .any(|p| p.id == "new-project-added-by-cli"),
            "CLI-added project should appear after external write + reload"
        );
        assert_eq!(
            settings.active_project_id.as_deref(),
            Some("new-project-added-by-cli"),
            "active project should be the newly added one"
        );
    }

    #[test]
    fn keeps_backchannel_routing_when_separate_key_is_saved() {
        let mut settings = Settings::default();
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("sk-or-test".to_string());
        settings.backchannel_base_url = Some("https://openrouter.ai/api/v1".to_string());
        settings.backchannel_model = Some("google/gemini-3-flash-preview".to_string());

        strip_ephemeral_env_defaults(&mut settings);

        assert_eq!(
            settings.backchannel_base_url.as_deref(),
            Some("https://openrouter.ai/api/v1")
        );
        assert_eq!(
            settings.backchannel_model.as_deref(),
            Some("google/gemini-3-flash-preview")
        );
    }
}
