use crate::args::Command;
use crate::error::CliError;
use margins_meeting_protocol::{SessionId, SessionMillis, WorkspaceMemoUpdateV1};
use margins_workflows::workspace::ResolvedWorkspace;
use margins_workflows::workspace_service::{ServicePrincipal, WorkspaceService};
use std::io::Write;

pub fn run(
    workspace: ResolvedWorkspace,
    command: Command,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let workspace_id = workspace.config.id.clone();
    let service = WorkspaceService::open("local", workspace).map_err(CliError::from_anyhow)?;
    let principal = ServicePrincipal::full("native-cli", workspace_id);
    let value = match command {
        Command::Memo {
            meeting_id,
            text,
            expected_revision,
            request_id,
            observed_at_ms,
            paused,
        } => {
            let session = resolve_session(&service, &principal, meeting_id.as_deref())?;
            if let Some(text) = text {
                let expected_revision = expected_revision
                    .ok_or_else(|| CliError::usage("memo --text requires --expected-revision"))?;
                serde_json::to_value(
                    service
                        .update_memo(
                            &principal,
                            &session,
                            &WorkspaceMemoUpdateV1 {
                                request_id: request_id
                                    .unwrap_or_else(|| operation_id("local-memo")),
                                expected_revision,
                                observed_at_ms: SessionMillis(observed_at_ms.unwrap_or(0)),
                                paused,
                                text,
                            },
                        )
                        .map_err(CliError::from_anyhow)?,
                )
            } else {
                if expected_revision.is_some()
                    || request_id.is_some()
                    || observed_at_ms.is_some()
                    || paused
                {
                    return Err(CliError::usage(
                        "memo edit flags require --text; omit them to read the memo",
                    ));
                }
                serde_json::to_value(
                    service
                        .memo(&principal, &session)
                        .map_err(CliError::from_anyhow)?,
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
            let session = resolve_session(&service, &principal, meeting_id.as_deref())?;
            if unlink {
                service
                    .unlink_note(
                        &principal,
                        &session,
                        &request_id.unwrap_or_else(|| operation_id("local-note-unlink")),
                        expected_revision.ok_or_else(|| {
                            CliError::usage(
                                "note-association --unlink requires --expected-revision",
                            )
                        })?,
                    )
                    .map_err(CliError::from_anyhow)?;
                Ok(serde_json::json!({"session_id":session, "association":null}))
            } else if let (Some(source), Some(path)) = (source, path) {
                serde_json::to_value(
                    service
                        .link_note(
                            &principal,
                            &session,
                            &margins_meeting_protocol::WorkspaceNoteAssociationUpdateV1 {
                                request_id: request_id
                                    .unwrap_or_else(|| operation_id("local-note-link")),
                                source_id: source,
                                relative_path: path,
                                observed_content_hash: hash,
                                expected_revision: expected_revision.ok_or_else(|| {
                                    CliError::usage("linking a note requires --expected-revision")
                                })?,
                            },
                        )
                        .map_err(CliError::from_anyhow)?,
                )
            } else {
                serde_json::to_value(
                    service
                        .note_association(&principal, &session)
                        .map_err(CliError::from_anyhow)?,
                )
            }
        }
        Command::ProcessingStatus { meeting_id } => {
            let session = resolve_session(&service, &principal, meeting_id.as_deref())?;
            serde_json::to_value(
                service
                    .latest_job(&principal, &session)
                    .map_err(CliError::from_anyhow)?,
            )
        }
        _ => unreachable!("application command was checked by caller"),
    }
    .map_err(|error| CliError::from_anyhow(error.into()))?;
    writeln!(stdout, "{}", serde_json::to_string(&value).unwrap())
        .map_err(|error| CliError::from_anyhow(error.into()))
}

fn resolve_session(
    service: &WorkspaceService,
    principal: &ServicePrincipal,
    requested: Option<&str>,
) -> Result<SessionId, CliError> {
    match requested.unwrap_or("current") {
        "current" => service
            .current(principal)
            .map_err(CliError::from_anyhow)?
            .ok_or_else(|| {
                CliError::new(
                    "current_session_unavailable",
                    "no current session is selected for native-cli in this Workspace",
                )
            }),
        "latest" => service
            .sessions(principal, None, 1)
            .map_err(CliError::from_anyhow)?
            .sessions
            .into_iter()
            .next()
            .map(|session| session.session_id)
            .ok_or_else(|| {
                CliError::new(
                    "latest_session_unavailable",
                    "this Workspace has no visible sessions",
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
