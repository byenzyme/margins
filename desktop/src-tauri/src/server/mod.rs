// ---------------------------------------------------------------------------
// server/ — WP2 headless axum server transport
// ---------------------------------------------------------------------------

pub mod asr_state;
pub mod assets;
pub mod auth;
pub mod events;
pub mod http;
#[cfg(any(
    feature = "parakeet-asr",
    all(feature = "coreml-asr", target_os = "macos")
))]
pub mod remote_asr;

#[cfg(windows)]
mod windows_atomic_replace;

use crate::ctx::Ctx;
use anyhow::Context;
use http::{build_router, CtxState, ServerState};
use margins_workflows::{
    workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

/// Prepare the machine-level speech cache before a Workspace server starts.
/// The Mac project service uses the same CoreML assets as local recording.
pub fn prepare_asr() -> anyhow::Result<()> {
    #[cfg(all(feature = "parakeet-asr", target_os = "linux"))]
    margins_media::model_registry::parakeet::configure_env()?;
    prepare_asr_assets(&|message, progress| {
        eprintln!("[margins-server] {message} {progress:?}");
    })?;
    anyhow::ensure!(
        crate::speech_models::transcription_runtime_available(&crate::settings::load_settings()),
        "speech model is still unavailable after preparation"
    );
    println!("{{\"stage\":\"ready\",\"progress\":1}}");
    Ok(())
}

fn prepare_asr_assets(
    progress: &(dyn Fn(String, Option<f32>) + Send + Sync),
) -> anyhow::Result<()> {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    {
        if !crate::speech_models::transcription_runtime_available(&crate::settings::load_settings())
        {
            margins_media::model_registry::prepare_model(
                margins_media::model_registry::ModelKind::CoreMl,
                progress,
            )?;
        }
        return Ok(());
    }
    #[cfg(all(feature = "parakeet-asr", target_os = "linux"))]
    return margins_media::model_registry::prepare_model(
        margins_media::model_registry::ModelKind::ParakeetOnnx,
        progress,
    )
    .map(|_| ());
    #[cfg(not(any(
        all(feature = "coreml-asr", target_os = "macos"),
        all(feature = "parakeet-asr", target_os = "linux")
    )))]
    anyhow::bail!("automatic ASR preparation is unavailable in this server build")
}

/// Entry point for the headless server.  Called from `server_main.rs`.
///
/// Reads:
///   - `MARGINS_PORT`     — listen port (default: 8787)
///   - `MARGINS_HOST`     — listen address (default: 127.0.0.1)
///   - `MARGINS_DATA_DIR` — data / token directory (default: ~/.margins-app)
///   - `MARGINS_PROFILE`  — settings profile (default: "default")
///   - `MARGINS_WORK_DIR` — explicit project root for the hosted capture store
///
/// Blocks until the server exits.
pub fn run() -> anyhow::Result<()> {
    #[cfg(all(feature = "parakeet-asr", target_os = "linux"))]
    margins_media::model_registry::parakeet::configure_env()?;
    // Build a tokio runtime — server_main just calls this synchronous wrapper.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_async())
}

