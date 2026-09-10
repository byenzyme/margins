use margins_desktop::pi_distill::{run_pi_distill_blocking, NoteConfig, PiDistillRequest};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Debug, Default, Deserialize)]
struct FixtureSettings {
    session_name: Option<String>,
    event_title: Option<String>,
    people: Option<Vec<String>>,
    inbox_folder: Option<String>,
    people_folder: Option<String>,
    created_date_format: Option<String>,
    note_filename_template: Option<String>,
    distill_instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct FixtureProcessingEvent {
    stage: String,
    message: String,
    progress: Option<f32>,
}

#[derive(Debug, Serialize)]
struct FixtureRunSummary {
    note_path: Option<String>,
    session_file: Option<String>,
    work_dir: String,
    vault_path: Option<String>,
    elapsed_ms: u128,
    events: Vec<FixtureProcessingEvent>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let fixture_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../test-harness/ux-loop/fixtures/customer-call")
        })
        .canonicalize()?;

    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let work_dir = std::env::var("MARGINS_UX_FIXTURE_WORK_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::temp_dir().join(format!("margins-ux-distill-fixture-{}", std::process::id()))
        });
    let margins_dir = work_dir.join(".margins");
    let trace_dir = margins_dir.join("traces");
    std::fs::create_dir_all(&margins_dir)?;
    std::fs::create_dir_all(&trace_dir)?;
    let vault_path = std::env::var("MARGINS_UX_FIXTURE_VAULT_PATH")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from);

    let settings = read_settings(&fixture_dir)?;
    let session_name = settings.session_name.clone().unwrap_or_else(|| {
        fixture_dir
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string()
    });

    let memo_src = fixture_dir.join("memo.md");
    let aligned_src = fixture_dir.join("aligned.md");
    let capture_src = fixture_dir.join("capture-context.md");
    let memo_path = work_dir.join(format!("{session_name}_memo.md"));
    let aligned_path = margins_dir.join(format!("{session_name}_aligned.md"));

    std::fs::copy(&memo_src, &memo_path)?;
    std::fs::copy(&aligned_src, &aligned_path)?;
    let aligned_context = std::fs::read_to_string(&aligned_src)?;
    let capture_context = std::fs::read_to_string(&capture_src).unwrap_or_default();

    let note_config = NoteConfig {
        inbox_folder: settings
            .inbox_folder
            .unwrap_or_else(|| "meetings".to_string()),
        people_folder: settings
            .people_folder
            .unwrap_or_else(|| "people".to_string()),
        created_date_format: settings
            .created_date_format
            .unwrap_or_else(|| "[[%Y-%m-%d]]".to_string()),
        note_filename_template: settings
            .note_filename_template
            .unwrap_or_else(|| "{{date:%Y-%m-%d-%-H-%M-%S}} {{event_title}}".to_string()),
        person_note_template: "# {{name}}\n".to_string(),
        distill_instructions: settings.distill_instructions.unwrap_or_default(),
        people: settings.people.unwrap_or_default(),
        people_candidates: Vec::new(),
        event_title: settings.event_title,
        event_start: None,
    };
    let env_file = load_fixture_dotenv(&repo);
    let included_fixture = clean_env("MARGINS_UX_E2E_AI_MODE").as_deref() == Some("included");
    let included_config = if included_fixture {
        Some(fetch_included_fixture_config()?)
    } else {
        None
    };
    let mut ai_api_key = included_config
        .as_ref()
        .map(|config| config.api_key.clone())
        .or_else(|| fixture_env("MARGINS_UX_E2E_OPENAI_API_KEY", None))
        .or_else(|| fixture_env("OPENAI_API_KEY", Some(&env_file)));
    let ai_base_url = included_config
        .as_ref()
        .and_then(|config| config.base_url.clone())
        .or_else(|| fixture_env("MARGINS_UX_E2E_OPENAI_BASE_URL", None))
        .or_else(|| fixture_env("OPENAI_BASE_URL", Some(&env_file)));
    let explicit_fixture_model = fixture_env("MARGINS_UX_E2E_OPENAI_MODEL", None);
    let ai_model = explicit_fixture_model
        .or_else(|| {
            included_config
                .as_ref()
                .and_then(|config| config.model.clone())
        })
        .or_else(|| fixture_env("OPENAI_MODEL", Some(&env_file)))
        .or_else(|| Some("gpt-5.5".to_string()));
    let ai_provider = fixture_env("MARGINS_UX_E2E_AI_PROVIDER", None)
        .or_else(|| infer_provider_for_fixture(ai_base_url.as_deref(), ai_api_key.as_deref()));
    let (ai_provider, ai_model) = if included_fixture {
        (ai_provider, ai_model)
    } else {
        fixture_model_defaults(ai_provider, ai_model, ai_base_url.as_deref())
    };
    let prep_ai_provider = fixture_env("MARGINS_UX_E2E_PREP_AI_PROVIDER", None)
        .or_else(|| included_fixture.then(|| ai_provider.clone()).flatten())
        .or_else(|| ai_provider.clone());
    let prep_ai_base_url = fixture_env("MARGINS_UX_E2E_PREP_OPENAI_BASE_URL", None)
        .or_else(|| {
            included_fixture
                .then(|| included_config.as_ref()?.base_url.clone())
                .flatten()
        })
        .or_else(|| ai_base_url.clone());
    let prep_ai_model = fixture_env("MARGINS_UX_E2E_PREP_OPENAI_MODEL", None)
        .or_else(|| included_fixture.then(|| "google/gemini-3-flash-preview".to_string()))
        .or_else(|| ai_model.clone());
    let mut prep_ai_api_key = fixture_env("MARGINS_UX_E2E_PREP_OPENAI_API_KEY", None)
        .or_else(|| {
            included_fixture
                .then(|| {
                    included_config
                        .as_ref()
                        .map(|config| config.api_key.clone())
                })
                .flatten()
        })
        .or_else(|| ai_api_key.clone());
    if ai_provider.as_deref() == Some("openai-codex") {
        // ChatGPT/Codex auth is OAuth-backed. Do not pass an ambient
        // OpenAI/OpenRouter API key through as the provider token; the Codex
        // provider expects a JWT from the Pi auth store.
        ai_api_key = None;
    }
    if prep_ai_provider.as_deref() == Some("openai-codex") {
        prep_ai_api_key = None;
    }
    prepare_fixture_pi_agent_dir(
        &work_dir,
        ai_api_key.as_deref(),
        ai_base_url.as_deref(),
        ai_model.as_deref(),
    )?;
    if ai_provider.as_deref() == Some("margins-openai-compatible")
        || prep_ai_provider.as_deref() == Some("margins-openai-compatible")
    {
        prepare_fixture_margins_compatible_provider(
            &work_dir,
            ai_api_key.as_deref().or(prep_ai_api_key.as_deref()),
            ai_base_url.as_deref().or(prep_ai_base_url.as_deref()),
            ai_model.as_deref().or(prep_ai_model.as_deref()),
        )?;
    }
    eprintln!(
        "ai_provider={} ai_model={} ai_api_key_present={}",
        ai_provider.as_deref().unwrap_or("(default)"),
        ai_model.as_deref().unwrap_or("(default)"),
        ai_api_key.is_some()
    );

    eprintln!("fixture={}", fixture_dir.display());
    eprintln!("work_dir={}", work_dir.display());
    if let Some(vault) = &vault_path {
        eprintln!("vault_path={}", vault.display());
    }
    eprintln!("session={session_name}");

    let started = std::time::Instant::now();
    let events: Arc<Mutex<Vec<FixtureProcessingEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let outcome = run_pi_distill_blocking(
        PiDistillRequest {
            work_dir: work_dir.clone(),
            margins_dir,
            trace_dir,
            session_name,
            memo_path,
            capture_context,
            aligned_context,
            vault_path: vault_path.clone(),
            note_config,
            ai_provider,
            ai_model,
            ai_api_key,
            ai_credential_generation: None,
            prep_ai_provider,
            prep_ai_model,
            prep_ai_api_key,
            prep_ai_credential_generation: None,
            skill_path: repo.join("skills/margins/hosts/desktop.md"),
            cancel: Arc::new(AtomicBool::new(false)),
            resume_session_path: None,
            refine_message: None,
            existing_note_path: None,
            save_generated_note: true,
        },
        {
            let started = started;
            let events = events.clone();
            move |stage, msg, progress| {
                let ms = started.elapsed().as_millis();
                eprintln!("[{ms:>6}ms] [{stage}] {progress:?} {msg}");
                if let Ok(mut events) = events.lock() {
                    events.push(FixtureProcessingEvent {
                        stage: stage.to_string(),
                        message: msg.to_string(),
                        progress,
                    });
                }
            }
        },
    )?;

    let note_path = outcome.note_path;
    let session_file = outcome.session_file;
    println!("note_path={}", note_path.clone().unwrap_or_default());
    println!("session_file={}", session_file.clone().unwrap_or_default());
    println!("work_dir={}", work_dir.display());
    let elapsed_ms = started.elapsed().as_millis();
    println!("elapsed_ms={elapsed_ms}");

    if let Ok(out_path) = std::env::var("MARGINS_UX_FIXTURE_OUT") {
        let summary = FixtureRunSummary {
            note_path,
            session_file,
            work_dir: work_dir.display().to_string(),
            vault_path: vault_path.map(|p| p.display().to_string()),
            elapsed_ms,
            events: events
                .lock()
                .map(|events| events.clone())
                .unwrap_or_default(),
        };
        std::fs::write(out_path, serde_json::to_string_pretty(&summary)?)?;
    }
    Ok(())
}

