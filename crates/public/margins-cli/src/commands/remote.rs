use crate::args::{Command, ServiceCommand, TranscriptFormat, TransfersCommand};
use crate::error::CliError;
use margins_meeting_protocol::{
    SessionId, SessionMillis, WorkspaceMemoUpdateV1, WorkspaceNoteAssociationUpdateV1,
    WorkspaceRenameV1,
};
use margins_workflows::{
    remote_workspace::{
        deliver_transfer, list_transfers, DurableTransferSpool, RemoteConnection,
        ServiceDiscoveryV1, ServiceStateV1,
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
    let explicit_data_dir = match &command {
        ServiceCommand::Discover { data_dir, .. } => data_dir.clone(),
        _ => None,
    };
    let data_dir = service_data_dir(explicit_data_dir)?;
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
        ServiceCommand::Discover {
            json: _,
            data_dir: _,
        } => {
            // The SSH transport has already authenticated the remote OS user.
            // Keep its client-scoped current pointer stable across invocations;
            // a per-process id would make bare `attach` select nothing on the
            // very next CLI run.
            let ssh_user = std::env::var("USER")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let principal = ServicePrincipal::full(format!("ssh-{ssh_user}"), selected.clone());
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
    if matches!(&command, Command::Recent { all: true }) {
        return Err(CliError::new(
            "remote_option_unsupported",
            "remote recent --all requires an instance-wide authorization endpoint; no request was sent",
        ));
    }
    if let Command::Transcribe { memo, speakers, .. } = &command {
        if memo.is_some() || speakers.is_some() {
            return Err(CliError::new(
                "remote_option_unsupported",
                "remote file intake does not run memo alignment or speaker processing; no file was read",
            ));
        }
    }
    if let Command::Memo {
        text,
        expected_revision,
        request_id,
        observed_at_ms,
        paused,
        ..
    } = &command
    {
        if text.is_some() && expected_revision.is_none() {
            return Err(CliError::usage("memo --text requires --expected-revision"));
        }
        if text.is_none()
            && (expected_revision.is_some()
                || request_id.is_some()
                || observed_at_ms.is_some()
                || *paused)
        {
            return Err(CliError::usage(
                "memo edit flags require --text; omit them to read the memo",
            ));
        }
    }
    if let Command::NoteAssociation {
        source,
        path,
        hash,
        unlink,
        expected_revision,
        request_id,
        ..
    } = &command
    {
        if *unlink && expected_revision.is_none() {
            return Err(CliError::usage(
                "note-association --unlink requires --expected-revision",
            ));
        }
        if !*unlink && source.is_some() && path.is_some() && expected_revision.is_none() {
            return Err(CliError::usage(
                "linking a note requires --expected-revision",
            ));
        }
        if !*unlink
            && ((source.is_some() != path.is_some())
                || (source.is_none()
                    && (hash.is_some() || expected_revision.is_some() || request_id.is_some())))
        {
            return Err(CliError::usage(
                "provide both --source and --path to link, or no mutation flags to read",
            ));
        }
    }
    if !matches!(
        &command,
        Command::Capabilities
            | Command::Note { .. }
            | Command::Current
            | Command::Ls
            | Command::Recent { .. }
            | Command::Rename { .. }
            | Command::Memo { .. }
            | Command::NoteAssociation { .. }
            | Command::ProcessingStatus { .. }
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
        Command::Ls | Command::Recent { all: false } => serde_json::to_value(
            connection
                .client
                .sessions(None, 100)
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Current => {
            serde_json::to_value(connection.client.current().map_err(CliError::from_anyhow)?)
        }
        Command::Rename { title } => {
            let session = resolve_session(&connection.client, None)?;
            serde_json::to_value(
                connection.client.rename(
                    session.as_ref(),
                    &WorkspaceRenameV1 {
                        request_id: operation_id("rename"),
                        title,
                    },
                ).map_err(CliError::from_anyhow)?,
            )
        }
        Command::Memo {
            meeting_id,
            text,
            expected_revision,
            request_id,
            observed_at_ms,
            paused,
        } => {
            let session = resolve_session(&connection.client, meeting_id.as_deref())?;
            if let Some(text) = text {
                let expected_revision = expected_revision.ok_or_else(|| {
                    CliError::usage("memo --text requires --expected-revision")
                })?;
                serde_json::to_value(
                    connection.client.update_memo(
                        session.as_ref(),
                        &WorkspaceMemoUpdateV1 {
                            request_id: request_id.unwrap_or_else(|| operation_id("memo")),
                            expected_revision,
                            observed_at_ms: SessionMillis(observed_at_ms.unwrap_or(0)),
                            paused,
                            text,
                        },
                    ).map_err(CliError::from_anyhow)?,
                )
            } else {
                if expected_revision.is_some() || request_id.is_some() || observed_at_ms.is_some() || paused {
                    return Err(CliError::usage(
                        "memo edit flags require --text; omit them to read the memo",
                    ));
                }
                serde_json::to_value(
                    connection.client.memo(session.as_ref()).map_err(CliError::from_anyhow)?,
                )
            }
        }
        Command::NoteAssociation {
            meeting_id,
            source,
            path,
            hash,
            unlink,
            expected_revision,
            request_id,
        } => {
            let session = resolve_session(&connection.client, meeting_id.as_deref())?;
            if unlink {
                let revision = expected_revision.ok_or_else(|| {
                    CliError::usage("note-association --unlink requires --expected-revision")
                })?;
                connection.client.unlink_note(
                    session.as_ref(),
                    &request_id.unwrap_or_else(|| operation_id("note-unlink")),
                    revision,
                )
                    .map_err(CliError::from_anyhow)?;
                Ok(serde_json::json!({"session_id":session, "association":null}))
            } else if let (Some(source_id), Some(relative_path)) =
                (source.as_deref(), path.as_deref())
            {
                let revision = expected_revision.ok_or_else(|| {
                    CliError::usage("linking a note requires --expected-revision")
                })?;
                serde_json::to_value(
                    connection.client.link_note(
                        session.as_ref(),
                        &WorkspaceNoteAssociationUpdateV1 {
                            request_id: request_id.unwrap_or_else(|| operation_id("note-link")),
                            source_id: source_id.to_string(),
                            relative_path: relative_path.to_string(),
                            observed_content_hash: hash,
                            expected_revision: revision,
                        },
                    ).map_err(CliError::from_anyhow)?,
                )
            } else {
                if source.is_some() || path.is_some() || hash.is_some() || expected_revision.is_some() || request_id.is_some() {
                    return Err(CliError::usage(
                        "provide both --source and --path to link, or no mutation flags to read",
                    ));
                }
                serde_json::to_value(
                    connection.client.note_association(session.as_ref())
                        .map_err(CliError::from_anyhow)?,
                )
            }
        }
        Command::ProcessingStatus { meeting_id } => {
            let session = resolve_session(&connection.client, meeting_id.as_deref())?;
            serde_json::to_value(
                connection.client.latest_job(session.as_ref()).map_err(CliError::from_anyhow)?,
            )
        }
        Command::Transcript { meeting_id, format } => {
            let session = resolve_session(&connection.client, meeting_id.as_deref().or(Some("latest")))?;
            let transcript = connection
                .client
                .transcript(session.as_ref())
                .map_err(CliError::from_anyhow)?;
            if format == TranscriptFormat::Text {
                writeln!(stdout, "{}", transcript.body)
                    .map_err(|error| CliError::from_anyhow(error.into()))?;
                return Ok(());
            }
            serde_json::to_value(transcript)
        }
        Command::Artifacts { meeting_id } => {
            let session = resolve_session(&connection.client, Some(&meeting_id))?;
            serde_json::to_value(
                connection
                    .client
                    .artifacts(session.as_ref())
                    .map_err(CliError::from_anyhow)?,
            )
        }
        Command::Recall { query, source } => serde_json::to_value(
            connection
                .client
                .recall(&query, source.as_deref())
                .map_err(CliError::from_anyhow)?,
        ),
        Command::Note { print } => {
            // Resolve once, then pin this identity through transcript, memo,
            // artifact, recall, and optional association work. The handoff
            // deliberately contains no note bytes or remote publishing API.
            let session = resolve_session(&connection.client, Some("latest"))?;
            let transcript = connection.client.transcript(session.as_ref())
                .map_err(CliError::from_anyhow)?;
            let memo = connection.client.memo(session.as_ref())
                .map_err(CliError::from_anyhow)?;
            let artifacts = connection.client.artifacts(session.as_ref())
                .map_err(CliError::from_anyhow)?;
            if !print {
                writeln!(stdout, "Continue in your coding agent with this pinned remote session context:")
                    .map_err(|error| CliError::from_anyhow(error.into()))?;
            }
            Ok(serde_json::json!({
                "schema":"margins.remote-note-handoff.v1",
                "remote":remote,
                "workspace_id":workspace,
                "session_id":session,
                "transcript":transcript,
                "memo":memo,
                "artifacts":artifacts,
                "local_notes_root":std::env::var("MARGINS_NOTES_ROOT").ok(),
                "instructions":"Keep this session id pinned. Use remote Margins recall for declared Sources, read and write ordinary note files through the locally synced native Source, then optionally link only its Source-relative reference with note-association. Do not send note bytes to Margins."
            }))
        }
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
            let receipt = connection
                    .client
                    .import(
                        &format!("upload-{session}"),
                        &session,
                        filename,
                        Some(&session),
                        bytes,
                    )
                    .map_err(CliError::from_anyhow)?;
            serde_json::to_value(serde_json::json!({
                "schema":"margins.remote-file-intake.v1",
                "receipt":receipt,
                "processing_outcome":"not_started",
                "message":"Original audio is durable. File intake does not start processing, so this command has not produced a transcript. Query processing-status after an explicitly supported server-side processing request."
            }))
        }
        _ => unreachable!("remote command was checked before transport setup"),
    }
    .map_err(|error| CliError::from_anyhow(error.into()))?;
    writeln!(stdout, "{}", serde_json::to_string(&value).unwrap())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

fn resolve_session(
    client: &margins_workflows::remote_workspace::WorkspaceHttpClient,
    requested: Option<&str>,
) -> Result<SessionId, CliError> {
    match requested.unwrap_or("current") {
        "current" => client
            .current()
            .map_err(CliError::from_anyhow)?
            .ok_or_else(|| {
                CliError::new(
                    "current_session_unavailable",
                    "no current session is selected for this client and Workspace",
                )
            }),
        "latest" => client
            .latest_session()
            .map_err(CliError::from_anyhow)?
            .ok_or_else(|| {
                CliError::new(
                    "latest_session_unavailable",
                    "this Workspace has no completed or visible sessions",
                )
            }),
        value => Ok(value.to_string().into()),
    }
}

fn operation_id(prefix: &str) -> String {
    format!(
        "{prefix}-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_millis()
    )
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
            let mut spool = DurableTransferSpool::open(&root, &transfer_id, 0)
                .map_err(CliError::from_anyhow)?;
            let manifest = spool.manifest().clone();
            let token = std::env::var("MARGINS_REMOTE_TOKEN").ok();
            let connection = RemoteConnection::connect(
                &manifest.remote_url,
                &manifest.workspace_id,
                token.as_deref(),
            )
            .map_err(CliError::from_anyhow)?;
            deliver_transfer(&mut spool, &connection.client).map_err(CliError::from_anyhow)?;
            writeln!(stdout, "transfer {transfer_id} delivered")
                .map_err(|error| CliError::from_anyhow(error.into()))
        }
    }
}

fn service_data_dir(explicit: Option<PathBuf>) -> Result<PathBuf, CliError> {
    if let Some(value) = explicit {
        return Ok(value);
    }
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
