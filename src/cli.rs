//! Private composition for the distributable `margins` binary.
//!
//! Parsing and non-interactive workflows remain in the public CLI crate. The
//! native recorder and memo TUI intentionally stay here, on the private side of
//! the open-core boundary.

use crate::note::AGENT_SKILL_DIRS;
use anyhow::Context;
use anyhow::{bail, Result};
#[cfg(feature = "audio-capture")]
use chrono::Local;
use clap::Parser;
use include_dir::{include_dir, Dir};
use margins_cli::args::{Args, Command, SetupLocalModelPolicyArg, SetupSkipArg, SetupStepArg};
use std::ffi::OsString;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;
#[cfg(feature = "audio-capture")]
use std::path::PathBuf;
#[cfg(feature = "audio-capture")]
use std::sync::atomic::AtomicBool;
#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
use std::sync::atomic::AtomicU64;
#[cfg(any(test, feature = "audio-capture"))]
use std::sync::atomic::AtomicU8;
#[cfg(any(test, feature = "audio-capture"))]
use std::sync::atomic::Ordering;
#[cfg(feature = "audio-capture")]
use std::sync::Mutex;
#[cfg(any(test, feature = "audio-capture"))]
use std::sync::{mpsc, Arc};

trait InteractiveSession {
    fn create(&self, work_dir: &Path, title: Option<&str>) -> Result<()>;
    fn attach(&self, work_dir: &Path, selected: Option<&str>) -> Result<()>;
}

struct NativeInteractiveSession;

#[cfg(any(test, feature = "audio-capture"))]
trait CapturePermissionSource {
    fn permission(&self, lane: margins_core::AudioLane) -> Result<margins_core::PermissionState>;
    fn request_permission(
        &self,
        lane: margins_core::AudioLane,
    ) -> Result<margins_core::PermissionState>;
}

#[cfg(feature = "audio-capture")]
struct NativeCapturePermissionSource;

#[cfg(feature = "audio-capture")]
impl CapturePermissionSource for NativeCapturePermissionSource {
    fn permission(&self, lane: margins_core::AudioLane) -> Result<margins_core::PermissionState> {
        use crate::recorder::MicrophoneAuthorization;
        use margins_core::{AudioLane, PermissionState};

        match lane {
            AudioLane::Microphone => Ok(match crate::recorder::microphone_authorization()? {
                MicrophoneAuthorization::NotDetermined => PermissionState::NotDetermined,
                MicrophoneAuthorization::Restricted => PermissionState::Restricted,
                MicrophoneAuthorization::Denied => PermissionState::Denied,
                MicrophoneAuthorization::Authorized => PermissionState::Granted,
            }),
            // macOS does not expose a dependable passive preflight for the
            // process tap. Starting from the user's explicit capture action and
            // evaluating delivered frames is the truthful probe.
            AudioLane::System => Ok(PermissionState::Unknown),
            _ => Ok(PermissionState::Unavailable),
        }
    }

    fn request_permission(
        &self,
        lane: margins_core::AudioLane,
    ) -> Result<margins_core::PermissionState> {
        use margins_core::{AudioLane, PermissionState};

        match lane {
            AudioLane::Microphone => Ok(if crate::recorder::request_microphone_access()? {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            }),
            AudioLane::System => Ok(PermissionState::Unknown),
            _ => Ok(PermissionState::Unavailable),
        }
    }
}

#[cfg(any(test, feature = "audio-capture"))]
fn ensure_capture_permissions(source: &dyn CapturePermissionSource) -> Result<()> {
    use margins_core::{permission_action, AudioLane, PermissionAction};

    for lane in [AudioLane::Microphone, AudioLane::System] {
        let mut state = source.permission(lane)?;
        if permission_action(state) == PermissionAction::Request {
            state = source.request_permission(lane)?;
        }
        match permission_action(state) {
            PermissionAction::Proceed | PermissionAction::ProbeOnStart => {}
            PermissionAction::Blocked => match lane {
                AudioLane::Microphone => bail!("Margins needs Microphone permission. Grant it to your terminal in System Settings > Privacy & Security > Microphone, then quit and reopen the terminal before running Margins again."),
                AudioLane::System => bail!("{}", margins_cli::error::MACOS_SYSTEM_AUDIO_PERMISSION_DENIED_MESSAGE),
                _ => bail!("Margins does not have the required capture permission."),
            },
            PermissionAction::Unavailable => {
                bail!("the requested audio capture lane is unavailable")
            }
            PermissionAction::Request => {
                bail!("capture permission was not granted after the permission request")
            }
            _ => bail!("capture permission state is unsupported by this CLI"),
        }
    }
    Ok(())
}

/// Entry point used by the production binary. Only interactive capture is
/// intercepted; every other command retains the public parser/dispatcher.
pub fn main_entry<I, T>(args: I) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    main_entry_with(args, &NativeInteractiveSession)
}

pub fn main_entry_from_env() -> i32 {
    main_entry(std::env::args_os())
}

const RELEASE_SMOKE_COMMAND: &str = "__release-smoke";

fn official_capabilities_json() -> serde_json::Value {
    let mut distillation_inputs = vec!["transcript", "memo"];
    if cfg!(any(feature = "coreml-asr", feature = "parakeet-asr")) {
        distillation_inputs.push("audio");
    }
    let catalyst = margins_workflows::workspace::margins_home()
        .map(|home| crate::hosted_credentials::redacted_status(&home))
        .unwrap_or_else(|_| {
            serde_json::json!({
                "mode": "none",
                "usable": false,
                "reason": "config_unreadable",
                "expiry": {"state": "unknown", "bucket": "unknown"},
            })
        });
    serde_json::json!({
        "schema": 1,
        "product": "margins",
        "composition": "official",
        "official": true,
        "autonomous": true,
        "build": margins_cli::build_info::get(),
        "recall": {
            "available": cfg!(feature = "recall"),
            "scan": cfg!(feature = "recall"),
            "indexing": cfg!(feature = "recall"),
            "lookup": cfg!(feature = "recall"),
            "local_model": cfg!(feature = "recall-local-model"),
        },
        "catalyst": catalyst,
        "capture": {
            "available": cfg!(feature = "audio-capture"),
            "provider": if cfg!(feature = "audio-capture") {
                "native-recorder"
            } else {
                "unavailable"
            },
        },
        "workspace": {
            "setup": true,
        },
        "audio_import": {
            "available": cfg!(any(feature = "coreml-asr", feature = "parakeet-asr")),
            "decoder": true,
        },
        "note_skill": {
            "available": true,
        },
        // Additive public workflow description used by the portable note skill.
        "distillation": {
            "available": true,
            "workflow": "connected_note",
            "inputs": distillation_inputs,
        },
        "tui": {
            "available": true,
        },
        // Backward-compatible fields consumed by the release smoke script.
        "capture_available": cfg!(feature = "audio-capture"),
        "capture_provider": if cfg!(feature = "audio-capture") {
            "native-recorder"
        } else {
            "unavailable"
        },
        "tui_available": true,
    })
}

/// Side-effect-free packaged-binary probe used by the private release pipeline.
///
/// Keeping this command in the private composition (rather than the public CLI
/// parser) makes a public-only binary with its no-capture service unable to
/// satisfy the release contract.
fn release_smoke(args: &[OsString]) -> Option<i32> {
    let [_, command] = args else {
        return None;
    };
    if command != RELEASE_SMOKE_COMMAND {
        return None;
    }

    println!("{}", official_capabilities_json());
    Some(0)
}

fn main_entry_with<I, T>(args: I, interactive: &dyn InteractiveSession) -> i32
where
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    if let Err(error) = crate::initialize_sqlite_runtime() {
        return report_error(&format!("SQLite runtime initialization failed: {error:#}"));
    }
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if let Some(code) = release_smoke(&args) {
        return code;
    }
    let (workspace_selector, workspace_args) =
        match margins_cli::args::strip_workspace_arg(args.clone()) {
            Ok(value) => value,
            Err(error) => return report_error(&error),
        };
    let (project_selector, parse_args) = match margins_cli::args::strip_project_arg(workspace_args)
    {
        Ok(value) => value,
        Err(error) => return report_error(&error),
    };
    let parsed = match Args::try_parse_from(parse_args) {
        Ok(value) => value,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };

    // Intercept Setup before the interactive dispatch.
    if let Some(Command::Setup {
        only,
        skip,
        local_model,
    }) = &parsed.command
    {
        return run_setup(workspace_selector.as_deref(), only, *skip, *local_model);
    }

    if matches!(parsed.command, Some(Command::Capabilities)) {
        println!("{}", official_capabilities_json());
        return 0;
    }

    #[cfg(feature = "recall")]
    if matches!(
        parsed.command,
        Some(Command::Workspace {
            command: margins_cli::args::WorkspaceCommand::Status { json: true }
        })
    ) {
        return run_workspace_status(workspace_selector.as_deref());
    }

    if let Some(Command::Connect {
        command:
            margins_cli::args::ConnectCommand::Google {
                account,
                headless,
                json,
            },
    }) = &parsed.command
    {
        let mut stdout = std::io::stdout().lock();
        let credential = Some(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/private/margins-google/resources/google-oauth-client.json"
            ))
            .as_slice(),
        );
        let home = match margins_workflows::workspace::margins_home() {
            Ok(home) => home,
            Err(error) => return report_error(&error.to_string()),
        };
        let result = margins_cli::commands::connect::google(
            &home,
            account.as_deref(),
            *headless,
            *json,
            credential,
            &mut stdout,
            &mut std::io::stderr().lock(),
        );
        return match result {
            Ok(()) => 0,
            Err(error) => {
                let message = margins_user_message(error.message());
                if *json {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "error": {"code": error.code(), "message": message}
                        })
                    );
                } else {
                    eprintln!(
                        "<margins_error code=\"{}\">{}</margins_error>",
                        error.code(),
                        message
                    );
                }
                error.exit_code()
            }
        };
    }

    if let Some(Command::Integrations {
        command:
            margins_cli::args::IntegrationsCommand::Reconcile {
                connector,
                account,
                if_revision,
                request_id,
                json,
            },
    }) = &parsed.command
    {
        debug_assert!(*json);
        if workspace_selector
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        {
            return report_json_cli_error(margins_cli::CliError::usage(
                "integrations reconcile requires literal --workspace <id>",
            ));
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let workspace = match margins_cli::commands::workspace::resolve(
            workspace_selector.as_deref(),
            &cwd,
            &mut stderr,
        ) {
            Ok(workspace) => workspace,
            Err(error) => return report_json_cli_error(error),
        };
        let fixture_base = std::env::var("MARGINS_GOOGLE_NATIVE_E2E_BASE_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let result = if let Some(base) = fixture_base {
            let native = margins_workflows::integrations::NativeGoogleClient::with_bases(
                None,
                Some("fixture-access-token".to_string()),
                &base,
                &base,
                &base,
                &base,
                &base,
            );
            margins_cli::commands::integrations::reconcile_with_native_google_client(
                &workspace.state_dir,
                connector.as_deref(),
                account.as_deref(),
                if_revision,
                request_id,
                &native,
                &mut stdout,
            )
        } else {
            let credential = include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/crates/private/margins-google/resources/google-oauth-client.json"
            ))
            .as_slice();
            margins_cli::commands::integrations::reconcile_with_google_credential(
                &workspace.state_dir,
                connector.as_deref(),
                account.as_deref(),
                if_revision,
                request_id,
                credential,
                &mut stdout,
            )
        };
        let code = result
            .as_ref()
            .map_or_else(|error| error.exit_code(), |_| 0);
        #[cfg(feature = "recall")]
        if code == 0 && workspace.ledger_path().is_file() {
            if let Err(error) = crate::recall::provision_workspace_for_init(&workspace) {
                return report_json_cli_error(
                    margins_cli::CliError::new(
                        "integration_index_refresh_failed",
                        margins_user_message(&format!(
                            "authoritative ledger mutation committed, but refreshing workspace recall failed: {error:#}"
                        )),
                    )
                    .with_details(serde_json::json!({
                        "ledger_committed": true,
                        "replay_safe": true,
                        "mutation_exit_code": code,
                    }))
                    .retryable(true),
                );
            }
        }
        let _ = io::stderr().write_all(&stderr);
        let _ = io::stdout().write_all(&stdout);
        return match result {
            Ok(()) => code,
            Err(error) => {
                if !error.is_reported() {
                    eprintln!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "ok": false,
                            "error": {"code": error.code(), "message": margins_user_message(error.message())}
                        })
                    );
                }
                code
            }
        };
    }

    if let Some(Command::Note { print }) = &parsed.command {
        return match crate::note::run(*print) {
            Ok(()) => 0,
            Err(error) => report_error(&format!("{error:#}")),
        };
    }

    // The official binary owns vault indexing because it composes the private
    // recall engine. Establish first, then publish success only after the index
    // and catalysts have been refreshed.
    #[cfg(feature = "recall")]
    if matches!(parsed.command, Some(Command::Init)) {
        return run_init(workspace_selector.as_deref());
    }

    // Intercept Recall: the official binary composes the vendored associative
    // search engine, so it never falls through to the public degradation path.
    #[cfg(feature = "recall")]
    if let Some(Command::Recall { query, source }) = &parsed.command {
        return run_recall(workspace_selector.as_deref(), query, source.as_deref());
    }

    #[cfg(feature = "recall")]
    if let Some(Command::Sync { source, json }) = &parsed.command {
        return run_sync(workspace_selector.as_deref(), source.as_deref(), *json);
    }

    // Scan is filesystem-only discovery. It shares Margins' explicit config
    // path but never opens or creates the recall database.
    #[cfg(feature = "recall")]
    if matches!(&parsed.command, Some(Command::Scan)) {
        return run_scan(workspace_selector.as_deref());
    }

    let interactive_command = match &parsed.command {
        None => Some((false, None, None, true)),
        Some(Command::New { title }) => Some((true, title.as_deref(), None, false)),
        Some(Command::Attach { session }) => Some((false, None, session.as_deref(), false)),
        _ => None,
    };
    let Some((create, title, selected, create_if_missing)) = interactive_command else {
        let refresh_after_reconcile = matches!(
            &parsed.command,
            Some(Command::Integrations {
                command: margins_cli::args::IntegrationsCommand::Reconcile { .. }
            })
        );
        let retention_apply = matches!(
            &parsed.command,
            Some(Command::Retention {
                command: margins_cli::args::RetentionCommand::Apply { .. }
            })
        );
        let granola_import = matches!(
            &parsed.command,
            Some(Command::Import {
                command: margins_cli::args::ImportCommand::Granola { .. }
            })
        );
        // Hold public mutation output until successful ledger writes have had
        // any required recall refresh. This preserves typed errors while never
        // publishing a materialization purge as complete ahead of Enzyme's
        // generic source reconciliation.
        let buffer_mutation = refresh_after_reconcile || retention_apply || granola_import;
        let (code, mutation_stdout, mutation_stderr) = if buffer_mutation {
            let services = margins_cli::standalone_services();
            let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let result = margins_cli::run(&services, &cwd, args.clone(), &mut stdout, &mut stderr);
            (
                result
                    .as_ref()
                    .map_or_else(|error| error.exit_code(), |_| 0),
                stdout,
                stderr,
            )
        } else {
            (margins_cli::main_entry(args), Vec::new(), Vec::new())
        };
        #[cfg(feature = "recall")]
        if granola_import && code == 0 {
            let workspace = match resolve_workspace(workspace_selector.as_deref()) {
                Ok(workspace) => workspace,
                Err(error) => return report_error(&error.to_string()),
            };
            if let Err(error) = crate::recall::provision_workspace_for_init(&workspace) {
                return report_json_cli_error(
                    margins_cli::CliError::new(
                        "granola_index_refresh_failed",
                        margins_user_message(&format!(
                            "Granola notes were imported, but refreshing Margins recall failed: {error:#}"
                        )),
                    )
                    .with_details(serde_json::json!({
                        "notes_committed": true,
                        "replay_safe": false,
                    })),
                );
            }
        }
        #[cfg(feature = "recall")]
        if (refresh_after_reconcile || retention_apply) && workspace_selector.is_some() {
            let retention_requires_refresh = retention_apply
                && serde_json::from_slice::<serde_json::Value>(&mutation_stdout)
                    .ok()
                    .and_then(|value| {
                        value
                            .get("index_refresh_required")
                            .and_then(|v| v.as_bool())
                    })
                    == Some(true);
            let refresh_required = refresh_after_reconcile || retention_requires_refresh;
            let workspace = match resolve_workspace(workspace_selector.as_deref()) {
                Ok(workspace) => workspace,
                Err(error) => {
                    return report_json_cli_error(margins_cli::CliError::new(
                        "workspace_resolution_failed",
                        margins_user_message(&error.to_string()),
                    ))
                }
            };
            // A mixed reconcile still commits successful connector deltas. Refresh
            // those ledger writes before preserving integration_failed's exit.
            if refresh_required && workspace.ledger_path().is_file() {
                if let Err(error) = crate::recall::provision_workspace_for_init(&workspace) {
                    let error_code = if retention_apply {
                        "retention_index_refresh_failed"
                    } else {
                        "integration_index_refresh_failed"
                    };
                    return report_json_cli_error(
                        margins_cli::CliError::new(
                            error_code,
                            margins_user_message(&format!(
                                "authoritative ledger mutation committed, but refreshing workspace recall failed: {error:#}"
                            )),
                        )
                        .with_details(serde_json::json!({
                            "ledger_committed": true,
                            "replay_safe": true,
                            "mutation_exit_code": code,
                        }))
                        .retryable(true),
                    );
                }
            }
        }
        if buffer_mutation {
            let _ = io::stdout().write_all(&mutation_stdout);
        }
        let _ = io::stderr().write_all(&mutation_stderr);
        return code;
    };

    let services = margins_cli::standalone_services();
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let project = match services
        .projects
        .resolve_vault(project_selector.as_deref(), &cwd)
    {
        Ok(project) => project,
        Err(error) => return report_error(&error.to_string()),
    };
    let create = if create_if_missing {
        let margins_dir = project.work_dir.join(".margins");
        let current = match services.sessions.current(&margins_dir) {
            Ok(current) => current,
            Err(error) => return report_error(&error.to_string()),
        };
        let current_exists = match current.as_deref().filter(|name| !name.trim().is_empty()) {
            Some(name) => match services.sessions.exists(&margins_dir, name) {
                Ok(exists) => exists,
                Err(error) => return report_error(&error.to_string()),
            },
            None => false,
        };
        bare_capture_creates(current.as_deref(), current_exists)
    } else {
        create
    };
    let result = if create {
        interactive.create(&project.work_dir, title)
    } else {
        interactive.attach(&project.work_dir, selected)
    };
    match result {
        Ok(()) => 0,
        Err(error) => report_error(&format!("{error:#}")),
    }
}