fn read_settings(path: &Path) -> Result<FixtureSettings, Box<dyn std::error::Error>> {
    let settings_path = path.join("settings.json");
    if !settings_path.exists() {
        return Ok(FixtureSettings::default());
    }
    Ok(serde_json::from_str(&std::fs::read_to_string(
        settings_path,
    )?)?)
}

fn clean_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn fixture_env(name: &str, dotenv: Option<&HashMap<String, String>>) -> Option<String> {
    if name.starts_with("MARGINS_UX_E2E_") {
        return clean_env(name);
    }
    dotenv
        .and_then(|values| values.get(name).cloned())
        .or_else(|| clean_env(name))
}

#[derive(Debug, Deserialize)]
struct IncludedFixtureConfig {
    api_key: String,
    base_url: Option<String>,
    model: Option<String>,
}

fn fetch_included_fixture_config() -> Result<IncludedFixtureConfig, Box<dyn std::error::Error>> {
    let url = clean_env("MARGINS_UX_E2E_INCLUDED_CONFIG_URL")
        .unwrap_or_else(|| "https://api.enzyme.garden/margins/free-config".to_string());
    let bootstrap_id = clean_env("MARGINS_UX_E2E_INCLUDED_BOOTSTRAP_ID")
        .unwrap_or_else(|| format!("margins-fixture-{}", std::process::id()));
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?
        .get(&url)
        .header("X-Margins-Bootstrap-Id", bootstrap_id.clone())
        .send()?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        return Err(format!("included fixture config failed with status {status}: {text}").into());
    }
    Ok(serde_json::from_str(&text)?)
}

