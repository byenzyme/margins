pub(crate) type DistillAiConfig = (Option<String>, Option<String>, Option<String>, Option<u64>);

const INCLUDED_OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api/v1";
const INCLUDED_DISTILL_MODEL: &str = "anthropic/claude-sonnet-4.6";
const INCLUDED_PREP_MODEL: &str = "anthropic/claude-haiku-4.5";
const INCLUDED_CONFIG_URL: &str = "https://api.enzyme.garden/margins/free-config";
const INCLUDED_OPERATION_SAFETY_SECS: i64 = 300;
const INCLUDED_NO_EXPIRY_SOFT_REFRESH_SECS: i64 = 24 * 60 * 60;
const INCLUDED_TRANSIENT_FAILURE_COOLDOWN_SECS: i64 = 5;
const INCLUDED_PERMANENT_FAILURE_COOLDOWN_SECS: i64 = 30;
static INCLUDED_LEASE_MANAGER: std::sync::OnceLock<IncludedLeaseManager> =
    std::sync::OnceLock::new();
static PROVIDER_CONFIG_WRITE_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> =
    std::sync::OnceLock::new();
static PROVIDER_CONFIG_TEMP_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static INCLUDED_CONFIG_URL_MEMORY: std::sync::OnceLock<String> = std::sync::OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiMode {
    Included,
    ChatGpt,
    ApiKey,
}

impl AiMode {
    fn from_settings(settings: &crate::Settings) -> Self {
        let fallback = || {
            if clean_optional(settings.api_key.as_deref()).is_some() {
                Self::ApiKey
            } else {
                Self::Included
            }
        };
        match clean_optional(settings.ai_mode.as_deref())
            .map(|mode| mode.to_ascii_lowercase())
            .as_deref()
        {
            Some("included" | "margins" | "hosted") => Self::Included,
            Some("chatgpt" | "codex") => Self::ChatGpt,
            Some("api" | "api_key" | "custom") => Self::ApiKey,
            Some("auto") | None => fallback(),
            Some(_) => fallback(),
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum ProviderKind {
    IncludedOpenRouter,
    ChatGptSubscription,
    OpenAi,
    OpenAiCompatible { base_url: String },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum CredentialsSource {
    IncludedBroker,
    ChatGptOAuth,
    ApiKey { api_key: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RoleModels {
    pub(crate) distill: String,
    pub(crate) prep: String,
    pub(crate) cue: String,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct ResolvedRoute {
    pub(crate) provider: ProviderKind,
    pub(crate) credentials: CredentialsSource,
    pub(crate) model: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AiRole {
    Distill,
    Prep,
    Cue,
    Reprocess,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ResolvedAi {
    pub(crate) mode: AiMode,
    pub(crate) provider: ProviderKind,
    pub(crate) credentials: CredentialsSource,
    pub(crate) models: RoleModels,
    cue_override: Option<ResolvedRoute>,
}

impl ResolvedAi {
    pub(crate) fn route(&self, role: AiRole) -> Option<ResolvedRoute> {
        let model = match role {
            AiRole::Distill => self.models.distill.clone(),
            AiRole::Prep => self.models.prep.clone(),
            AiRole::Cue => self.models.cue.clone(),
            AiRole::Reprocess => self.models.cue.clone(),
        };
        if matches!(role, AiRole::Cue | AiRole::Reprocess) {
            if let Some(route) = &self.cue_override {
                return Some(route.clone());
            }
        }
        Some(ResolvedRoute {
            provider: self.provider.clone(),
            credentials: self.credentials.clone(),
            model,
        })
    }
}

pub(crate) fn resolve_ai(settings: &crate::Settings) -> Result<ResolvedAi, String> {
    let mode = AiMode::from_settings(settings);
    let (provider, credentials, distill_model, prep_model) = match mode {
        AiMode::Included => (
            ProviderKind::IncludedOpenRouter,
            CredentialsSource::IncludedBroker,
            INCLUDED_DISTILL_MODEL.to_string(),
            INCLUDED_PREP_MODEL.to_string(),
        ),
        AiMode::ChatGpt => {
            let model = clean_optional(settings.chatgpt_model.as_deref())
                .unwrap_or_else(|| "gpt-5.5".to_string());
            (
                ProviderKind::ChatGptSubscription,
                CredentialsSource::ChatGptOAuth,
                model.clone(),
                model,
            )
        }
        AiMode::ApiKey => {
            let api_key = clean_optional(settings.api_key.as_deref()).ok_or_else(|| {
                "Add an API key, or choose Included or ChatGPT in Settings.".to_string()
            })?;
            let base_url = normalized_base_url_for_key(
                &api_key,
                clean_optional(settings.ai_base_url.as_deref()),
            );
            let provider = provider_kind_for_base_url(base_url.as_deref())?;
            let distill_model = clean_optional(settings.ai_model.as_deref())
                .unwrap_or_else(|| default_model_for_role(base_url.as_deref(), AiRole::Distill));
            let prep_model = clean_optional(settings.ai_model.as_deref())
                .unwrap_or_else(|| default_model_for_role(base_url.as_deref(), AiRole::Prep));
            (
                provider,
                CredentialsSource::ApiKey { api_key },
                distill_model,
                prep_model,
            )
        }
    };

    let separate_cues = settings.backchannel_same_as_distill == Some(false);
    let separate_key = separate_cues
        .then(|| clean_optional(settings.backchannel_api_key.as_deref()))
        .flatten();
    let model_override = separate_cues
        .then(|| clean_optional(settings.backchannel_model.as_deref()))
        .flatten();
    let cue_override = if let Some(api_key) = separate_key.as_ref() {
        let base_url = normalized_base_url_for_key(
            api_key,
            clean_optional(settings.backchannel_base_url.as_deref()),
        );
        Some(ResolvedRoute {
            provider: provider_kind_for_base_url(base_url.as_deref())?,
            credentials: CredentialsSource::ApiKey {
                api_key: api_key.clone(),
            },
            model: model_override
                .clone()
                .unwrap_or_else(|| default_model_for_role(base_url.as_deref(), AiRole::Cue)),
        })
    } else {
        model_override.as_ref().map(|model| ResolvedRoute {
            provider: provider.clone(),
            credentials: credentials.clone(),
            model: model.clone(),
        })
    };
    let cue_model = cue_override
        .as_ref()
        .map(|route| route.model.clone())
        .unwrap_or_else(|| prep_model.clone());

    Ok(ResolvedAi {
        mode,
        provider,
        credentials,
        models: RoleModels {
            distill: distill_model,
            prep: prep_model,
            cue: cue_model,
        },
        cue_override,
    })
}

fn mode_slug(mode: AiMode) -> &'static str {
    match mode {
        AiMode::Included => "included",
        AiMode::ChatGpt => "chatgpt",
        AiMode::ApiKey => "api",
    }
}

/// Readiness for a single AI mode: whether resolution would succeed with the
/// current persisted settings, plus a short human-readable reason.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ModeReadiness {
    pub(crate) ready: bool,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct AiReadinessModes {
    pub(crate) included: ModeReadiness,
    pub(crate) chatgpt: ModeReadiness,
    pub(crate) api: ModeReadiness,
}

/// Single-shot readiness for all three AI modes plus the current effective
/// mode. Replaces the frontend's stitching of `aiStatus` + `includedAiStatus` +
/// settings-derived checks. The two external readiness signals — a fresh cached
/// included lease and a valid ChatGPT credential — are computed by the caller
/// (they need the lease manager / `pi::auth`); the API-key check and the mode
/// selection are derived purely here so the assembly stays testable.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct AiReadiness {
    pub(crate) mode: String,
    pub(crate) modes: AiReadinessModes,
}

pub(crate) fn ai_readiness(
    settings: &crate::Settings,
    included_ready: bool,
    chatgpt_ready: bool,
) -> AiReadiness {
    let api_ready = clean_optional(settings.api_key.as_deref()).is_some();
    AiReadiness {
        mode: mode_slug(AiMode::from_settings(settings)).to_string(),
        modes: AiReadinessModes {
            included: ModeReadiness {
                ready: included_ready,
                reason: if included_ready {
                    "Ready to write notes — no account or key needed.".to_string()
                } else {
                    "A usage-limited key is created before your first note.".to_string()
                },
            },
            chatgpt: ModeReadiness {
                ready: chatgpt_ready,
                reason: if chatgpt_ready {
                    "Connected to your ChatGPT subscription.".to_string()
                } else {
                    "Sign in with your ChatGPT Plus or Pro subscription.".to_string()
                },
            },
            api: ModeReadiness {
                ready: api_ready,
                reason: if api_ready {
                    "API key saved on this Mac.".to_string()
                } else {
                    "Add a key from OpenAI or a compatible provider.".to_string()
                },
            },
        },
    }
}

/// One resolved activity in the settings preview: which provider/model a role
/// will use, plus whether the quick-assists override moved it off the mode
/// default. `role` is the display-facing name (`indexing` for the enzyme role).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct PreviewRole {
    pub(crate) role: String,
    pub(crate) provider_label: String,
    pub(crate) model_id: String,
    pub(crate) model_label: String,
    pub(crate) changed_by_override: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize)]
pub(crate) struct ResolutionPreview {
    pub(crate) mode: String,
    pub(crate) roles: Vec<PreviewRole>,
}

/// Display-friendly summary of what `resolve_ai` would produce for a set of
/// (possibly unsaved) settings. PURE: no network, no keychain, no side effects —
/// safe to call on every keystroke. Roles the current mode does not use (e.g.
/// indexing outside Included mode) are omitted.
pub(crate) fn preview_ai_resolution(
    settings: &crate::Settings,
) -> Result<ResolutionPreview, String> {
    let resolved = resolve_ai(settings)?;
    let mut roles = Vec::new();
    for (role, slug) in [
        (AiRole::Distill, "distill"),
        (AiRole::Cue, "cue"),
        (AiRole::Reprocess, "reprocess"),
    ] {
        let Some(route) = resolved.route(role) else {
            continue;
        };
        let changed_by_override =
            matches!(role, AiRole::Cue | AiRole::Reprocess) && resolved.cue_override.is_some();
        roles.push(PreviewRole {
            role: slug.to_string(),
            provider_label: provider_label(&route.provider).to_string(),
            model_id: route.model.clone(),
            model_label: model_label(&route.model).to_string(),
            changed_by_override,
        });
    }
    Ok(ResolutionPreview {
        mode: mode_slug(resolved.mode).to_string(),
        roles,
    })
}

fn provider_label(provider: &ProviderKind) -> &'static str {
    match provider {
        ProviderKind::IncludedOpenRouter => "Included",
        ProviderKind::ChatGptSubscription => "ChatGPT subscription",
        ProviderKind::OpenAi => "OpenAI",
        ProviderKind::OpenAiCompatible { .. } => "Your provider",
    }
}

/// Friendly display name for a model slug. Unknown slugs fall back to the raw id
/// so a user-typed custom model always shows exactly what will be used.
fn model_label(model_id: &str) -> &str {
    match model_id {
        "anthropic/claude-sonnet-4.6" => "Claude Sonnet",
        "anthropic/claude-haiku-4.5" => "Claude Haiku",
        "google/gemini-3.1-flash-lite" => "Gemini Flash Lite",
        "gpt-5.5" => "GPT-5.5",
        other => other,
    }
}

fn provider_kind_for_base_url(base_url: Option<&str>) -> Result<ProviderKind, String> {
    match base_url {
        None => Ok(ProviderKind::OpenAi),
        Some(base_url) if is_default_openai_base_url(base_url) => Ok(ProviderKind::OpenAi),
        Some(base_url) => Ok(ProviderKind::OpenAiCompatible {
            base_url: canonical_provider_endpoint(base_url)?,
        }),
    }
}

fn default_model_for_role(base_url: Option<&str>, role: AiRole) -> String {
    if base_url
        .map(|url| {
            url.trim_end_matches('/')
                .eq_ignore_ascii_case(INCLUDED_OPENROUTER_BASE_URL)
        })
        .unwrap_or(false)
    {
        return if role == AiRole::Distill {
            INCLUDED_DISTILL_MODEL
        } else {
            INCLUDED_PREP_MODEL
        }
        .to_string();
    }
    "gpt-5.5".to_string()
}

pub(crate) fn load_openai_env_config() -> OpenAiEnvConfig {
    OpenAiEnvConfig::load()
}

#[derive(Clone)]
pub(crate) struct ProvisionedAiConfig {
    pub(crate) distill: DistillAiConfig,
    pub(crate) prep: DistillAiConfig,
    pub(crate) cue: DistillAiConfig,
    pub(crate) reprocess: DistillAiConfig,
    included_lease: Option<IncludedLeaseAcquisition>,
}

pub(crate) async fn provision(resolved: &ResolvedAi) -> Result<ProvisionedAiConfig, String> {
    let included_lease = if resolved.mode == AiMode::Included {
        Some(included_openrouter_lease().await?)
    } else {
        None
    };
    let mut cache = std::collections::HashMap::new();
    let distill = provision_resolved_route(
        resolved.route(AiRole::Distill).unwrap(),
        included_lease.clone(),
        &mut cache,
    )
    .await?;
    let prep = provision_resolved_route(
        resolved.route(AiRole::Prep).unwrap(),
        included_lease.clone(),
        &mut cache,
    )
    .await?;
    let cue = provision_resolved_route(
        resolved.route(AiRole::Cue).unwrap(),
        included_lease.clone(),
        &mut cache,
    )
    .await?;
    let reprocess = provision_resolved_route(
        resolved.route(AiRole::Reprocess).unwrap(),
        included_lease.clone(),
        &mut cache,
    )
    .await?;
    Ok(ProvisionedAiConfig {
        distill,
        prep,
        cue,
        reprocess,
        included_lease,
    })
}

async fn provision_resolved_route(
    route: ResolvedRoute,
    included_lease: Option<IncludedLeaseAcquisition>,
    cache: &mut std::collections::HashMap<ResolvedRoute, DistillAiConfig>,
) -> Result<DistillAiConfig, String> {
    if let Some(config) = cache.get(&route) {
        return Ok(config.clone());
    }
    let config = if matches!(
        route.provider,
        ProviderKind::IncludedOpenRouter | ProviderKind::OpenAiCompatible { .. }
    ) {
        let route_for_task = route.clone();
        tokio::task::spawn_blocking(move || {
            provision_route(&route_for_task, included_lease.as_ref())
        })
        .await
        .map_err(|error| format!("AI provider provisioning task failed: {error}"))??
    } else {
        provision_route(&route, included_lease.as_ref())?
    };
    cache.insert(route, config.clone());
    Ok(config)
}

fn provision_route(
    route: &ResolvedRoute,
    included_lease: Option<&IncludedLeaseAcquisition>,
) -> Result<DistillAiConfig, String> {
    match (&route.provider, &route.credentials) {
        (ProviderKind::IncludedOpenRouter, CredentialsSource::IncludedBroker) => {
            let lease = included_lease
                .ok_or_else(|| "included AI credentials were not provisioned".to_string())?;
            let mut config = openai_compatible_config(
                lease.api_key.clone(),
                Some(INCLUDED_OPENROUTER_BASE_URL.to_string()),
                route.model.clone(),
            )?;
            config.3 = Some(lease.generation);
            Ok(config)
        }
        (ProviderKind::ChatGptSubscription, CredentialsSource::ChatGptOAuth) => Ok((
            Some("openai-codex".to_string()),
            Some(route.model.clone()),
            None,
            None,
        )),
        (ProviderKind::OpenAi, CredentialsSource::ApiKey { api_key }) => Ok((
            Some("openai".to_string()),
            Some(route.model.clone()),
            Some(api_key.clone()),
            None,
        )),
        (ProviderKind::OpenAiCompatible { base_url }, CredentialsSource::ApiKey { api_key }) => {
            openai_compatible_config(api_key.clone(), Some(base_url.clone()), route.model.clone())
        }
        _ => Err("resolved AI provider and credentials do not match".to_string()),
    }
}

async fn provision_role(resolved: &ResolvedAi, role: AiRole) -> Result<DistillAiConfig, String> {
    let route = resolved
        .route(role)
        .ok_or_else(|| format!("AI role {role:?} is unavailable"))?;
    let included_lease = if route.credentials == CredentialsSource::IncludedBroker {
        Some(included_openrouter_lease().await?)
    } else {
        None
    };
    provision_resolved_route(route, included_lease, &mut std::collections::HashMap::new()).await
}

pub(crate) async fn configure_ai_for_distill(
    settings: &crate::Settings,
) -> Result<DistillAiConfig, String> {
    // Explicit Settings always win. Environment / .env values reach distillation
    // only because `apply_ai_env_defaults` seeded them into blank settings fields
    // at load time (where Keychain takes precedence over env). We deliberately do
    // NOT re-read raw env here: that short-circuit let `OPENAI_MODEL` silently
    // override the model a user picked in Settings.
    let resolved = resolve_ai(settings)?;
    provision_role(&resolved, AiRole::Distill).await
}

pub(crate) async fn configure_ai_for_backchannel(
    settings: &crate::Settings,
) -> Result<DistillAiConfig, String> {
    let resolved = resolve_ai(settings)?;
    provision_role(&resolved, AiRole::Cue).await
}

pub(crate) async fn configure_ai_for_prep(
    settings: &crate::Settings,
) -> Result<DistillAiConfig, String> {
    let resolved = resolve_ai(settings)?;
    provision_role(&resolved, AiRole::Prep).await
}

/// Reprocess-with-people runs on the same "fast model" that powers live cues,
/// NOT the heavier distillation model. It must resolve through the exact same
/// path as live cues so a user's separate cue model / api_key / base_url is
/// honored identically — otherwise the two would silently diverge whenever a
/// user opts into a distinct backchannel model. This is intentionally a thin
/// delegator (not a duplicated resolver) so the coupling can't drift.
pub(crate) async fn configure_ai_for_reprocess(
    settings: &crate::Settings,
) -> Result<DistillAiConfig, String> {
    let resolved = resolve_ai(settings)?;
    provision_role(&resolved, AiRole::Reprocess).await
}

pub(crate) async fn configure_ai_for_note_job(
    settings: &crate::Settings,
) -> Result<(DistillAiConfig, DistillAiConfig), String> {
    let resolved = resolve_ai(settings)?;
    let config = provision(&resolved).await?;
    Ok((config.distill, config.prep))
}

fn normalized_base_url_for_key(key: &str, base_url: Option<String>) -> Option<String> {
    if looks_like_openrouter_key(key)
        && base_url
            .as_deref()
            .map(is_default_openai_base_url)
            .unwrap_or(true)
    {
        return Some(INCLUDED_OPENROUTER_BASE_URL.to_string());
    }
    base_url
}

fn looks_like_openrouter_key(key: &str) -> bool {
    key.trim_start().starts_with("sk-or-")
}

fn openai_compatible_config(
    key: String,
    base_url: Option<String>,
    model: String,
) -> Result<DistillAiConfig, String> {
    let path = crate::pi_agent_dir().join("models.json");
    openai_compatible_config_at(&path, key, base_url, model)
}

fn openai_compatible_config_at(
    models_path: &std::path::Path,
    key: String,
    base_url: Option<String>,
    model: String,
) -> Result<DistillAiConfig, String> {
    if let Some(base_url) = base_url {
        if is_default_openai_base_url(&base_url) {
            Ok((Some("openai".to_string()), Some(model), Some(key), None))
        } else {
            let provider_id =
                ensure_margins_openai_compatible_model_at(models_path, &base_url, &model)?;
            Ok((Some(provider_id), Some(model), Some(key), None))
        }
    } else {
        Ok((Some("openai".to_string()), Some(model), Some(key), None))
    }
}

#[derive(Default)]
pub(crate) struct OpenAiEnvConfig {
    pub(crate) api_key: Option<String>,
    pub(crate) base_url: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) included_config_url: Option<String>,
}

impl OpenAiEnvConfig {
    fn load() -> Self {
        let mut config = parse_dotenv_files();
        if let Some(value) = read_process_env("OPENAI_API_KEY") {
            config.api_key = Some(value);
        }
        if let Some(value) = read_process_env("OPENAI_BASE_URL") {
            config.base_url = Some(value);
        }
        if let Some(value) = read_process_env("OPENAI_MODEL") {
            config.model = Some(value);
        }
        if let Some(value) = read_process_env("MARGINS_INCLUDED_AI_CONFIG_URL") {
            config.included_config_url = Some(value);
        }
        config
    }
}

fn read_process_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .and_then(|value| clean_optional(Some(&value)))
}

fn parse_dotenv_files() -> OpenAiEnvConfig {
    dotenv_candidate_paths()
        .into_iter()
        .find_map(|path| parse_dotenv_file(&path))
        .unwrap_or_default()
}

fn dotenv_candidate_paths() -> Vec<std::path::PathBuf> {
    let mut roots = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        roots.push(cwd);
    }

    // Installed app bundles can carry a local .env in Contents/Resources for
    // private/internal builds. Do not print these values; this is intentionally
    // only a file-discovery path for OpenAI-compatible auth.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(macos_dir) = exe.parent() {
            roots.push(macos_dir.to_path_buf());
            if let Some(contents_dir) = macos_dir.parent() {
                roots.push(contents_dir.to_path_buf());
                roots.push(contents_dir.join("Resources"));
            }
        }
    }

    // Source-checkout .env discovery is a development convenience only.
    #[cfg(debug_assertions)]
    {
        roots.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")));
        roots.push(std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    }

    let mut paths = Vec::new();
    for root in roots {
        for ancestor in root.ancestors() {
            let path = ancestor.join(".env");
            if !paths.iter().any(|existing| existing == &path) {
                paths.push(path);
            }
        }
    }
    paths
}

