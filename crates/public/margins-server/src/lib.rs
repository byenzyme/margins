//! HTTP adapter over the public Workspace service.

pub mod asr;
pub mod auth;
pub mod browser;
pub mod http;
pub mod logging;
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
use std::{
    io::Write as _,
    net::{IpAddr, SocketAddr},
    path::PathBuf,
    sync::Arc,
};

/// Error code for a launch that names a Workspace this machine does not have.
pub const WORKSPACE_NOT_FOUND: &str = "workspace_not_found";

/// How long a bb-launched server that could not open its Workspace stays up to
/// report why; the launcher normally stops it as soon as it has read the error.
const UNAVAILABLE_REPORT_SECS: u64 = 120;

fn missing_workspace_message(workspace_id: &str) -> String {
    format!(
        "Margins Workspace '{workspace_id}' does not exist on this machine. Run `margins init` in your notes folder, then choose that Workspace."
    )
}

fn selected_workspace(
    margins_home: &std::path::Path,
    workspace_id: &str,
    work_dir: &std::path::Path,
    provision: bool,
) -> anyhow::Result<workspace::ResolvedWorkspace> {
    if workspace::workspace_state_dir(margins_home, workspace_id)?.exists() {
        // BB starts the service for a Workspace the CLI has already declared.
        // Its Home and capture bindings are authoritative; the server's cwd
        // is only a launcher detail and must never rewrite those paths.
        // The server is a writer (it records sessions), so resolving here may
        // migrate a retired layout; it is upgraded together with the CLI.
        workspace::resolve_workspace(margins_home, Some(workspace_id), work_dir)
    } else if provision {
        // Only an operator's explicit `MARGINS_SERVICE_PROVISION` creates a
        // Workspace. A bb launch naming a missing one is refused: creating it
        // here would put both its notes and its recordings in the launcher's
        // directory.
        workspace::ensure_service_workspace(
            margins_home,
            workspace_id,
            Some(workspace_id),
            work_dir,
            work_dir,
        )
    } else {
        Err(anyhow::Error::new(MissingWorkspace(
            missing_workspace_message(workspace_id),
        )))
    }
}

#[derive(Debug)]
struct MissingWorkspace(String);

