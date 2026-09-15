use crate::args::{Command, ServiceCommand, TransfersCommand};
use crate::error::CliError;
use margins_workflows::{
    remote_workspace::{
        list_transfers, DurableTransferSpool, RemoteConnection, ServiceDiscoveryV1, ServiceStateV1,
    },
    workspace_service::{ScopedCredentialStore, ServicePrincipal},
};
use std::io::Write;
use std::path::PathBuf;

pub fn service(
    command: ServiceCommand,
    workspace_selector: Option<&str>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let data_dir = service_data_dir()?;
    let state: ServiceStateV1 = serde_json::from_slice(
        &std::fs::read(data_dir.join("service.json"))
            .map_err(|error| CliError::from_anyhow(error.into()))?,
    )
    .map_err(|error| CliError::from_anyhow(error.into()))?;
    let selected = workspace_selector
        .map(str::to_string)
        .or_else(|| std::env::var("MARGINS_WORKSPACE").ok())
        .or_else(|| (state.workspace_ids.len() == 1).then(|| state.workspace_ids[0].clone()))
        .ok_or_else(|| CliError::usage("service command requires --workspace <id>"))?;
    if !state.workspace_ids.iter().any(|value| value == &selected) {
        return Err(CliError::new(
            "workspace_not_found",
            "Workspace is not served by this instance",
        ));
    }
    let credentials = ScopedCredentialStore::open(data_dir.join("credentials.json"))
        .map_err(CliError::from_anyhow)?;
    let output = match command {
        ServiceCommand::Discover { json: _ } => {
            let principal =
                ServicePrincipal::full(format!("ssh-{}", std::process::id()), selected.clone());
            let credential = credentials
                .issue(
                    &principal.id,
                    principal.workspace_ids.iter().cloned().collect(),
                    principal.operations.iter().cloned().collect(),
                    Some(std::time::Duration::from_secs(300)),
                )
                .map_err(CliError::from_anyhow)?;
            serde_json::to_value(ServiceDiscoveryV1 {
                protocol_version: state.protocol_version,
                instance_id: state.instance_id,
                workspace_ids: vec![selected],
                loopback_port: state.loopback_port,
                credential,
            })
            .map_err(|error| CliError::from_anyhow(error.into()))?
        }
        ServiceCommand::PairShortcut { principal, json: _ } => {
            let scoped = ServicePrincipal::upload_only(principal.clone(), selected.clone());
            let credential = credentials
                .issue(
                    &scoped.id,
                    scoped.workspace_ids.iter().cloned().collect(),
                    scoped.operations.iter().cloned().collect(),
                    None,
                )
                .map_err(CliError::from_anyhow)?;
            serde_json::json!({
                "schema": "margins.shortcut-pairing.v1",
                "principal": principal,
                "workspace_id": selected,
                "credential": credential,
                "operations": scoped.operations,
            })
        }
        ServiceCommand::Revoke { principal, json: _ } => {
            let revoked = credentials
                .revoke(&principal)
                .map_err(CliError::from_anyhow)?;
            serde_json::json!({"schema":"margins.credential-revocation.v1", "principal":principal, "revoked":revoked})
        }
    };
    writeln!(stdout, "{}", serde_json::to_string(&output).unwrap())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn run(
    remote: &str,
    workspace: &str,
    command: Command,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    // Reject placeholder commands before opening a socket, starting SSH, or
    // reading any local input. A command is supported remotely only when this
    // adapter has a complete production implementation for it.
    if !matches!(
        &command,
        Command::Capabilities
            | Command::Current
            | Command::Ls
            | Command::Recent { .. }
            | Command::Transcript { .. }
            | Command::Artifacts { .. }
            | Command::Recall { .. }
            | Command::Transcribe { .. }
    ) {
        return Err(CliError::new(
            "remote_command_unsupported",
            format!("command {command:?} is not supported by the remote adapter"),
        ));
    }
    let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
    let connection = RemoteConnection::connect(remote, workspace, token.as_deref())
        .map_err(CliError::from_anyhow)?;
    let value = match command {
        Command::Capabilities => serde_json::to_value(
            connection
                .client
                .capabilities()
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Ls | Command::Recent { .. } => serde_json::to_value(
            connection
                .client
                .sessions(None, 100)
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Current => {
            serde_json::to_value(connection.client.current().map_err(CliError::from_anyhow)?)
        }
        Command::Transcript { meeting_id, .. } => serde_json::to_value(
            connection
                .client
                .transcript(meeting_id.as_deref().unwrap_or("latest"))
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Artifacts { meeting_id } => serde_json::to_value(
            connection
                .client
                .artifacts(&meeting_id)
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Recall { query, source } => serde_json::to_value(
            connection
                .client
                .recall(&query, source.as_deref())
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Transcribe {
            audio_path, name, ..
        } => {
            let bytes =
                std::fs::read(&audio_path).map_err(|error| CliError::from_anyhow(error.into()))?;
            let filename = audio_path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("recording");
            let session =
                name.unwrap_or_else(|| format!("import-{}", chrono::Utc::now().timestamp_millis()));
            serde_json::to_value(
                connection
                    .client
                    .import(
                        &format!("upload-{session}"),
                        &session,
                        filename,
                        Some(&session),
                        bytes,
                    )
                    .map_err(CliError::from_anyhow)?,
            )
        }
        _ => unreachable!("remote command was checked before transport setup"),
    }
    .map_err(|error| CliError::from_anyhow(error.into()))?;
    writeln!(stdout, "{}", serde_json::to_string(&value).unwrap())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

pub fn transfers(command: TransfersCommand, stdout: &mut dyn Write) -> Result<(), CliError> {
    let root = transfer_root()?;
    match command {
        TransfersCommand::List { json } => {
            let ids = list_transfers(&root).map_err(CliError::from_anyhow)?;
            if json {
                writeln!(
                    stdout,
                    "{}",
                    serde_json::json!({"schema":"margins.transfers.v1", "transfers":ids})
                )
            } else {
                writeln!(stdout, "{}", ids.join("\n"))
            }
            .map_err(|error| CliError::from_anyhow(error.into()))
        }
        TransfersCommand::Retry { transfer_id } => {
            let spool = DurableTransferSpool::open(&root, &transfer_id, 0)
                .map_err(CliError::from_anyhow)?;
            let manifest = spool.manifest().clone();
            let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
            let connection = RemoteConnection::connect(
                &manifest.remote_url,
                &manifest.workspace_id,
                token.as_deref(),
            )
            .map_err(CliError::from_anyhow)?;
            let capabilities = connection
                .client
                .capabilities()
                .map_err(CliError::from_anyhow)?;
            if capabilities.instance_id.as_ref() != manifest.instance_id {
                return Err(CliError::new(
                    "remote_instance_changed",
                    "remote instance identity changed; preserved transfer was not sent",
                ));
            }
            for chunk in spool.pending_chunks().map_err(CliError::from_anyhow)? {
                connection
                    .client
                    .upload_chunk(&manifest.producer_token, &chunk.command)
                    .map_err(CliError::from_anyhow)?;
                spool.acknowledge(&chunk).map_err(CliError::from_anyhow)?;
            }
            if let Some(close) = &manifest.close_command {
                connection
                    .client
                    .execute(&manifest.session_id, &manifest.producer_token, close)
                    .map_err(CliError::from_anyhow)?;
            }
            if let Some(finalize) = &manifest.finalize_command {
                connection
                    .client
                    .execute(&manifest.session_id, &manifest.producer_token, finalize)
                    .map_err(CliError::from_anyhow)?;
            }
            writeln!(stdout, "transfer {transfer_id} delivered")
                .map_err(|error| CliError::from_anyhow(error.into()))
        }
    }
}

fn service_data_dir() -> Result<PathBuf, CliError> {
    if let Some(value) = std::env::var_os("MARGINS_DATA_DIR") {
        return Ok(PathBuf::from(value));
    }
    Ok(dirs::home_dir()
        .ok_or_else(|| CliError::new("home_unavailable", "Could not determine home directory"))?
        .join(".margins-app"))
}

fn transfer_root() -> Result<PathBuf, CliError> {
    if let Some(value) = std::env::var_os("MARGINS_TRANSFER_DIR") {
        return Ok(PathBuf::from(value));
    }
    Ok(dirs::home_dir()
        .ok_or_else(|| CliError::new("home_unavailable", "Could not determine home directory"))?
        .join(".margins/transfers"))
}