fn bare_capture_creates(current: Option<&str>, current_exists: bool) -> bool {
    current
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .is_none()
        || !current_exists
}

/// Resolve the vault git-style and run associative recall against it. Interactive
/// terminals receive Enzyme's catalyze tree; piped consumers receive the
/// compatible JSON envelope. Recall fails closed when catalysts cannot serve
/// the request; callers receive a non-zero command result and an explicit hint.
#[cfg(feature = "recall")]
fn run_recall(workspace_selector: Option<&str>, query: &str, source: Option<&str>) -> i32 {
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    let terminal = io::stdout().is_terminal();
    match crate::recall::recall(&workspace, query, source) {
        Ok(output) => match crate::recall::render_recall_for_stdout(&output, terminal) {
            Ok(rendered) => {
                print!("{rendered}");
                0
            }
            Err(error) => report_error(&format!("rendering recall: {error:#}")),
        },
        Err(error) => report_error(&format!("{error:#}")),
    }
}

#[cfg(feature = "recall")]
fn run_sync(workspace_selector: Option<&str>, source_filter: Option<&str>, json: bool) -> i32 {
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    if let Some(source) = source_filter {
        if !workspace.config.bindings.contains_key(source) {
            return report_sync_error(
                json,
                "source_not_configured",
                &format!(
                    "workspace '{}' has no declared source '{}'",
                    workspace.config.id, source
                ),
            );
        }
    }
    let revision = match margins_workflows::workspace::workspace_revision(&workspace.config) {
        Ok(revision) => revision,
        Err(error) => return report_error(&format!("reading workspace revision: {error:#}")),
    };
    let request_id = generated_sync_request_id();
    let mut sources = local_sync_rows(&workspace, source_filter);
    let external_rows =
        match sync_external_sources(&workspace, source_filter, &revision, &request_id) {
            Ok(rows) => rows,
            Err(error) => {
                return report_sync_error(
                    json,
                    error.code(),
                    &margins_user_message(error.message()),
                )
            }
        };
    sources.extend(external_rows.into_iter().map(sync_connector_row_json));

    let recall = match crate::recall::provision_workspace_for_init(&workspace) {
        Ok(status) => serde_json::json!({
            "ok": status.status != "catalysts_pending",
            "status": status.status,
            "reason": status.reason,
            "readiness": {
                "entities_curated": status.readiness.entities_curated,
                "catalysts_present": status.readiness.catalysts_present,
                "items_pending": status.readiness.items_pending,
                "reasons": status.readiness.reasons,
            },
        }),
        Err(error) => serde_json::json!({
            "ok": false,
            "status": "error",
            "reason": "index_refresh_failed",
            "error": margins_user_message(&format!("{error:#}")),
        }),
    };
    let ok = sources
        .iter()
        .all(|source| source.get("ok").and_then(serde_json::Value::as_bool) == Some(true))
        && recall.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    let envelope = serde_json::json!({
        "schema_version": "margins.sync.v1",
        "ok": ok,
        "request_id": request_id,
        "workspace": {
            "id": workspace.config.id,
            "path": workspace.state_dir,
        },
        "revision": {
            "before": revision,
            "after": revision,
        },
        "sources": sources,
        "recall": recall,
    });
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&envelope).unwrap_or_else(|_| envelope.to_string())
        );
    } else {
        println!("Sync workspace {}", workspace.config.id);
        for source in envelope
            .get("sources")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            let binding = source
                .get("binding")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let status = source
                .get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            println!("- {binding}: {status}");
        }
        let recall_status = envelope
            .get("recall")
            .and_then(|recall| recall.get("status"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        println!("- recall: {recall_status}");
    }
    if ok {
        0
    } else {
        1
    }
}

#[cfg(feature = "recall")]
fn sync_external_sources(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    source_filter: Option<&str>,
    revision: &str,
    request_id: &str,
) -> Result<Vec<margins_cli::commands::integrations::SyncConnectorRow>, margins_cli::CliError> {
    if let Some(base) = std::env::var("MARGINS_GOOGLE_NATIVE_E2E_BASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        let native = margins_workflows::integrations::NativeGoogleClient::with_bases(
            None,
            Some("fixture-access-token".to_string()),
            &base,
            &base,
            &base,
            &base,
            &base,
        );
        return margins_cli::commands::integrations::sync_declared_with_native_google_client(
            &workspace.state_dir,
            source_filter,
            revision,
            request_id,
            &native,
        );
    }
    let credential = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/private/margins-google/resources/google-oauth-client.json"
    ))
    .as_slice();
    margins_cli::commands::integrations::sync_declared_with_google_credential(
        &workspace.state_dir,
        source_filter,
        revision,
        request_id,
        Some(credential),
    )
}

#[cfg(feature = "recall")]
fn local_sync_rows(
    workspace: &margins_workflows::workspace::ResolvedWorkspace,
    source_filter: Option<&str>,
) -> Vec<serde_json::Value> {
    workspace
        .config
        .bindings
        .iter()
        .filter(|(name, _)| source_filter.is_none_or(|filter| filter == name.as_str()))
        .filter_map(|(name, binding)| {
            let kind = match binding {
                margins_workflows::workspace::WorkspaceBinding::NativeMarkdown { .. } => "notes",
                margins_workflows::workspace::WorkspaceBinding::Captures { .. } => "captures",
                _ => return None,
            };
            Some(serde_json::json!({
                "binding": name,
                "connector_id": kind,
                "account": null,
                "ok": true,
                "status": "synced",
                "records_written": 0,
                "records_updated": 0,
                "records_unchanged": 0,
                "tombstones": 0,
            }))
        })
        .collect()
}

#[cfg(feature = "recall")]
fn sync_connector_row_json(
    row: margins_cli::commands::integrations::SyncConnectorRow,
) -> serde_json::Value {
    serde_json::to_value(row).unwrap_or_else(|error| {
        serde_json::json!({
            "binding": "unknown",
            "connector_id": "unknown",
            "account": "",
            "ok": false,
            "status": "error",
            "error": {
                "code": "sync_result_encoding_failed",
                "message": error.to_string(),
                "retryable": false,
            },
        })
    })
}

#[cfg(feature = "recall")]
fn report_sync_error(json: bool, code: &str, message: &str) -> i32 {
    if json {
        eprintln!(
            "{}",
            serde_json::json!({
                "schema_version": "margins.sync.v1",
                "ok": false,
                "error": {"code": code, "message": message}
            })
        );
        1
    } else {
        report_error(message)
    }
}

#[cfg(feature = "recall")]
fn generated_sync_request_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    format!("sync-{millis}-{}", std::process::id())
}

#[cfg(feature = "recall")]
fn run_scan(workspace_selector: Option<&str>) -> i32 {
    let cwd = std::env::current_dir().unwrap_or_else(|_| Path::new(".").to_path_buf());
    let workspace =
        match margins_cli::commands::workspace::resolve_existing(workspace_selector, &cwd) {
            Ok(workspace) => workspace,
            Err(error) => return report_error(&error.to_string()),
        };
    match crate::scan::run_scan(&workspace) {
        Ok(()) => 0,
        Err(error) => report_error(&format!("scanning vault: {error:#}")),
    }
}

#[cfg(feature = "recall")]
fn run_init(workspace_selector: Option<&str>) -> i32 {
    let workspace = match resolve_workspace(workspace_selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    let margins_home = match crate::hosted_credentials::margins_home() {
        Ok(home) => home,
        Err(error) => return report_error(&error.to_string()),
    };
    eprintln!("Building or refreshing recall index and catalysts…");
    match crate::recall::provision_workspace_for_init(&workspace) {
        Ok(status) => {
            if status.status == "catalysts_pending" {
                return report_error(&status.readiness.message());
            }
            let catalyst = margins_workflows::catalyst::selected_status(&margins_home);
            match margins_cli::commands::projects::write_init(
                &mut io::stdout(),
                &workspace.home_dir,
                status.status,
                Some(&workspace.config_path),
                Some(&catalyst),
            ) {
                Ok(()) => 0,
                Err(error) => report_error(&error.to_string()),
            }
        }
        Err(error) => report_error(&format!("indexing vault: {error:#}")),
    }
}

fn resolve_workspace(
    selector: Option<&str>,
) -> anyhow::Result<margins_workflows::workspace::ResolvedWorkspace> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let home = margins_workflows::workspace::margins_home()?;
    let roots = margins_workflows::workspace::ImplicitWorkspaceRoots::from_process(&home)?;
    let resolution =
        margins_workflows::workspace::resolve_or_create_workspace(&roots, selector, &cwd)?;
    if resolution.created_implicitly {
        eprintln!(
            "Created workspace {} with home {}",
            resolution.workspace.config.id,
            resolution.workspace.home_dir.display()
        );
    }
    Ok(resolution.workspace)
}

#[cfg(feature = "recall")]
fn run_workspace_status(selector: Option<&str>) -> i32 {
    let workspace = match resolve_workspace(selector) {
        Ok(workspace) => workspace,
        Err(error) => return report_error(&error.to_string()),
    };
    let source_refresh_staleness =
        match crate::recall::workspace_source_refresh_staleness(&workspace) {
            Ok(status) => status,
            Err(error) => {
                return report_error(&format!("reading recall source freshness: {error:#}"))
            }
        };
    match margins_cli::commands::workspace::render_status(
        &workspace,
        true,
        &source_refresh_staleness,
        &mut io::stdout(),
    ) {
        Ok(()) => 0,
        Err(error) => report_error(&error.to_string()),
    }
}

fn report_error(message: &str) -> i32 {
    let message = margins_user_message(message);
    eprintln!(
        "<margins_error code=\"command_failed\">{}</margins_error>",
        margins_cli::output::xml_escape_text(&message)
    );
    1
}

