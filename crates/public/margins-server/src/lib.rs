//! HTTP adapter over the public Workspace service.

pub mod auth;
pub mod http;
pub mod webm;

/// BB capture wire contract; checked against the plugin before remote use.
pub const HOSTED_CAPTURE_PROTOCOL_VERSION: u8 = 3;

#[cfg(windows)]
mod windows_atomic_replace;

use anyhow::Context as _;
use margins_workflows::{
    remote_workspace::ServiceStateV1,
    workspace,
    workspace_service::{ScopedCredentialStore, ServicePrincipal, WorkspaceService},
};
use std::{io::Write as _, net::SocketAddr, path::PathBuf, sync::Arc};

#[derive(Clone)]
pub struct ServerState {
    pub workspace_service: Arc<WorkspaceService>,
    pub credential_store: ScopedCredentialStore,
    pub service_principal: ServicePrincipal,
}

/// Starts one loopback Workspace service selected explicitly by the launcher.
pub fn run() -> anyhow::Result<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_async())
}

async fn run_async() -> anyhow::Result<()> {
    let port = std::env::var("MARGINS_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(8787);
    let host = std::env::var("MARGINS_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    anyhow::ensure!(
        matches!(host.as_str(), "127.0.0.1" | "localhost" | "::1"),
        "margins-server only binds loopback"
    );
    let data_dir = std::env::var("MARGINS_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".margins-app")
        });
    std::fs::create_dir_all(&data_dir)?;
    let work_dir = std::env::var("MARGINS_WORK_DIR")
        .map(PathBuf::from)
        .context("MARGINS_WORK_DIR is required for margins-server")?;
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
    let instance_id = std::env::var("MARGINS_INSTANCE_ID").unwrap_or_else(|_| "local".into());
    let workspace_service = Arc::new(WorkspaceService::open_with_capabilities(
        &instance_id,
        workspace,
        false,
        true,
    )?);
    let token = auth::load_or_create_token(&data_dir)?;
    let credential_store = ScopedCredentialStore::open(data_dir.join("credentials.json"))?;
    let service_principal =
        ServicePrincipal::full("server-admin", workspace_service.workspace_id());
    credential_store.register(
        &service_principal.id,
        &token,
        service_principal.workspace_ids.iter().cloned().collect(),
        service_principal.operations.iter().cloned().collect(),
        None,
    )?;
    let state = ServerState {
        workspace_service,
        credential_store,
        service_principal,
    };
    let listener =
        tokio::net::TcpListener::bind(format!("{host}:{port}").parse::<SocketAddr>()?).await?;
    let service_state = ServiceStateV1 {
        schema: "margins.service-state.v1".into(),
        protocol_version: 1,
        instance_id,
        workspace_ids: vec![workspace_id],
        loopback_port: listener.local_addr()?.port(),
    };
    let state_path = data_dir.join("service.json");
    let temporary_path = data_dir.join(format!(".service.{}.tmp", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary_path)?;
    file.write_all(&serde_json::to_vec_pretty(&service_state)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary_path, &state_path)?;
    std::fs::File::open(&data_dir)?.sync_all()?;
    let app = http::build_router(state);
    eprintln!(
        "[margins-server] listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}