fn parse_dotenv_file(path: &std::path::Path) -> Option<OpenAiEnvConfig> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut config = OpenAiEnvConfig::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = clean_optional(Some(&unquote_dotenv_value(value.trim())));
        match key.trim() {
            "OPENAI_API_KEY" => config.api_key = value,
            "OPENAI_BASE_URL" => config.base_url = value,
            "OPENAI_MODEL" => config.model = value,
            "MARGINS_INCLUDED_AI_CONFIG_URL" => config.included_config_url = value,
            _ => {}
        }
    }
    if config.api_key.is_none() && config.included_config_url.is_none() {
        return None;
    }
    Some(config)
}

fn unquote_dotenv_value(value: &str) -> String {
    let value = value.trim();
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
        {
            return value[1..value.len() - 1].to_string();
        }
    }
    value.to_string()
}

fn clean_optional(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn is_default_openai_base_url(base_url: &str) -> bool {
    let normalized = base_url.trim().trim_end_matches('/');
    normalized.eq_ignore_ascii_case("https://api.openai.com/v1")
}

fn canonical_provider_endpoint(base_url: &str) -> Result<String, String> {
    let mut url = reqwest::Url::parse(base_url.trim())
        .map_err(|_| "OpenAI-compatible base URL is invalid".to_string())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err("OpenAI-compatible base URL must use HTTP or HTTPS".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() || url.query().is_some() {
        return Err(
            "OpenAI-compatible base URL must not contain credentials or query parameters"
                .to_string(),
        );
    }
    url.set_fragment(None);
    let mut canonical = url.to_string();
    while canonical.ends_with('/') {
        canonical.pop();
    }
    Ok(canonical)
}

fn provider_id_for_endpoint(base_url: &str) -> Result<String, String> {
    use sha2::Digest as _;
    let canonical = canonical_provider_endpoint(base_url)?;
    let digest = sha2::Sha256::digest(canonical.as_bytes());
    let suffix = digest[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    Ok(format!("margins-openai-compatible-{suffix}"))
}

fn ensure_margins_openai_compatible_model_at(
    path: &std::path::Path,
    base_url: &str,
    model: &str,
) -> Result<String, String> {
    ensure_margins_openai_compatible_model_at_with_hook(path, base_url, model, || {})
}

fn ensure_margins_openai_compatible_model_at_with_hook(
    path: &std::path::Path,
    base_url: &str,
    model: &str,
    before_commit: impl FnOnce(),
) -> Result<String, String> {
    let canonical_base_url = canonical_provider_endpoint(base_url)?;
    let provider_id = provider_id_for_endpoint(&canonical_base_url)?;
    let _write_guard = PROVIDER_CONFIG_WRITE_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _file_guard = ProviderConfigFileLock::acquire(path)?;
    let mut root = if path.exists() {
        std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read model settings {}: {e}", path.display()))?
            .parse::<serde_json::Value>()
            .map_err(|e| format!("model settings are not valid JSON: {e}"))?
    } else {
        serde_json::json!({})
    };
    before_commit();

    if !root.is_object() {
        root = serde_json::json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let providers = obj
        .entry("providers")
        .or_insert_with(|| serde_json::json!({}));
    if !providers.is_object() {
        *providers = serde_json::json!({});
    }
    let provider = providers
        .as_object_mut()
        .unwrap()
        .entry(provider_id.clone())
        .or_insert_with(|| serde_json::json!({}));
    *provider = merge_margins_compatible_provider(provider.take(), &canonical_base_url, model);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let serialized = serde_json::to_string_pretty(&root)
        .map_err(|e| format!("failed to encode model settings: {e}"))?;
    atomic_replace_utf8(path, &serialized)?;
    Ok(provider_id)
}

struct ProviderConfigFileLock {
    file: std::fs::File,
}

impl ProviderConfigFileLock {
    fn acquire(models_path: &std::path::Path) -> Result<Self, String> {
        let lock_path = models_path.with_extension("json.lock");
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&lock_path)
            .map_err(|error| {
                format!(
                    "failed to open model settings lock {}: {error}",
                    lock_path.display()
                )
            })?;
        for _ in 0..200 {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
                Err(std::fs::TryLockError::Error(error)) => {
                    return Err(format!(
                        "failed to lock model settings {}: {error}",
                        models_path.display()
                    ));
                }
            }
        }
        Err(format!(
            "timed out waiting for model settings lock {}",
            models_path.display()
        ))
    }
}

impl Drop for ProviderConfigFileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn atomic_replace_utf8(path: &std::path::Path, content: &str) -> Result<(), String> {
    use std::io::Write as _;
    let parent = path
        .parent()
        .ok_or_else(|| "model settings path has no parent".to_string())?;
    let counter = PROVIDER_CONFIG_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("models.json");
    let temp_path = parent.join(format!(
        ".{file_name}.{}.{}.tmp",
        std::process::id(),
        counter
    ));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|e| format!("failed to create model settings replacement: {e}"))?;
        file.write_all(content.as_bytes())
            .map_err(|e| format!("failed to write model settings replacement: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("failed to sync model settings replacement: {e}"))?;
        std::fs::rename(&temp_path, path)
            .map_err(|e| format!("failed to replace model settings {}: {e}", path.display()))?;
        if let Ok(directory) = std::fs::File::open(parent) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn merge_margins_compatible_provider(
    existing: serde_json::Value,
    base_url: &str,
    model: &str,
) -> serde_json::Value {
    let mut models = existing
        .get("models")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    if !models
        .iter()
        .any(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(model))
    {
        models.push(serde_json::json!({
            "id": model,
            "name": format!("{} (Margins)", model),
            "reasoning": false,
            "input": ["text"],
            "contextWindow": 128000,
            "maxTokens": 16384
        }));
    }
    serde_json::json!({
            "baseUrl": base_url,
            "api": "openai-completions",
            "apiKey": "env:MARGINS_API_KEY",
            "authHeader": true,
            "compat": {
                "supportsDeveloperRole": false,
                "supportsReasoningEffort": false
            },
            "models": models
    })
}

#[derive(Clone)]
struct IncludedLease {
    api_key: String,
    hard_expires_at: Option<i64>,
    refresh_after: i64,
    generation: u64,
    provider_auth_rejected: bool,
}

#[derive(Clone)]
struct IncludedLeaseFailure {
    message: String,
    transient: bool,
    retry_after: i64,
}

#[derive(Default)]
struct IncludedLeaseSlot {
    lease: Option<IncludedLease>,
    failure: Option<IncludedLeaseFailure>,
}

#[derive(Default)]
struct IncludedLeaseState {
    slots: std::collections::HashMap<String, IncludedLeaseSlot>,
    next_generation: u64,
    rejected_api_keys: std::collections::HashSet<String>,
}

struct IncludedLeaseManager {
    state: std::sync::Mutex<IncludedLeaseState>,
    refresh_gates:
        std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<tokio::sync::Mutex<()>>>>,
    bootstrap_id: tokio::sync::Mutex<Option<String>>,
}

impl IncludedLeaseManager {
    fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(IncludedLeaseState::default()),
            refresh_gates: std::sync::Mutex::new(std::collections::HashMap::new()),
            bootstrap_id: tokio::sync::Mutex::new(None),
        }
    }

    async fn bootstrap_id(&self) -> Result<String, String> {
        let mut cached = self.bootstrap_id.lock().await;
        if let Some(id) = cached.as_ref() {
            return Ok(id.clone());
        }
        let id = tokio::task::spawn_blocking(included_bootstrap_id_blocking)
            .await
            .map_err(|error| format!("included AI bootstrap task failed: {error}"))??;
        *cached = Some(id.clone());
        Ok(id)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, IncludedLeaseState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn refresh_gate(&self, config_fingerprint: &str) -> std::sync::Arc<tokio::sync::Mutex<()>> {
        let mut gates = self
            .refresh_gates
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        gates
            .entry(config_fingerprint.to_string())
            .or_insert_with(|| std::sync::Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    fn cached_decision(
        &self,
        config_fingerprint: &str,
        now_seconds: i64,
        wait_ms: u128,
    ) -> Result<Option<IncludedLeaseAcquisition>, String> {
        let state = self.lock_state();
        let slot = state.slots.get(config_fingerprint);
        if let Some(lease) = slot.and_then(|slot| slot.lease.as_ref()) {
            if lease.fresh_at(now_seconds) {
                return Ok(Some(IncludedLeaseAcquisition::from_lease(
                    lease,
                    IncludedLeaseSource::Memory,
                    wait_ms,
                )));
            }
        }
        if let Some(failure) = slot
            .and_then(|slot| slot.failure.as_ref())
            .filter(|failure| now_seconds < failure.retry_after)
        {
            return Err(failure.message.clone());
        }
        Ok(None)
    }

    async fn acquire_with<F, Fut>(
        &self,
        config_fingerprint: String,
        now_seconds: i64,
        fetch: F,
    ) -> Result<IncludedLeaseAcquisition, String>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<IncludedBrokerKey, BrokerFetchError>>,
    {
        if let Some(acquisition) = self.cached_decision(&config_fingerprint, now_seconds, 0)? {
            return Ok(acquisition);
        }

        let wait_started = std::time::Instant::now();
        let refresh_gate = self.refresh_gate(&config_fingerprint);
        let _refresh_guard = refresh_gate.lock().await;
        let wait_ms = wait_started.elapsed().as_millis();
        let after_wait_seconds = advance_unix_seconds(now_seconds, wait_started.elapsed());
        if let Some(acquisition) =
            self.cached_decision(&config_fingerprint, after_wait_seconds, wait_ms)?
        {
            return Ok(acquisition);
        }

        let fetched = fetch().await;
        let observed_seconds = advance_unix_seconds(now_seconds, wait_started.elapsed());
        match fetched.and_then(|broker_key| {
            IncludedLease::from_fetched(broker_key, observed_seconds)
                .map_err(BrokerFetchError::permanent)
        }) {
            Ok(mut lease) => {
                let mut state = self.lock_state();
                let generation = state.next_generation.checked_add(1).ok_or_else(|| {
                    "included AI lease generation space is exhausted; restart required".to_string()
                })?;
                state.next_generation = generation;
                lease.generation = generation;
                let slot = state.slots.entry(config_fingerprint.clone()).or_default();
                slot.failure = None;
                slot.lease = Some(lease.clone());
                Ok(IncludedLeaseAcquisition::from_lease(
                    &lease,
                    IncludedLeaseSource::Broker,
                    wait_ms,
                ))
            }
            Err(error) => {
                let cooldown = if error.transient {
                    INCLUDED_TRANSIENT_FAILURE_COOLDOWN_SECS
                } else {
                    INCLUDED_PERMANENT_FAILURE_COOLDOWN_SECS
                };
                let mut state = self.lock_state();
                let slot = state.slots.entry(config_fingerprint).or_default();
                slot.failure = Some(IncludedLeaseFailure {
                    message: error.message.clone(),
                    transient: error.transient,
                    retry_after: observed_seconds.saturating_add(cooldown),
                });
                Err(error.message)
            }
        }
    }

    fn fresh_cached_acquisition(
        &self,
        config_fingerprint: &str,
        now_seconds: i64,
    ) -> Option<IncludedLeaseAcquisition> {
        self.lock_state()
            .slots
            .get(config_fingerprint)?
            .lease
            .as_ref()
            .filter(|lease| lease.fresh_at(now_seconds))
            .map(|lease| {
                IncludedLeaseAcquisition::from_lease(lease, IncludedLeaseSource::Memory, 0)
            })
    }

    fn active_generation(&self, config_fingerprint: &str) -> Option<u64> {
        self.lock_state()
            .slots
            .get(config_fingerprint)?
            .lease
            .as_ref()
            .map(|lease| lease.generation)
    }

    fn generation_for_key(&self, config_fingerprint: &str, api_key: &str) -> Option<u64> {
        self.lock_state()
            .slots
            .get(config_fingerprint)?
            .lease
            .as_ref()
            .filter(|lease| lease.api_key == api_key)
            .map(|lease| lease.generation)
    }

    fn api_key_for_generation(&self, generation: u64) -> Option<String> {
        self.lock_state()
            .slots
            .values()
            .filter_map(|slot| slot.lease.as_ref())
            .find(|lease| lease.generation == generation)
            .map(|lease| lease.api_key.clone())
    }

    fn failure_telemetry(
        &self,
        config_fingerprint: &str,
        now_seconds: i64,
    ) -> Option<(&'static str, i64)> {
        self.lock_state()
            .slots
            .get(config_fingerprint)?
            .failure
            .as_ref()
            .map(|failure| {
                (
                    if failure.transient {
                        "broker_transient"
                    } else {
                        "broker_permanent_or_contract"
                    },
                    failure.retry_after.saturating_sub(now_seconds).max(0),
                )
            })
    }

    fn invalidate_generation(&self, generation: u64) -> bool {
        let mut state = self.lock_state();
        let rejected_api_key = state.slots.values_mut().find_map(|slot| {
            let Some(lease) = slot.lease.as_mut() else {
                return None;
            };
            if lease.generation == generation {
                lease.provider_auth_rejected = true;
                slot.failure = None;
                return Some(lease.api_key.clone());
            }
            None
        });
        if let Some(api_key) = rejected_api_key {
            state.rejected_api_keys.insert(api_key);
            true
        } else {
            false
        }
    }

    fn provider_rejected_api_key(&self, api_key: &str) -> bool {
        self.lock_state().rejected_api_keys.contains(api_key)
    }
}

impl IncludedLease {
    fn from_fetched(broker_key: IncludedBrokerKey, now_seconds: i64) -> Result<Self, String> {
        let api_key = clean_optional(Some(&broker_key.api_key))
            .ok_or_else(|| "included AI key service did not return a usable API key".to_string())?;
        let operation_deadline = included_operation_deadline(now_seconds)?;
        if broker_key
            .expires_at
            .is_some_and(|expires_at| expires_at <= operation_deadline)
        {
            return Err(
                "included AI key service returned a credential that expires too soon".to_string(),
            );
        }
        let refresh_after = broker_key
            .expires_at
            .map(|expires_at| expires_at.saturating_sub(INCLUDED_OPERATION_SAFETY_SECS))
            .unwrap_or_else(|| now_seconds.saturating_add(INCLUDED_NO_EXPIRY_SOFT_REFRESH_SECS));
        Ok(Self {
            api_key,
            hard_expires_at: broker_key.expires_at,
            refresh_after,
            generation: 0,
            provider_auth_rejected: false,
        })
    }

    fn fresh_at(&self, now_seconds: i64) -> bool {
        let Some(operation_deadline) = now_seconds.checked_add(INCLUDED_OPERATION_SAFETY_SECS)
        else {
            return false;
        };
        !self.provider_auth_rejected
            && now_seconds < self.refresh_after
            && self
                .hard_expires_at
                .map(|expires_at| expires_at > operation_deadline)
                .unwrap_or(true)
    }
}

fn included_operation_deadline(now_seconds: i64) -> Result<i64, String> {
    now_seconds
        .checked_add(INCLUDED_OPERATION_SAFETY_SECS)
        .ok_or_else(|| "system clock is outside the supported included AI lease range".to_string())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IncludedLeaseSource {
    Memory,
    Broker,
}

impl IncludedLeaseSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Broker => "broker",
        }
    }
}

#[derive(Clone, Debug)]
struct IncludedLeaseAcquisition {
    api_key: String,
    hard_expires_at: Option<i64>,
    generation: u64,
    source: IncludedLeaseSource,
    singleflight_wait_ms: u128,
}

impl IncludedLeaseAcquisition {
    fn from_lease(
        lease: &IncludedLease,
        source: IncludedLeaseSource,
        singleflight_wait_ms: u128,
    ) -> Self {
        Self {
            api_key: lease.api_key.clone(),
            hard_expires_at: lease.hard_expires_at,
            generation: lease.generation,
            source,
            singleflight_wait_ms,
        }
    }
}

#[derive(Clone, Debug)]
struct BrokerFetchError {
    message: String,
    transient: bool,
}

impl BrokerFetchError {
    fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: true,
        }
    }

    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: false,
        }
    }
}