fn report_json_cli_error(error: margins_cli::CliError) -> i32 {
    let exit_code = error.exit_code();
    let _ = margins_cli::output::write_json_error(&mut io::stderr(), &error);
    exit_code
}

fn margins_user_message(message: &str) -> String {
    let mut result = message.to_string();
    for (internal, replacement) in [
        ("enzyme login", "margins setup"),
        ("--use-env-llm", "margins setup"),
        ("api.enzyme.garden", "hosted catalyst service"),
        ("openrouter", "hosted catalyst provider"),
    ] {
        while let Some(start) = result.to_ascii_lowercase().find(internal) {
            result.replace_range(start..start + internal.len(), replacement);
        }
    }
    result
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HostedCatalystSetup {
    Hosted { model: String },
    Offline { reason: String },
}

trait SetupMachineProvisioner {
    fn provision_hosted_catalyst(&self, margins_home: &Path) -> Result<HostedCatalystSetup>;
    fn provision_speech(&self) -> Result<Option<std::path::PathBuf>>;
    fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>>;
}

struct NativeSetupMachineProvisioner;

static MARGINS_SKILL: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills/margins");
static WATERMARK_SKILL: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/skills/watermark");

const EMBEDDED_SKILLS: [(&str, &Dir<'static>); 2] =
    [("margins", &MARGINS_SKILL), ("watermark", &WATERMARK_SKILL)];

impl SetupMachineProvisioner for NativeSetupMachineProvisioner {
    fn provision_hosted_catalyst(&self, margins_home: &Path) -> Result<HostedCatalystSetup> {
        #[cfg(feature = "recall")]
        {
            return match crate::hosted_credentials::provision_hosted_or_local(margins_home)? {
                crate::hosted_credentials::HostedSetupOutcome::Hosted(report) => {
                    Ok(HostedCatalystSetup::Hosted {
                        model: report.model,
                    })
                }
                crate::hosted_credentials::HostedSetupOutcome::LocalFallback { reason } => {
                    Ok(HostedCatalystSetup::Offline { reason })
                }
            };
        }
        #[cfg(not(feature = "recall"))]
        {
            let _ = margins_home;
            bail!("hosted catalyst provisioning is unavailable in this build")
        }
    }

    fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
        ensure_speech_model(true)?;
        Ok(crate::speech_model_setup::find_installed_model())
    }

    fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
        #[cfg(feature = "recall-local-model")]
        {
            return crate::catalyst_model_setup::ensure_installed().map(Some);
        }
        #[cfg(not(feature = "recall-local-model"))]
        {
            Ok(None)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SetupSelection {
    catalyst: bool,
    skills: bool,
    speech: bool,
    local_model: SetupLocalModelPolicyArg,
}

impl SetupSelection {
    fn from_args(
        only: &[SetupStepArg],
        skip: Option<SetupSkipArg>,
        local_model: SetupLocalModelPolicyArg,
    ) -> Self {
        let all = only.is_empty();
        Self {
            catalyst: all || only.contains(&SetupStepArg::Catalyst),
            skills: all || only.contains(&SetupStepArg::Skills),
            speech: (all || only.contains(&SetupStepArg::Speech))
                && skip != Some(SetupSkipArg::Speech),
            local_model,
        }
    }
}

/// Machine-level setup is deliberately directory-agnostic. Each selected step
/// runs even when an earlier step failed; the final handoff names only an
/// explicitly selected Workspace and never derives one from the process cwd.
fn run_setup(
    workspace_selector: Option<&str>,
    only: &[SetupStepArg],
    skip: Option<SetupSkipArg>,
    local_model: SetupLocalModelPolicyArg,
) -> i32 {
    let selection = SetupSelection::from_args(only, skip, local_model);
    let skill_home = home_dir();
    let margins_home = margins_workflows::workspace::margins_home();
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let env_workspace = std::env::var("MARGINS_WORKSPACE")
        .ok()
        .filter(|value| !value.trim().is_empty());
    let workspace = workspace_selector.or(env_workspace.as_deref());
    match run_setup_with(
        &NativeSetupMachineProvisioner,
        selection,
        margins_home.as_deref().ok(),
        margins_home
            .as_ref()
            .err()
            .map(ToString::to_string)
            .as_deref(),
        skill_home.as_deref(),
        workspace,
        &mut stdout,
        &mut stderr,
    ) {
        Ok(failed) => i32::from(failed),
        Err(error) => report_error(&format!("writing setup report: {error}")),
    }
}

fn run_setup_with(
    provisioner: &dyn SetupMachineProvisioner,
    selection: SetupSelection,
    margins_home: Option<&Path>,
    margins_home_error: Option<&str>,
    skill_home: Option<&Path>,
    workspace: Option<&str>,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<bool> {
    let mut hosted_ready = false;
    let mut local_model_failed = false;

    if selection.catalyst {
        match margins_home {
            Some(home) => match provisioner.provision_hosted_catalyst(home) {
                Ok(HostedCatalystSetup::Hosted { model }) => {
                    hosted_ready = true;
                    setup_status(
                        stderr,
                        "hosted catalyst",
                        true,
                        &format!("ready with {model}"),
                    )?;
                }
                Ok(HostedCatalystSetup::Offline { reason }) => {
                    setup_status(stderr, "hosted catalyst", false, &reason)?;
                }
                Err(error) => {
                    setup_status(stderr, "hosted catalyst", false, &format!("{error:#}"))?;
                }
            },
            None => {
                setup_status(
                    stderr,
                    "hosted catalyst",
                    false,
                    margins_home_error.unwrap_or("could not resolve MARGINS_HOME"),
                )?;
            }
        }
    }

    if selection.skills {
        match skill_home {
            Some(home) => match install_embedded_skills(home, stderr) {
                Ok(()) => setup_status(
                    stderr,
                    "skills",
                    true,
                    "installed embedded margins and watermark skills",
                )?,
                Err(error) => {
                    setup_status(stderr, "skills", false, &format!("{error:#}"))?;
                }
            },
            None => {
                setup_status(
                    stderr,
                    "skills",
                    false,
                    "could not resolve HOME for agent skill installation",
                )?;
            }
        }
    }

    if selection.speech {
        match provisioner.provision_speech() {
            Ok(Some(model_dir)) => setup_status(
                stderr,
                "speech",
                true,
                &format!(
                    "model cache {} ({})",
                    model_dir.display(),
                    human_bytes(dir_size(&model_dir))
                ),
            )?,
            Ok(None) => setup_status(
                stderr,
                "speech",
                true,
                "no local speech model is required by this build",
            )?,
            Err(error) => {
                setup_status(stderr, "speech", false, &format!("{error:#}"))?;
            }
        }
    }

    if selection.catalyst {
        let install_local =
            !hosted_ready || selection.local_model == SetupLocalModelPolicyArg::Always;
        if install_local {
            match provisioner.provision_local_catalyst() {
                Ok(Some(path)) => setup_status(
                    stderr,
                    "local catalyst",
                    true,
                    &format!(
                        "installed at {} ({})",
                        path.display(),
                        human_bytes(std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0))
                    ),
                )?,
                Ok(None) => setup_status(
                    stderr,
                    "local catalyst",
                    true,
                    "selected without a bundled local-model installer",
                )?,
                Err(error) => {
                    local_model_failed = true;
                    setup_status(stderr, "local catalyst", false, &format!("{error:#}"))?;
                }
            }
        } else {
            setup_status(
                stderr,
                "local catalyst",
                true,
                "not installed under fallback policy because hosted catalyst is selected",
            )?;
        }
    }

    let usable_generator = if let Some(home) = margins_home {
        let catalyst = margins_workflows::catalyst::selected_status(home);
        writeln!(
            stderr,
            "catalyst mode: {} — {}",
            catalyst.mode.as_str(),
            catalyst.reason
        )?;
        match catalyst.mode {
            margins_workflows::catalyst::CatalystMode::Hosted => true,
            margins_workflows::catalyst::CatalystMode::Local => !local_model_failed,
            margins_workflows::catalyst::CatalystMode::None => false,
        }
    } else {
        writeln!(stderr, "catalyst mode: none — config_unreadable")?;
        false
    };

    margins_cli::commands::guide::setup_handoff(workspace, stdout)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    Ok(!usable_generator)
}

fn setup_status(report: &mut dyn Write, step: &str, ok: bool, reason: &str) -> Result<()> {
    let reason = margins_user_message(reason);
    let reason = reason.split_whitespace().collect::<Vec<_>>().join(" ");
    writeln!(
        report,
        "setup {step}: {} — {reason}",
        if ok { "ok" } else { "failed" }
    )?;
    Ok(())
}

fn install_embedded_skills(home: &Path, report: &mut dyn Write) -> Result<()> {
    let canonical_root = home.join(".margins/skills");
    for (name, embedded) in EMBEDDED_SKILLS {
        let destination = canonical_root.join(name);
        remove_existing_canonical_skill(&destination)?;
        materialize_embedded_dir(embedded, &destination)?;
    }
    let canonical_root = std::fs::canonicalize(&canonical_root).with_context(|| {
        format!(
            "failed to resolve canonical skill directory {}",
            canonical_root.display()
        )
    })?;

    for (agent, agent_home_name) in AGENT_SKILL_DIRS {
        let agent_home = home.join(agent_home_name);
        if !agent_home.is_dir() {
            writeln!(
                report,
                "{} skills: skipped (not installed)",
                agent.display()
            )?;
            continue;
        }
        install_agent_skill_links(agent.display(), &agent_home, &canonical_root, report)?;
    }
    Ok(())
}

fn remove_existing_canonical_skill(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir_all(path)
            .with_context(|| format!("failed to replace canonical skill {}", path.display()))?,
        Ok(_) => std::fs::remove_file(path)
            .with_context(|| format!("failed to replace canonical skill {}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect canonical skill {}", path.display()));
        }
    }
    Ok(())
}

fn materialize_embedded_dir(embedded: &Dir<'_>, destination: &Path) -> Result<()> {
    std::fs::create_dir_all(destination).with_context(|| {
        format!(
            "failed to create embedded skill directory {}",
            destination.display()
        )
    })?;

    for file in embedded.files() {
        let name = file.path().file_name().with_context(|| {
            format!("embedded skill file has no name: {}", file.path().display())
        })?;
        let destination = destination.join(name);
        std::fs::write(&destination, file.contents())
            .with_context(|| format!("failed to write embedded skill {}", destination.display()))?;
    }
    for directory in embedded.dirs() {
        let name = directory.path().file_name().with_context(|| {
            format!(
                "embedded skill directory has no name: {}",
                directory.path().display()
            )
        })?;
        materialize_embedded_dir(directory, &destination.join(name))?;
    }
    Ok(())
}

#[cfg(unix)]
fn install_agent_skill_links(
    agent: &str,
    agent_home: &Path,
    canonical_root: &Path,
    report: &mut dyn Write,
) -> Result<()> {
    let skills_dir = agent_home.join("skills");
    std::fs::create_dir_all(&skills_dir).with_context(|| {
        format!(
            "failed to create {agent} skill directory {}",
            skills_dir.display()
        )
    })?;

    let mut linked = Vec::new();
    let mut preserved = Vec::new();
    for (skill, _) in EMBEDDED_SKILLS {
        let target = canonical_root.join(skill);
        let link = skills_dir.join(skill);
        match std::fs::symlink_metadata(&link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                if std::fs::canonicalize(&link).ok() != Some(target.clone()) {
                    std::fs::remove_file(&link).with_context(|| {
                        format!(
                            "failed to remove stale {agent} skill link {}",
                            link.display()
                        )
                    })?;
                    std::os::unix::fs::symlink(&target, &link).with_context(|| {
                        format!("failed to link {agent} skill {}", link.display())
                    })?;
                }
                linked.push(skill);
            }
            Ok(_) => preserved.push(skill),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::os::unix::fs::symlink(&target, &link)
                    .with_context(|| format!("failed to link {agent} skill {}", link.display()))?;
                linked.push(skill);
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to inspect {agent} skill path {}", link.display())
                });
            }
        }
    }

    write_agent_skill_report(agent, &linked, &preserved, report)
}

#[cfg(not(unix))]
fn install_agent_skill_links(
    agent: &str,
    _agent_home: &Path,
    _canonical_root: &Path,
    report: &mut dyn Write,
) -> Result<()> {
    writeln!(
        report,
        "{agent} skills: skipped (symlinks unsupported on this platform)"
    )?;
    Ok(())
}

#[cfg(unix)]
fn write_agent_skill_report(
    agent: &str,
    linked: &[&str],
    preserved: &[&str],
    report: &mut dyn Write,
) -> Result<()> {
    match (linked.is_empty(), preserved.is_empty()) {
        (false, true) => writeln!(report, "{agent} skills: linked {}", linked.join(", "))?,
        (true, false) => writeln!(
            report,
            "{agent} skills: skipped (user dir present: {})",
            preserved.join(", ")
        )?,
        (false, false) => writeln!(
            report,
            "{agent} skills: linked {}; skipped {} (user dir present)",
            linked.join(", "),
            preserved.join(", ")
        )?,
        (true, true) => unreachable!("Margins always embeds at least one skill"),
    }
    Ok(())
}

pub(crate) fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
}

/// Recursively sum file sizes under `path`; best-effort, ignores unreadable entries.
fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = std::fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => total += dir_size(&entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(feature = "audio-capture")]
fn post_new_session_hint(session_name: &str) -> String {
    let home = home_dir();
    post_new_session_hint_for_home(home.as_deref(), session_name)
}

fn post_new_session_hint_for_home(home: Option<&Path>, _session_name: &str) -> String {
    if distillation_skill_installed_for_home(home) {
        "To distill your latest session, run `margins note`.".to_string()
    } else {
        "Run `margins setup` once to finish setup, then `margins note`.".to_string()
    }
}

fn distillation_skill_installed_for_home(home: Option<&Path>) -> bool {
    home.map(|home| home.join(".margins/skills/margins/SKILL.md").is_file())
        .unwrap_or(true)
}

#[cfg(any(test, feature = "audio-capture"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PostCaptureAction {
    Distill,
    SavedOnly,
    NotOffered,
}