impl std::fmt::Display for MissingWorkspace {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for MissingWorkspace {}

#[derive(Clone)]
pub struct ServerState {
    pub workspace_service: Arc<WorkspaceService>,
    pub credential_store: ScopedCredentialStore,
    pub service_principal: ServicePrincipal,
    pub asr_setup: asr::SpeechSetup,
    pub remote_asr_jobs: asr::RemoteAsrJobs,
}

/// Starts one loopback Workspace service selected explicitly by the launcher.
pub fn run() -> anyhow::Result<()> {
    asr::configure_model_environment()?;
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
    let listen_addr = loopback_socket_addr(&host, port)?;
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
        .unwrap_or(std::env::current_dir().context("cannot resolve server working directory")?);
    let margins_home = std::env::var("MARGINS_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| data_dir.join("margins-home"));
    let workspace_id = std::env::var("MARGINS_WORKSPACE")
        .context("MARGINS_WORKSPACE is required for margins-server")?;
    let workspace = match selected_workspace(
        &margins_home,
        &workspace_id,
        &work_dir,
        std::env::var_os("MARGINS_SERVICE_PROVISION").is_some(),
    ) {
        Ok(workspace) => workspace,
        Err(error) => {
            let Some(missing) = error.downcast_ref::<MissingWorkspace>() else {
                return Err(error);
            };
            eprintln!(
                "{}",
                serde_json::json!({
                    "schema_version": 1,
                    "ok": false,
                    "error": {"code": WORKSPACE_NOT_FOUND, "message": missing.0},
                })
            );
            if std::env::var_os("MARGINS_BB_CAPTURE_WORKSPACE").is_none() {
                return Err(error);
            }
            return report_unavailable(listen_addr, &data_dir, missing.0.clone()).await;
        }
    };
    let instance_id = std::env::var("MARGINS_INSTANCE_ID").unwrap_or_else(|_| "local".into());
    let workspace_service = Arc::new(WorkspaceService::open_with_capabilities(
        &instance_id,
        workspace,
        asr::runtime_available(),
        true,
    )?);
    let asr_setup = asr::SpeechSetup::new(workspace_service.asr_available(), asr::supported());
    if asr::supported() {
        workspace_service.enable_deferred_asr();
    }
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
    let remote_asr_jobs = asr::RemoteAsrJobs::default();
    if workspace_service.asr_available() {
        remote_asr_jobs.schedule_pending(workspace_service.clone(), service_principal.clone())?;
    } else if asr::supported() {
        asr_setup.start(
            workspace_service.clone(),
            service_principal.clone(),
            remote_asr_jobs.clone(),
        )?;
    }
    let state = ServerState {
        workspace_service,
        credential_store,
        service_principal,
        asr_setup,
        remote_asr_jobs,
    };
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
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
    let sweeper_state = state.clone();
    tokio::spawn(async move {
        loop {
            let state = sweeper_state.clone();
            match tokio::task::spawn_blocking(move || {
                let observed = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                browser::sweep_expired_browser_captures(&state, observed)
            })
            .await
            {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => eprintln!("[margins-server] browser lease sweep: {error:#}"),
                Err(error) => eprintln!("[margins-server] browser lease worker: {error}"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(
                browser::BROWSER_OWNER_SWEEP_INTERVAL_SECS,
            ))
            .await;
        }
    });
    let app = http::build_router(state);
    eprintln!(
        "[margins-server] listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}

/// Serve the startup error to the bb launcher, which reads `/v1/capabilities`
/// after `/health` and shows a non-404 error message to the user. Writes no
/// discovery state and opens no Workspace.
async fn report_unavailable(
    listen_addr: SocketAddr,
    data_dir: &std::path::Path,
    message: String,
) -> anyhow::Result<()> {
    auth::load_or_create_token(data_dir)?;
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let app = http::unavailable_router(WORKSPACE_NOT_FOUND, message);
    let shutdown = tokio::time::sleep(std::time::Duration::from_secs(UNAVAILABLE_REPORT_SECS));
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    anyhow::bail!("{WORKSPACE_NOT_FOUND}: Workspace is missing; nothing was created")
}

fn loopback_socket_addr(host: &str, port: u16) -> anyhow::Result<SocketAddr> {
    let ip = match host {
        "127.0.0.1" | "localhost" => IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        "::1" => IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
        _ => anyhow::bail!("margins-server only binds loopback"),
    };
    Ok(SocketAddr::new(ip, port))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_host_spellings_resolve_to_socket_addresses() {
        assert_eq!(
            loopback_socket_addr("127.0.0.1", 8787).unwrap().to_string(),
            "127.0.0.1:8787"
        );
        assert_eq!(
            loopback_socket_addr("localhost", 8787).unwrap().to_string(),
            "127.0.0.1:8787"
        );
        assert_eq!(
            loopback_socket_addr("::1", 8787).unwrap().to_string(),
            "[::1]:8787"
        );
        assert!(loopback_socket_addr("0.0.0.0", 8787).is_err());
        assert!(loopback_socket_addr("localhost.example", 8787).is_err());
    }

    #[test]
    fn bb_launch_naming_a_missing_workspace_creates_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let launcher = temp.path().join("launcher");
        std::fs::create_dir_all(&launcher).unwrap();
        let error = selected_workspace(&home, "practice", &launcher, false).unwrap_err();
        assert!(error.downcast_ref::<MissingWorkspace>().is_some());
        assert!(error.to_string().contains("margins init"));
        assert!(!workspace::workspace_state_dir(&home, "practice")
            .unwrap()
            .exists());
        assert!(std::fs::read_dir(&launcher).unwrap().next().is_none());
    }

    #[test]
    fn bb_launch_uses_existing_workspace_bindings_instead_of_launcher_directory() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let notes = temp.path().join("notes");
        let captures = temp.path().join("captures");
        let launcher = temp.path().join("launcher");
        for path in [&home, &notes, &captures, &launcher] {
            std::fs::create_dir_all(path).unwrap();
        }
        workspace::ensure_service_workspace(&home, "practice", None, &notes, &captures).unwrap();
        let selected = selected_workspace(&home, "practice", &launcher, true).unwrap();
        assert_eq!(selected.home_dir, notes.canonicalize().unwrap());
        assert_eq!(
            selected.capture_store_dir().unwrap(),
            captures.canonicalize().unwrap()
        );
    }
}