fn included_lease_manager() -> &'static IncludedLeaseManager {
    INCLUDED_LEASE_MANAGER.get_or_init(IncludedLeaseManager::new)
}

#[cfg(test)]
pub(crate) fn install_test_included_lease_generation() -> u64 {
    install_test_included_lease_at(format!(
        "test-generation-slot-{}",
        PROVIDER_CONFIG_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ))
}

#[cfg(test)]
fn install_test_included_lease_for_settings(settings: &crate::Settings) -> u64 {
    let _ = settings;
    install_test_included_lease_at(included_config_fingerprint())
}

#[cfg(test)]
fn install_test_included_lease_at(config_fingerprint: String) -> u64 {
    let manager = included_lease_manager();
    let mut state = manager.lock_state();
    let generation = state.next_generation.checked_add(1).unwrap();
    state.next_generation = generation;
    state.slots.insert(
        config_fingerprint,
        IncludedLeaseSlot {
            lease: Some(IncludedLease {
                api_key: "test-only-key".to_string(),
                hard_expires_at: None,
                refresh_after: i64::MAX,
                generation,
                provider_auth_rejected: false,
            }),
            failure: None,
        },
    );
    generation
}

#[cfg(test)]
pub(crate) fn test_included_lease_generation_is_usable(generation: u64) -> bool {
    included_lease_manager()
        .lock_state()
        .slots
        .values()
        .filter_map(|slot| slot.lease.as_ref())
        .any(|lease| lease.generation == generation && !lease.provider_auth_rejected)
}

fn now_unix_seconds() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

fn advance_unix_seconds(now_seconds: i64, elapsed: std::time::Duration) -> i64 {
    now_seconds.saturating_add(i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX))
}

fn included_config_fingerprint() -> String {
    use sha2::Digest as _;
    let url = included_config_url();
    let material = format!(
        "mode=included\nurl={url}\nbase={INCLUDED_OPENROUTER_BASE_URL}\ndistill={INCLUDED_DISTILL_MODEL}\nprep={INCLUDED_PREP_MODEL}"
    );
    let digest = sha2::Sha256::digest(material.as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn included_config_url() -> String {
    INCLUDED_CONFIG_URL_MEMORY
        .get_or_init(|| {
            load_openai_env_config()
                .included_config_url
                .unwrap_or_else(|| INCLUDED_CONFIG_URL.to_string())
        })
        .clone()
}

async fn included_openrouter_lease() -> Result<IncludedLeaseAcquisition, String> {
    let now_seconds =
        now_unix_seconds().ok_or_else(|| "system clock is unavailable".to_string())?;
    let config_fingerprint = included_config_fingerprint();
    let url = included_config_url();
    let manager = included_lease_manager();
    let acquisition = match manager
        .acquire_with(config_fingerprint.clone(), now_seconds, || async move {
            if let Some(broker_key) = load_persisted_included_broker_key().await? {
                if manager.provider_rejected_api_key(&broker_key.api_key) {
                    evict_persisted_included_broker_key(broker_key.api_key).await?;
                } else if validate_broker_key_safety(&broker_key, now_seconds).is_ok() {
                    let bootstrap_id = manager
                        .bootstrap_id()
                        .await
                        .map_err(BrokerFetchError::permanent)?;
                    persist_included_recall_bundle(bootstrap_id, broker_key.clone()).await?;
                    return Ok(broker_key);
                }
            }
            let bootstrap_id = manager
                .bootstrap_id()
                .await
                .map_err(BrokerFetchError::permanent)?;
            let broker_key =
                fetch_included_openrouter_api_key_with_bootstrap(&url, &bootstrap_id).await?;
            let completed_at = now_unix_seconds().ok_or_else(|| {
                BrokerFetchError::permanent("system clock is unavailable after key refresh")
            })?;
            validate_broker_key_safety(&broker_key, completed_at)
                .map_err(BrokerFetchError::permanent)?;
            if manager.provider_rejected_api_key(&broker_key.api_key) {
                return Err(BrokerFetchError::transient(
                    "included AI key service returned a credential rejected by the provider",
                ));
            }
            persist_included_broker_key(broker_key.clone()).await?;
            Ok(broker_key)
        })
        .await
    {
        Ok(acquisition) => acquisition,
        Err(error) => {
            let (failure_class, cooldown_remaining_secs) = included_lease_manager()
                .failure_telemetry(&config_fingerprint, now_seconds)
                .unwrap_or(("lease_state", 0));
            append_app_log_async(
                "included_ai_lease",
                format!(
                    "fingerprint={config_fingerprint} outcome=failed failure_class={failure_class} cooldown_remaining_secs={cooldown_remaining_secs}"
                ),
            )
            .await;
            return Err(error);
        }
    };
    if acquisition.source != IncludedLeaseSource::Memory || acquisition.singleflight_wait_ms > 0 {
        append_app_log_async(
            "included_ai_lease",
            format!(
                "fingerprint={config_fingerprint} generation={} source={} singleflight_wait_ms={} ttl_bucket={}",
                acquisition.generation,
                acquisition.source.as_str(),
                acquisition.singleflight_wait_ms,
                ttl_bucket(acquisition.hard_expires_at, now_seconds),
            ),
        )
        .await;
    }
    Ok(acquisition)
}

fn validate_broker_key_safety(
    broker_key: &IncludedBrokerKey,
    now_seconds: i64,
) -> Result<(), String> {
    let operation_deadline = included_operation_deadline(now_seconds)?;
    if broker_key
        .expires_at
        .is_some_and(|expires_at| expires_at <= operation_deadline)
    {
        return Err(
            "included AI key service returned a credential that expires too soon".to_string(),
        );
    }
    Ok(())
}

async fn included_openrouter_api_key() -> Result<String, String> {
    included_openrouter_lease().await.map(|lease| lease.api_key)
}

fn ttl_bucket(hard_expires_at: Option<i64>, now_seconds: i64) -> &'static str {
    match hard_expires_at.map(|expires_at| expires_at.saturating_sub(now_seconds)) {
        None => "none",
        Some(ttl) if ttl <= 0 => "expired",
        Some(ttl) if ttl <= INCLUDED_OPERATION_SAFETY_SECS => "under_safety_horizon",
        Some(ttl) if ttl <= 60 * 60 => "under_1h",
        Some(ttl) if ttl <= 24 * 60 * 60 => "under_24h",
        Some(_) => "over_24h",
    }
}

pub(crate) fn included_ai_key_ready() -> bool {
    included_cached_openrouter_api_key().is_some()
}

pub(crate) fn included_cached_openrouter_api_key() -> Option<String> {
    included_cached_openrouter_lease().map(|lease| lease.api_key)
}

pub(crate) fn included_recall_api_key(settings: &crate::Settings) -> Option<String> {
    (AiMode::from_settings(settings) == AiMode::Included)
        .then(included_cached_openrouter_api_key)
        .flatten()
}

fn included_cached_openrouter_lease() -> Option<IncludedLeaseAcquisition> {
    let now_seconds = now_unix_seconds()?;
    let config_fingerprint = included_config_fingerprint();
    included_lease_manager().fresh_cached_acquisition(&config_fingerprint, now_seconds)
}

/// Append a durable, session-independent app event to
/// `<vault>/.margins/margins_app.jsonl`. Key/indexing events (key expiry,
/// enzyme falling back to hosted credits, degraded cue context) are not tied to
/// a recording session, so they can't ride the per-session
/// `*_backchannel_trace.jsonl` files. Best effort — never fails the caller, and
/// no-ops when no vault is configured yet.
pub(crate) fn append_app_log(kind: &str, message: &str) {
    use std::io::Write as _;
    let settings = crate::settings::load_settings();
    let Some(vault) = crate::vault_root(&settings) else {
        return;
    };
    // Best-effort telemetry must not materialize the vault. On a clean first run
    // the default vault (~/Documents/margins) does not exist yet; startup AI-key
    // provisioning logs here before any capture. Skip logging (rather than
    // create_dir_all) until the vault exists — capture start creates it.
    if !vault.exists() {
        return;
    }
    let margins_dir = vault.join(".margins");
    if std::fs::create_dir_all(&margins_dir).is_err() {
        return;
    }
    let event = serde_json::json!({
        "ts": chrono::Local::now().to_rfc3339(),
        "kind": kind,
        "message": message,
    });
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(margins_dir.join("margins_app.jsonl"))
    {
        let _ = writeln!(
            file,
            "{}",
            serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string())
        );
    }
}