#[cfg(any(test, feature = "audio-capture"))]
fn ask_post_capture_action(
    input: &mut dyn BufRead,
    output: &mut dyn Write,
) -> Result<PostCaptureAction> {
    loop {
        write!(output, "Turn this session into a note? [Y/n] ")?;
        output.flush()?;

        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            return Ok(PostCaptureAction::SavedOnly);
        }
        match answer.trim().to_ascii_lowercase().as_str() {
            "" | "y" | "yes" => return Ok(PostCaptureAction::Distill),
            "n" | "no" => return Ok(PostCaptureAction::SavedOnly),
            _ => writeln!(output, "Please answer y or n.")?,
        }
    }
}

#[cfg(feature = "audio-capture")]
fn choose_post_capture_action() -> Result<PostCaptureAction> {
    let stdin = io::stdin();
    let stderr = io::stderr();
    let home = home_dir();
    if !stdin.is_terminal()
        || !stderr.is_terminal()
        || !distillation_skill_installed_for_home(home.as_deref())
    {
        return Ok(PostCaptureAction::NotOffered);
    }
    ask_post_capture_action(&mut stdin.lock(), &mut stderr.lock())
}

impl InteractiveSession for NativeInteractiveSession {
    fn create(&self, work_dir: &Path, title: Option<&str>) -> Result<()> {
        #[cfg(not(feature = "audio-capture"))]
        {
            let _ = (work_dir, title);
            bail!("audio capture is unavailable: rebuild Margins with the `audio-capture` feature")
        }
        #[cfg(feature = "audio-capture")]
        {
            ensure_capture_permissions(&NativeCapturePermissionSource)?;
            create_native_session(work_dir, title)
        }
    }

    fn attach(&self, work_dir: &Path, selected: Option<&str>) -> Result<()> {
        #[cfg(not(feature = "audio-capture"))]
        {
            let _ = (work_dir, selected);
            bail!("audio capture is unavailable: rebuild Margins with the `audio-capture` feature")
        }
        #[cfg(feature = "audio-capture")]
        {
            ensure_capture_permissions(&NativeCapturePermissionSource)?;
            attach_native_session(work_dir, selected)
        }
    }
}

#[cfg(feature = "audio-capture")]
fn create_native_session(work_dir: &Path, title: Option<&str>) -> Result<()> {
    ensure_speech_model(false)?;
    let margins_dir = work_dir.join(".margins");
    let started_at = Local::now();
    let name = margins_cli::commands::sessions::unique_session_name(
        &margins_cli::standalone_services(),
        &margins_dir,
        &started_at.format("%Y-%m-%d-%H-%M-%S").to_string(),
    )?;
    let memo_path = work_dir.join(".margins").join(format!("{name}.md"));
    let audio_path = work_dir.join(".margins").join(format!("{name}_seg0.wav"));
    let initial_offset_ms = 0i64;
    let live_artifact_ordinal: i64 = 0;
    let checkpoint_uri = format!(".margins/{name}_seg{live_artifact_ordinal}.live-transcript.json");
    let checkpoint_path = work_dir.join(&checkpoint_uri);
    // Begin model loading as soon as the stable session identity exists. It
    // overlaps with durable session bookkeeping, device lookup, and TUI setup.
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = start_live_transcript_worker(
        checkpoint_path,
        initial_offset_ms as u64,
        live_status.clone(),
    );

    // Open both native lanes before reserving any session state. Permission or
    // device-start failures therefore leave no memo, segment, or current
    // pointer behind.
    let initial_live_sink = live
        .as_ref()
        .map(|worker| worker.sink_for_offset(initial_offset_ms as u64));
    let initial_stop = Arc::new(AtomicBool::new(false));
    let initial_recorder = crate::recorder::RecorderHandle::start_with_live_audio(
        initial_stop.clone(),
        None,
        initial_live_sink,
    )?;

    std::fs::create_dir_all(&margins_dir).context("failed to create .margins directory")?;
    // Silent bookkeeping so desktop and `recent --all` can enumerate this folder.
    margins_workflows::project::register_vault_silently(work_dir);
    std::fs::write(&memo_path, "")
        .with_context(|| format!("failed to initialize memo {}", memo_path.display()))?;
    margins_store::legacy::create_session(
        &margins_dir,
        &name,
        &started_at,
        &relative_artifact(&memo_path, work_dir),
    )?;
    if let Some(title) = title {
        margins_store::legacy::set_title(&margins_dir, &name, Some(title.to_string()))?;
    }
    margins_store::legacy::add_segment(
        &margins_dir,
        &name,
        0,
        &relative_artifact(&audio_path, work_dir),
        0,
        None,
    )?;
    std::fs::write(margins_dir.join("current"), format!("{name}\n"))?;

    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::new(
        memo_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.live_transcription_status = live_status;

    let action = run_segment(
        &mut app,
        &margins_dir,
        work_dir,
        &name,
        0,
        audio_path,
        live,
        &checkpoint_uri,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
    )?;
    match action {
        PostCaptureAction::Distill => crate::note::run(false),
        PostCaptureAction::SavedOnly => {
            eprintln!("Session saved.");
            Ok(())
        }
        PostCaptureAction::NotOffered => {
            // One-line pointer at the moment of truth. A first capture nudges
            // setup only until the bundled distillation skill is installed.
            eprintln!("{}", post_new_session_hint(&name));
            Ok(())
        }
    }
}

#[cfg(feature = "audio-capture")]
fn attach_native_session(work_dir: &Path, selected: Option<&str>) -> Result<()> {
    ensure_speech_model(false)?;
    let margins_dir = work_dir.join(".margins");
    let name = match selected {
        Some(name) => name.to_string(),
        None => std::fs::read_to_string(margins_dir.join("current"))
            .context("No current session. Run `margins new` to start one.")?
            .trim()
            .to_string(),
    };
    if name.is_empty() || !margins_store::legacy::session_exists(&margins_dir, &name)? {
        bail!("Session '{name}' not found. Run `margins ls` to choose one.");
    }
    let meta = margins_store::legacy::get_session_meta(&margins_dir, &name)?;
    let started_at = margins_store::legacy::get_session_start_time(&margins_dir, &name)?;
    let memo_path = resolve_artifact(work_dir, &meta.notes_path);
    if !memo_path.exists() && meta.vault_note_path.is_none() {
        std::fs::write(&memo_path, "")?;
    }
    let parsed = crate::parser::parse_markdown(
        &std::fs::read_to_string(&memo_path).context("failed to read memo")?,
        &started_at,
    );
    let ordinal = margins_store::legacy::next_segment_index(&margins_dir, &name)?;
    let audio_path = margins_dir.join(format!("{name}_seg{ordinal}.wav"));
    let offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
    let live_artifact_ordinal = ordinal;
    let initial_offset_ms = offset_ms;
    let checkpoint_uri = format!(".margins/{name}_seg{live_artifact_ordinal}.live-transcript.json");
    let checkpoint_path = work_dir.join(&checkpoint_uri);
    let live_status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_WARMING));
    let live = start_live_transcript_worker(
        checkpoint_path,
        initial_offset_ms as u64,
        live_status.clone(),
    );
    let initial_live_sink = live
        .as_ref()
        .map(|worker| worker.sink_for_offset(initial_offset_ms as u64));
    let initial_stop = Arc::new(AtomicBool::new(false));
    let initial_recorder = crate::recorder::RecorderHandle::start_with_live_audio(
        initial_stop.clone(),
        None,
        initial_live_sink,
    )?;
    margins_store::legacy::add_segment(
        &margins_dir,
        &name,
        ordinal,
        &relative_artifact(&audio_path, work_dir),
        offset_ms,
        None,
    )?;
    std::fs::write(margins_dir.join("current"), format!("{name}\n"))?;
    let mic_name = crate::recorder::default_input_device_name().unwrap_or_else(|| "Unknown".into());
    let mut app = crate::app::App::from_parsed(
        parsed,
        memo_path.to_string_lossy().into_owned(),
        started_at,
        mic_name,
    );
    app.live_transcription_status = live_status;

    let action = run_segment(
        &mut app,
        &margins_dir,
        work_dir,
        &name,
        ordinal,
        audio_path,
        live,
        &checkpoint_uri,
        live_artifact_ordinal,
        started_at,
        initial_offset_ms,
        initial_recorder,
        initial_stop,
    )?;
    match action {
        PostCaptureAction::Distill => crate::note::run(false),
        PostCaptureAction::SavedOnly => {
            eprintln!("Session saved.");
            Ok(())
        }
        PostCaptureAction::NotOffered => Ok(()),
    }
}