async fn run_async() -> anyhow::Result<()> {
    // WorkspaceService opens the canonical sessions.sqlite before AppState is
    // constructed. Initialize libsql's shared native SQLite runtime first;
    // build_app_state is now too late for the composed hosted process.
    margins::initialize_sqlite_runtime()
        .map_err(|error| anyhow::anyhow!("failed to initialize shared SQLite runtime: {error}"))?;
    crate::settings::configure_pi_agent_dir();

    match crate::webm_opus::HostedWebmFinalizer::from_env() {
        crate::webm_opus::HostedWebmFinalizer::Native => {
            eprintln!("[margins-server] hosted WebM finalization ready: native")
        }
        crate::webm_opus::HostedWebmFinalizer::FfmpegCompatibility => {
            match crate::ffmpeg::resolve_binary() {
                Ok(path) => eprintln!(
                    "[margins-server] hosted WebM ffmpeg compatibility finalization ready: {}",
                    path.display()
                ),
                Err(error) => eprintln!(
                    "[margins-server] WARNING: hosted WebM ffmpeg compatibility finalization is unavailable: {error}"
                ),
            }
        }
    }

    // --- Configuration from environment ---
    let port: u16 = std::env::var("MARGINS_PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8787);

    let host = std::env::var("MARGINS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    if !matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1") {
        anyhow::bail!(
            "margins-server only binds loopback; publish it through an explicitly configured trusted HTTPS proxy"
        );
    }

    let data_dir: PathBuf = std::env::var("MARGINS_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".margins-app")
        });
    let materialized_distill_skill =
        crate::hosted_distill_skill::materialize_default_bundle(&data_dir).with_context(|| {
            format!(
                "failed to materialize embedded Margins distillation bundle under {}",
                data_dir.display()
            )
        })?;
    let effective_distill_skill = crate::hosted_distill_skill::resolve_hosted_distill_skill_path(
        Some(&materialized_distill_skill),
        crate::hosted_distill_skill::HostedDistillCaller::ProcessSession,
    )
    .map_err(anyhow::Error::msg)?;

    // --- Load settings (same logic as desktop lib.rs startup) ---
    let explicit_work_dir = std::env::var("MARGINS_WORK_DIR").ok().map(PathBuf::from);
    let mut work_dir = explicit_work_dir
        .clone()
        .unwrap_or_else(crate::settings::default_work_dir);
    let mut settings = crate::settings::load_settings();
    if explicit_work_dir.is_some() {
        // A project-scoped server must not let the user's desktop vault
        // preference redirect capture away from the project selected by its
        // launcher. This override is process-local and is never persisted.
        settings.vault_path = Some(work_dir.to_string_lossy().into_owned());
        settings.active_project_id = None;
    }
    std::fs::create_dir_all(&work_dir)?;

    // Resolve the hosted store from an explicit Workspace mapping. The BB
    // launcher supplies a stable id; standalone servers must select one with
    // MARGINS_WORKSPACE. No service route infers authority from process cwd.
    let margins_home = std::env::var("MARGINS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("margins-home"));
    let workspace_id = std::env::var("MARGINS_WORKSPACE")
        .context("MARGINS_WORKSPACE is required for margins-server")?;
    let workspace = if std::env::var_os("MARGINS_SERVICE_PROVISION").is_some() {
        workspace::ensure_service_workspace(
            &margins_home,
            &workspace_id,
            Some(&workspace_id),
            &work_dir,
            &work_dir,
        )?
    } else {
        workspace::resolve_workspace(&margins_home, Some(&workspace_id), &work_dir)?
    };
    if std::env::var_os("MARGINS_BB_CAPTURE_WORKSPACE").is_some() {
        work_dir = workspace.capture_store_dir()?;
        std::fs::create_dir_all(&work_dir)?;
        settings.vault_path = Some(work_dir.to_string_lossy().into_owned());
    }
    let workspace_service = Arc::new(WorkspaceService::open_with_capabilities(
        std::env::var("MARGINS_INSTANCE_ID").unwrap_or_else(|_| "local".to_string()),
        workspace,
        crate::speech_models::transcription_runtime_available(&settings),
        cfg!(feature = "recall"),
    )?);
    let asr_setup = asr_state::SpeechSetup::new(
        workspace_service.asr_available(),
        cfg!(any(
            feature = "parakeet-asr",
            all(feature = "coreml-asr", target_os = "macos")
        )),
    );
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    workspace_service.enable_deferred_asr();

    // --- Build shared app state ---
    let app_state = crate::build_app_state(work_dir, settings);
    *app_state
        .hosted_distill_skill_path
        .lock()
        .expect("hosted distillation skill path lock poisoned") =
        Some(materialized_distill_skill.clone());

    // --- Build WsSink (EventSink impl for web path) ---
    let ws_sink = Arc::new(events::WsSink::new(256));

    // --- Build Ctx ---
    let ctx = Ctx {
        state: app_state,
        sink: ws_sink.clone(),
    };

    // --- Auth token ---
    let token = auth::load_or_create_token(&data_dir)?;
    let credential_store = ScopedCredentialStore::open(data_dir.join("credentials.json"))?;
    let administrator = ServicePrincipal::full("server-admin", workspace_service.workspace_id());
    credential_store.register(
        &administrator.id,
        &token,
        administrator.workspace_ids.iter().cloned().collect(),
        administrator.operations.iter().cloned().collect(),
        None,
    )?;

    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    let remote_asr_jobs = remote_asr::RemoteAsrJobs::default();

    // --- Build router ---
    let server_state = ServerState {
        ctx: Arc::new(CtxState(ctx)),
        sink: ws_sink,
        token: token.clone(),
        workspace_service: workspace_service.clone(),
        credential_store,
        service_principal: administrator.clone(),
        asr_setup: asr_setup.clone(),
        #[cfg(any(
            feature = "parakeet-asr",
            all(feature = "coreml-asr", target_os = "macos")
        ))]
        remote_asr_jobs: remote_asr_jobs.clone(),
    };
    #[cfg(any(
        feature = "parakeet-asr",
        all(feature = "coreml-asr", target_os = "macos")
    ))]
    if workspace_service.asr_available() {
        for job in workspace_service.pending_transcription_jobs()? {
            remote_asr_jobs.schedule(workspace_service.clone(), administrator.clone(), job);
        }
    } else {
        asr_setup.start(
            workspace_service.clone(),
            administrator.clone(),
            remote_asr_jobs.clone(),
        )?;
    }
    let app = build_router(server_state);

    // --- Bind and serve ---
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("invalid MARGINS_HOST/MARGINS_PORT");

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    let bound_port = listener.local_addr()?.port();
    let service_state = margins_workflows::remote_workspace::ServiceStateV1 {
        schema: "margins.service-state.v1".to_string(),
        protocol_version: 1,
        instance_id: std::env::var("MARGINS_INSTANCE_ID").unwrap_or_else(|_| "local".to_string()),
        workspace_ids: vec![workspace_service.workspace_id().to_string()],
        loopback_port: bound_port,
    };
    let state_path = data_dir.join("service.json");
    let state_temp_path = data_dir.join(format!(".service.{}.tmp", std::process::id()));
    let mut state_file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&state_temp_path)?;
    use std::io::Write as _;
    state_file.write_all(&serde_json::to_vec_pretty(&service_state)?)?;
    state_file.sync_all()?;
    drop(state_file);
    std::fs::rename(&state_temp_path, &state_path)?;
    std::fs::File::open(&data_dir)?.sync_all()?;

    eprintln!(
        "[margins-server] listening on http://{}",
        listener.local_addr()?
    );
    eprintln!(
        "[margins-server] full token at: {}",
        data_dir.join("token").display()
    );
    eprintln!(
        "[margins-server] distillation skill ready: {}",
        effective_distill_skill.display()
    );

    axum::serve(listener, app).await?;
    Ok(())
}