fn load_fixture_dotenv(repo: &Path) -> HashMap<String, String> {
    let mut values = HashMap::new();
    for path in [
        repo.join(".env"),
        repo.join("desktop/.env"),
        repo.join("desktop/src-tauri/.env"),
    ] {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let value = unquote_dotenv_value(value.trim());
            if !value.trim().is_empty() {
                values.insert(key.trim().to_string(), value);
            }
        }
    }
    values
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

fn infer_provider_for_fixture(base_url: Option<&str>, api_key: Option<&str>) -> Option<String> {
    api_key?;
    let Some(base_url) = base_url
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())
    else {
        return Some("openai".to_string());
    };
    if base_url.eq_ignore_ascii_case("https://api.openai.com/v1") {
        return Some("openai".to_string());
    }
    if base_url.eq_ignore_ascii_case("https://openrouter.ai/api/v1") {
        return Some("openrouter".to_string());
    }
    Some("margins-openai-compatible".to_string())
}

fn fixture_model_defaults(
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<&str>,
) -> (Option<String>, Option<String>) {
    let explicit_model = clean_env("MARGINS_UX_E2E_OPENAI_MODEL").is_some();
    let is_openrouter = provider.as_deref() == Some("openrouter")
        || base_url
            .map(|url| {
                url.trim()
                    .trim_end_matches('/')
                    .eq_ignore_ascii_case("https://openrouter.ai/api/v1")
            })
            .unwrap_or(false);
    if is_openrouter && !explicit_model {
        // Keep the default UX fixture affordable and reliable. Use
        // MARGINS_UX_E2E_OPENAI_MODEL to evaluate a higher-quality model.
        return (provider, Some("openai/gpt-4.1-nano".to_string()));
    }
    (provider, model)
}

