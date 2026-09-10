//! Revision-aware integration reconciliation derived from Workspace bindings.

use crate::error::CliError;
use anyhow::{Context, Result};
use chrono::Utc;
use margins_workflows::integrations::{
    granola_failure_info, sync_granola_binding, Connector, ConnectorCtx, GoogleCalendarConnector,
    GoogleCalendarScope, GoogleCredentialBackendKind, GoogleEmailConnector, GoogleMeetConnector,
    GoogleTokenProvider, GranolaCredentialBackendKind, HealthReport, HealthStatus,
    IntegrationsStore, NativeGoogleClient, ReconcileResult, EMAIL_CONNECTOR_ID,
    GOOGLE_CALENDAR_CONNECTOR_ID, GOOGLE_MEET_CONNECTOR_ID,
    GOOGLE_MEET_MATERIALIZATION_FINGERPRINT, GRANOLA_CONNECTOR_ID,
};
use margins_workflows::workspace::{
    resolve_state_dir, validate_mutation_request_id, workspace_revision, WorkspaceBinding,
    WorkspaceConfig, WorkspaceMutationError,
};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Path, PathBuf};

const RECONCILE_SCHEMA: &str = "margins.integrations.reconcile.v1";
const STATUS_SCHEMA: &str = "margins.integrations.status.v1";

#[derive(Debug, Clone)]
enum DeclaredConnector {
    Gmail {
        binding: String,
        account: String,
        selector: margins_workflows::workspace::GmailCollectionSelector,
    },
    Calendar {
        binding: String,
        account: String,
        selector: margins_workflows::workspace::CalendarCollectionSelector,
    },
    Meet {
        binding: String,
        account: String,
    },
    Granola {
        binding: String,
        account: String,
        collection: margins_workflows::workspace::GranolaCollectionSelector,
    },
}

impl DeclaredConnector {
    fn binding(&self) -> &str {
        match self {
            Self::Gmail { binding, .. }
            | Self::Calendar { binding, .. }
            | Self::Meet { binding, .. }
            | Self::Granola { binding, .. } => binding,
        }
    }