pub(crate) async fn append_app_log_async(kind: &'static str, message: String) {
    let _ = tokio::task::spawn_blocking(move || append_app_log(kind, &message)).await;
}

pub(crate) async fn log_backchannel_snapshot_config_error(error: String) {
    append_app_log_async(
        "backchannel_snapshot_ai_config",
        backchannel_snapshot_config_error_message(&error),
    )
    .await;
}

fn backchannel_snapshot_config_error_message(error: &str) -> String {
    format!("could not refresh live snapshot AI config: {error}")
}

pub(crate) async fn prepare_included_ai_key() -> Result<(), String> {
    included_openrouter_api_key().await.map(|_| ())
}

pub(crate) fn schedule_capture_ai_preflight(settings: crate::Settings, capture_lane: &'static str) {
    let Ok(resolved) = resolve_ai(&settings) else {
        return;
    };
    if resolved.mode != AiMode::Included {
        return;
    }
    let task = async move {
        let started = std::time::Instant::now();
        let config_fingerprint = included_config_fingerprint();
        let outcome = provision(&resolved).await;
        match outcome {
            Ok(config) => {
                let lease = config.included_lease.unwrap();
                append_app_log_async(
                    "included_ai_preflight",
                    format!(
                        "lane={capture_lane} fingerprint={config_fingerprint} outcome=ready generation={} source={} elapsed_ms={} ttl_bucket={}",
                        lease.generation,
                        lease.source.as_str(),
                        started.elapsed().as_millis(),
                        ttl_bucket(lease.hard_expires_at, now_unix_seconds().unwrap_or_default()),
                    ),
                )
                .await;
            }
            Err(_) => append_app_log_async(
                "included_ai_preflight",
                format!(
                    "lane={capture_lane} fingerprint={config_fingerprint} outcome=failed elapsed_ms={} error_class=lease_unavailable",
                    started.elapsed().as_millis(),
                ),
            )
            .await,
        }
    };

    #[cfg(feature = "tauri-app")]
    {
        crate::async_runtime::spawn(task);
    }
    #[cfg(not(feature = "tauri-app"))]
    {
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(task);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProviderErrorDisposition {
    Cancelled,
    Invalidated,
    Untracked,
}

/// Handle a generic provider failure conservatively. Callers pass the exact
/// generation captured for the failed request. Explicit local cancellation is
/// checked at the error boundary and never invalidates a credential.
pub(crate) fn handle_included_ai_provider_error(
    generation: Option<u64>,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
    failure_stage: &'static str,
) -> ProviderErrorDisposition {
    let disposition = handle_included_ai_provider_error_with(
        included_lease_manager(),
        generation,
        cancellation,
        failure_stage,
    );
    if disposition == ProviderErrorDisposition::Invalidated {
        if let Some(generation) = generation {
            if let Some(api_key) = included_lease_manager().api_key_for_generation(generation) {
                std::thread::spawn(move || {
                    let _ = evict_persisted_included_broker_key_with(
                        &SystemIncludedLeaseKeychain,
                        &api_key,
                    );
                });
            }
            #[cfg(not(test))]
            append_app_log(
                "included_ai_lease_invalidated",
                &format!("generation={generation} reason=provider_failure stage={failure_stage}"),
            );
            #[cfg(test)]
            let _ = (generation, failure_stage);
        }
    }
    disposition
}

fn handle_included_ai_provider_error_with(
    manager: &IncludedLeaseManager,
    generation: Option<u64>,
    cancellation: Option<&std::sync::atomic::AtomicBool>,
    _failure_stage: &'static str,
) -> ProviderErrorDisposition {
    if cancellation.is_some_and(|cancel| cancel.load(std::sync::atomic::Ordering::SeqCst)) {
        return ProviderErrorDisposition::Cancelled;
    }
    let Some(generation) = generation else {
        return ProviderErrorDisposition::Untracked;
    };
    if manager.invalidate_generation(generation) {
        ProviderErrorDisposition::Invalidated
    } else {
        ProviderErrorDisposition::Untracked
    }
}

// The lease Keychain coordinates and JSON shape live in the shared root-crate
// contract. This is an OpenRouter provider lease for desktop note-making, not an
// Enzyme account identity or bearer; recall intentionally does not consume it.
use margins::included_lease::IncludedLease as IncludedBrokerKey;

const INCLUDED_LEASE_ACCOUNT: &str = margins::included_lease::KEYCHAIN_ACCOUNT;

trait IncludedLeaseKeychain {
    fn get(&self, account: &str) -> Option<String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

struct SystemIncludedLeaseKeychain;

impl IncludedLeaseKeychain for SystemIncludedLeaseKeychain {
    fn get(&self, account: &str) -> Option<String> {
        let service = crate::settings::keychain_service(margins::included_lease::KEYCHAIN_SCOPE);
        crate::settings::keychain_get_service(&service, account)
    }

    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        let service = crate::settings::keychain_service(margins::included_lease::KEYCHAIN_SCOPE);
        crate::settings::keychain_set_service(&service, account, value)
    }

    fn delete(&self, account: &str) -> Result<(), String> {
        let service = crate::settings::keychain_service(margins::included_lease::KEYCHAIN_SCOPE);
        crate::settings::keychain_delete_service(&service, account)
    }
}

fn load_persisted_included_broker_key_with(
    keychain: &impl IncludedLeaseKeychain,
) -> Result<Option<IncludedBrokerKey>, String> {
    if let Some(serialized) = keychain.get(INCLUDED_LEASE_ACCOUNT) {
        match serde_json::from_str::<IncludedBrokerKey>(&serialized) {
            Ok(lease) if clean_optional(Some(&lease.api_key)).is_some() => {
                let _ = keychain.delete("api-key");
                let _ = keychain.delete("expires-at");
                return Ok(Some(lease));
            }
            _ => {
                let _ = keychain.delete(INCLUDED_LEASE_ACCOUNT);
            }
        }
    }

    let Some(api_key) = keychain
        .get("api-key")
        .and_then(|value| clean_optional(Some(&value)))
    else {
        return Ok(None);
    };
    let expires_at = keychain
        .get("expires-at")
        .map(|value| normalize_expiry_text(&value))
        .transpose()?
        .flatten();
    let lease = IncludedBrokerKey {
        api_key,
        expires_at,
    };
    let serialized = serde_json::to_string(&lease)
        .map_err(|error| format!("failed to encode included AI lease: {error}"))?;
    keychain.set(INCLUDED_LEASE_ACCOUNT, &serialized)?;
    keychain.delete("api-key")?;
    keychain.delete("expires-at")?;
    Ok(Some(lease))
}

fn persist_included_broker_key_with(
    keychain: &impl IncludedLeaseKeychain,
    lease: &IncludedBrokerKey,
) -> Result<(), String> {
    let serialized = serde_json::to_string(lease)
        .map_err(|error| format!("failed to encode included AI lease: {error}"))?;
    keychain.set(INCLUDED_LEASE_ACCOUNT, &serialized)
}

fn evict_persisted_included_broker_key_with(
    keychain: &impl IncludedLeaseKeychain,
    rejected_api_key: &str,
) -> Result<(), String> {
    let mut evicted = false;
    if keychain
        .get(INCLUDED_LEASE_ACCOUNT)
        .and_then(|serialized| serde_json::from_str::<IncludedBrokerKey>(&serialized).ok())
        .is_some_and(|lease| lease.api_key == rejected_api_key)
    {
        keychain.delete(INCLUDED_LEASE_ACCOUNT)?;
        evicted = true;
    }
    if keychain
        .get("api-key")
        .is_some_and(|api_key| api_key.trim() == rejected_api_key)
    {
        keychain.delete("api-key")?;
        keychain.delete("expires-at")?;
        evicted = true;
    }
    #[cfg(not(test))]
    if evicted {
        if let Ok(home) = margins::hosted_credentials::margins_home() {
            margins::hosted_credentials::invalidate_bundle_if_key(&home, rejected_api_key)
                .map_err(|error| format!("failed to invalidate hosted catalyst bundle: {error}"))?;
        }
    }
    #[cfg(test)]
    let _ = evicted;
    Ok(())
}

async fn load_persisted_included_broker_key() -> Result<Option<IncludedBrokerKey>, BrokerFetchError>
{
    tokio::task::spawn_blocking(|| {
        load_persisted_included_broker_key_with(&SystemIncludedLeaseKeychain)
    })
    .await
    .map_err(|error| {
        BrokerFetchError::permanent(format!("included AI Keychain task failed: {error}"))
    })?
    .map_err(BrokerFetchError::permanent)
}

async fn persist_included_broker_key(lease: IncludedBrokerKey) -> Result<(), BrokerFetchError> {
    tokio::task::spawn_blocking(move || {
        persist_included_broker_key_with(&SystemIncludedLeaseKeychain, &lease)
    })
    .await
    .map_err(|error| {
        BrokerFetchError::permanent(format!("included AI Keychain task failed: {error}"))
    })?
    .map_err(BrokerFetchError::permanent)
}

#[cfg(not(test))]
async fn persist_included_recall_bundle(
    bootstrap_id: String,
    lease: IncludedBrokerKey,
) -> Result<(), BrokerFetchError> {
    tokio::task::spawn_blocking(move || {
        let home = margins::hosted_credentials::margins_home()
            .map_err(|error| format!("failed to resolve Margins home: {error}"))?;
        margins::hosted_credentials::ensure_included_bundle(
            &home,
            &bootstrap_id,
            &lease.api_key,
            lease.expires_at,
        )
        .map(|_| ())
        .map_err(|error| format!("failed to provision hosted catalyst bundle: {error}"))
    })
    .await
    .map_err(|error| {
        BrokerFetchError::permanent(format!("hosted catalyst bundle task failed: {error}"))
    })?
    .map_err(BrokerFetchError::permanent)
}

#[cfg(test)]
async fn persist_included_recall_bundle(
    _bootstrap_id: String,
    _lease: IncludedBrokerKey,
) -> Result<(), BrokerFetchError> {
    Ok(())
}

async fn evict_persisted_included_broker_key(
    rejected_api_key: String,
) -> Result<(), BrokerFetchError> {
    tokio::task::spawn_blocking(move || {
        evict_persisted_included_broker_key_with(&SystemIncludedLeaseKeychain, &rejected_api_key)
    })
    .await
    .map_err(|error| {
        BrokerFetchError::permanent(format!("included AI Keychain task failed: {error}"))
    })?
    .map_err(BrokerFetchError::permanent)
}

async fn fetch_included_openrouter_api_key_with_bootstrap(
    url: &str,
    bootstrap_id: &str,
) -> Result<IncludedBrokerKey, BrokerFetchError> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| {
            BrokerFetchError::permanent(format!("failed to prepare included AI key request: {e}"))
        })?;
    let response = client
        .get(url)
        .header("X-Margins-Bootstrap-Id", bootstrap_id)
        .send()
        .await
        .map_err(|e| {
            BrokerFetchError::transient(format!("failed to contact included AI key service: {e}"))
        })?;
    let status = response.status();
    if !status.is_success() {
        let message = format!(
            "included note-making could not create a usage-limited key (status {status}). Choose ChatGPT or add your own API key in Settings."
        );
        return Err(
            if status.as_u16() == 408 || status.as_u16() == 429 || status.is_server_error() {
                BrokerFetchError::transient(message)
            } else {
                BrokerFetchError::permanent(message)
            },
        );
    }
    let body_text = response.text().await.map_err(|e| {
        BrokerFetchError::transient(format!(
            "included AI key service returned an unreadable success response: {e}"
        ))
    })?;
    let body: serde_json::Value = serde_json::from_str(&body_text).map_err(|e| {
        BrokerFetchError::permanent(format!(
            "included AI key service returned invalid JSON: {e}"
        ))
    })?;
    let api_key = body
        .pointer("/data/key")
        .or_else(|| body.pointer("/data/api_key"))
        .or_else(|| body.get("key"))
        .or_else(|| body.get("api_key"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .filter(|key| !key.trim().is_empty())
        .ok_or_else(|| {
            BrokerFetchError::permanent("included AI key service did not return a usable API key")
        })?;
    let expires_at = normalize_expiry_json(
        body.pointer("/data/expires_at")
            .or_else(|| body.get("expires_at")),
    )
    .map_err(BrokerFetchError::permanent)?;
    #[cfg(not(test))]
    {
        let base_url = body
            .pointer("/data/base_url")
            .or_else(|| body.get("base_url"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(margins::hosted_credentials::INCLUDED_BASE_URL);
        let model = body
            .pointer("/data/model")
            .or_else(|| body.get("model"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or(margins::hosted_credentials::INCLUDED_CATALYST_MODEL);
        let home = margins::hosted_credentials::margins_home().map_err(|error| {
            BrokerFetchError::permanent(format!("failed to resolve Margins home: {error}"))
        })?;
        margins::hosted_credentials::install_bundle(
            &home,
            bootstrap_id,
            &api_key,
            base_url,
            model,
            expires_at,
        )
        .map_err(|error| {
            BrokerFetchError::permanent(format!(
                "failed to provision hosted catalyst bundle: {error}"
            ))
        })?;
    }
    Ok(IncludedBrokerKey {
        api_key,
        expires_at,
    })
}

fn normalize_expiry_json(value: Option<&serde_json::Value>) -> Result<Option<i64>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    if let Some(raw) = value.as_i64() {
        return normalize_expiry_integer(raw).map(Some);
    }
    if let Some(raw) = value.as_u64() {
        let raw = i64::try_from(raw)
            .map_err(|_| "included AI key expiry is outside the supported range".to_string())?;
        return normalize_expiry_integer(raw).map(Some);
    }
    if let Some(raw) = value.as_str() {
        return normalize_expiry_text(raw);
    }
    Err("included AI key expiry has an unsupported representation".to_string())
}

fn normalize_expiry_text(raw: &str) -> Result<Option<i64>, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("included AI key expiry is empty".to_string());
    }
    if let Ok(integer) = raw.parse::<i64>() {
        return normalize_expiry_integer(integer).map(Some);
    }
    chrono::DateTime::parse_from_rfc3339(raw)
        .map_err(|_| "included AI key expiry is neither epoch time nor RFC3339".to_string())
        .and_then(|value| normalize_expiry_integer(value.timestamp()).map(Some))
}

fn normalize_expiry_integer(raw: i64) -> Result<i64, String> {
    const MAX_SUPPORTED_UNIX_SECONDS: i64 = 32_503_680_000; // 3000-01-01 UTC
    const MIN_SUPPORTED_UNIX_MILLIS: i64 = 1_000_000_000_000; // 2001-09-09 UTC
    let seconds = if raw > MAX_SUPPORTED_UNIX_SECONDS {
        if raw < MIN_SUPPORTED_UNIX_MILLIS {
            return Err("included AI key expiry is outside the supported range".to_string());
        }
        raw.checked_div(1_000)
            .filter(|seconds| *seconds <= MAX_SUPPORTED_UNIX_SECONDS)
            .ok_or_else(|| "included AI key expiry is outside the supported range".to_string())?
    } else {
        raw
    };
    if seconds <= 0 {
        return Err("included AI key expiry must be a positive Unix timestamp".to_string());
    }
    Ok(seconds)
}

fn included_bootstrap_id_blocking() -> Result<String, String> {
    let service = crate::settings::keychain_service(margins::included_lease::KEYCHAIN_SCOPE);
    let home = margins::hosted_credentials::margins_home()
        .map_err(|error| format!("failed to resolve Margins home: {error}"))?;
    if let Some(id) = margins::hosted_credentials::existing_bootstrap_id(&home)
        .map_err(|error| format!("failed to read shared bootstrap identity: {error}"))?
    {
        crate::settings::keychain_set_service(&service, "bootstrap-id", &id)?;
        return Ok(id);
    }
    if let Some(id) = crate::settings::keychain_get_service(&service, "bootstrap-id") {
        return margins::hosted_credentials::ensure_bootstrap_id(&home, Some(&id))
            .map_err(|error| format!("failed to mirror shared bootstrap identity: {error}"));
    }
    let output = std::process::Command::new("uuidgen")
        .output()
        .map_err(|e| format!("failed to create included AI bootstrap id: {e}"))?;
    if !output.status.success() {
        return Err("failed to create included AI bootstrap id".to_string());
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        return Err("failed to create included AI bootstrap id".to_string());
    }
    crate::settings::keychain_set_service(&service, "bootstrap-id", &id)?;
    margins::hosted_credentials::ensure_bootstrap_id(&home, Some(&id))
        .map_err(|error| format!("failed to mirror shared bootstrap identity: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_models_path(tag: &str) -> std::path::PathBuf {
        let counter =
            PROVIDER_CONFIG_TEMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!(
                "margins-ai-config-{tag}-{}-{counter}",
                std::process::id()
            ))
            .join("models.json")
    }

    fn credential_kind(credentials: &CredentialsSource) -> &'static str {
        match credentials {
            CredentialsSource::IncludedBroker => "included",
            CredentialsSource::ChatGptOAuth => "chatgpt",
            CredentialsSource::ApiKey { .. } => "api-key",
        }
    }

    #[test]
    fn pure_resolution_mode_role_matrix() {
        struct Case {
            label: &'static str,
            mode: Option<&'static str>,
            api_key: Option<&'static str>,
            expected_mode: AiMode,
            provider: ProviderKind,
            credentials: &'static str,
            distill: &'static str,
            prep: &'static str,
        }
        let cases = [
            Case {
                label: "included",
                mode: Some("included"),
                api_key: None,
                expected_mode: AiMode::Included,
                provider: ProviderKind::IncludedOpenRouter,
                credentials: "included",
                distill: INCLUDED_DISTILL_MODEL,
                prep: INCLUDED_PREP_MODEL,
            },
            Case {
                label: "included alias",
                mode: Some("hosted"),
                api_key: Some("ignored-key"),
                expected_mode: AiMode::Included,
                provider: ProviderKind::IncludedOpenRouter,
                credentials: "included",
                distill: INCLUDED_DISTILL_MODEL,
                prep: INCLUDED_PREP_MODEL,
            },
            Case {
                label: "chatgpt",
                mode: Some("chatgpt"),
                api_key: None,
                expected_mode: AiMode::ChatGpt,
                provider: ProviderKind::ChatGptSubscription,
                credentials: "chatgpt",
                distill: "gpt-5.1-codex",
                prep: "gpt-5.1-codex",
            },
            Case {
                label: "chatgpt alias",
                mode: Some("codex"),
                api_key: Some("ignored-key"),
                expected_mode: AiMode::ChatGpt,
                provider: ProviderKind::ChatGptSubscription,
                credentials: "chatgpt",
                distill: "gpt-5.1-codex",
                prep: "gpt-5.1-codex",
            },
            Case {
                label: "api",
                mode: Some("api"),
                api_key: Some("test-api-key"),
                expected_mode: AiMode::ApiKey,
                provider: ProviderKind::OpenAi,
                credentials: "api-key",
                distill: "api-model",
                prep: "api-model",
            },
            Case {
                label: "api alias",
                mode: Some("custom"),
                api_key: Some("test-api-key"),
                expected_mode: AiMode::ApiKey,
                provider: ProviderKind::OpenAi,
                credentials: "api-key",
                distill: "api-model",
                prep: "api-model",
            },
            Case {
                label: "garbage without key",
                mode: Some("surprise"),
                api_key: None,
                expected_mode: AiMode::Included,
                provider: ProviderKind::IncludedOpenRouter,
                credentials: "included",
                distill: INCLUDED_DISTILL_MODEL,
                prep: INCLUDED_PREP_MODEL,
            },
            Case {
                label: "garbage with key",
                mode: Some("surprise"),
                api_key: Some("test-api-key"),
                expected_mode: AiMode::ApiKey,
                provider: ProviderKind::OpenAi,
                credentials: "api-key",
                distill: "api-model",
                prep: "api-model",
            },
            Case {
                label: "blank without key",
                mode: Some("  "),
                api_key: None,
                expected_mode: AiMode::Included,
                provider: ProviderKind::IncludedOpenRouter,
                credentials: "included",
                distill: INCLUDED_DISTILL_MODEL,
                prep: INCLUDED_PREP_MODEL,
            },
            Case {
                label: "blank with key",
                mode: None,
                api_key: Some("test-api-key"),
                expected_mode: AiMode::ApiKey,
                provider: ProviderKind::OpenAi,
                credentials: "api-key",
                distill: "api-model",
                prep: "api-model",
            },
        ];

        for case in cases {
            let mut settings = crate::Settings::default();
            settings.ai_mode = case.mode.map(str::to_string);
            settings.api_key = case.api_key.map(str::to_string);
            settings.ai_model = Some("api-model".to_string());
            settings.chatgpt_model = Some("gpt-5.1-codex".to_string());
            let resolved = resolve_ai(&settings)
                .unwrap_or_else(|error| panic!("{} failed to resolve: {error}", case.label));
            assert_eq!(resolved.mode, case.expected_mode, "{} mode", case.label);
            for (role, expected_model) in [
                (AiRole::Distill, case.distill),
                (AiRole::Prep, case.prep),
                (AiRole::Cue, case.prep),
                (AiRole::Reprocess, case.prep),
            ] {
                let route = resolved.route(role).unwrap();
                assert_eq!(route.provider, case.provider, "{} {role:?}", case.label);
                assert_eq!(
                    credential_kind(&route.credentials),
                    case.credentials,
                    "{} {role:?}",
                    case.label
                );
                assert_eq!(route.model, expected_model, "{} {role:?}", case.label);
            }
        }
    }

    #[test]
    fn pure_resolution_keeps_mode_models_namespaced_across_switches() {
        let mut settings = crate::Settings::default();
        settings.api_key = Some("sk-or-test".to_string());
        settings.ai_model = Some("anthropic/claude-sonnet-4.6".to_string());
        settings.chatgpt_model = Some("gpt-5.1-codex".to_string());

        settings.ai_mode = Some("chatgpt".to_string());
        let chatgpt = resolve_ai(&settings).unwrap();
        assert_eq!(
            chatgpt.route(AiRole::Distill).unwrap().model,
            "gpt-5.1-codex"
        );
        assert_eq!(chatgpt.route(AiRole::Cue).unwrap().model, "gpt-5.1-codex");
        assert_eq!(
            chatgpt.route(AiRole::Reprocess).unwrap().model,
            "gpt-5.1-codex"
        );

        settings.ai_mode = Some("api".to_string());
        let api = resolve_ai(&settings).unwrap();
        assert_eq!(
            api.route(AiRole::Distill).unwrap().model,
            "anthropic/claude-sonnet-4.6"
        );
    }

    #[test]
    fn pure_resolution_folds_backchannel_override_once() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("included".to_string());

        let automatic = resolve_ai(&settings).unwrap();
        assert_eq!(
            automatic.route(AiRole::Prep).unwrap().model,
            INCLUDED_PREP_MODEL
        );
        assert_eq!(
            automatic.route(AiRole::Cue).unwrap().model,
            INCLUDED_PREP_MODEL
        );

        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_model = Some("google/gemini-fast".to_string());
        let model_only = resolve_ai(&settings).unwrap();
        assert_eq!(
            model_only.route(AiRole::Prep).unwrap().model,
            INCLUDED_PREP_MODEL
        );
        assert_eq!(
            model_only.route(AiRole::Cue).unwrap().model,
            "google/gemini-fast"
        );
        assert_eq!(
            model_only.route(AiRole::Reprocess).unwrap(),
            model_only.route(AiRole::Cue).unwrap()
        );

        settings.backchannel_api_key = Some("separate-key".to_string());
        settings.backchannel_base_url = Some("https://api.openai.com/v1".to_string());
        let separate = resolve_ai(&settings).unwrap();
        let cue = separate.route(AiRole::Cue).unwrap();
        assert_eq!(cue.provider, ProviderKind::OpenAi);
        assert_eq!(credential_kind(&cue.credentials), "api-key");
        assert_eq!(cue.model, "google/gemini-fast");
    }

    fn preview_role<'a>(preview: &'a ResolutionPreview, role: &str) -> Option<&'a PreviewRole> {
        preview.roles.iter().find(|entry| entry.role == role)
    }

    #[test]
    fn preview_lists_friendly_labels_for_included_default() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("included".to_string());
        let preview = preview_ai_resolution(&settings).unwrap();

        assert_eq!(preview.mode, "included");
        let distill = preview_role(&preview, "distill").unwrap();
        assert_eq!(distill.model_id, INCLUDED_DISTILL_MODEL);
        assert_eq!(distill.model_label, "Claude Sonnet");
        assert_eq!(distill.provider_label, "Included");
        assert!(!distill.changed_by_override);

        let cue = preview_role(&preview, "cue").unwrap();
        assert_eq!(cue.model_label, "Claude Haiku");
        assert!(!cue.changed_by_override);

        assert!(preview_role(&preview, "indexing").is_none());
    }

    #[test]
    fn preview_marks_cue_override_without_recall_route() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("included".to_string());
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_model = Some("google/gemini-3.1-flash-lite".to_string());
        let preview = preview_ai_resolution(&settings).unwrap();

        let cue = preview_role(&preview, "cue").unwrap();
        assert_eq!(cue.model_label, "Gemini Flash Lite");
        assert!(cue.changed_by_override);
        let reprocess = preview_role(&preview, "reprocess").unwrap();
        assert!(reprocess.changed_by_override);

        // Distillation is untouched by the quick-assists override.
        let distill = preview_role(&preview, "distill").unwrap();
        assert_eq!(distill.model_label, "Claude Sonnet");
        assert!(!distill.changed_by_override);

        assert!(preview_role(&preview, "indexing").is_none());
    }

    #[test]
    fn preview_chatgpt_omits_indexing_and_labels_subscription() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.chatgpt_model = None;
        let preview = preview_ai_resolution(&settings).unwrap();

        assert_eq!(preview.mode, "chatgpt");
        assert!(preview_role(&preview, "indexing").is_none());
        let distill = preview_role(&preview, "distill").unwrap();
        assert_eq!(distill.provider_label, "ChatGPT subscription");
        assert_eq!(distill.model_id, "gpt-5.5");
        assert_eq!(distill.model_label, "GPT-5.5");
        let cue = preview_role(&preview, "cue").unwrap();
        assert_eq!(cue.model_label, "GPT-5.5");
        assert!(!cue.changed_by_override);
    }

    #[test]
    fn preview_unknown_model_falls_back_to_raw_id() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("api".to_string());
        settings.api_key = Some("test-api-key".to_string());
        settings.ai_model = Some("my-cool-model".to_string());
        let preview = preview_ai_resolution(&settings).unwrap();

        let distill = preview_role(&preview, "distill").unwrap();
        assert_eq!(distill.provider_label, "OpenAI");
        assert_eq!(distill.model_id, "my-cool-model");
        assert_eq!(distill.model_label, "my-cool-model");
    }

    #[test]
    fn readiness_serializes_all_modes_and_effective_mode() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        let readiness = ai_readiness(&settings, true, false);
        let value = serde_json::to_value(&readiness).unwrap();

        assert_eq!(value["mode"], "chatgpt");
        assert_eq!(value["modes"]["included"]["ready"], true);
        assert_eq!(value["modes"]["chatgpt"]["ready"], false);
        assert_eq!(value["modes"]["api"]["ready"], false);
        assert!(value["modes"]["included"]["reason"].is_string());
        assert!(value["modes"]["chatgpt"]["reason"]
            .as_str()
            .unwrap()
            .contains("Sign in"));

        // API readiness derives purely from a present key.
        settings.api_key = Some("sk-test".to_string());
        let with_key = ai_readiness(&settings, false, false);
        assert!(with_key.modes.api.ready);
        assert!(!with_key.modes.included.ready);
    }

    #[test]
    fn openrouter_byok_blank_model_has_role_specific_defaults() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("api".to_string());
        settings.api_key = Some("sk-or-test".to_string());
        settings.ai_model = None;
        let resolved = resolve_ai(&settings).unwrap();
        assert_eq!(
            resolved.route(AiRole::Distill).unwrap().model,
            INCLUDED_DISTILL_MODEL
        );
        assert_eq!(
            resolved.route(AiRole::Prep).unwrap().model,
            INCLUDED_PREP_MODEL
        );
        assert_eq!(
            resolved.route(AiRole::Cue).unwrap().model,
            INCLUDED_PREP_MODEL
        );
    }

    #[test]
    fn snapshot_refresh_errors_are_formatted_for_durable_logging() {
        assert_eq!(
            backchannel_snapshot_config_error_message("missing API key"),
            "could not refresh live snapshot AI config: missing API key"
        );
    }

    #[tokio::test]
    async fn trims_optional_strings() {
        assert_eq!(
            clean_optional(Some("  gpt-5.5  ")).as_deref(),
            Some("gpt-5.5")
        );
        assert_eq!(clean_optional(Some("  ")), None);
        assert_eq!(clean_optional(None), None);
    }

    #[tokio::test]
    async fn recognizes_default_openai_base_url() {
        assert!(is_default_openai_base_url("https://api.openai.com/v1/"));
        assert!(is_default_openai_base_url(" HTTPS://API.OPENAI.COM/V1 "));
        assert!(!is_default_openai_base_url("https://example.test/v1"));
    }

    #[tokio::test]
    async fn selects_included_mode_by_default() {
        let settings = crate::Settings::default();
        assert_eq!(AiMode::from_settings(&settings), AiMode::Included);
    }

    #[tokio::test]
    async fn chatgpt_mode_honors_explicit_model() {
        // Codex via the ChatGPT subscription: chatgpt mode + a codex model id.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.chatgpt_model = Some("gpt-5.1-codex".to_string());

        let config = configure_ai_for_distill(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai-codex"));
        assert_eq!(config.1.as_deref(), Some("gpt-5.1-codex"));
        assert_eq!(config.2, None);
    }

    #[tokio::test]
    async fn chatgpt_mode_ignores_openrouter_model_slug() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.chatgpt_model = None;
        settings.ai_model = Some("google/gemini-3-flash-preview".to_string());

        for config in [
            configure_ai_for_distill(&settings).await.unwrap(),
            configure_ai_for_backchannel(&settings).await.unwrap(),
            configure_ai_for_reprocess(&settings).await.unwrap(),
        ] {
            assert_eq!(config.0.as_deref(), Some("openai-codex"));
            assert_eq!(config.1.as_deref(), Some("gpt-5.5"));
            assert_eq!(config.2, None);
        }
    }

    #[tokio::test]
    async fn selects_api_key_mode_without_custom_provider_file() {
        // Explicit Settings win: the chosen model is used regardless of any
        // ambient OPENAI_MODEL in the environment.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("api".to_string());
        settings.api_key = Some(" key ".to_string());
        settings.ai_model = Some(" model ".to_string());
        settings.ai_base_url = Some("https://api.openai.com/v1/".to_string());

        let config = configure_ai_for_distill(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai"));
        assert_eq!(config.1.as_deref(), Some("model"));
        assert_eq!(config.2.as_deref(), Some("key"));
    }

    #[tokio::test]
    async fn backchannel_reuses_distill_by_default() {
        // Default flag (absent → same as distillation): cues follow the distill
        // model even if separate backchannel fields happen to be populated.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.backchannel_same_as_distill = None;
        settings.backchannel_api_key = Some("cue-key".to_string());
        settings.backchannel_model = Some("cheap-model".to_string());

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai-codex"));
        assert_eq!(config.1.as_deref(), Some("gpt-5.5"));
        assert_eq!(config.2, None);
    }

    #[tokio::test]
    async fn included_backchannel_uses_fast_model() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("included".to_string());
        settings.backchannel_model = None;

        let resolved = resolve_ai(&settings).unwrap();
        assert_eq!(resolved.models.distill, INCLUDED_DISTILL_MODEL);
        assert_eq!(resolved.models.prep, INCLUDED_PREP_MODEL);
    }

    #[tokio::test]
    async fn compatible_provider_merge_preserves_existing_models() {
        let existing = serde_json::json!({
            "models": [{
                "id": INCLUDED_DISTILL_MODEL,
                "name": "Claude",
                "reasoning": false,
                "input": ["text"],
                "contextWindow": 128000,
                "maxTokens": 16384
            }]
        });

        let merged = merge_margins_compatible_provider(
            existing,
            INCLUDED_OPENROUTER_BASE_URL,
            INCLUDED_PREP_MODEL,
        );
        let models = merged["models"].as_array().unwrap();
        assert!(models
            .iter()
            .any(|model| model["id"] == INCLUDED_DISTILL_MODEL));
        assert!(models
            .iter()
            .any(|model| model["id"] == INCLUDED_PREP_MODEL));
    }

    #[test]
    fn provider_ids_are_canonical_endpoint_specific_and_secret_free() {
        let canonical = provider_id_for_endpoint("https://example.test/v1/").unwrap();
        assert_eq!(
            canonical,
            provider_id_for_endpoint("https://example.test/v1").unwrap()
        );
        assert_ne!(
            canonical,
            provider_id_for_endpoint("https://other.test/v1").unwrap()
        );
        assert!(canonical.starts_with("margins-openai-compatible-"));
        assert!(!canonical.contains("example"));
        assert!(canonical_provider_endpoint("https://user:secret@example.test/v1").is_err());
        assert!(canonical_provider_endpoint("https://example.test/v1?key=secret").is_err());
    }

    #[test]
    fn concurrent_endpoint_registration_keeps_sessions_routed_and_json_atomic() {
        let path = temp_models_path("concurrent-routing");
        let parent = path.parent().unwrap().to_path_buf();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let first_path = path.clone();
        let first_barrier = barrier.clone();
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            openai_compatible_config_at(
                &first_path,
                "request-a-key".to_string(),
                Some("https://included.test/v1/".to_string()),
                "model-a".to_string(),
            )
            .unwrap()
        });
        let second_path = path.clone();
        let second = std::thread::spawn(move || {
            barrier.wait();
            openai_compatible_config_at(
                &second_path,
                "request-b-key".to_string(),
                Some("https://custom.test/v1".to_string()),
                "model-b".to_string(),
            )
            .unwrap()
        });
        let first = first.join().unwrap();
        let second = second.join().unwrap();
        assert_ne!(first.0, second.0);

        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let first_id = first.0.as_deref().unwrap();
        let second_id = second.0.as_deref().unwrap();
        assert_eq!(
            root["providers"][first_id]["baseUrl"],
            "https://included.test/v1"
        );
        assert_eq!(
            root["providers"][second_id]["baseUrl"],
            "https://custom.test/v1"
        );
        assert_eq!(first.3, None);
        assert_eq!(second.3, None);
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn interleaved_stale_preflight_cannot_retarget_current_session_provider() {
        let path = temp_models_path("settings-switch");
        let parent = path.parent().unwrap().to_path_buf();
        let (stale_entered_tx, stale_entered_rx) = std::sync::mpsc::channel();
        let (release_stale_tx, release_stale_rx) = std::sync::mpsc::channel();
        let stale_path = path.clone();
        let stale = std::thread::spawn(move || {
            ensure_margins_openai_compatible_model_at_with_hook(
                &stale_path,
                INCLUDED_OPENROUTER_BASE_URL,
                INCLUDED_DISTILL_MODEL,
                || {
                    stale_entered_tx.send(()).unwrap();
                    release_stale_rx.recv().unwrap();
                },
            )
            .unwrap()
        });
        stale_entered_rx.recv().unwrap();

        let (current_started_tx, current_started_rx) = std::sync::mpsc::channel();
        let (current_done_tx, current_done_rx) = std::sync::mpsc::channel();
        let current_path = path.clone();
        let current = std::thread::spawn(move || {
            current_started_tx.send(()).unwrap();
            let config = openai_compatible_config_at(
                &current_path,
                "current-key".to_string(),
                Some("https://current.test/v1".to_string()),
                "current-model".to_string(),
            )
            .unwrap();
            current_done_tx.send(()).unwrap();
            config
        });
        current_started_rx.recv().unwrap();
        assert!(current_done_rx
            .recv_timeout(std::time::Duration::from_millis(50))
            .is_err());
        release_stale_tx.send(()).unwrap();
        let stale = stale.join().unwrap();
        let current = current.join().unwrap();

        assert_ne!(current.0.as_deref(), Some(stale.as_str()));
        let root: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let current_id = current.0.as_deref().unwrap();
        assert_eq!(
            root["providers"][current_id]["baseUrl"],
            "https://current.test/v1"
        );
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn atomic_provider_replacement_never_exposes_partial_json_to_readers() {
        let path = temp_models_path("atomic-readers");
        let parent = path.parent().unwrap().to_path_buf();
        ensure_margins_openai_compatible_model_at(&path, "https://seed.test/v1", "seed-model")
            .unwrap();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let observed_provider_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let reader_path = path.clone();
        let reader_done = done.clone();
        let reader_observed = observed_provider_count.clone();
        let reader = std::thread::spawn(move || {
            while !reader_done.load(std::sync::atomic::Ordering::Acquire) {
                let raw = std::fs::read_to_string(&reader_path).unwrap();
                let root = serde_json::from_str::<serde_json::Value>(&raw).unwrap();
                let count = root["providers"]
                    .as_object()
                    .map_or(0, |providers| providers.len());
                reader_observed.fetch_max(count, std::sync::atomic::Ordering::Release);
            }
        });
        for index in 0..10 {
            ensure_margins_openai_compatible_model_at(
                &path,
                &format!("https://endpoint-{index}.test/v1"),
                &format!("model-{index}"),
            )
            .unwrap();
            let expected = index + 2;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
            while observed_provider_count.load(std::sync::atomic::Ordering::Acquire) < expected {
                assert!(
                    std::time::Instant::now() < deadline,
                    "reader missed replacement {index}"
                );
                std::thread::yield_now();
            }
        }
        done.store(true, std::sync::atomic::Ordering::Release);
        reader.join().unwrap();
        assert_eq!(
            observed_provider_count.load(std::sync::atomic::Ordering::Acquire),
            11
        );
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    #[ignore]
    fn provider_lock_child() {
        let models_path =
            std::path::PathBuf::from(std::env::var("MARGINS_LOCK_TEST_PATH").unwrap());
        let acquired_path =
            std::path::PathBuf::from(std::env::var("MARGINS_LOCK_TEST_ACQUIRED").unwrap());
        let hold_ms = std::env::var("MARGINS_LOCK_TEST_HOLD_MS")
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let _lock = ProviderConfigFileLock::acquire(&models_path).unwrap();
        std::fs::write(acquired_path, b"acquired").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(hold_ms));
    }

    #[test]
    fn provider_lock_serializes_distinct_processes_even_when_lockfile_looks_old() {
        let path = temp_models_path("cross-process-lock");
        let parent = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&parent).unwrap();
        let first_acquired = parent.join("first-acquired");
        let second_acquired = parent.join("second-acquired");
        let test_binary = std::env::current_exe().unwrap();
        let spawn_child = |acquired: &std::path::Path, hold_ms: u64| {
            std::process::Command::new(&test_binary)
                .args([
                    "--exact",
                    "ai_config::tests::provider_lock_child",
                    "--ignored",
                    "--nocapture",
                ])
                .env("MARGINS_LOCK_TEST_PATH", &path)
                .env("MARGINS_LOCK_TEST_ACQUIRED", acquired)
                .env("MARGINS_LOCK_TEST_HOLD_MS", hold_ms.to_string())
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap()
        };

        let mut first = spawn_child(&first_acquired, 350);
        wait_for_test_path(&first_acquired);
        let lock_file = std::fs::OpenOptions::new()
            .write(true)
            .open(path.with_extension("json.lock"))
            .unwrap();
        lock_file
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(60))
            .unwrap();
        let mut second = spawn_child(&second_acquired, 0);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!second_acquired.exists());
        assert!(first.wait().unwrap().success());
        assert!(second.wait().unwrap().success());
        assert!(second_acquired.exists());
        let _ = std::fs::remove_dir_all(parent);
    }

    fn wait_for_test_path(path: &std::path::Path) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !path.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for child process"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[tokio::test]
    async fn included_broker_fetch_sends_bootstrap_and_reads_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let n = std::io::Read::read(&mut stream, &mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.contains("GET /free-config HTTP/1.1"));
            assert!(request.contains("x-margins-bootstrap-id: test-bootstrap"));
            assert!(!request.contains("x-enzyme-bootstrap-id:"));
            let body = r#"{"data":{"key":"sk-or-test"}}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
        });

        let key = fetch_included_openrouter_api_key_with_bootstrap(
            &format!("http://{addr}/free-config"),
            "test-bootstrap",
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(key.api_key, "sk-or-test");
    }

    #[tokio::test]
    async fn included_broker_fetch_accepts_top_level_api_key() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0_u8; 1024];
            loop {
                let n = std::io::Read::read(&mut stream, &mut buf).unwrap();
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8_lossy(&request);
            assert!(request.contains("GET /margins/free-config HTTP/1.1"));
            assert!(request.contains("x-margins-bootstrap-id: margins-bootstrap"));
            let body = r#"{"api_key":"sk-or-margins-test","base_url":"https://openrouter.ai/api/v1","model":"anthropic/claude-sonnet-4.6","provider":"margins-openrouter-included"}"#;
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
        });

        let key = fetch_included_openrouter_api_key_with_bootstrap(
            &format!("http://{addr}/margins/free-config"),
            "margins-bootstrap",
        )
        .await
        .unwrap();
        server.join().unwrap();
        assert_eq!(key.api_key, "sk-or-margins-test");
    }

    #[tokio::test]
    async fn broker_auth_status_is_classified_before_unreadable_body() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 1024];
            let _ = std::io::Read::read(&mut stream, &mut request);
            std::io::Write::write_all(
                &mut stream,
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 99\r\n\r\ntruncated",
            )
            .unwrap();
        });
        let result = fetch_included_openrouter_api_key_with_bootstrap(
            &format!("http://{addr}/free-config"),
            "test-bootstrap",
        )
        .await;
        let error = match result {
            Err(error) => error,
            Ok(_) => panic!("401 response must fail before body parsing"),
        };
        server.join().unwrap();
        assert!(!error.transient);
        assert!(error.message.contains("401"));
        assert!(!error.message.contains("unreadable"));
    }

    struct FakeIncludedLeaseKeychain {
        entries: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    }

    impl FakeIncludedLeaseKeychain {
        fn new(entries: &[(&str, &str)]) -> Self {
            Self {
                entries: std::sync::Mutex::new(
                    entries
                        .iter()
                        .map(|(account, value)| (account.to_string(), value.to_string()))
                        .collect(),
                ),
            }
        }

        fn snapshot(&self) -> std::collections::BTreeMap<String, String> {
            self.entries.lock().unwrap().clone()
        }
    }

    impl IncludedLeaseKeychain for FakeIncludedLeaseKeychain {
        fn get(&self, account: &str) -> Option<String> {
            self.entries.lock().unwrap().get(account).cloned()
        }

        fn set(&self, account: &str, value: &str) -> Result<(), String> {
            self.entries
                .lock()
                .unwrap()
                .insert(account.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, account: &str) -> Result<(), String> {
            self.entries.lock().unwrap().remove(account);
            Ok(())
        }
    }

    #[test]
    fn included_keychain_round_trip_uses_one_serialized_item() {
        let keychain = FakeIncludedLeaseKeychain::new(&[]);
        let expected = IncludedBrokerKey {
            api_key: "sk-or-round-trip".to_string(),
            expires_at: Some(1_700_000_600),
        };
        persist_included_broker_key_with(&keychain, &expected).unwrap();
        let entries = keychain.snapshot();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(INCLUDED_LEASE_ACCOUNT));
        assert_eq!(
            load_persisted_included_broker_key_with(&keychain).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn included_keychain_migrates_old_two_item_layout_atomically() {
        let keychain = FakeIncludedLeaseKeychain::new(&[
            ("api-key", "sk-or-legacy"),
            ("expires-at", "1700000600"),
        ]);
        let lease = load_persisted_included_broker_key_with(&keychain)
            .unwrap()
            .unwrap();
        assert_eq!(lease.api_key, "sk-or-legacy");
        assert_eq!(lease.expires_at, Some(1_700_000_600));
        let entries = keychain.snapshot();
        assert_eq!(entries.len(), 1);
        assert!(entries.contains_key(INCLUDED_LEASE_ACCOUNT));
        assert!(!entries.contains_key("api-key"));
        assert!(!entries.contains_key("expires-at"));
    }

    #[tokio::test]
    async fn provider_rejected_persisted_lease_is_not_reinstalled() {
        let now = 1_700_000_000;
        let keychain = FakeIncludedLeaseKeychain::new(&[]);
        persist_included_broker_key_with(
            &keychain,
            &IncludedBrokerKey {
                api_key: "persisted-rejected-key".to_string(),
                expires_at: Some(now + 3_600),
            },
        )
        .unwrap();
        let manager = IncludedLeaseManager::new();
        let first = manager
            .acquire_with("config".to_string(), now, || async {
                Ok(load_persisted_included_broker_key_with(&keychain)
                    .unwrap()
                    .unwrap())
            })
            .await
            .unwrap();
        assert_eq!(first.api_key, "persisted-rejected-key");
        assert!(manager.invalidate_generation(first.generation));
        assert!(manager.provider_rejected_api_key("persisted-rejected-key"));

        let second = manager
            .acquire_with("config".to_string(), now + 1, || async {
                if let Some(persisted) = load_persisted_included_broker_key_with(&keychain).unwrap()
                {
                    if manager.provider_rejected_api_key(&persisted.api_key) {
                        evict_persisted_included_broker_key_with(&keychain, &persisted.api_key)
                            .unwrap();
                    } else {
                        return Ok(persisted);
                    }
                }
                Ok(IncludedBrokerKey {
                    api_key: "replacement-key".to_string(),
                    expires_at: Some(now + 3_601),
                })
            })
            .await
            .unwrap();
        assert_eq!(second.api_key, "replacement-key");
        assert_eq!(
            load_persisted_included_broker_key_with(&keychain).unwrap(),
            None
        );
    }

    #[test]
    fn included_expiry_normalizes_epoch_seconds_millis_and_rfc3339() {
        let seconds = 1_700_000_600;
        assert_eq!(
            normalize_expiry_json(Some(&serde_json::json!(seconds))).unwrap(),
            Some(seconds)
        );
        assert_eq!(
            normalize_expiry_json(Some(&serde_json::json!(seconds * 1_000))).unwrap(),
            Some(seconds)
        );
        assert_eq!(
            normalize_expiry_json(Some(&serde_json::json!(seconds.to_string()))).unwrap(),
            Some(seconds)
        );
        assert_eq!(
            normalize_expiry_json(Some(&serde_json::json!("2023-11-14T22:23:20Z"))).unwrap(),
            Some(seconds)
        );
        assert!(normalize_expiry_json(Some(&serde_json::json!(1.5))).is_err());
        assert!(normalize_expiry_json(Some(&serde_json::json!(0))).is_err());
        assert!(normalize_expiry_json(Some(&serde_json::json!("1969-12-31T23:59:59Z"))).is_err());
        assert!(normalize_expiry_json(Some(&serde_json::json!(32_503_680_001_i64))).is_err());
    }

    #[test]
    fn included_config_fingerprint_contains_only_provisioning_inputs() {
        let fingerprint = included_config_fingerprint();
        assert_eq!(fingerprint.len(), 16);
        assert!(fingerprint
            .chars()
            .all(|character| character.is_ascii_hexdigit()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn included_lease_singleflight_collapses_concurrent_cold_misses() {
        let manager = std::sync::Arc::new(IncludedLeaseManager::new());
        let fetches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let now = 1_700_000_000;
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let manager = manager.clone();
            let fetches = fetches.clone();
            tasks.push(tokio::spawn(async move {
                manager
                    .acquire_with("config-a".to_string(), now, || async move {
                        fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                        Ok(IncludedBrokerKey {
                            api_key: "singleflight-key".to_string(),
                            expires_at: Some(now + 3_600),
                        })
                    })
                    .await
                    .unwrap()
            }));
        }
        let mut generations = Vec::new();
        for task in tasks {
            generations.push(task.await.unwrap().generation);
        }
        assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(generations.iter().all(|generation| *generation == 1));
    }

    #[tokio::test]
    async fn newly_fetched_lease_must_clear_operation_safety_horizon() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        let rejected = manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "near-expiry".to_string(),
                    expires_at: Some(now + INCLUDED_OPERATION_SAFETY_SECS),
                })
            })
            .await;
        let error = match rejected {
            Err(error) => error,
            Ok(_) => panic!("near-expiry credential should be rejected"),
        };
        assert!(error.contains("credential that expires too soon"));
        assert_eq!(manager.active_generation("config-a"), None);

        let manager = IncludedLeaseManager::new();
        let accepted = manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "just-valid".to_string(),
                    expires_at: Some(now + INCLUDED_OPERATION_SAFETY_SECS + 1),
                })
            })
            .await
            .unwrap();
        assert_eq!(accepted.source, IncludedLeaseSource::Broker);
    }

    #[test]
    fn operation_horizon_overflow_fails_closed() {
        let fetched = IncludedLease::from_fetched(
            IncludedBrokerKey {
                api_key: "overflow-key".to_string(),
                expires_at: None,
            },
            i64::MAX - INCLUDED_OPERATION_SAFETY_SECS + 1,
        );
        assert!(fetched.is_err());
        assert!(validate_broker_key_safety(
            &IncludedBrokerKey {
                api_key: "overflow-key".to_string(),
                expires_at: None,
            },
            i64::MAX - INCLUDED_OPERATION_SAFETY_SECS + 1,
        )
        .is_err());
    }

    #[tokio::test]
    async fn refresh_failure_never_reuses_lease_inside_operation_horizon() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        let fresh = manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "leased".to_string(),
                    expires_at: Some(now + 600),
                })
            })
            .await
            .unwrap();
        assert_eq!(fresh.source, IncludedLeaseSource::Broker);

        let stale = manager
            .acquire_with("config-a".to_string(), now + 301, || async {
                Err(BrokerFetchError::transient("broker unavailable"))
            })
            .await;
        assert!(stale.is_err());

        let hard_expired = manager
            .acquire_with("config-a".to_string(), now + 600, || async {
                Err(BrokerFetchError::transient("broker unavailable"))
            })
            .await;
        assert!(hard_expired.is_err());

        let manager = IncludedLeaseManager::new();
        manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "leased".to_string(),
                    expires_at: Some(now + 600),
                })
            })
            .await
            .unwrap();
        let permanent_failure = manager
            .acquire_with("config-a".to_string(), now + 301, || async {
                Err(BrokerFetchError::permanent("broker contract rejected"))
            })
            .await;
        assert!(permanent_failure.is_err());
    }

    #[tokio::test]
    async fn cold_failure_cooldown_prevents_repeat_fetches() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        let fetches = std::sync::atomic::AtomicUsize::new(0);
        let first = manager
            .acquire_with("config-a".to_string(), now, || async {
                fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(BrokerFetchError::transient("broker unavailable"))
            })
            .await;
        assert!(first.is_err());
        let second = manager
            .acquire_with("config-a".to_string(), now + 1, || async {
                fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(BrokerFetchError::transient("must not run"))
            })
            .await;
        assert!(second.is_err());
        assert_eq!(fetches.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn included_lease_singleflight_sends_one_broker_http_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut requests = 0;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut last_request = None;
            loop {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        requests += 1;
                        last_request = Some(std::time::Instant::now());
                        let mut request = [0_u8; 2048];
                        let _ = std::io::Read::read(&mut stream, &mut request);
                        let body = r#"{"data":{"key":"sk-or-singleflight"}}"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if last_request.is_some_and(|last| {
                            last.elapsed() >= std::time::Duration::from_millis(200)
                        }) || std::time::Instant::now() >= deadline
                        {
                            return requests;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                    Err(error) => panic!("mock broker failed: {error}"),
                }
            }
        });

        let manager = std::sync::Arc::new(IncludedLeaseManager::new());
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(12));
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let manager = manager.clone();
            let barrier = barrier.clone();
            let url = format!("http://{addr}/free-config");
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                manager
                    .acquire_with("mock-broker".to_string(), 1_700_000_000, || async move {
                        fetch_included_openrouter_api_key_with_bootstrap(&url, "bootstrap-test")
                            .await
                    })
                    .await
                    .unwrap()
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().api_key, "sk-or-singleflight");
        }
        assert_eq!(server.join().unwrap(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn configuration_refresh_gates_are_independent() {
        let manager = std::sync::Arc::new(IncludedLeaseManager::new());
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let now = 1_700_000_000;
        let first_manager = manager.clone();
        let first_barrier = barrier.clone();
        let first = tokio::spawn(async move {
            first_manager
                .acquire_with("config-a".to_string(), now, || async move {
                    first_barrier.wait().await;
                    Ok(IncludedBrokerKey {
                        api_key: "config-a-key".to_string(),
                        expires_at: Some(now + 3_600),
                    })
                })
                .await
        });
        let second_manager = manager.clone();
        let second = tokio::spawn(async move {
            second_manager
                .acquire_with("config-b".to_string(), now, || async move {
                    barrier.wait().await;
                    Ok(IncludedBrokerKey {
                        api_key: "config-b-key".to_string(),
                        expires_at: Some(now + 3_600),
                    })
                })
                .await
        });
        let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            tokio::join!(first, second)
        })
        .await
        .expect("different configuration refreshes must not share one gate");
        assert!(first.unwrap().is_ok());
        assert!(second.unwrap().is_ok());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_a_failure_does_not_clear_b_success_or_share_cooldown() {
        let manager = std::sync::Arc::new(IncludedLeaseManager::new());
        let barrier = std::sync::Arc::new(tokio::sync::Barrier::new(2));
        let now = 1_700_000_000;
        let a_manager = manager.clone();
        let a_barrier = barrier.clone();
        let a = tokio::spawn(async move {
            a_manager
                .acquire_with("config-a".to_string(), now, || async move {
                    a_barrier.wait().await;
                    Err(BrokerFetchError::transient("config-a unavailable"))
                })
                .await
        });
        let b_manager = manager.clone();
        let b = tokio::spawn(async move {
            b_manager
                .acquire_with("config-b".to_string(), now, || async move {
                    barrier.wait().await;
                    Ok(IncludedBrokerKey {
                        api_key: "config-b-key".to_string(),
                        expires_at: Some(now + 3_600),
                    })
                })
                .await
        });
        assert!(a.await.unwrap().is_err());
        let b = b.await.unwrap().unwrap();
        assert_eq!(manager.active_generation("config-b"), Some(b.generation));

        let a_fetches = std::sync::atomic::AtomicUsize::new(0);
        assert!(manager
            .acquire_with("config-a".to_string(), now + 1, || async {
                a_fetches.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(BrokerFetchError::transient("must remain in cooldown"))
            })
            .await
            .is_err());
        assert_eq!(a_fetches.load(std::sync::atomic::Ordering::SeqCst), 0);
        let b_cached = manager
            .acquire_with("config-b".to_string(), now + 1, || async {
                Err(BrokerFetchError::permanent("must not fetch"))
            })
            .await
            .unwrap();
        assert_eq!(b_cached.generation, b.generation);
    }

    #[tokio::test]
    async fn config_fingerprint_mismatch_never_reuses_stale_lease() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        manager
            .acquire_with("config-a".to_string(), now, || async {
                Ok(IncludedBrokerKey {
                    api_key: "config-a-key".to_string(),
                    expires_at: None,
                })
            })
            .await
            .unwrap();

        let mismatch = manager
            .acquire_with("config-b".to_string(), now + 1, || async {
                Err(BrokerFetchError::transient("config-b unavailable"))
            })
            .await;
        assert!(mismatch.is_err());
        assert_eq!(manager.active_generation("config-b"), None);
    }

    #[tokio::test]
    async fn generation_invalidation_blocks_stale_and_ignores_old_generation() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        let first = manager
            .acquire_with("config-a".to_string(), now, || async {
                Ok(IncludedBrokerKey {
                    api_key: "generation-one".to_string(),
                    expires_at: None,
                })
            })
            .await
            .unwrap();
        assert_eq!(
            manager.generation_for_key("config-a", "generation-one"),
            Some(first.generation)
        );
        assert_eq!(manager.generation_for_key("config-a", "other-key"), None);
        assert!(manager.invalidate_generation(first.generation));
        let rejected_stale = manager
            .acquire_with("config-a".to_string(), now + 1, || async {
                Err(BrokerFetchError::transient("broker unavailable"))
            })
            .await;
        assert!(rejected_stale.is_err());

        let second = manager
            .acquire_with(
                "config-a".to_string(),
                now + 1 + INCLUDED_TRANSIENT_FAILURE_COOLDOWN_SECS,
                || async {
                    Ok(IncludedBrokerKey {
                        api_key: "generation-two".to_string(),
                        expires_at: None,
                    })
                },
            )
            .await
            .unwrap();
        assert!(second.generation > first.generation);
        assert!(!manager.invalidate_generation(first.generation));
        assert!(manager.invalidate_generation(second.generation));
    }

    #[tokio::test]
    async fn provider_error_cancellation_interleavings_are_fail_safe() {
        let now = 1_700_000_000;
        let cancelled_manager = IncludedLeaseManager::new();
        let cancelled_lease = cancelled_manager
            .acquire_with("cancel-first".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "cancel-first-key".to_string(),
                    expires_at: Some(now + 3_600),
                })
            })
            .await
            .unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(true);
        assert_eq!(
            handle_included_ai_provider_error_with(
                &cancelled_manager,
                Some(cancelled_lease.generation),
                Some(&cancel),
                "distill_setup",
            ),
            ProviderErrorDisposition::Cancelled
        );
        assert_eq!(
            cancelled_manager.active_generation("cancel-first"),
            Some(cancelled_lease.generation)
        );

        let error_manager = IncludedLeaseManager::new();
        let error_lease = error_manager
            .acquire_with("error-first".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "error-first-key".to_string(),
                    expires_at: Some(now + 3_600),
                })
            })
            .await
            .unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        assert_eq!(
            handle_included_ai_provider_error_with(
                &error_manager,
                Some(error_lease.generation),
                Some(&cancel),
                "prep_prompt",
            ),
            ProviderErrorDisposition::Invalidated
        );
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(error_manager
            .fresh_cached_acquisition("error-first", now + 1)
            .is_none());
    }

    #[tokio::test]
    async fn generation_overflow_fails_closed_without_reusing_max() {
        let now = 1_700_000_000;
        let manager = IncludedLeaseManager::new();
        manager.lock_state().next_generation = u64::MAX;
        let result = manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "must-not-be-installed".to_string(),
                    expires_at: Some(now + 3_600),
                })
            })
            .await;
        assert!(result
            .unwrap_err()
            .contains("generation space is exhausted"));
        assert_eq!(manager.active_generation("config-a"), None);
    }

    #[tokio::test]
    async fn poisoned_state_mutex_recovers_without_permanent_outage() {
        let manager = std::sync::Arc::new(IncludedLeaseManager::new());
        let poison_manager = manager.clone();
        assert!(std::thread::spawn(move || {
            let _guard = poison_manager.state.lock().unwrap();
            panic!("intentional poison");
        })
        .join()
        .is_err());
        let now = 1_700_000_000;
        let lease = manager
            .acquire_with("config-a".to_string(), now, || async move {
                Ok(IncludedBrokerKey {
                    api_key: "post-poison-key".to_string(),
                    expires_at: Some(now + 3_600),
                })
            })
            .await
            .unwrap();
        assert_eq!(lease.generation, 1);
    }

    #[tokio::test]
    async fn included_models_ignore_stale_hidden_overrides() {
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("included".to_string());
        settings.ai_model = Some("openai/gpt-4.1-nano".to_string());
        settings.backchannel_model = Some("openai/gpt-4.1-nano".to_string());

        let resolved = resolve_ai(&settings).unwrap();
        assert_eq!(resolved.models.distill, INCLUDED_DISTILL_MODEL);
        assert_eq!(resolved.models.prep, INCLUDED_PREP_MODEL);
    }

    #[tokio::test]
    async fn backchannel_uses_separate_model_when_opted_in() {
        let mut settings = crate::Settings::default();
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("cue-key".to_string());
        settings.backchannel_model = Some("cheap-model".to_string());

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai"));
        assert_eq!(config.1.as_deref(), Some("cheap-model"));
        assert_eq!(config.2.as_deref(), Some("cue-key"));
    }

    #[tokio::test]
    async fn backchannel_infers_openrouter_for_openrouter_key_without_base_url() {
        let temp = std::env::temp_dir().join(format!(
            "margins-ai-config-openrouter-backchannel-test-{}",
            std::process::id(),
        ));
        std::env::set_var("MARGINS_PI_AGENT_DIR", &temp);

        let mut settings = crate::Settings::default();
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("sk-or-v1-test".to_string());
        settings.backchannel_base_url = None;
        settings.backchannel_model = None;

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert!(config
            .0
            .as_deref()
            .is_some_and(|provider| provider.starts_with("margins-openai-compatible-")));
        assert_eq!(config.1.as_deref(), Some(INCLUDED_PREP_MODEL));
        assert_eq!(config.2.as_deref(), Some("sk-or-v1-test"));

        let _ = std::fs::remove_dir_all(temp);
        std::env::remove_var("MARGINS_PI_AGENT_DIR");
    }

    #[tokio::test]
    async fn backchannel_corrects_default_openai_base_url_for_openrouter_key() {
        let temp = std::env::temp_dir().join(format!(
            "margins-ai-config-openrouter-default-base-test-{}",
            std::process::id(),
        ));
        std::env::set_var("MARGINS_PI_AGENT_DIR", &temp);

        let mut settings = crate::Settings::default();
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("sk-or-v1-test".to_string());
        settings.backchannel_base_url = Some("https://api.openai.com/v1".to_string());
        settings.backchannel_model = None;

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert!(config
            .0
            .as_deref()
            .is_some_and(|provider| provider.starts_with("margins-openai-compatible-")));
        assert_eq!(config.1.as_deref(), Some(INCLUDED_PREP_MODEL));
        assert_eq!(config.2.as_deref(), Some("sk-or-v1-test"));

        let _ = std::fs::remove_dir_all(temp);
        std::env::remove_var("MARGINS_PI_AGENT_DIR");
    }

    #[tokio::test]
    async fn backchannel_honors_custom_base_url_for_openrouter_shaped_key() {
        let temp = std::env::temp_dir().join(format!(
            "margins-ai-config-openrouter-custom-base-test-{}",
            std::process::id(),
        ));
        std::env::set_var("MARGINS_PI_AGENT_DIR", &temp);

        let mut settings = crate::Settings::default();
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("sk-or-v1-test".to_string());
        settings.backchannel_base_url = Some("https://example.test/v1".to_string());
        settings.backchannel_model = Some("custom-model".to_string());

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert!(config
            .0
            .as_deref()
            .is_some_and(|provider| provider.starts_with("margins-openai-compatible-")));
        assert_eq!(config.1.as_deref(), Some("custom-model"));
        assert_eq!(config.2.as_deref(), Some("sk-or-v1-test"));

        let _ = std::fs::remove_dir_all(temp);
        std::env::remove_var("MARGINS_PI_AGENT_DIR");
    }

    #[tokio::test]
    async fn api_mode_infers_openrouter_for_openrouter_key_without_base_url() {
        let temp = std::env::temp_dir().join(format!(
            "margins-ai-config-openrouter-api-test-{}",
            std::process::id(),
        ));
        std::env::set_var("MARGINS_PI_AGENT_DIR", &temp);

        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("api".to_string());
        settings.api_key = Some("sk-or-v1-test".to_string());
        settings.ai_base_url = None;
        settings.ai_model = None;

        let config = configure_ai_for_distill(&settings).await.unwrap();
        assert!(config
            .0
            .as_deref()
            .is_some_and(|provider| provider.starts_with("margins-openai-compatible-")));
        assert_eq!(config.1.as_deref(), Some(INCLUDED_DISTILL_MODEL));
        assert_eq!(config.2.as_deref(), Some("sk-or-v1-test"));

        let _ = std::fs::remove_dir_all(temp);
        std::env::remove_var("MARGINS_PI_AGENT_DIR");
    }

    #[tokio::test]
    async fn reprocess_follows_backchannel_resolution_for_separate_cue_model() {
        // When the user opts into a distinct cue/fast model, reprocess must use
        // that same model (via the backchannel path), never the distill model or
        // a hardcoded constant.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = Some("cue-key".to_string());
        settings.backchannel_model = Some("cheap-model".to_string());

        let reprocess = configure_ai_for_reprocess(&settings).await.unwrap();
        let backchannel = configure_ai_for_backchannel(&settings).await.unwrap();
        // Reprocess resolution is identical to live-cues resolution.
        assert_eq!(reprocess, backchannel);
        assert_eq!(reprocess.1.as_deref(), Some("cheap-model"));
        assert_eq!(reprocess.2.as_deref(), Some("cue-key"));
        // And it diverges from the distill model, proving it isn't reusing distill.
        let distill = configure_ai_for_distill(&settings).await.unwrap();
        assert_ne!(reprocess.1, distill.1);
    }

    #[tokio::test]
    async fn reprocess_reuses_distill_when_cues_not_separated() {
        // Default (cues == distill): reprocess follows distill, matching cues.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.backchannel_same_as_distill = None;

        let reprocess = configure_ai_for_reprocess(&settings).await.unwrap();
        let backchannel = configure_ai_for_backchannel(&settings).await.unwrap();
        assert_eq!(reprocess, backchannel);
    }

    #[tokio::test]
    async fn backchannel_model_only_override_uses_api_credentials() {
        // api mode + model-only override (no separate cue key) → the note-making
        // API key/base is reused with the cue model swapped in.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("api".to_string());
        settings.api_key = Some("api-key".to_string());
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = None;
        settings.backchannel_model = Some("gpt-4.1-mini".to_string());

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai"));
        assert_eq!(config.1.as_deref(), Some("gpt-4.1-mini"));
        assert_eq!(config.2.as_deref(), Some("api-key"));
    }

    #[tokio::test]
    async fn backchannel_model_only_override_uses_codex_for_chatgpt() {
        // chatgpt mode + model-only override → codex provider carrying the cue model.
        let mut settings = crate::Settings::default();
        settings.ai_mode = Some("chatgpt".to_string());
        settings.backchannel_same_as_distill = Some(false);
        settings.backchannel_api_key = None;
        settings.backchannel_model = Some("gpt-5.1-codex".to_string());

        let config = configure_ai_for_backchannel(&settings).await.unwrap();
        assert_eq!(config.0.as_deref(), Some("openai-codex"));
        assert_eq!(config.1.as_deref(), Some("gpt-5.1-codex"));
        assert_eq!(config.2, None);
    }

    #[tokio::test]
    async fn included_model_only_override_builds_included_config_with_cue_model() {
        // included mode + model-only override → included OpenRouter credentials with
        // the cue model. Exercised via the pure builder so the test needs no live
        // Keychain/network key; the models.json write is redirected to a temp dir.
        let temp =
            std::env::temp_dir().join(format!("margins-ai-config-test-{}", std::process::id(),));
        std::env::set_var("MARGINS_PI_AGENT_DIR", &temp);

        let lease = IncludedLeaseAcquisition {
            api_key: "sk-or-test".to_string(),
            hard_expires_at: Some(1_700_003_600),
            generation: 42,
            source: IncludedLeaseSource::Broker,
            singleflight_wait_ms: 0,
        };
        let config = provision_route(
            &ResolvedRoute {
                provider: ProviderKind::IncludedOpenRouter,
                credentials: CredentialsSource::IncludedBroker,
                model: "google/gemini-3-flash".to_string(),
            },
            Some(&lease),
        )
        .unwrap();

        std::env::remove_var("MARGINS_PI_AGENT_DIR");
        let _ = std::fs::remove_dir_all(&temp);

        assert!(config
            .0
            .as_deref()
            .is_some_and(|provider| provider.starts_with("margins-openai-compatible-")));
        assert_eq!(config.1.as_deref(), Some("google/gemini-3-flash"));
        assert_eq!(config.2.as_deref(), Some("sk-or-test"));
        assert_eq!(config.3, Some(42));
    }

    #[tokio::test]
    async fn unquotes_dotenv_values() {
        assert_eq!(unquote_dotenv_value("\"abc\""), "abc");
        assert_eq!(unquote_dotenv_value("'abc'"), "abc");
        assert_eq!(unquote_dotenv_value("abc"), "abc");
    }
}