#[cfg(feature = "audio-capture")]
fn start_live_transcript_worker(
    checkpoint_path: PathBuf,
    offset_ms: u64,
    status: Arc<AtomicU8>,
) -> Option<LiveTranscriptWorker> {
    match LiveTranscriptWorker::start(checkpoint_path, offset_ms, status.clone()) {
        Ok(worker) => worker,
        Err(error) => {
            // The TUI surfaces LIVE_TRANSCRIPTION_DEGRADED visually; keep stderr
            // clean and record the detail in diagnostics.
            crate::cli_log::event(
                "live_worker_unavailable",
                crate::cli_log::error_summary(&error),
            );
            status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(feature = "audio-capture")]
fn run_segment(
    app: &mut crate::app::App,
    margins_dir: &Path,
    work_dir: &Path,
    session_name: &str,
    initial_ordinal: i64,
    initial_audio_path: PathBuf,
    live: Option<LiveTranscriptWorker>,
    live_checkpoint_uri: &str,
    live_artifact_ordinal: i64,
    started_at: chrono::DateTime<Local>,
    initial_offset_ms: i64,
    initial_recorder: crate::recorder::RecorderHandle,
    initial_stop: Arc<AtomicBool>,
) -> Result<PostCaptureAction> {
    let mut ordinal = initial_ordinal;
    let mut audio_path = initial_audio_path;
    let mut selected_device: Option<crate::recorder::InputDevice> = None;
    let mut live_timeline_duration_ms: u64 = 0;
    let mut initial_recorder = Some(initial_recorder);
    let mut initial_stop = Some(initial_stop);

    // Memo durability must not depend on the optional transcription path.
    let mut tui_error: Option<anyhow::Error> = None;

    loop {
        let segment_offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
        let stop = initial_stop
            .take()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let recorder = if let Some(recorder) = initial_recorder.take() {
            recorder
        } else {
            let live_sink = live
                .as_ref()
                .map(|worker| worker.sink_for_offset(segment_offset_ms as u64));
            crate::recorder::RecorderHandle::start_with_live_audio(
                stop.clone(),
                selected_device.as_ref(),
                live_sink,
            )?
        };

        app.mic_level = recorder.mic_peak();
        app.spk_level = recorder.spk_peak();
        app.mic_drops = recorder.mic_drops();
        app.spk_drops = recorder.spk_drops();
        app.spk_silence = recorder.spk_silence();
        app.spk_frames = recorder.spk_frames();
        app.spk_rate = recorder.spk_rate();

        let tui_result = crate::tui::run_tui(app, stop.clone())
            .map_err(|error| anyhow::anyhow!(error.to_string()));
        stop.store(true, Ordering::SeqCst);
        let duration = recorder.stop_and_write(&audio_path.to_string_lossy())?;

        margins_store::legacy::update_segment_duration(
            margins_dir,
            session_name,
            ordinal,
            duration,
        )?;
        let artifact_uri = audio_path
            .strip_prefix(work_dir)
            .unwrap_or(&audio_path)
            .to_string_lossy();
        margins_store::legacy::upsert_session_artifact(
            margins_dir,
            session_name,
            "audio",
            ordinal,
            &artifact_uri,
            "durable",
            None,
        )?;

        live_timeline_duration_ms = live_timeline_duration_ms.max(
            (segment_offset_ms as u64)
                .saturating_sub(initial_offset_ms as u64)
                .saturating_add((duration * 1000.0).round().max(0.0) as u64),
        );

        match tui_result {
            Err(error) => {
                tui_error = Some(error);
                break;
            }
            Ok(crate::tui::TuiAction::Quit) => break,
            Ok(crate::tui::TuiAction::SwitchDevice(index)) => {
                let mut devices = crate::recorder::list_input_devices();
                if index >= devices.len() {
                    return Err(anyhow::anyhow!(
                        "selected audio input is no longer available"
                    ));
                }
                let (device_name, device) = devices.swap_remove(index);
                app.current_mic_name = device_name;
                selected_device = Some(device);
                ordinal = margins_store::legacy::next_segment_index(margins_dir, session_name)?;
                audio_path = margins_dir.join(format!("{session_name}_seg{ordinal}.wav"));
                let new_offset_ms = (Local::now() - started_at).num_milliseconds().max(0);
                margins_store::legacy::add_segment(
                    margins_dir,
                    session_name,
                    ordinal,
                    &relative_artifact(&audio_path, work_dir),
                    new_offset_ms,
                    None,
                )?;
            }
        }
    }

    // Signal terminal decoding before asking what to do next. The rolling
    // worker continues draining queued live audio while the user decides.
    let mut live_finalizer = live.map(|worker| worker.begin_finish(live_timeline_duration_ms));

    // Save memo regardless of transcription result.
    let memo_result = std::fs::write(&app.output_path, app.export())
        .with_context(|| format!("could not save memo {}", &app.output_path));
    if let Err(error) = memo_result {
        if let Some(finalizer) = live_finalizer.take() {
            let _ = finalizer.complete();
        }
        return Err(error);
    }

    let action_result = if tui_error.is_none() {
        choose_post_capture_action()
    } else {
        Ok(PostCaptureAction::NotOffered)
    };
    let show_progress = matches!(
        &action_result,
        Ok(PostCaptureAction::Distill | PostCaptureAction::SavedOnly)
    );
    let distill_requested = matches!(&action_result, Ok(PostCaptureAction::Distill));
    let live_result = (|| -> Result<()> {
        if let Some(finalizer) = live_finalizer.take() {
            let completed = if show_progress {
                finalizer.wait_with_spinner(distill_requested)?
            } else {
                finalizer.complete()?
            };
            if completed {
                margins_store::legacy::upsert_session_artifact(
                    margins_dir,
                    session_name,
                    "transcript",
                    live_artifact_ordinal,
                    live_checkpoint_uri,
                    "durable",
                    None,
                )?;
            }
        }
        Ok(())
    })();

    if let Some(error) = tui_error {
        return Err(error);
    }
    let action = action_result?;
    live_result?;
    Ok(action)
}

#[cfg(feature = "audio-capture")]
fn resolve_artifact(work_dir: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        work_dir.join(path)
    }
}

#[cfg(feature = "audio-capture")]
fn relative_artifact(path: &Path, work_dir: &Path) -> String {
    path.strip_prefix(work_dir)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// ── Live transcription ────────────────────────────────────────────────────────

const LIVE_QUEUE_MAX_SAMPLES: u64 = 16_000 * 30;

#[cfg(feature = "audio-capture")]
struct LiveTranscriptWorker {
    tx: mpsc::Sender<crate::recorder::LiveAudioChunk>,
    generation_clock: Arc<Mutex<crate::recorder::LiveGenerationClock>>,
    mic_accepted_samples: Arc<std::sync::atomic::AtomicU64>,
    system_accepted_samples: Arc<std::sync::atomic::AtomicU64>,
    mic_dropped_samples: Arc<std::sync::atomic::AtomicU64>,
    system_dropped_samples: Arc<std::sync::atomic::AtomicU64>,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
    status: Arc<AtomicU8>,
    finish_tx: mpsc::Sender<u64>,
    join: std::thread::JoinHandle<Result<()>>,
}

#[cfg(any(test, feature = "audio-capture"))]
struct LiveTranscriptFinalizer {
    status: Arc<AtomicU8>,
    send_result: std::result::Result<(), mpsc::SendError<u64>>,
    join: std::thread::JoinHandle<Result<()>>,
}

#[cfg(feature = "audio-capture")]
impl LiveTranscriptWorker {
    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    fn start(checkpoint: PathBuf, offset_ms: u64, status: Arc<AtomicU8>) -> Result<Option<Self>> {
        let Some(model_dir) = coreml_model_dir() else {
            // Degraded state is shown in the TUI; record the cause quietly.
            crate::cli_log::event(
                "live_worker_unavailable",
                "category=setup details=coreml_model_dir_not_found",
            );
            status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            return Ok(None);
        };
        let (tx, rx) = mpsc::channel();
        let (finish_tx, finish_rx) = mpsc::channel();
        let queued_samples = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let queued_for_thread = queued_samples.clone();
        let status_for_thread = status.clone();
        let join = std::thread::Builder::new()
            .name("margins-cli-live-transcription".into())
            .spawn(move || {
                use margins_media::providers::coreml::{
                    CoreMlStreamingConfig, FluidCoreMlAsr, StereoCoreMlAsrSession,
                };
                let warm_started = std::time::Instant::now();
                crate::cli_log::event("live_worker_warmup_started", "channels=2 backend=coreml");
                let prepare = || -> Result<StereoCoreMlAsrSession> {
                    let mut mic = FluidCoreMlAsr::from_dir_auto(&model_dir)?;
                    let mut system = FluidCoreMlAsr::from_dir_auto(&model_dir)?;
                    mic.warmup_models()?;
                    system.warmup_models()?;
                    Ok(StereoCoreMlAsrSession::new(
                        mic,
                        system,
                        CoreMlStreamingConfig::default(),
                    ))
                };
                let rolling = match prepare() {
                    Ok(rolling) => {
                        crate::cli_log::event(
                            "live_worker_ready",
                            format!("warmup_ms={}", warm_started.elapsed().as_millis()),
                        );
                        status_for_thread
                            .store(crate::app::LIVE_TRANSCRIPTION_READY, Ordering::Release);
                        rolling
                    }
                    Err(error) => {
                        crate::cli_log::event(
                            "live_worker_warmup_failed",
                            crate::cli_log::error_summary(&error),
                        );
                        status_for_thread
                            .store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
                        return Err(error);
                    }
                };
                run_live_worker(
                    rolling,
                    rx,
                    finish_rx,
                    checkpoint,
                    offset_ms,
                    queued_for_thread,
                )
            })?;
        Ok(Some(Self {
            tx,
            generation_clock: Arc::new(Mutex::new(crate::recorder::LiveGenerationClock {
                generation: 0,
                session_offset_ms: offset_ms,
            })),
            mic_accepted_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            system_accepted_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            mic_dropped_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            system_dropped_samples: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            queued_samples,
            status,
            finish_tx,
            join,
        }))
    }

    #[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
    fn start(_checkpoint: PathBuf, _offset_ms: u64, status: Arc<AtomicU8>) -> Result<Option<Self>> {
        status.store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
        Ok(None)
    }

    fn sink_for_offset(&self, session_offset_ms: u64) -> crate::recorder::LiveAudioSink {
        let generation = {
            let mut clock = self
                .generation_clock
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            clock.generation = clock.generation.saturating_add(1);
            clock.session_offset_ms = session_offset_ms;
            clock.generation
        };
        crate::recorder::LiveAudioSink {
            sender: self.tx.clone(),
            generation,
            generation_clock: self.generation_clock.clone(),
            mic_accepted_samples: self.mic_accepted_samples.clone(),
            system_accepted_samples: self.system_accepted_samples.clone(),
            mic_dropped_samples: self.mic_dropped_samples.clone(),
            system_dropped_samples: self.system_dropped_samples.clone(),
            queued_samples: self.queued_samples.clone(),
            queue_max_samples: LIVE_QUEUE_MAX_SAMPLES,
        }
    }

    fn begin_finish(self, duration_ms: u64) -> LiveTranscriptFinalizer {
        crate::cli_log::event(
            "live_worker_finish_requested",
            format!(
                "duration_ms={duration_ms} mic_accepted={} system_accepted={} mic_dropped={} system_dropped={} queued={}",
                self.mic_accepted_samples.load(Ordering::Acquire),
                self.system_accepted_samples.load(Ordering::Acquire),
                self.mic_dropped_samples.load(Ordering::Acquire),
                self.system_dropped_samples.load(Ordering::Acquire),
                self.queued_samples.load(Ordering::Acquire),
            ),
        );
        let send_result = self.finish_tx.send(duration_ms);
        drop(self.tx);
        LiveTranscriptFinalizer {
            status: self.status,
            send_result,
            join: self.join,
        }
    }
}

#[cfg(any(test, feature = "audio-capture"))]
impl LiveTranscriptFinalizer {
    fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    fn complete(self) -> Result<bool> {
        let finish_error = self.send_result.err().map(|error| error.to_string());
        let join_error = match self.join.join() {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(crate::cli_log::error_summary(&error)),
            Err(_) => Some("live transcription worker panicked".to_string()),
        };
        if let Some(reason) = optional_worker_failure_reason(
            self.status.load(Ordering::Acquire) == crate::app::LIVE_TRANSCRIPTION_DEGRADED,
            finish_error.as_deref(),
            join_error.as_deref(),
        ) {
            self.status
                .store(crate::app::LIVE_TRANSCRIPTION_DEGRADED, Ordering::Release);
            crate::cli_log::event(
                "live_worker_finished",
                format!("status=unavailable reason={reason}"),
            );
            return Ok(false);
        }
        crate::cli_log::event("live_worker_finished", "status=ok");
        Ok(true)
    }

    fn wait_with_spinner(self, distill_requested: bool) -> Result<bool> {
        if self.is_finished() {
            return self.complete();
        }

        let message = if distill_requested {
            "Finishing transcription before creating the note…"
        } else {
            "Finishing transcription…"
        };
        let mut stderr = io::stderr().lock();
        let terminal = stderr.is_terminal();
        if terminal {
            const FRAMES: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];
            let mut frame = 0usize;
            while !self.is_finished() {
                write!(stderr, "\r{} {message}", FRAMES[frame % FRAMES.len()])?;
                stderr.flush()?;
                frame = frame.wrapping_add(1);
                std::thread::sleep(std::time::Duration::from_millis(80));
            }
            write!(stderr, "\r\x1b[2K")?;
            stderr.flush()?;
        } else {
            writeln!(stderr, "{message}")?;
        }
        drop(stderr);

        let completed = self.complete()?;
        if terminal && completed {
            eprintln!("✓ Transcript ready.");
        }
        Ok(completed)
    }
}

#[cfg(any(test, feature = "audio-capture"))]
fn optional_worker_failure_reason(
    status_degraded: bool,
    finish_error: Option<&str>,
    join_error: Option<&str>,
) -> Option<String> {
    if let Some(error) = finish_error {
        Some(format!("finish_channel_closed: {error}"))
    } else if let Some(error) = join_error {
        Some(format!("worker_failed: {error}"))
    } else if status_degraded {
        Some("worker_reported_degraded".to_string())
    } else {
        None
    }
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn coreml_model_dir() -> Option<PathBuf> {
    let configured = std::env::var_os("MARGINS_FLUID_COREML_MODEL_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    if let Some(configured) = configured {
        return has_coreml_assets(&configured).then_some(configured);
    }
    let root = dirs::home_dir()?.join("Library/Application Support/FluidAudio/Models");
    let preferred = match std::env::var("MARGINS_FLUID_COREML_VERSION")
        .ok()
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        Some("v3") | Some("3") => "parakeet-tdt-0.6b-v3",
        _ => "parakeet-tdt-0.6b-v2",
    };
    [
        root.join(preferred),
        root.join("parakeet-tdt-0.6b-v2"),
        root.join("parakeet-tdt-0.6b-v3"),
    ]
    .into_iter()
    .find(|candidate| has_coreml_assets(candidate))
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn has_coreml_assets(dir: &Path) -> bool {
    dir.join("Preprocessor.mlmodelc").exists()
        && dir.join("Encoder.mlmodelc").exists()
        && dir.join("Decoder.mlmodelc").exists()
        && dir.join("JointDecision.mlmodelc").exists()
        && dir.join("parakeet_vocab.json").exists()
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn run_live_worker(
    mut rolling: margins_media::providers::coreml::StereoCoreMlAsrSession,
    rx: mpsc::Receiver<crate::recorder::LiveAudioChunk>,
    finish_rx: mpsc::Receiver<u64>,
    checkpoint: PathBuf,
    offset_ms: u64,
    queued_samples: Arc<std::sync::atomic::AtomicU64>,
) -> Result<()> {
    use crate::recorder::LiveAudioChannel;
    let mut mic_resampler = None;
    let mut system_resampler = None;
    let mut last_update_local_ms = 0;
    let mut mic_samples = 0u64;
    let mut system_samples = 0u64;
    let mut active_generation = None;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(100)) {
            Ok(chunk) => {
                let consumed = chunk.samples.len() as u64;
                debit_queued_samples(&queued_samples, consumed);
                if active_generation != Some(chunk.generation) {
                    let current_local_ms =
                        mic_samples.max(system_samples).saturating_mul(1_000) / 16_000;
                    let gap_samples =
                        timeline_gap_samples(chunk.session_offset_ms, offset_ms, current_local_ms);
                    if gap_samples > 0 {
                        let silence = vec![0.0; gap_samples as usize];
                        rolling.mic.append_audio(&silence);
                        rolling.system.append_audio(&silence);
                        mic_samples = mic_samples.saturating_add(gap_samples);
                        system_samples = system_samples.saturating_add(gap_samples);
                    }
                    active_generation = Some(chunk.generation);
                }
                let slot = match chunk.channel {
                    LiveAudioChannel::Mic => &mut mic_resampler,
                    LiveAudioChannel::System => &mut system_resampler,
                };
                if slot
                    .as_ref()
                    .is_none_or(|(rate, _)| *rate != chunk.sample_rate)
                {
                    *slot = Some((
                        chunk.sample_rate,
                        margins_media::timeline::RationalResampler::new(chunk.sample_rate, 16_000)?,
                    ));
                }
                let samples = slot
                    .as_mut()
                    .expect("resampler initialized")
                    .1
                    .process(&chunk.samples)?;
                match chunk.channel {
                    LiveAudioChannel::Mic => {
                        mic_samples = mic_samples.saturating_add(samples.len() as u64);
                        rolling.mic.append_audio(&samples);
                    }
                    LiveAudioChannel::System => {
                        system_samples = system_samples.saturating_add(samples.len() as u64);
                        rolling.system.append_audio(&samples);
                    }
                }
                let local_end_ms = mic_samples.max(system_samples).saturating_mul(1_000) / 16_000;
                if local_end_ms.saturating_sub(last_update_local_ms) >= 3_000 {
                    let mic = rolling.mic.update_until(local_end_ms)?;
                    let system = rolling.system.update_until(local_end_ms)?;
                    write_checkpoint(&checkpoint, &mic, &system, offset_ms, false)?;
                    last_update_local_ms = local_end_ms;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let duration_ms = finish_rx
                    .recv()
                    .context("live transcription ended without a final duration")?;
                let local_duration_ms =
                    duration_ms.min(mic_samples.max(system_samples).saturating_mul(1_000) / 16_000);
                let mic = rolling.mic.finish_until(local_duration_ms)?;
                let system = rolling.system.finish_until(local_duration_ms)?;
                write_checkpoint(&checkpoint, &mic, &system, offset_ms, true)?;
                return Ok(());
            }
        }
    }
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn debit_queued_samples(queued_samples: &AtomicU64, consumed: u64) {
    // Recorder send currently publishes to the channel immediately before it
    // increments this accounting counter. A fast consumer can therefore win
    // that tiny race; wait for the producer's publication instead of leaving a
    // phantom queued balance that would eventually disable live transcription.
    for _ in 0..128 {
        if queued_samples
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
                (queued >= consumed).then_some(queued - consumed)
            })
            .is_ok()
        {
            return;
        }
        std::thread::yield_now();
    }
    let _ = queued_samples.fetch_update(Ordering::AcqRel, Ordering::Acquire, |queued| {
        Some(queued.saturating_sub(consumed))
    });
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn timeline_gap_samples(
    session_offset_ms: u64,
    initial_offset_ms: u64,
    current_local_ms: u64,
) -> u64 {
    session_offset_ms
        .saturating_sub(initial_offset_ms)
        .saturating_sub(current_local_ms)
        .saturating_mul(16_000)
        / 1_000
}

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn write_checkpoint(
    path: &Path,
    mic: &margins_media::providers::coreml::StreamingTranscriptUpdate,
    system: &margins_media::providers::coreml::StreamingTranscriptUpdate,
    offset_ms: u64,
    terminal: bool,
) -> Result<()> {
    let mut entries =
        margins_media::transcript::words_to_transcript_entries(&mic.committed, 0, offset_ms);
    entries.extend(margins_media::transcript::words_to_transcript_entries(
        &system.committed,
        1,
        offset_ms,
    ));
    let decoded_until_ms = mic
        .decoded_until_ms
        .max(system.decoded_until_ms)
        .saturating_add(offset_ms);
    let committed_until_ms = mic
        .committed_until_ms
        .min(system.committed_until_ms)
        .saturating_add(offset_ms)
        .min(decoded_until_ms);
    let value = serde_json::json!({
        "version": 2,
        "terminal": terminal,
        "decoded_until_ms": decoded_until_ms,
        "committed_until_ms": committed_until_ms,
        "transcripts": [{ "words": entries }],
    });
    write_checkpoint_value(path, &value)?;
    crate::cli_log::event(
        "live_checkpoint_written",
        format!(
            "terminal={terminal} decoded_until_ms={} committed_until_ms={} mic_words={} system_words={}",
            decoded_until_ms,
            committed_until_ms,
            mic.committed.len(),
            system.committed.len(),
        ),
    );
    Ok(())
}

#[cfg(any(test, all(feature = "coreml-asr", target_os = "macos")))]
fn write_checkpoint_value(path: &Path, value: &serde_json::Value) -> Result<()> {
    let parent = path.parent().context("checkpoint has no parent")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temp, &value)?;
    use std::io::Write as _;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

// ── Speech model setup ────────────────────────────────────────────────────────

#[cfg(all(feature = "coreml-asr", target_os = "macos"))]
fn ensure_speech_model(explicit_setup: bool) -> Result<()> {
    if crate::speech_model_setup::find_installed_model().is_some() {
        if explicit_setup {
            let speaker_setup = start_speaker_recognition()?;
            finish_speaker_recognition(speaker_setup, 0)?;
            print_local_speech_ready()?;
        }
        return Ok(());
    }

    let mut stderr = io::stderr().lock();
    if !io::stdin().is_terminal() {
        anyhow::bail!(
            "local transcription is not installed; run `margins setup` in a terminal before `margins new`"
        );
    }
    write!(
        stderr,
        "Local speech needs a one-time download. Download now? [Y/n] "
    )?;
    stderr.flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let accepted = matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "" | "y" | "yes"
    );
    if !accepted {
        if explicit_setup {
            anyhow::bail!("setup canceled");
        }
        anyhow::bail!(
            "local transcription is required before `margins new`; run `margins setup` when ready"
        );
    }

    let speaker_setup = start_speaker_recognition()?;
    let terminal = stderr.is_terminal();
    let mut last_percent = u8::MAX;
    let mut last_render = std::time::Instant::now();
    let mut transfer_started = None;
    let mut starting_bytes = None;
    let mut transcription_total = 0;
    crate::speech_model_setup::download_model(|downloaded, total| {
        transcription_total = total;
        let speaker_downloaded = speaker_setup.downloaded_bytes();
        let combined_downloaded = downloaded.saturating_add(speaker_downloaded);
        let combined_total = total.saturating_add(speaker_setup.total_bytes());
        let initial = *starting_bytes.get_or_insert(combined_downloaded);
        let started = *transfer_started.get_or_insert_with(std::time::Instant::now);
        let percent = if combined_total == 0 {
            0
        } else {
            ((combined_downloaded.saturating_mul(100) / combined_total).min(100)) as u8
        };
        if percent == last_percent && last_render.elapsed() < std::time::Duration::from_millis(250)
        {
            return;
        }
        last_percent = percent;
        last_render = std::time::Instant::now();
        let transferred = combined_downloaded.saturating_sub(initial);
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let bytes_per_second = transferred as f64 / elapsed;
        let remaining = combined_total.saturating_sub(combined_downloaded);
        let eta = if bytes_per_second >= 1.0 {
            Some(remaining as f64 / bytes_per_second)
        } else {
            None
        };
        let action = if initial > 0 {
            "Resuming"
        } else {
            "Downloading"
        };
        if terminal {
            let filled = usize::from(percent) / 4;
            let bar = format!("{}{}", "=".repeat(filled), " ".repeat(25 - filled));
            let downloaded_mb = combined_downloaded as f64 / 1_048_576.0;
            let total_mb = combined_total as f64 / 1_048_576.0;
            let speed_mb = bytes_per_second / 1_048_576.0;
            let eta = eta
                .map(|seconds| format!("  {:>3}s", seconds.ceil() as u64))
                .unwrap_or_default();
            let _ = write!(
                stderr,
                "\r{action} local speech [{bar}] {percent:>3}%  {downloaded_mb:.0}/{total_mb:.0} MB  {speed_mb:.1} MB/s{eta}   "
            );
            let _ = stderr.flush();
        } else if percent % 10 == 0 {
            let _ = writeln!(stderr, "Downloading: {percent}%");
        }
    })?;
    drop(stderr);
    finish_speaker_recognition(speaker_setup, transcription_total)?;
    print_local_speech_ready()?;
    Ok(())
}

#[cfg(not(all(feature = "coreml-asr", target_os = "macos")))]
fn ensure_speech_model(_explicit_setup: bool) -> Result<()> {
    // Non-macOS/non-CoreML builds skip model setup silently and let capture proceed.
    Ok(())
}

#[cfg(feature = "polyvoice-diarization")]
struct SpeakerSetup {
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
    files: Vec<(std::path::PathBuf, u64)>,
}

#[cfg(feature = "polyvoice-diarization")]
impl SpeakerSetup {
    fn total_bytes(&self) -> u64 {
        self.files.iter().map(|(_, size)| size).sum()
    }

    fn downloaded_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|(path, size)| {
                let complete = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                let partial_name = format!(
                    ".{}.partial",
                    path.file_name().unwrap_or_default().to_string_lossy()
                );
                let partial = path
                    .parent()
                    .and_then(|parent| std::fs::metadata(parent.join(partial_name)).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                complete.max(partial).min(*size)
            })
            .sum()
    }
}

