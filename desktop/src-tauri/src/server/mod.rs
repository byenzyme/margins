// ---------------------------------------------------------------------------
// server/ — WP2 headless axum server transport
// ---------------------------------------------------------------------------

pub mod assets;
pub mod auth;
pub mod events;
pub mod http;

#[cfg(windows)]
mod windows_atomic_replace;

use crate::ctx::Ctx;
use anyhow::Context;
use http::{build_router, CtxState, ServerState};
use std::{net::SocketAddr, path::PathBuf, sync::Arc};

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
    // Build a tokio runtime — server_main just calls this synchronous wrapper.
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_async())
}

async fn run_async() -> anyhow::Result<()> {
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
    let work_dir = explicit_work_dir
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

    // --- Build router ---
    let server_state = ServerState {
        ctx: Arc::new(CtxState(ctx)),
        sink: ws_sink,
        token: token.clone(),
    };
    let app = build_router(server_state);

    // --- Bind and serve ---
    let addr: SocketAddr = format!("{}:{}", host, port)
        .parse()
        .expect("invalid MARGINS_HOST/MARGINS_PORT");

    let listener = tokio::net::TcpListener::bind(&addr).await?;

    eprintln!(
        "[margins-server] listening on http://{}  (token: {}...)",
        addr,
        &token[..8]
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