    fn connector_id(&self) -> &'static str {
        match self {
            Self::Gmail { .. } => EMAIL_CONNECTOR_ID,
            Self::Calendar { .. } => GOOGLE_CALENDAR_CONNECTOR_ID,
            Self::Meet { .. } => GOOGLE_MEET_CONNECTOR_ID,
            Self::Granola { .. } => GRANOLA_CONNECTOR_ID,
        }
    }

    fn account(&self) -> &str {
        match self {
            Self::Gmail { account, .. }
            | Self::Calendar { account, .. }
            | Self::Meet { account, .. }
            | Self::Granola { account, .. } => account,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReconcileError {
    pub code: &'static str,
    pub message: String,
    pub stage: &'static str,
    pub reason: &'static str,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ReconcileRow {
    binding: String,
    connector_id: String,
    account: String,
    status: &'static str,
    records_written: u64,
    records_updated: u64,
    records_unchanged: u64,
    tombstones: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ReconcileError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncConnectorRow {
    pub binding: String,
    pub connector_id: String,
    pub account: String,
    pub ok: bool,
    pub status: SyncSourceStatus,
    pub records_written: u64,
    pub records_updated: u64,
    pub records_unchanged: u64,
    pub tombstones: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ReconcileError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncSourceStatus {
    Synced,
    NeedsAction,
    Error,
}

impl SyncConnectorRow {
    fn synced(source: &DeclaredConnector, result: ReconcileResult) -> Self {
        Self {
            binding: source.binding().to_string(),
            connector_id: source.connector_id().to_string(),
            account: source.account().to_string(),
            ok: true,
            status: SyncSourceStatus::Synced,
            records_written: result.records_written,
            records_updated: result.records_updated,
            records_unchanged: result.records_unchanged,
            tombstones: result.tombstones,
            error: None,
        }
    }

    fn failed(source: &DeclaredConnector, error: anyhow::Error) -> Self {
        let row = ReconcileRow::failed(source, error);
        Self {
            binding: row.binding,
            connector_id: row.connector_id,
            account: row.account,
            ok: false,
            status: SyncSourceStatus::Error,
            records_written: row.records_written,
            records_updated: row.records_updated,
            records_unchanged: row.records_unchanged,
            tombstones: row.tombstones,
            error: row.error,
        }
    }
}

impl ReconcileRow {
    fn applied(source: &DeclaredConnector, result: ReconcileResult) -> Self {
        Self {
            binding: source.binding().to_string(),
            connector_id: source.connector_id().to_string(),
            account: source.account().to_string(),
            status: "applied",
            records_written: result.records_written,
            records_updated: result.records_updated,
            records_unchanged: result.records_unchanged,
            tombstones: result.tombstones,
            error: None,
        }
    }

    fn failed(source: &DeclaredConnector, error: anyhow::Error) -> Self {
        let granola = matches!(source, DeclaredConnector::Granola { .. });
        let failure = granola.then(|| granola_failure_info(&error)).flatten();
        let (code, message, stage, reason, retryable) = failure
            .map(|failure| {
                (
                    failure.code,
                    "Granola reconciliation failed; inspect the typed stage and reason."
                        .to_string(),
                    failure.stage,
                    failure.reason,
                    failure.retryable,
                )
            })
            .unwrap_or((
                if granola {
                    "granola_materialization_failed"
                } else {
                    "connector_reconcile_failed"
                },
                "Connector reconciliation failed; no provider response details were retained."
                    .to_string(),
                if granola {
                    "materialization"
                } else {
                    "reconcile"
                },
                "internal",
                false,
            ));
        Self {
            binding: source.binding().to_string(),
            connector_id: source.connector_id().to_string(),
            account: source.account().to_string(),
            status: "error",
            records_written: 0,
            records_updated: 0,
            records_unchanged: 0,
            tombstones: 0,
            error: Some(ReconcileError {
                code,
                message,
                stage,
                reason,
                retryable,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct ReconcileEnvelope {
    schema_version: &'static str,
    ok: bool,
    workspace_id: String,
    revision: String,
    request_id: String,
    replayed: bool,
    results: Vec<ReconcileRow>,
}

#[derive(Debug, Serialize)]
struct StatusRow {
    binding: String,
    connector_id: String,
    account: String,
    #[serde(flatten)]
    health: HealthReport,
}

#[derive(Debug, Serialize)]
struct StatusEnvelope {
    schema_version: &'static str,
    workspace_id: String,
    revision: String,
    results: Vec<StatusRow>,
}

pub fn reconcile(
    workspace_state_dir: &Path,
    connector_filter: Option<&str>,
    account_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    reconcile_inner(
        workspace_state_dir,
        connector_filter,
        account_filter,
        expected_revision,
        request_id,
        GoogleReconcileTransport::Credential(None),
        stdout,
    )
}

pub fn reconcile_with_google_credential(
    workspace_state_dir: &Path,
    connector_filter: Option<&str>,
    account_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    credential_json: &[u8],
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    reconcile_inner(
        workspace_state_dir,
        connector_filter,
        account_filter,
        expected_revision,
        request_id,
        GoogleReconcileTransport::Credential(Some(credential_json)),
        stdout,
    )
}

pub fn reconcile_with_native_google_client(
    workspace_state_dir: &Path,
    connector_filter: Option<&str>,
    account_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    native: &NativeGoogleClient,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    reconcile_inner(
        workspace_state_dir,
        connector_filter,
        account_filter,
        expected_revision,
        request_id,
        GoogleReconcileTransport::NativeClient(native),
        stdout,
    )
}

pub fn sync_declared_with_google_credential(
    workspace_state_dir: &Path,
    source_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    credential_json: Option<&[u8]>,
) -> Result<Vec<SyncConnectorRow>, CliError> {
    sync_declared_inner(
        workspace_state_dir,
        source_filter,
        expected_revision,
        request_id,
        GoogleReconcileTransport::Credential(credential_json),
    )
}

pub fn sync_declared_with_native_google_client(
    workspace_state_dir: &Path,
    source_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    native: &NativeGoogleClient,
) -> Result<Vec<SyncConnectorRow>, CliError> {
    sync_declared_inner(
        workspace_state_dir,
        source_filter,
        expected_revision,
        request_id,
        GoogleReconcileTransport::NativeClient(native),
    )
}

enum GoogleReconcileTransport<'a> {
    Credential(Option<&'a [u8]>),
    NativeClient(&'a NativeGoogleClient),
}

fn sync_declared_inner(
    workspace_state_dir: &Path,
    source_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    google_transport: GoogleReconcileTransport<'_>,
) -> Result<Vec<SyncConnectorRow>, CliError> {
    validate_mutation_request_id(request_id).map_err(CliError::from_anyhow)?;
    let workspace = resolve_state_dir(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let actual_revision = workspace_revision(&workspace.config).map_err(CliError::from_anyhow)?;
    if actual_revision != expected_revision {
        return Err(CliError::from_anyhow(
            WorkspaceMutationError::RevisionConflict {
                expected: expected_revision.to_string(),
                actual: actual_revision,
            }
            .into(),
        ));
    }
    let store = IntegrationsStore::open(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let _lock = store
        .acquire_reconcile_lock()
        .map_err(CliError::from_anyhow)?;
    let sources = declared_connectors_by_source(&workspace.config, source_filter);
    let mut results = Vec::with_capacity(sources.len());
    for source in &sources {
        let row = match reconcile_one(
            workspace_state_dir,
            source,
            expected_revision,
            &google_transport,
        ) {
            Ok(result) => SyncConnectorRow::synced(source, result),
            Err(error)
                if matches!(
                    error.downcast_ref::<WorkspaceMutationError>(),
                    Some(WorkspaceMutationError::RevisionConflict { .. })
                ) =>
            {
                return Err(CliError::from_anyhow(error));
            }
            Err(error) => SyncConnectorRow::failed(source, error),
        };
        results.push(row);
    }
    if results.is_empty()
        && source_filter.is_some_and(|filter| !source_is_local(&workspace.config, filter))
    {
        let source = source_filter.unwrap_or_default().to_string();
        results.push(SyncConnectorRow {
            binding: source.clone(),
            connector_id: source.clone(),
            account: String::new(),
            ok: false,
            status: SyncSourceStatus::NeedsAction,
            records_written: 0,
            records_updated: 0,
            records_unchanged: 0,
            tombstones: 0,
            error: Some(ReconcileError {
                code: "source_not_configured",
                message: "No declared integration binding matched this source.".to_string(),
                stage: "desired_state",
                reason: "source_not_configured",
                retryable: false,
            }),
        });
    }
    Ok(results)
}

fn reconcile_inner(
    workspace_state_dir: &Path,
    connector_filter: Option<&str>,
    account_filter: Option<&str>,
    expected_revision: &str,
    request_id: &str,
    google_transport: GoogleReconcileTransport<'_>,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    validate_mutation_request_id(request_id).map_err(CliError::from_anyhow)?;
    let initial_workspace =
        resolve_state_dir(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let store = IntegrationsStore::open(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let _lock = store
        .acquire_reconcile_lock()
        .map_err(CliError::from_anyhow)?;
    let request_hash = reconcile_request_hash(
        &initial_workspace.config.id,
        expected_revision,
        connector_filter,
        account_filter,
    )
    .map_err(CliError::from_anyhow)?;
    if let Some((stored_hash, mut receipt)) = store
        .load_reconcile_receipt(request_id)
        .map_err(CliError::from_anyhow)?
    {
        if stored_hash != request_hash {
            return Err(CliError::new(
                "idempotency_conflict",
                format!("request id '{request_id}' was already used for a different reconcile"),
            )
            .with_details(json!({"request_id": request_id})));
        }
        receipt["replayed"] = Value::Bool(true);
        write_json(stdout, &receipt)?;
        return if receipt.get("ok").and_then(Value::as_bool) == Some(false) {
            Err(CliError::new(
                "integration_reconcile_failed",
                "One or more integration connectors failed to reconcile",
            )
            .reported())
        } else {
            Ok(())
        };
    }

    // Re-read desired state only after the durable replay gate. An exact replay
    // returns its original receipt even if later workspace mutations changed or
    // removed the binding; a new request must still match the current revision.
    let workspace = resolve_state_dir(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let actual_revision = workspace_revision(&workspace.config).map_err(CliError::from_anyhow)?;
    if actual_revision != expected_revision {
        return Err(CliError::from_anyhow(
            WorkspaceMutationError::RevisionConflict {
                expected: expected_revision.to_string(),
                actual: actual_revision,
            }
            .into(),
        ));
    }

    let sources = declared_connectors(&workspace.config, connector_filter, account_filter);
    if sources.is_empty() {
        return Err(CliError::new(
            "integration_binding_not_found",
            "No declared integration binding matched the requested connector and account",
        )
        .with_details(json!({
            "connector_id": connector_filter,
            "account": account_filter,
        })));
    }

    let mut results = Vec::with_capacity(sources.len());
    for source in &sources {
        let row = match reconcile_one(
            workspace_state_dir,
            source,
            expected_revision,
            &google_transport,
        ) {
            Ok(result) => ReconcileRow::applied(source, result),
            Err(error)
                if matches!(
                    error.downcast_ref::<WorkspaceMutationError>(),
                    Some(WorkspaceMutationError::RevisionConflict { .. })
                ) =>
            {
                return Err(CliError::from_anyhow(error));
            }
            Err(error) => ReconcileRow::failed(source, error),
        };
        results.push(row);
    }
    let ok = results.iter().all(|row| row.error.is_none());
    let envelope = ReconcileEnvelope {
        schema_version: RECONCILE_SCHEMA,
        ok,
        workspace_id: workspace.config.id,
        revision: expected_revision.to_string(),
        request_id: request_id.to_string(),
        replayed: false,
        results,
    };
    let receipt =
        serde_json::to_value(&envelope).map_err(|error| CliError::from_anyhow(error.into()))?;
    store
        .store_reconcile_receipt(request_id, &request_hash, &receipt)
        .map_err(CliError::from_anyhow)?;
    write_json(stdout, &envelope)?;
    if ok {
        Ok(())
    } else {
        Err(CliError::new(
            "integration_reconcile_failed",
            "One or more integration connectors failed to reconcile",
        )
        .reported())
    }
}

pub fn status(
    workspace_state_dir: &Path,
    json_output: bool,
    stdout: &mut dyn Write,
) -> Result<(), CliError> {
    let workspace = resolve_state_dir(workspace_state_dir).map_err(CliError::from_anyhow)?;
    let store = IntegrationsStore::open_existing_read_only(workspace_state_dir)
        .map_err(CliError::from_anyhow)?;
    let mut results = Vec::new();
    for source in declared_connectors(&workspace.config, None, None) {
        let ctx = connector_context(workspace_state_dir, &source, None);
        let health = if let Some(store) = store.as_ref() {
            match &source {
                DeclaredConnector::Gmail { selector, .. } => selector
                    .materialization_fingerprint()
                    .and_then(|fingerprint| {
                        store.health_report_for_materialization(&ctx, &fingerprint)
                    }),
                DeclaredConnector::Calendar { selector, .. } => selector
                    .materialization_fingerprint()
                    .and_then(|fingerprint| {
                        let scope = GoogleCalendarScope::for_selector(selector, Utc::now())?;
                        store.health_report_for_calendar_materialization(
                            &ctx,
                            &fingerprint,
                            &scope.as_range(),
                        )
                    }),
                DeclaredConnector::Meet { .. } => store.health_report_for_materialization(
                    &ctx,
                    GOOGLE_MEET_MATERIALIZATION_FINGERPRINT,
                ),
                DeclaredConnector::Granola { collection, .. } => collection
                    .materialization_fingerprint()
                    .and_then(|fingerprint| {
                        store.health_report_for_materialization(&ctx, &fingerprint)
                    }),
            }
            .map_err(CliError::from_anyhow)?
        } else {
            HealthReport {
                status: HealthStatus::Stale,
                reason: Some("never_refreshed".to_string()),
                last_successful_sync: None,
                cursor_age_secs: None,
                detail: Some("connector has not completed a successful reconcile".to_string()),
            }
        };
        results.push(StatusRow {
            binding: source.binding().to_string(),
            connector_id: source.connector_id().to_string(),
            account: source.account().to_string(),
            health,
        });
    }
    let envelope = StatusEnvelope {
        schema_version: STATUS_SCHEMA,
        workspace_id: workspace.config.id.clone(),
        revision: workspace_revision(&workspace.config).map_err(CliError::from_anyhow)?,
        results,
    };
    if json_output {
        write_json(stdout, &envelope)
    } else if envelope.results.is_empty() {
        writeln!(stdout, "No integration bindings declared.")
            .map_err(|error| CliError::new("output_failed", error.to_string()))
    } else {
        for row in envelope.results {
            writeln!(
                stdout,
                "{} {} ({}): {}",
                row.connector_id,
                row.account,
                row.binding,
                row.health.status.as_str()
            )
            .map_err(|error| CliError::new("output_failed", error.to_string()))?;
        }
        Ok(())
    }
}

fn declared_connectors(
    config: &WorkspaceConfig,
    connector_filter: Option<&str>,
    account_filter: Option<&str>,
) -> Vec<DeclaredConnector> {
    config
        .bindings
        .iter()
        .filter_map(|(binding, source)| match source {
            WorkspaceBinding::Gmail { account, gmail } => Some(DeclaredConnector::Gmail {
                binding: binding.clone(),
                account: account.clone(),
                selector: gmail.clone(),
            }),
            WorkspaceBinding::GoogleCalendar { account, calendar } => {
                Some(DeclaredConnector::Calendar {
                    binding: binding.clone(),
                    account: account.clone(),
                    selector: calendar.clone(),
                })
            }
            WorkspaceBinding::GoogleMeet { account } => Some(DeclaredConnector::Meet {
                binding: binding.clone(),
                account: account.clone(),
            }),
            WorkspaceBinding::Granola {
                account,
                collection,
            } => Some(DeclaredConnector::Granola {
                binding: binding.clone(),
                account: account.clone(),
                collection: collection.clone(),
            }),
            WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. } => None,
        })
        .filter(|source| {
            connector_filter.is_none_or(|filter| filter == source.connector_id())
                && account_filter.is_none_or(|filter| filter == source.account())
        })
        .collect()
}

fn declared_connectors_by_source(
    config: &WorkspaceConfig,
    source_filter: Option<&str>,
) -> Vec<DeclaredConnector> {
    declared_connectors(config, None, None)
        .into_iter()
        .filter(|source| source_filter.is_none_or(|filter| filter == source.binding()))
        .collect()
}

fn source_is_local(config: &WorkspaceConfig, source: &str) -> bool {
    config.bindings.get(source).is_some_and(|binding| {
        matches!(
            binding,
            WorkspaceBinding::NativeMarkdown { .. } | WorkspaceBinding::Captures { .. }
        )
    })
}

fn reconcile_one(
    workspace_state_dir: &Path,
    source: &DeclaredConnector,
    expected_revision: &str,
    google_transport: &GoogleReconcileTransport<'_>,
) -> Result<ReconcileResult> {
    if let DeclaredConnector::Granola {
        account,
        collection,
        ..
    } = source
    {
        let margins_home = margins_workflows::workspace::margins_home()?;
        return sync_granola_binding(
            &margins_home,
            workspace_state_dir,
            account,
            collection,
            Some(expected_revision),
            GranolaCredentialBackendKind::File0600,
            &|_, _, _| {},
        );
    }
    let native = match google_transport {
        GoogleReconcileTransport::Credential(google_credential) => {
            let credential = google_credential
                .context("Google reconcile requires the official Margins build")?;
            let margins_home = workspace_state_dir
                .parent()
                .and_then(Path::parent)
                .context("workspace state directory has no parent")?;
            let token_provider = GoogleTokenProvider::new_with_backend(
                margins_home,
                source.account(),
                credential,
                GoogleCredentialBackendKind::File0600,
            )?;
            NativeGoogleClient::new(token_provider)
        }
        GoogleReconcileTransport::NativeClient(native) => (*native).clone(),
    };
    let ctx = connector_context(workspace_state_dir, source, None);
    match source {
        DeclaredConnector::Gmail { selector, .. } => {
            GoogleEmailConnector::new(selector.clone(), native)
                .reconcile(&ctx, Some(expected_revision))
        }
        DeclaredConnector::Calendar { selector, .. } => {
            GoogleCalendarConnector::new(native, selector.clone())
                .reconcile(&ctx, Some(expected_revision))
        }
        DeclaredConnector::Meet { .. } => {
            GoogleMeetConnector::new(native).reconcile(&ctx, Some(expected_revision))
        }
        DeclaredConnector::Granola { .. } => {
            unreachable!("Granola reconciles before Google transport setup")
        }
    }
}

fn connector_context(
    workspace_state_dir: &Path,
    source: &DeclaredConnector,
    command_path: Option<PathBuf>,
) -> ConnectorCtx {
    ConnectorCtx {
        vault_root: workspace_state_dir.to_path_buf(),
        connector_id: source.connector_id().to_string(),
        account: source.account().to_string(),
        command_path,
    }
}

fn reconcile_request_hash(
    workspace_id: &str,
    revision: &str,
    connector: Option<&str>,
    account: Option<&str>,
) -> Result<String> {
    #[derive(Serialize)]
    struct Identity<'a> {
        workspace_id: &'a str,
        revision: &'a str,
        connector: Option<&'a str>,
        account: Option<&'a str>,
    }
    let bytes = serde_json::to_vec(&Identity {
        workspace_id,
        revision,
        connector,
        account,
    })
    .context("serializing reconcile request identity")?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn write_json(stdout: &mut dyn Write, value: &impl Serialize) -> Result<(), CliError> {
    serde_json::to_writer_pretty(&mut *stdout, value)
        .map_err(|error| CliError::from_anyhow(error.into()))?;
    writeln!(stdout).map_err(|error| CliError::new("output_failed", error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconcile_request_identity_is_deterministic_and_filter_sensitive() {
        let first = reconcile_request_hash("practice", "abc", Some("email"), None).unwrap();
        let same = reconcile_request_hash("practice", "abc", Some("email"), None).unwrap();
        let different = reconcile_request_hash("practice", "abc", Some("gcal"), None).unwrap();
        assert_eq!(first, same);
        assert_ne!(first, different);
    }
}