#[cfg(feature = "polyvoice-diarization")]
fn start_speaker_recognition() -> Result<SpeakerSetup> {
    let files = margins_media::providers::polyvoice::balanced_model_downloads()
        .context("could not inspect speaker recognition models")?;
    let preparation = std::thread::spawn(|| {
        margins_media::providers::polyvoice::PolyvoiceDiarization::from_default_registry()
            .map(|_| ())
            .map_err(|error| error.to_string())
    });
    Ok(SpeakerSetup { preparation, files })
}

#[cfg(not(feature = "polyvoice-diarization"))]
struct SpeakerSetup {
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
}

#[cfg(not(feature = "polyvoice-diarization"))]
impl SpeakerSetup {
    fn total_bytes(&self) -> u64 {
        0
    }
    fn downloaded_bytes(&self) -> u64 {
        0
    }
}

#[cfg(not(feature = "polyvoice-diarization"))]
fn start_speaker_recognition() -> Result<SpeakerSetup> {
    anyhow::bail!("this Margins build does not include speaker recognition")
}

fn finish_speaker_recognition(
    setup: SpeakerSetup,
    completed_transcription_bytes: u64,
) -> Result<()> {
    let mut stderr = io::stderr().lock();
    let terminal = stderr.is_terminal();
    let speaker_total = setup.total_bytes();
    let combined_total = completed_transcription_bytes.saturating_add(speaker_total);
    while !setup.preparation.is_finished() && setup.downloaded_bytes() < speaker_total {
        if terminal && combined_total > 0 {
            let downloaded = completed_transcription_bytes.saturating_add(setup.downloaded_bytes());
            let percent = ((downloaded.saturating_mul(100) / combined_total).min(100)) as u8;
            let filled = usize::from(percent) / 4;
            let bar = format!("{}{}", "=".repeat(filled), " ".repeat(25 - filled));
            write!(stderr, "\rInstalling local speech [{bar}] {percent:>3}%   ")?;
            stderr.flush()?;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    drop(stderr);
    finish_animated_task("Validating local speech", setup.preparation)
}

fn finish_animated_task(
    active_label: &str,
    preparation: std::thread::JoinHandle<std::result::Result<(), String>>,
) -> Result<()> {
    let mut stderr = io::stderr().lock();
    let terminal = stderr.is_terminal();
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let mut frame = 0;
    let started = std::time::Instant::now();
    while !preparation.is_finished() {
        if terminal {
            write!(
                stderr,
                "\r  {}  {active_label}  ·  {:.1}s   ",
                frames[frame % frames.len()],
                started.elapsed().as_secs_f64(),
            )?;
            stderr.flush()?;
            frame += 1;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let result = preparation
        .join()
        .map_err(|_| anyhow::anyhow!("{active_label} stopped unexpectedly"))?;
    result.map_err(|error| anyhow::anyhow!("{active_label} failed: {error}"))?;
    Ok(())
}

fn print_local_speech_ready() -> Result<()> {
    let mut stderr = io::stderr().lock();
    if stderr.is_terminal() {
        writeln!(stderr, "\r\x1b[2K  ✓  Local speech ready")?;
    } else {
        writeln!(stderr, "Local speech ready.")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    static PROCESS_ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn cli_error_boundary_rewords_transport_and_engine_internals() {
        let output =
            margins_user_message("enzyme login --use-env-llm api.enzyme.garden OpenRouter");
        let lower = output.to_ascii_lowercase();
        for forbidden in [
            "enzyme login",
            "--use-env-llm",
            "api.enzyme.garden",
            "openrouter",
        ] {
            assert!(!lower.contains(forbidden), "leaked {forbidden}: {output}");
        }
        assert!(output.contains("margins setup"));
        assert!(output.contains("hosted catalyst service"));
        assert!(output.contains("hosted catalyst provider"));
    }

    struct ScopedTestSettings {
        _override: margins_workflows::project::TestSettingsPathOverride,
        _dir: tempfile::TempDir,
    }

    impl ScopedTestSettings {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path_override = margins_workflows::project::override_settings_path_for_test(
                dir.path().join("settings.json"),
            );
            Self {
                _override: path_override,
                _dir: dir,
            }
        }
    }

    struct SpyInteractive(AtomicUsize);
    impl InteractiveSession for SpyInteractive {
        fn create(&self, _work_dir: &Path, _title: Option<&str>) -> Result<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
        fn attach(&self, _work_dir: &Path, _selected: Option<&str>) -> Result<()> {
            self.0.fetch_add(10, Ordering::SeqCst);
            Ok(())
        }
    }

    struct ScriptedPermissionSource {
        mic: margins_core::PermissionState,
        requested_mic: margins_core::PermissionState,
        system: margins_core::PermissionState,
        request_count: AtomicUsize,
    }

    impl CapturePermissionSource for ScriptedPermissionSource {
        fn permission(
            &self,
            lane: margins_core::AudioLane,
        ) -> Result<margins_core::PermissionState> {
            match lane {
                margins_core::AudioLane::Microphone => Ok(self.mic),
                margins_core::AudioLane::System => Ok(self.system),
                _ => Ok(margins_core::PermissionState::Unavailable),
            }
        }

        fn request_permission(
            &self,
            lane: margins_core::AudioLane,
        ) -> Result<margins_core::PermissionState> {
            self.request_count.fetch_add(1, Ordering::SeqCst);
            assert_eq!(lane, margins_core::AudioLane::Microphone);
            Ok(self.requested_mic)
        }
    }

    #[test]
    fn native_permission_preflight_requests_mic_only_from_explicit_capture() {
        let source = ScriptedPermissionSource {
            mic: margins_core::PermissionState::NotDetermined,
            requested_mic: margins_core::PermissionState::Granted,
            system: margins_core::PermissionState::Unknown,
            request_count: AtomicUsize::new(0),
        };

        ensure_capture_permissions(&source).unwrap();

        assert_eq!(source.request_count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn native_permission_preflight_blocks_denied_mic_without_requesting() {
        let source = ScriptedPermissionSource {
            mic: margins_core::PermissionState::Denied,
            requested_mic: margins_core::PermissionState::Granted,
            system: margins_core::PermissionState::Unknown,
            request_count: AtomicUsize::new(0),
        };

        let error = ensure_capture_permissions(&source).unwrap_err().to_string();

        assert!(error.contains("Microphone permission"));
        assert_eq!(source.request_count.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn optional_transcript_shutdown_failures_degrade_instead_of_failing_capture() {
        assert_eq!(optional_worker_failure_reason(false, None, None), None);
        assert!(
            optional_worker_failure_reason(false, Some("sending on a closed channel"), None)
                .unwrap()
                .contains("finish_channel_closed")
        );
        assert!(
            optional_worker_failure_reason(false, None, Some("worker panicked"))
                .unwrap()
                .contains("worker_failed")
        );
        assert_eq!(
            optional_worker_failure_reason(true, None, None).as_deref(),
            Some("worker_reported_degraded")
        );
    }

    #[test]
    fn closed_optional_transcript_worker_is_nonfatal_at_the_real_finalizer_boundary() {
        let (finish_tx, finish_rx) = mpsc::channel();
        drop(finish_rx);
        let status = Arc::new(AtomicU8::new(crate::app::LIVE_TRANSCRIPTION_READY));
        let finalizer = LiveTranscriptFinalizer {
            status: status.clone(),
            send_result: finish_tx.send(1_000),
            join: std::thread::spawn(|| -> Result<()> {
                bail!("synthetic worker stopped before finalization")
            }),
        };

        assert!(!finalizer.complete().unwrap());
        assert_eq!(
            status.load(Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_DEGRADED
        );
    }

    struct SpySetupProvisioner {
        hosted_calls: AtomicUsize,
        speech_calls: AtomicUsize,
        local_calls: AtomicUsize,
        hosted: HostedCatalystSetup,
        speech_model: Option<std::path::PathBuf>,
        speech_error: Option<&'static str>,
        local_model: Option<std::path::PathBuf>,
    }

    impl SetupMachineProvisioner for SpySetupProvisioner {
        fn provision_hosted_catalyst(&self, _home: &Path) -> Result<HostedCatalystSetup> {
            self.hosted_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.hosted.clone())
        }

        fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
            self.speech_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(error) = self.speech_error {
                bail!(error);
            }
            Ok(self.speech_model.clone())
        }

        fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
            self.local_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.local_model.clone())
        }
    }

    struct FailingSpeechNativeProvisioner {
        speech_marker: std::path::PathBuf,
        local_calls: AtomicUsize,
    }

    impl SetupMachineProvisioner for FailingSpeechNativeProvisioner {
        fn provision_hosted_catalyst(&self, home: &Path) -> Result<HostedCatalystSetup> {
            NativeSetupMachineProvisioner.provision_hosted_catalyst(home)
        }

        fn provision_speech(&self) -> Result<Option<std::path::PathBuf>> {
            std::fs::write(&self.speech_marker, "speech attempted")?;
            bail!("fixture speech setup failed")
        }

        fn provision_local_catalyst(&self) -> Result<Option<std::path::PathBuf>> {
            self.local_calls.fetch_add(1, Ordering::SeqCst);
            Ok(None)
        }
    }

    struct EnvRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl EnvRestore {
        fn capture(names: &[&'static str]) -> Self {
            Self(
                names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            )
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[cfg(feature = "recall")]
    fn start_fake_setup_broker() -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write as _};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 2048];
            loop {
                let read = stream.read(&mut buffer).unwrap_or(0);
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = r#"{"api_key":"sk-or-v1-secret-shaped-setup-fixture","base_url":"https://fixture.invalid/v1","model":"fixture-catalyst-model","expires_at":4102444800}"#;
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/llm/free-config"), server)
    }

    struct ArtifactWritingInteractive;

    impl InteractiveSession for ArtifactWritingInteractive {
        fn create(&self, work_dir: &Path, _title: Option<&str>) -> Result<()> {
            let margins_dir = work_dir.join(".margins");
            std::fs::create_dir_all(&margins_dir)?;
            let name = "capture-root-regression";
            let memo = margins_dir.join(format!("{name}.md"));
            let audio = margins_dir.join(format!("{name}_seg0.wav"));
            let checkpoint = margins_dir.join(format!("{name}_seg0.live-transcript.json"));
            std::fs::write(&memo, "test memo")?;
            std::fs::write(&audio, b"test wav fixture")?;
            std::fs::write(&checkpoint, br#"{"terminal":false,"transcripts":[]}"#)?;
            margins_store::legacy::create_session(
                &margins_dir,
                name,
                &chrono::Local::now(),
                &format!(".margins/{name}.md"),
            )?;
            margins_store::legacy::add_segment(
                &margins_dir,
                name,
                0,
                &format!(".margins/{name}_seg0.wav"),
                0,
                None,
            )?;
            Ok(())
        }

        fn attach(&self, _work_dir: &Path, _selected: Option<&str>) -> Result<()> {
            unreachable!("capture-root regression only invokes margins new")
        }
    }

    fn seed_capture_root_settings(active: &Path, launcher: &Path) {
        let settings = margins_workflows::project::settings_path();
        std::fs::create_dir_all(settings.parent().unwrap()).unwrap();
        let value = serde_json::json!({
            "vault_path": active.to_string_lossy(),
            "active_project_id": "active-vault",
            "projects": [
                {
                    "id": "active-vault",
                    "name": "Obsidian",
                    "path": active.to_string_lossy(),
                    "readiness": "ready"
                },
                {
                    "id": "launcher-vault",
                    "name": "Explicit launcher fixture",
                    "path": launcher.to_string_lossy(),
                    "readiness": "ready"
                }
            ]
        });
        std::fs::write(&settings, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    fn assert_capture_artifacts(root: &Path, expected: bool) {
        let margins_dir = root.join(".margins");
        for path in [
            margins_dir.join("capture-root-regression.md"),
            margins_dir.join("capture-root-regression_seg0.wav"),
            margins_dir.join("capture-root-regression_seg0.live-transcript.json"),
            margins_store::legacy::database_path(&margins_dir),
        ] {
            assert_eq!(
                path.is_file(),
                expected,
                "unexpected state for {}",
                path.display()
            );
        }
    }

    #[test]
    fn setup_materializes_every_embedded_skill_file() {
        let temp = tempfile::tempdir().unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        let expected = [
            (
                "margins/SKILL.md",
                MARGINS_SKILL.get_file("SKILL.md").unwrap().contents(),
            ),
            (
                "margins/distillation-core.md",
                MARGINS_SKILL
                    .get_file("distillation-core.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/hosts/desktop.md",
                MARGINS_SKILL
                    .get_file("hosts/desktop.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/1on1-idea-exchange.md",
                MARGINS_SKILL
                    .get_file("templates/1on1-idea-exchange.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/design-scoping-session.md",
                MARGINS_SKILL
                    .get_file("templates/design-scoping-session.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/discovery-call.md",
                MARGINS_SKILL
                    .get_file("templates/discovery-call.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/group-conversation.md",
                MARGINS_SKILL
                    .get_file("templates/group-conversation.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "margins/templates/talk-reflection.md",
                MARGINS_SKILL
                    .get_file("templates/talk-reflection.md")
                    .unwrap()
                    .contents(),
            ),
            (
                "watermark/SKILL.md",
                WATERMARK_SKILL.get_file("SKILL.md").unwrap().contents(),
            ),
        ];

        for (relative, contents) in expected {
            assert_eq!(
                std::fs::read(temp.path().join(".margins/skills").join(relative)).unwrap(),
                contents,
                "materialized content differed for {relative}"
            );
        }
    }

    #[test]
    fn setup_skips_absent_agents_without_creating_their_home_dirs() {
        let temp = tempfile::tempdir().unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        for agent_home in [".claude", ".codex", ".cursor"] {
            assert!(!temp.path().join(agent_home).exists());
        }
        assert_eq!(
            String::from_utf8(report).unwrap(),
            "Claude Code skills: skipped (not installed)\nCodex skills: skipped (not installed)\nCursor skills: skipped (not installed)\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn setup_links_skills_for_every_detected_agent() {
        let temp = tempfile::tempdir().unwrap();
        for agent_home in [".claude", ".codex", ".cursor"] {
            std::fs::create_dir(temp.path().join(agent_home)).unwrap();
        }
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        for agent_home in [".claude", ".codex", ".cursor"] {
            for skill in ["margins", "watermark"] {
                let link = temp.path().join(agent_home).join("skills").join(skill);
                assert!(std::fs::symlink_metadata(&link)
                    .unwrap()
                    .file_type()
                    .is_symlink());
                assert_eq!(
                    std::fs::canonicalize(link).unwrap(),
                    std::fs::canonicalize(temp.path().join(".margins/skills").join(skill)).unwrap()
                );
            }
        }
        let report = String::from_utf8(report).unwrap();
        assert!(report.contains("Claude Code skills: linked margins, watermark"));
        assert!(report.contains("Codex skills: linked margins, watermark"));
        assert!(report.contains("Cursor skills: linked margins, watermark"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_preserves_preexisting_real_skill_directory() {
        let temp = tempfile::tempdir().unwrap();
        let user_skill = temp.path().join(".claude/skills/margins");
        std::fs::create_dir_all(&user_skill).unwrap();
        std::fs::write(user_skill.join("user-owned.txt"), "keep me").unwrap();
        let mut report = Vec::new();

        install_embedded_skills(temp.path(), &mut report).unwrap();

        assert_eq!(
            std::fs::read_to_string(user_skill.join("user-owned.txt")).unwrap(),
            "keep me"
        );
        assert!(!std::fs::symlink_metadata(&user_skill)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(
            std::fs::symlink_metadata(temp.path().join(".claude/skills/watermark"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(String::from_utf8(report)
            .unwrap()
            .contains("Claude Code skills: linked watermark; skipped margins (user dir present)"));
    }

    #[cfg(unix)]
    #[test]
    fn setup_repoints_stale_link_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let codex_skills = temp.path().join(".codex/skills");
        let stale_target = temp.path().join("old-margins-skill");
        std::fs::create_dir_all(&codex_skills).unwrap();
        std::fs::create_dir(&stale_target).unwrap();
        std::os::unix::fs::symlink(&stale_target, codex_skills.join("margins")).unwrap();
        let mut first_report = Vec::new();

        install_embedded_skills(temp.path(), &mut first_report).unwrap();
        let link = codex_skills.join("margins");
        let first_target = std::fs::read_link(&link).unwrap();
        std::fs::write(
            temp.path().join(".margins/skills/margins/SKILL.md"),
            "stale canonical content",
        )
        .unwrap();
        std::fs::write(
            temp.path()
                .join(".margins/skills/margins/removed-from-binary.md"),
            "stale removed file",
        )
        .unwrap();

        let mut second_report = Vec::new();
        install_embedded_skills(temp.path(), &mut second_report).unwrap();

        assert_eq!(std::fs::read_link(&link).unwrap(), first_target);
        assert_eq!(
            std::fs::canonicalize(&link).unwrap(),
            std::fs::canonicalize(temp.path().join(".margins/skills/margins")).unwrap()
        );
        assert_eq!(
            std::fs::read(temp.path().join(".margins/skills/margins/SKILL.md")).unwrap(),
            MARGINS_SKILL.get_file("SKILL.md").unwrap().contents()
        );
        assert!(!temp
            .path()
            .join(".margins/skills/margins/removed-from-binary.md")
            .exists());
        assert_eq!(first_report, second_report);
    }

    #[test]
    fn setup_runs_independent_steps_and_names_only_the_selected_workspace() {
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_model_dir = temp.path().join("speech-cache");
        std::fs::create_dir_all(&speech_model_dir).unwrap();
        std::fs::write(speech_model_dir.join("fixture.bin"), [0u8; 2048]).unwrap();
        crate::hosted_credentials::install_bundle(
            &margins_home,
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Hosted {
                model: "fixture-model".into(),
            },
            speech_model: Some(speech_model_dir.clone()),
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(&[], None, SetupLocalModelPolicyArg::Fallback),
            Some(&margins_home),
            None,
            Some(&skill_home),
            Some("practice"),
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(!failed);
        assert_eq!(provisioner.hosted_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provisioner.speech_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nHelp me set up Margins workspace practice so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: ok"));
        assert!(stderr.contains("ready with fixture-model"));
        assert!(!stderr.contains(&margins_home.to_string_lossy().to_string()));
        assert!(stderr.contains("setup local catalyst: ok — not installed under fallback policy"));
        assert!(stderr.contains("setup skills: ok"));
        assert!(stderr.contains(&format!(
            "setup speech: ok — model cache {}",
            speech_model_dir.display()
        )));
        assert!(stderr.contains("2.0 KB"));
        assert!(stderr.contains("catalyst mode: hosted — hosted_bundle_ready"));
    }

    #[test]
    fn setup_without_workspace_never_uses_the_process_directory_for_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("home");
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Offline {
                reason: "unused".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Skills],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            Some(&home),
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed, "no catalyst is configured in this fixture");
        assert_eq!(provisioner.hosted_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nGo to my notes folder, then help me set up Margins so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("Claude Code skills: skipped (not installed)"));
        assert!(stderr.contains("Codex skills: skipped (not installed)"));
        assert!(stderr.contains("Cursor skills: skipped (not installed)"));
        assert!(stderr.contains("setup skills: ok"));
        assert!(!temp.path().join(".margins").exists());
    }

    #[cfg(all(feature = "recall", unix))]
    #[test]
    fn setup_fake_broker_provisions_credentials_and_exits_zero_despite_failing_speech() {
        use std::os::unix::fs::PermissionsExt;

        let _global_env_lock = crate::test_process_env_lock().lock().unwrap();
        let _env_lock = PROCESS_ENV_LOCK.lock().unwrap();
        let names = [
            "ENZYME_FREE_CONFIG_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        ];
        let _restore = EnvRestore::capture(&names);
        for name in &names[1..] {
            std::env::remove_var(name);
        }
        let (broker_url, broker) = start_fake_setup_broker();
        std::env::set_var("ENZYME_FREE_CONFIG_URL", broker_url);
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_marker = temp.path().join("speech-attempted");
        let provisioner = FailingSpeechNativeProvisioner {
            speech_marker: speech_marker.clone(),
            local_calls: AtomicUsize::new(0),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(&[], None, SetupLocalModelPolicyArg::Fallback),
            Some(&margins_home),
            None,
            Some(&skill_home),
            Some("fixture-workspace"),
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        let request = broker.join().unwrap();

        assert!(
            !failed,
            "a usable hosted generator makes setup successful despite speech failure"
        );
        assert!(speech_marker.is_file());
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert!(request.starts_with("GET /llm/free-config HTTP/1.1\r\n"));
        assert!(request.lines().any(|line| line
            .to_ascii_lowercase()
            .starts_with("x-enzyme-bootstrap-id:")));
        for name in [
            crate::hosted_credentials::BOOTSTRAP_FILE,
            crate::hosted_credentials::BUNDLE_FILE,
        ] {
            let path = margins_home.join(name);
            assert!(path.is_file(), "missing {}", path.display());
            assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        assert!(std::fs::read_to_string(margins_home.join("config.toml"))
            .unwrap()
            .contains("mode = \"hosted\""));
        let stderr = String::from_utf8(stderr).unwrap();
        let hosted = stderr.find("setup hosted catalyst: ok").unwrap();
        let skills = stderr.find("setup skills: ok").unwrap();
        let speech = stderr
            .find("setup speech: failed — fixture speech setup failed")
            .unwrap();
        let local = stderr.find("setup local catalyst: ok").unwrap();
        assert!(
            hosted < skills && skills < speech && speech < local,
            "{stderr}"
        );
        assert!(stderr.contains("catalyst mode: hosted — hosted_bundle_ready"));
        assert!(!stderr.contains(&margins_home.to_string_lossy().to_string()));
        assert!(!stderr.contains(crate::hosted_credentials::BUNDLE_FILE));
        assert!(!stderr.contains("sk-or-v1-secret-shaped-setup-fixture"));
        assert_eq!(
            String::from_utf8(stdout).unwrap(),
            "Paste into your agent:\nHelp me set up Margins workspace fixture-workspace so it reflects how I use these notes. Run margins guide workspace-setup and follow it end to end.\n"
        );
    }

    #[cfg(feature = "recall")]
    #[test]
    fn setup_only_catalyst_with_hosted_broker_touches_no_model_or_skill_paths() {
        let _global_env_lock = crate::test_process_env_lock().lock().unwrap();
        let _env_lock = PROCESS_ENV_LOCK.lock().unwrap();
        let names = [
            "ENZYME_FREE_CONFIG_URL",
            "OPENAI_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_MODEL",
            "OPENROUTER_API_KEY",
            "OPENROUTER_BASE_URL",
            "OPENROUTER_MODEL",
        ];
        let _restore = EnvRestore::capture(&names);
        for name in &names[1..] {
            std::env::remove_var(name);
        }
        let (broker_url, broker) = start_fake_setup_broker();
        std::env::set_var("ENZYME_FREE_CONFIG_URL", broker_url);
        let temp = tempfile::tempdir().unwrap();
        let margins_home = temp.path().join("margins-home");
        let skill_home = temp.path().join("user-home");
        let speech_marker = temp.path().join("speech-attempted");
        let provisioner = FailingSpeechNativeProvisioner {
            speech_marker: speech_marker.clone(),
            local_calls: AtomicUsize::new(0),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(&margins_home),
            None,
            Some(&skill_home),
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();
        broker.join().unwrap();

        assert!(!failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 0);
        assert!(!speech_marker.exists());
        assert!(!margins_home.join("models").exists());
        assert!(!skill_home.join(".margins/skills").exists());
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: ok"));
        assert!(stderr.contains("setup local catalyst: ok — not installed under fallback policy"));
        assert!(!stderr.contains(&margins_home.to_string_lossy().to_string()));
        assert!(!stderr.contains(crate::hosted_credentials::BUNDLE_FILE));
        assert!(!stderr.contains("sk-or-v1-secret-shaped-setup-fixture"));
        assert!(!stderr.contains("setup skills:"));
        assert!(!stderr.contains("setup speech:"));
    }

    #[test]
    fn setup_exits_nonzero_when_no_generator_is_usable() {
        let temp = tempfile::tempdir().unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Offline {
                reason: "fixture broker offline".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: None,
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Fallback,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 1);
        let stderr = String::from_utf8(stderr).unwrap();
        assert!(stderr.contains("setup hosted catalyst: failed — fixture broker offline"));
        assert!(stderr.contains("catalyst mode: none — setup_required"));
    }

    #[test]
    fn setup_local_model_always_policy_installs_after_hosted_success() {
        let temp = tempfile::tempdir().unwrap();
        let local_model = temp.path().join("models/fixture.gguf");
        std::fs::create_dir_all(local_model.parent().unwrap()).unwrap();
        std::fs::write(&local_model, [0u8; 1024]).unwrap();
        crate::hosted_credentials::install_bundle(
            temp.path(),
            "fixture-machine",
            "fixture-key",
            "https://fixture.invalid/v1",
            "fixture-model",
            Some(4_102_444_800),
        )
        .unwrap();
        let provisioner = SpySetupProvisioner {
            hosted_calls: AtomicUsize::new(0),
            speech_calls: AtomicUsize::new(0),
            local_calls: AtomicUsize::new(0),
            hosted: HostedCatalystSetup::Hosted {
                model: "fixture-model".into(),
            },
            speech_model: None,
            speech_error: None,
            local_model: Some(local_model.clone()),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();

        let failed = run_setup_with(
            &provisioner,
            SetupSelection::from_args(
                &[SetupStepArg::Catalyst],
                None,
                SetupLocalModelPolicyArg::Always,
            ),
            Some(temp.path()),
            None,
            None,
            None,
            &mut stdout,
            &mut stderr,
        )
        .unwrap();

        assert!(!failed);
        assert_eq!(provisioner.local_calls.load(Ordering::SeqCst), 1);
        assert!(String::from_utf8(stderr).unwrap().contains(&format!(
            "setup local catalyst: ok — installed at {}",
            local_model.display()
        )));
    }

    #[test]
    fn post_capture_prompt_defaults_to_distill() {
        let mut input = "\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::Distill);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Turn this session into a note? [Y/n] "
        );
    }

    #[test]
    fn post_capture_prompt_accepts_saving_without_distilling() {
        let mut input = "no\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::SavedOnly);
    }

    #[test]
    fn post_capture_prompt_retries_an_invalid_answer() {
        let mut input = "later\ny\n".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::Distill);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Turn this session into a note? [Y/n] Please answer y or n.\n\
             Turn this session into a note? [Y/n] "
        );
    }

    #[test]
    fn post_capture_prompt_treats_eof_as_save_only() {
        let mut input = "".as_bytes();
        let mut output = Vec::new();

        let action = ask_post_capture_action(&mut input, &mut output).unwrap();

        assert_eq!(action, PostCaptureAction::SavedOnly);
    }

    #[test]
    fn production_new_from_macos_launcher_temp_roots_all_capture_files_in_active_vault() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let launcher = tempfile::Builder::new()
            .prefix(".tmp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        seed_capture_root_settings(&active, launcher.path());
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(launcher.path()).unwrap();
        let code = main_entry_with(["margins", "new"], &ArtifactWritingInteractive);
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&active, true);
        assert_capture_artifacts(launcher.path(), false);
    }

    #[test]
    fn production_new_from_obsidian_inbox_uses_obsidian_root() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("obsidian");
        let inbox = vault.join("inbox");
        std::fs::create_dir_all(vault.join(".obsidian")).unwrap();
        std::fs::create_dir_all(&inbox).unwrap();
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(&inbox).unwrap();
        let code = main_entry_with(["margins", "new"], &ArtifactWritingInteractive);
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(&vault, true);
        assert!(!inbox.join(".margins").exists());
    }

    #[test]
    fn production_new_allows_launcher_temp_only_when_explicitly_selected() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let _settings = ScopedTestSettings::new();
        let stable_parent = tempfile::tempdir().unwrap();
        let active = stable_parent.path().join("obsidian");
        std::fs::create_dir_all(active.join(".margins")).unwrap();
        let launcher = tempfile::Builder::new()
            .prefix(".tmp")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        seed_capture_root_settings(&active, launcher.path());
        let old_cwd = std::env::current_dir().unwrap();

        std::env::set_current_dir(launcher.path()).unwrap();
        let code = main_entry_with(
            ["margins", "new", "--project=launcher-vault"],
            &ArtifactWritingInteractive,
        );
        std::env::set_current_dir(&old_cwd).unwrap();

        assert_eq!(code, 0);
        assert_capture_artifacts(launcher.path(), true);
        assert_capture_artifacts(&active, false);
    }

    #[test]
    fn production_new_is_routed_through_private_interactive_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let spy = SpyInteractive(AtomicUsize::new(0));
        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(temp.path()).unwrap();
        let code = main_entry_with(["margins", "new"], &spy);
        std::env::set_current_dir(old).unwrap();
        assert_eq!(code, 0);
        assert_eq!(spy.0.load(Ordering::SeqCst), 1);

        #[test]
        fn bare_margins_creates_without_a_current_session_and_resumes_when_present() {
            let _guard = PROCESS_ENV_LOCK.lock().unwrap();
            let _settings = ScopedTestSettings::new();
            let temp = tempfile::tempdir().unwrap();
            let old = std::env::current_dir().unwrap();
            std::env::set_current_dir(temp.path()).unwrap();

            let without_current = SpyInteractive(AtomicUsize::new(0));
            let first_code = main_entry_with(["margins"], &without_current);

            let margins_dir = temp.path().join(".margins");
            std::fs::create_dir_all(&margins_dir).unwrap();
            margins_store::legacy::create_session(
                &margins_dir,
                "current-session",
                &chrono::Local::now(),
                ".margins/current-session.md",
            )
            .unwrap();
            std::fs::write(margins_dir.join("current"), "current-session\n").unwrap();

            let with_current = SpyInteractive(AtomicUsize::new(0));
            let second_code = main_entry_with(["margins"], &with_current);
            std::env::set_current_dir(old).unwrap();

            assert_eq!(first_code, 0);
            assert_eq!(without_current.0.load(Ordering::SeqCst), 1);
            assert_eq!(second_code, 0);
            assert_eq!(with_current.0.load(Ordering::SeqCst), 10);
        }
    }

    #[test]
    fn release_smoke_is_exact_and_does_not_invoke_interactive_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore =
            EnvRestore::capture(&["MARGINS_HOME", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("margins-home"));
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY");
        let spy = SpyInteractive(AtomicUsize::new(0));
        assert_eq!(main_entry_with(["margins", RELEASE_SMOKE_COMMAND], &spy), 0);
        assert_eq!(spy.0.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn official_capabilities_report_recall_composition() {
        let _guard = PROCESS_ENV_LOCK.lock().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let _restore =
            EnvRestore::capture(&["MARGINS_HOME", "OPENAI_API_KEY", "OPENROUTER_API_KEY"]);
        std::env::set_var("MARGINS_HOME", temp.path().join("margins-home"));
        std::env::remove_var("OPENAI_API_KEY");
        std::env::remove_var("OPENROUTER_API_KEY");
        let value = official_capabilities_json();
        assert_eq!(value["schema"], 1);
        assert_eq!(value["product"], "margins");
        assert_eq!(value["composition"], "official");
        assert_eq!(value["official"], true);
        assert_eq!(
            value["build"]["commit"],
            margins_cli::build_info::get().commit
        );
        assert_eq!(value["recall"]["indexing"], cfg!(feature = "recall"));
        assert_eq!(value["recall"]["lookup"], cfg!(feature = "recall"));
        assert_eq!(value["recall"]["scan"], cfg!(feature = "recall"));
        assert_eq!(
            value["recall"]["local_model"],
            cfg!(feature = "recall-local-model")
        );
        assert_eq!(value["distillation"]["workflow"], "connected_note");
        assert!(value["distillation"]["inputs"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("transcript")));
        assert!(value["catalyst"]["mode"].is_string());
        assert!(value["catalyst"]["usable"].is_boolean());
        assert!(value["catalyst"]["reason"].is_string());
        assert!(value["catalyst"]["expiry"]["state"].is_string());
        assert!(value["catalyst"]["expiry"]["bucket"].is_string());
        assert!(value["catalyst"].get("path").is_none());
        assert!(value["catalyst"].get("api_key").is_none());
        assert!(value["catalyst"].get("bootstrap_id").is_none());
        assert!(value["catalyst"].get("profile").is_none());
        assert!(value["catalyst"].get("expires_at").is_none());
    }

    #[test]
    fn checkpoint_replace_is_atomic_and_does_not_follow_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let checkpoint = root.path().join("live.json");
        let outside = root.path().join("outside.json");
        std::fs::write(&outside, b"private").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &checkpoint).unwrap();
        #[cfg(not(unix))]
        std::fs::write(&checkpoint, b"old").unwrap();

        let value = serde_json::json!({"version": 1, "terminal": false});
        write_checkpoint_value(&checkpoint, &value).unwrap();

        assert_eq!(std::fs::read(&outside).unwrap(), b"private");
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(checkpoint).unwrap())
                .unwrap(),
            value
        );
    }

    #[test]
    fn recorder_generation_gaps_preserve_the_session_timeline() {
        assert_eq!(timeline_gap_samples(12_000, 10_000, 1_500), 8_000);
        assert_eq!(timeline_gap_samples(12_000, 10_000, 2_000), 0);
        assert_eq!(timeline_gap_samples(9_000, 10_000, 0), 0);
    }

    #[test]
    fn queue_accounting_handles_consumer_winning_the_send_increment_race() {
        let queued = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let consumer_counter = queued.clone();
        let consumer = std::thread::spawn(move || debit_queued_samples(&consumer_counter, 512));
        queued.fetch_add(512, std::sync::atomic::Ordering::Release);
        consumer.join().unwrap();
        assert_eq!(queued.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[cfg(all(feature = "coreml-asr", target_os = "macos"))]
    #[test]
    #[ignore = "requires local FluidAudio CoreML assets"]
    fn native_worker_warms_on_its_thread_and_writes_terminal_checkpoint() {
        let root = tempfile::tempdir().unwrap();
        let checkpoint = root.path().join("segment.live-transcript.json");
        let status = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(
            crate::app::LIVE_TRANSCRIPTION_WARMING,
        ));
        let started = std::time::Instant::now();
        let worker = super::LiveTranscriptWorker::start(checkpoint.clone(), 4_250, status.clone())
            .unwrap()
            .expect("local CoreML models are required for this smoke test");
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "worker startup must not block capture on model warmup"
        );
        assert_eq!(
            status.load(std::sync::atomic::Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_WARMING
        );
        let sink = worker.sink_for_offset(4_250);
        let samples = vec![0.0; 16_000 * 3];
        sink.queued_samples
            .fetch_add(samples.len() as u64, std::sync::atomic::Ordering::Release);
        sink.sender
            .send(crate::recorder::LiveAudioChunk {
                channel: crate::recorder::LiveAudioChannel::Mic,
                generation: sink.generation,
                session_offset_ms: 4_250,
                sample_rate: 16_000,
                samples,
            })
            .unwrap();

        let mut live_value = None;
        for _ in 0..1_200 {
            if let Ok(bytes) = std::fs::read(&checkpoint) {
                let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                if value["terminal"] == false {
                    live_value = Some(value);
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let live_value = live_value.expect("incremental checkpoint was not written within 120s");
        assert!(live_value["decoded_until_ms"].as_u64().unwrap() > 4_250);
        assert_eq!(
            status.load(std::sync::atomic::Ordering::Acquire),
            crate::app::LIVE_TRANSCRIPTION_READY
        );

        drop(sink);
        assert!(worker.begin_finish(3_000).complete().unwrap());

        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(checkpoint).unwrap()).unwrap();
        assert_eq!(value["terminal"], true);
        assert_eq!(value["decoded_until_ms"], 7_250);
    }
}
#[cfg(test)]
mod bare_capture_decision_tests {
    use super::*;

    #[test]
    fn bare_capture_creates_only_when_current_is_absent_or_stale() {
        assert!(bare_capture_creates(None, false));
        assert!(bare_capture_creates(Some(""), false));
        assert!(bare_capture_creates(Some("stale"), false));
        assert!(!bare_capture_creates(Some("current"), true));
    }
}