fn prepare_fixture_pi_agent_dir(
    work_dir: &Path,
    api_key: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if api_key.is_none() {
        return Ok(());
    }
    let agent_dir = std::env::var_os("MARGINS_UX_E2E_PI_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| work_dir.join(".pi-agent-fixture"));
    std::fs::create_dir_all(&agent_dir)?;
    std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir);
    std::env::set_var("MARGINS_PI_AGENT_DIR", &agent_dir);

    let Some(base_url) = base_url.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    if base_url
        .trim_end_matches('/')
        .eq_ignore_ascii_case("https://api.openai.com/v1")
        || base_url
            .trim_end_matches('/')
            .eq_ignore_ascii_case("https://openrouter.ai/api/v1")
    {
        return Ok(());
    }

    let model = model.unwrap_or("gpt-5.5");
    let models_path = agent_dir.join("models.json");
    let mut root = if models_path.exists() {
        std::fs::read_to_string(&models_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let providers = obj.entry("providers").or_insert_with(|| json!({}));
    if !providers.is_object() {
        *providers = json!({});
    }
    providers.as_object_mut().unwrap().insert(
        "margins-openai-compatible".to_string(),
        json!({
            "baseUrl": base_url,
            "api": "openai-completions",
            "apiKey": "env:MARGINS_API_KEY",
            "authHeader": true,
            "compat": {
                "supportsDeveloperRole": false,
                "supportsReasoningEffort": false
            },
            "models": [{
                "id": model,
                "name": format!("{} (Margins fixture)", model),
                "reasoning": false,
                "input": ["text"],
                "contextWindow": 128000,
                "maxTokens": 16384
            }]
        }),
    );
    std::fs::write(models_path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}

fn prepare_fixture_margins_compatible_provider(
    work_dir: &Path,
    api_key: Option<&str>,
    base_url: Option<&str>,
    model: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let Some(api_key) = api_key.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    let Some(base_url) = base_url.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(());
    };
    let agent_dir = std::env::var_os("MARGINS_UX_E2E_PI_AGENT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| work_dir.join(".pi-agent-fixture"));
    std::fs::create_dir_all(&agent_dir)?;
    std::env::set_var("PI_CODING_AGENT_DIR", &agent_dir);
    std::env::set_var("MARGINS_PI_AGENT_DIR", &agent_dir);
    std::env::set_var("MARGINS_API_KEY", api_key);
    write_margins_compatible_provider(&agent_dir, base_url, model.unwrap_or("gpt-5.5"))
}

fn write_margins_compatible_provider(
    agent_dir: &Path,
    base_url: &str,
    model: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let models_path = agent_dir.join("models.json");
    let mut root = if models_path.exists() {
        std::fs::read_to_string(&models_path)
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .unwrap_or_else(|| json!({}))
    } else {
        json!({})
    };
    if !root.is_object() {
        root = json!({});
    }
    let obj = root.as_object_mut().unwrap();
    let providers = obj.entry("providers").or_insert_with(|| json!({}));
    if !providers.is_object() {
        *providers = json!({});
    }
    providers.as_object_mut().unwrap().insert(
        "margins-openai-compatible".to_string(),
        json!({
            "baseUrl": base_url,
            "api": "openai-completions",
            "apiKey": "env:MARGINS_API_KEY",
            "authHeader": true,
            "compat": {
                "supportsDeveloperRole": false,
                "supportsReasoningEffort": false
            },
            "models": [{
                "id": model,
                "name": format!("{} (Margins)", model),
                "reasoning": false,
                "input": ["text"],
                "contextWindow": 128000,
                "maxTokens": 16384
            }]
        }),
    );

    std::fs::write(&models_path, serde_json::to_string_pretty(&root)?)?;
    Ok(())
}
